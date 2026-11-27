//! The health engine: runs an agent's checks and publishes health records.
//!
//! - TCP and HTTP checks run on their interval, at most `max_concurrent_checks` at a time.
//! - TTL checks turn critical when the application stops reporting.
//! - A check changes state only after `failures_before_critical` consecutive failures or
//!   `successes_before_passing` consecutive successes.
//! - Any state change is published immediately; unchanged health is refreshed periodically
//!   so it does not expire. One publisher task writes records in order, always the latest state.
//! - A check critical for longer than `deregister_critical_after` asks the agent to deregister its service.

mod probe;

use crate::catalog::{Catalog, Kind};
use crate::{RegistryError, Result};
use bytes::Bytes;
use lictor_store::{Bucket, Entry, WatchEvent};
use lictor_types::{CheckDefinition, CheckKind, CheckState, HealthRecord, HealthStatus, SCHEMA_VERSION, ServiceRecord};
use probe::Outcome;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tokio::sync::{Semaphore, mpsc};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

/// Identity written into health records.
#[derive(Debug, Clone)]
pub(crate) struct Owner {
    pub node_id: String,
    pub generation: String,
}

/// Defaults for checks that do not set their own.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Defaults {
    pub interval: Duration,
    pub timeout: Duration,
    pub max_concurrent: usize,
}

struct Check {
    def: CheckDefinition,
    state: CheckState,
    ok_streak: u32,
    fail_streak: u32,
    critical_since: Option<Instant>,
    ttl_deadline: Option<Instant>,
    deregister_requested: bool,
}

impl Check {
    fn new(def: CheckDefinition) -> Self {
        let ttl_deadline = match def.kind {
            CheckKind::Ttl { ttl } => Some(Instant::now() + ttl),
            _ => None,
        };
        let status = def.initial_status;
        Check {
            state: CheckState { status, output: String::new(), duration_ms: 0 },
            critical_since: (status == HealthStatus::Critical).then(Instant::now),
            def,
            ok_streak: 0,
            fail_streak: 0,
            ttl_deadline,
            deregister_requested: false,
        }
    }

    /// Apply a run result with thresholds. Returns true if the status changed.
    fn record(&mut self, o: Outcome) -> bool {
        let before = self.state.status;
        self.state.output = o.output;
        self.state.duration_ms = o.duration.as_millis() as u64;
        if o.status == HealthStatus::Passing {
            self.ok_streak += 1;
            self.fail_streak = 0;
            if self.ok_streak >= self.def.successes_before_passing {
                self.state.status = HealthStatus::Passing;
            }
        } else {
            self.fail_streak += 1;
            self.ok_streak = 0;
            if self.fail_streak >= self.def.failures_before_critical {
                self.state.status = o.status;
            }
        }
        self.after_change(before)
    }

    /// Set a state directly (TTL reports and expiry). Returns true if the status changed.
    fn set(&mut self, status: HealthStatus, output: String) -> bool {
        let before = self.state.status;
        self.state.status = status;
        self.state.output = output;
        self.after_change(before)
    }

    fn after_change(&mut self, before: HealthStatus) -> bool {
        if self.state.status == HealthStatus::Critical {
            self.critical_since.get_or_insert_with(Instant::now);
        } else {
            self.critical_since = None;
            self.deregister_requested = false;
        }
        before != self.state.status
    }

    /// Whether the service should be deregistered now (reported once).
    fn wants_deregister(&mut self) -> bool {
        match (self.def.deregister_critical_after, self.critical_since) {
            (Some(after), Some(since)) if !self.deregister_requested && since.elapsed() >= after => {
                self.deregister_requested = true;
                true
            }
            _ => false,
        }
    }
}

struct Service {
    key: String,
    checks: BTreeMap<String, Check>,
    cancel: CancellationToken,
}

struct Inner {
    owner: Owner,
    bucket: Arc<dyn Bucket>,
    catalog: Arc<Catalog>,
    defaults: Defaults,
    permits: Arc<Semaphore>,
    services: Mutex<HashMap<String, Service>>,
    publish: mpsc::UnboundedSender<String>,
    deregister: mpsc::UnboundedSender<String>,
    cancel: CancellationToken,
}

/// Runs checks for the services of one agent.
#[derive(Clone)]
pub(crate) struct HealthEngine {
    inner: Arc<Inner>,
}

impl HealthEngine {
    /// Start the engine. The receiver yields service ids that asked to be deregistered.
    pub(crate) fn start(
        owner: Owner,
        bucket: Arc<dyn Bucket>,
        catalog: Arc<Catalog>,
        defaults: Defaults,
        cancel: CancellationToken,
    ) -> (HealthEngine, mpsc::UnboundedReceiver<String>) {
        let (publish, publish_rx) = mpsc::unbounded_channel();
        let (deregister, deregister_rx) = mpsc::unbounded_channel();
        let inner = Arc::new(Inner {
            owner,
            bucket,
            catalog,
            permits: Arc::new(Semaphore::new(defaults.max_concurrent)),
            defaults,
            services: Mutex::new(HashMap::new()),
            publish,
            deregister,
            cancel,
        });
        tokio::spawn(publisher(Arc::downgrade(&inner), publish_rx, inner.cancel.clone()));
        (HealthEngine { inner }, deregister_rx)
    }

    /// Start (or restart) the checks of a service. Checks whose definition is unchanged keep their state.
    pub(crate) fn set_service(&self, record: &ServiceRecord) {
        let mut services = self.inner.services.lock().unwrap();
        let mut previous = services.remove(&record.service_id);
        if let Some(p) = &previous {
            p.cancel.cancel();
        }
        if record.checks.is_empty() {
            // The caller deletes the health record (see `remove_service`).
            return;
        }
        let cancel = self.inner.cancel.child_token();
        let mut checks = BTreeMap::new();
        for def in &record.checks {
            let check = match previous.as_mut().and_then(|p| p.checks.remove(&def.id)) {
                Some(old) if old.def == *def => old,
                _ => Check::new(def.clone()),
            };
            checks.insert(def.id.clone(), check);
            let weak = Arc::downgrade(&self.inner);
            let (sid, cid, def, c) = (record.service_id.clone(), def.id.clone(), def.clone(), cancel.clone());
            match def.kind {
                CheckKind::Ttl { ttl } => tokio::spawn(ttl_watcher(weak, sid, cid, ttl, c)),
                _ => tokio::spawn(runner(weak, sid, cid, def, c)),
            };
        }
        services.insert(record.service_id.clone(), Service { key: record.key(), checks, cancel });
        let _ = self.inner.publish.send(record.service_id.clone());
    }

    /// Stop the checks of a service and delete its health record.
    pub(crate) async fn remove_service(&self, service_id: &str, key: &str) {
        if let Some(s) = self.inner.services.lock().unwrap().remove(service_id) {
            s.cancel.cancel();
        }
        delete_record(&self.inner, key).await;
    }

    /// Report the state of a TTL check.
    pub(crate) fn report(&self, check_id: &str, status: HealthStatus, output: String) -> Result<()> {
        let mut services = self.inner.services.lock().unwrap();
        for (sid, svc) in services.iter_mut() {
            if let Some(c) = svc.checks.get_mut(check_id) {
                let CheckKind::Ttl { ttl } = c.def.kind else {
                    return Err(RegistryError::Invalid(format!(
                        "check '{check_id}' is a {} check; only TTL checks accept updates",
                        c.def.kind.type_name()
                    )));
                };
                c.ttl_deadline = Some(Instant::now() + ttl);
                c.set(status, probe::truncate(output));
                c.state.duration_ms = 0;
                let _ = self.inner.publish.send(sid.clone());
                return Ok(());
            }
        }
        Err(RegistryError::NotFound(format!("check '{check_id}'")))
    }

    /// Rewrite every health record so none expires.
    pub(crate) fn refresh_all(&self) {
        for sid in self.inner.services.lock().unwrap().keys() {
            let _ = self.inner.publish.send(sid.clone());
        }
    }

    /// Number of checks being run.
    pub(crate) fn check_count(&self) -> usize {
        self.inner.services.lock().unwrap().values().map(|s| s.checks.len()).sum()
    }
}

impl Inner {
    fn record(&self, service_id: &str, check_id: &str, outcome: Outcome) {
        let mut services = self.services.lock().unwrap();
        let Some(c) = services.get_mut(service_id).and_then(|s| s.checks.get_mut(check_id)) else { return };
        let changed = c.record(outcome);
        if changed {
            tracing::info!(service = service_id, check = check_id, status = %c.state.status, output = %c.state.output, "check changed state");
            let _ = self.publish.send(service_id.to_string());
        }
        if c.wants_deregister() {
            let _ = self.deregister.send(service_id.to_string());
        }
    }

    /// The current TTL deadline, `None` if expired, or `Err(())` if the check is gone.
    fn ttl_deadline(&self, service_id: &str, check_id: &str) -> std::result::Result<Option<Instant>, ()> {
        let services = self.services.lock().unwrap();
        let c = services.get(service_id).and_then(|s| s.checks.get(check_id)).ok_or(())?;
        Ok(c.ttl_deadline)
    }

    fn ttl_tick(&self, service_id: &str, check_id: &str, ttl: Duration) {
        let mut services = self.services.lock().unwrap();
        let Some(c) = services.get_mut(service_id).and_then(|s| s.checks.get_mut(check_id)) else { return };
        if c.ttl_deadline.is_some_and(|d| d <= Instant::now()) {
            c.ttl_deadline = None;
            if c.set(HealthStatus::Critical, format!("TTL expired (no update within {ttl:?})")) {
                tracing::info!(service = service_id, check = check_id, "TTL check expired");
                let _ = self.publish.send(service_id.to_string());
            }
        }
        if c.wants_deregister() {
            let _ = self.deregister.send(service_id.to_string());
        }
    }

    fn snapshot(&self, service_id: &str) -> Option<(String, HealthRecord)> {
        let services = self.services.lock().unwrap();
        let s = services.get(service_id)?;
        let checks: BTreeMap<String, CheckState> = s.checks.iter().map(|(k, c)| (k.clone(), c.state.clone())).collect();
        Some((
            s.key.clone(),
            HealthRecord {
                schema_version: SCHEMA_VERSION,
                node_id: self.owner.node_id.clone(),
                generation: self.owner.generation.clone(),
                service_id: service_id.to_string(),
                status: HealthStatus::aggregate(checks.values().map(|c| c.status)),
                checks,
            },
        ))
    }
}

async fn publisher(inner: std::sync::Weak<Inner>, mut rx: mpsc::UnboundedReceiver<String>, cancel: CancellationToken) {
    loop {
        let first = tokio::select! {
            _ = cancel.cancelled() => return,
            id = rx.recv() => match id { Some(id) => id, None => return },
        };
        // Coalesce queued requests: each service is written once, with its latest state.
        let mut ids = vec![first];
        while let Ok(id) = rx.try_recv() {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        let Some(inner) = inner.upgrade() else { return };
        for id in ids {
            let Some((key, record)) = inner.snapshot(&id) else { continue };
            let value = Bytes::from(serde_json::to_vec(&record).expect("serialize health"));
            match inner.bucket.put(&key, value.clone()).await {
                Ok(revision) => inner
                    .catalog
                    .apply(Kind::Health, WatchEvent::Put(Entry { key, value, revision, timestamp: SystemTime::now() })),
                Err(e) => tracing::warn!(service = %id, error = %e, "cannot publish health; will retry on refresh"),
            }
        }
    }
}

async fn delete_record(inner: &Inner, key: &str) {
    match inner.bucket.delete(key, None).await {
        Ok(()) => inner.catalog.forget(Kind::Health, key),
        Err(e) => tracing::warn!(key, error = %e, "cannot delete health record; it will expire"),
    }
}

async fn runner(
    inner: std::sync::Weak<Inner>,
    service_id: String,
    check_id: String,
    def: CheckDefinition,
    cancel: CancellationToken,
) {
    let (interval, timeout, permits) = match inner.upgrade() {
        Some(i) => {
            (def.interval.unwrap_or(i.defaults.interval), def.timeout.unwrap_or(i.defaults.timeout), i.permits.clone())
        }
        None => return,
    };
    loop {
        let outcome = tokio::select! {
            _ = cancel.cancelled() => return,
            o = async {
                let _permit = permits.acquire().await;
                match &def.kind {
                    CheckKind::Tcp { address } => probe::tcp(address, timeout).await,
                    CheckKind::Http { url, method, headers, tls_skip_verify } => {
                        probe::http(url, method, headers, *tls_skip_verify, timeout).await
                    }
                    CheckKind::Ttl { .. } => unreachable!("TTL checks have no runner"),
                }
            } => o,
        };
        match inner.upgrade() {
            Some(i) => i.record(&service_id, &check_id, outcome),
            None => return,
        }
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tokio::time::sleep(interval) => {}
        }
    }
}

async fn ttl_watcher(
    inner: std::sync::Weak<Inner>,
    service_id: String,
    check_id: String,
    ttl: Duration,
    cancel: CancellationToken,
) {
    loop {
        let deadline = match inner.upgrade().map(|i| i.ttl_deadline(&service_id, &check_id)) {
            Some(Ok(d)) => d,
            _ => return,
        };
        // When expired, poll at the TTL so deregistration and new reports are noticed.
        let wake = deadline.unwrap_or_else(|| Instant::now() + ttl.min(Duration::from_secs(5)));
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tokio::time::sleep_until(wake) => {}
        }
        match inner.upgrade() {
            Some(i) => i.ttl_tick(&service_id, &check_id, ttl),
            None => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(status: HealthStatus) -> Outcome {
        Outcome { status, output: status.to_string(), duration: Duration::from_millis(3) }
    }

    fn tcp_check() -> CheckDefinition {
        let mut d = CheckDefinition::new(CheckKind::Tcp { address: "127.0.0.1:1".into() });
        d.id = "c".into();
        d
    }

    #[test]
    fn thresholds() {
        let mut d = tcp_check();
        d.failures_before_critical = 3;
        d.successes_before_passing = 2;
        let mut c = Check::new(d);
        assert_eq!(c.state.status, HealthStatus::Critical, "initial status");
        assert!(!c.record(outcome(HealthStatus::Passing)), "one success is not enough");
        assert!(c.record(outcome(HealthStatus::Passing)));
        assert_eq!(c.state.status, HealthStatus::Passing);
        assert_eq!(c.state.duration_ms, 3);
        assert!(!c.record(outcome(HealthStatus::Critical)));
        assert!(!c.record(outcome(HealthStatus::Critical)));
        assert!(!c.record(outcome(HealthStatus::Passing)), "a success resets the failure streak");
        for _ in 0..2 {
            c.record(outcome(HealthStatus::Critical));
        }
        assert_eq!(c.state.status, HealthStatus::Passing);
        assert!(c.record(outcome(HealthStatus::Critical)));
        assert_eq!(c.state.status, HealthStatus::Critical);
    }

    #[test]
    fn warning_counts_as_failure() {
        let mut d = tcp_check();
        d.initial_status = HealthStatus::Passing;
        let mut c = Check::new(d);
        assert!(c.record(outcome(HealthStatus::Warning)));
        assert_eq!(c.state.status, HealthStatus::Warning);
    }

    #[tokio::test(start_paused = true)]
    async fn deregister_after_critical_period() {
        let mut d = tcp_check();
        d.deregister_critical_after = Some(Duration::from_secs(60));
        let mut c = Check::new(d);
        assert!(!c.wants_deregister());
        tokio::time::advance(Duration::from_secs(61)).await;
        assert!(c.wants_deregister());
        assert!(!c.wants_deregister(), "requested once");
        c.record(outcome(HealthStatus::Passing));
        assert!(c.critical_since.is_none());
        c.record(outcome(HealthStatus::Critical));
        assert!(!c.wants_deregister(), "the clock restarts");
    }
}
