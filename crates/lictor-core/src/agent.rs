//! The agent: one per node.
//!
//! On start it loads the catalog, claims the node's membership under a fresh
//! *generation*, reconciles registrations left by its previous run, and then:
//!
//! - heartbeats the membership record with a compare-and-set on its revision;
//!   losing that race to a record of another generation means a newer agent took
//!   over this node, and this agent stops (it is *fenced*);
//! - runs health checks and publishes health;
//! - deletes registrations whose node has been gone longer than the retention period.

use crate::catalog::{Catalog, Kind};
use crate::config::{AgentConfig, ReconcilePolicy};
use crate::health::{Defaults, HealthEngine, Owner};
use crate::registry::{Registry, RegistryEventStream};
use crate::sync::{Buckets, start_catalog};
use crate::{RegistryError, Result};
use async_trait::async_trait;
use bytes::Bytes;
use lictor_store::{Entry, Store, StoreError, WatchEvent};
use lictor_types::{
    AgentStatus, CheckDefinition, CheckView, DiscoveryQuery, HealthCheck, HealthUpdate, NodeInfo, NodeRecord,
    RegistrationReceipt, RegistryScope, SCHEMA_VERSION, ServiceInstance, ServiceRecord, ServiceRegistration,
    ServiceSummary, keys,
};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, SystemTime};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

const WRITE_ATTEMPTS: usize = 5;

#[derive(Clone)]
struct Local {
    record: ServiceRecord,
    revision: u64,
}

struct Inner {
    cfg: AgentConfig,
    store: Arc<dyn Store>,
    buckets: Buckets,
    catalog: Arc<Catalog>,
    generation: String,
    node_key: String,
    node_revision: Mutex<Option<u64>>,
    local: Mutex<HashMap<String, Local>>,
    /// Serializes mutating operations.
    ops: tokio::sync::Mutex<()>,
    health: HealthEngine,
    cancel: CancellationToken,
    fenced: AtomicBool,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// A running agent. Cloning shares it.
#[derive(Clone)]
pub struct Agent {
    inner: Arc<Inner>,
}

fn new_generation() -> String {
    format!("{:016x}", rand::random::<u64>())
}

fn json<T: serde::Serialize>(v: &T) -> Bytes {
    Bytes::from(serde_json::to_vec(v).expect("records serialize"))
}

impl Agent {
    /// Connect to the registry in `store` and start the agent.
    pub async fn start(store: Arc<dyn Store>, cfg: AgentConfig) -> Result<Agent> {
        cfg.validate().map_err(RegistryError::Invalid)?;
        let cancel = CancellationToken::new();
        let buckets = Buckets::open(&*store, &cfg.registry).await?;
        if let Some(ttl) = buckets.nodes.ttl()
            && cfg.heartbeat_interval >= ttl
        {
            return Err(RegistryError::Invalid(format!(
                "heartbeat interval {:?} is not shorter than the existing nodes bucket TTL {ttl:?}",
                cfg.heartbeat_interval
            )));
        }
        if let Some(ttl) = buckets.health.ttl()
            && cfg.health_refresh() >= ttl
        {
            return Err(RegistryError::Invalid(format!(
                "health refresh {:?} is not shorter than the existing health bucket TTL {ttl:?}",
                cfg.health_refresh()
            )));
        }
        let (catalog, tasks) = start_catalog(&buckets, &cfg.registry, &cancel).await?;
        let generation = new_generation();
        let (health, deregister_rx) = HealthEngine::start(
            Owner { node_id: cfg.node_id.clone(), generation: generation.clone() },
            buckets.health.clone(),
            catalog.clone(),
            Defaults {
                interval: cfg.check_interval,
                timeout: cfg.check_timeout,
                max_concurrent: cfg.max_concurrent_checks,
            },
            cancel.child_token(),
        );
        let inner = Arc::new(Inner {
            node_key: keys::node(&cfg.namespace, &cfg.region, &cfg.node_id),
            cfg,
            store,
            buckets,
            catalog,
            generation,
            node_revision: Mutex::new(None),
            local: Mutex::new(HashMap::new()),
            ops: tokio::sync::Mutex::new(()),
            health,
            cancel,
            fenced: AtomicBool::new(false),
            tasks: Mutex::new(tasks),
        });
        if let Err(e) = inner.claim_membership().await {
            inner.cancel.cancel();
            return Err(e);
        }
        inner.reconcile().await;
        let weak = Arc::downgrade(&inner);
        let mut tasks = vec![
            tokio::spawn(heartbeat_loop(weak.clone())),
            tokio::spawn(refresh_loop(weak.clone())),
            tokio::spawn(deregister_loop(weak.clone(), deregister_rx)),
        ];
        if let Some(retention) = inner.cfg.stale_retention {
            tasks.push(tokio::spawn(gc_loop(weak, retention)));
        }
        inner.tasks.lock().unwrap().extend(tasks);
        tracing::info!(
            node = %inner.cfg.node_id, namespace = %inner.cfg.namespace, region = %inner.cfg.region,
            generation = %inner.generation, backend = inner.store.backend(), "agent started"
        );
        Ok(Agent { inner })
    }

    /// This run's generation.
    pub fn generation(&self) -> &str {
        &self.inner.generation
    }

    /// The configuration the agent runs with.
    pub fn config(&self) -> &AgentConfig {
        &self.inner.cfg
    }

    /// The local catalog (for adapters that need direct access, e.g. change notifications).
    pub fn catalog(&self) -> &Arc<Catalog> {
        &self.inner.catalog
    }

    /// Whether a newer agent took over this node.
    pub fn is_fenced(&self) -> bool {
        self.inner.fenced.load(Ordering::SeqCst)
    }

    /// Resolves when the agent stops: after [`Agent::shutdown`] or when it is fenced.
    pub async fn stopped(&self) {
        self.inner.cancel.cancelled().await;
    }

    /// Stop the agent. With `leave`, delete the membership record so this node's
    /// services stop being discoverable immediately rather than when the lease runs out.
    /// Registrations are kept and adopted by the next run.
    pub async fn shutdown(&self, leave: bool) {
        let inner = &self.inner;
        if leave && !self.is_fenced() {
            let rev = *inner.node_revision.lock().unwrap();
            match inner.buckets.nodes.delete(&inner.node_key, rev).await {
                Ok(()) => tracing::info!("left the registry"),
                Err(e) => tracing::warn!(error = %e, "could not remove membership; it will expire"),
            }
        }
        inner.cancel.cancel();
        let tasks: Vec<_> = inner.tasks.lock().unwrap().drain(..).collect();
        for t in tasks {
            let _ = t.await;
        }
    }
}

impl Inner {
    fn node_record(&self) -> NodeRecord {
        NodeRecord {
            schema_version: SCHEMA_VERSION,
            node_id: self.cfg.node_id.clone(),
            generation: self.generation.clone(),
            namespace: self.cfg.namespace.clone(),
            region: self.cfg.region.clone(),
            hostname: self.cfg.hostname.clone(),
            addresses: self.cfg.addresses.clone(),
            metadata: self.cfg.metadata.clone(),
            agent_version: self.cfg.agent_version.clone(),
        }
    }

    fn write_through(&self, kind: Kind, key: &str, value: Bytes, revision: u64) {
        self.catalog
            .apply(kind, WatchEvent::Put(Entry { key: key.into(), value, revision, timestamp: SystemTime::now() }));
    }

    fn check_fenced(&self) -> Result<()> {
        if self.fenced.load(Ordering::SeqCst) { Err(RegistryError::Fenced(self.cfg.node_id.clone())) } else { Ok(()) }
    }

    /// Write the membership record at startup, taking over from any previous generation.
    async fn claim_membership(&self) -> Result<()> {
        let value = json(&self.node_record());
        let nodes = &self.buckets.nodes;
        for _ in 0..WRITE_ATTEMPTS {
            let result = match nodes.get(&self.node_key).await? {
                Some(existing) => {
                    if let Ok(prev) = serde_json::from_slice::<NodeRecord>(&existing.value) {
                        tracing::info!(previous = %prev.generation, "taking over node membership from a previous generation");
                    }
                    nodes.update(&self.node_key, value.clone(), existing.revision).await
                }
                None => nodes.create(&self.node_key, value.clone()).await,
            };
            match result {
                Ok(rev) => {
                    *self.node_revision.lock().unwrap() = Some(rev);
                    self.write_through(Kind::Nodes, &self.node_key, value, rev);
                    return Ok(());
                }
                Err(StoreError::Conflict { .. }) => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Err(RegistryError::Conflict(format!("could not claim membership of node '{}'", self.cfg.node_id)))
    }

    /// Refresh membership. Returns false when fenced.
    async fn heartbeat(&self) -> bool {
        let value = json(&self.node_record());
        let nodes = &self.buckets.nodes;
        let rev = *self.node_revision.lock().unwrap();
        let result = match rev {
            Some(r) => nodes.update(&self.node_key, value.clone(), r).await,
            None => nodes.create(&self.node_key, value.clone()).await,
        };
        let result = match result {
            Err(StoreError::Conflict { .. }) => match nodes.get(&self.node_key).await {
                Ok(None) => {
                    tracing::warn!("membership lease had expired; re-creating it");
                    nodes.create(&self.node_key, value.clone()).await
                }
                Ok(Some(e)) => match serde_json::from_slice::<NodeRecord>(&e.value) {
                    Ok(r) if r.generation == self.generation => {
                        nodes.update(&self.node_key, value.clone(), e.revision).await
                    }
                    Ok(r) => {
                        tracing::error!(
                            node = %self.cfg.node_id, ours = %self.generation, theirs = %r.generation,
                            "another agent took over this node; stopping (fenced)"
                        );
                        return false;
                    }
                    Err(_) => nodes.update(&self.node_key, value.clone(), e.revision).await,
                },
                Err(e) => Err(e),
            },
            other => other,
        };
        match result {
            Ok(rev) => {
                *self.node_revision.lock().unwrap() = Some(rev);
                self.write_through(Kind::Nodes, &self.node_key, value, rev);
            }
            Err(e) => tracing::warn!(error = %e, "heartbeat failed; retrying"),
        }
        true
    }

    fn fence(&self) {
        self.fenced.store(true, Ordering::SeqCst);
        self.cancel.cancel();
    }

    /// Handle registrations of this node left by previous generations.
    async fn reconcile(&self) {
        let (ns, region, node) = (&self.cfg.namespace, &self.cfg.region, &self.cfg.node_id);
        let stale = self.catalog.services_where(|r| {
            &r.namespace == ns && &r.region == region && &r.node_id == node && r.generation != self.generation
        });
        for (mut record, rev) in stale {
            let key = record.key();
            match self.cfg.reconcile {
                ReconcilePolicy::Adopt => {
                    record.generation = self.generation.clone();
                    let value = json(&record);
                    match self.buckets.services.update(&key, value.clone(), rev).await {
                        Ok(new_rev) => {
                            tracing::info!(service = %record.service_id, "adopted registration from previous run");
                            self.write_through(Kind::Services, &key, value, new_rev);
                            if !record.checks.is_empty() {
                                self.health.set_service(&record);
                            }
                            self.local
                                .lock()
                                .unwrap()
                                .insert(record.service_id.clone(), Local { record, revision: new_rev });
                        }
                        Err(e) => {
                            tracing::warn!(service = %record.service_id, error = %e, "could not adopt registration")
                        }
                    }
                }
                ReconcilePolicy::Drop => match self.buckets.services.delete(&key, Some(rev)).await {
                    Ok(()) => {
                        tracing::info!(service = %record.service_id, "dropped registration from previous run");
                        self.catalog.forget(Kind::Services, &key);
                        let _ = self.buckets.health.delete(&key, None).await;
                    }
                    Err(e) => tracing::warn!(service = %record.service_id, error = %e, "could not drop registration"),
                },
                ReconcilePolicy::Keep => {
                    tracing::info!(service = %record.service_id, "registration from previous run left inactive");
                }
            }
        }
    }

    /// Fill defaults and build the stored record for a registration.
    fn build_record(&self, reg: ServiceRegistration, local: &HashMap<String, Local>) -> Result<ServiceRecord> {
        reg.validate().map_err(RegistryError::Invalid)?;
        let id = reg.effective_id().to_string();
        let mut checks = reg.checks;
        let total = checks.len();
        let mut taken: HashSet<String> = checks.iter().filter(|c| !c.id.is_empty()).map(|c| c.id.clone()).collect();
        for (n, c) in checks.iter_mut().enumerate() {
            if c.id.is_empty() {
                let mut candidate =
                    if total == 1 { format!("service:{id}") } else { format!("service:{id}:{}", n + 1) };
                let mut k = n + 1;
                while taken.contains(&candidate) {
                    k += 1;
                    candidate = format!("service:{id}:{k}");
                }
                taken.insert(candidate.clone());
                c.id = candidate;
            }
            if c.name.is_empty() {
                c.name = format!("Service '{}' check", reg.name);
            }
        }
        for c in &checks {
            if let Some(other) =
                local.values().find(|l| l.record.service_id != id && l.record.checks.iter().any(|x| x.id == c.id))
            {
                return Err(RegistryError::Invalid(format!(
                    "check id '{}' is already used by service '{}'",
                    c.id, other.record.service_id
                )));
            }
        }
        Ok(ServiceRecord {
            schema_version: SCHEMA_VERSION,
            service_id: id,
            service_name: reg.name,
            node_id: self.cfg.node_id.clone(),
            generation: self.generation.clone(),
            namespace: self.cfg.namespace.clone(),
            region: self.cfg.region.clone(),
            address: reg.address.unwrap_or_else(|| self.cfg.addresses.first().cloned().unwrap_or_default()),
            port: reg.port,
            protocol: reg.protocol.unwrap_or_else(|| "tcp".into()),
            tags: reg.tags,
            metadata: reg.metadata,
            checks,
        })
    }

    /// Persist a record, creating or replacing whatever is at its key.
    async fn write_record(&self, record: &ServiceRecord, known: Option<u64>) -> Result<u64> {
        let key = record.key();
        let value = json(record);
        let services = &self.buckets.services;
        let mut expected = known;
        for _ in 0..WRITE_ATTEMPTS {
            let result = match expected {
                Some(rev) => services.update(&key, value.clone(), rev).await,
                None => match services.get(&key).await? {
                    Some(e) => services.update(&key, value.clone(), e.revision).await,
                    None => services.create(&key, value.clone()).await,
                },
            };
            match result {
                Ok(rev) => {
                    self.write_through(Kind::Services, &key, value, rev);
                    return Ok(rev);
                }
                Err(StoreError::Conflict { .. }) => expected = None,
                Err(e) => return Err(e.into()),
            }
        }
        Err(RegistryError::Conflict(format!("registration '{key}' kept changing")))
    }

    async fn delete_record(&self, record: &ServiceRecord, revision: u64) -> Result<()> {
        let key = record.key();
        match self.buckets.services.delete(&key, Some(revision)).await {
            Ok(()) => {}
            Err(StoreError::Conflict { .. }) => {
                // Changed since we wrote it: delete only if it is still ours.
                if let Some(e) = self.buckets.services.get(&key).await?
                    && serde_json::from_slice::<ServiceRecord>(&e.value).is_ok_and(|r| r.generation == self.generation)
                {
                    self.buckets.services.delete(&key, Some(e.revision)).await?;
                }
            }
            Err(e) => return Err(e.into()),
        }
        self.catalog.forget(Kind::Services, &key);
        Ok(())
    }

    async fn register_locked(&self, reg: ServiceRegistration) -> Result<RegistrationReceipt> {
        self.check_fenced()?;
        let (record, previous) = {
            let local = self.local.lock().unwrap();
            let record = self.build_record(reg, &local)?;
            let previous = local.get(&record.service_id).cloned();
            (record, previous)
        };
        let key = record.key();
        let mut known = None;
        if let Some(prev) = &previous {
            if prev.record.key() == key {
                known = Some(prev.revision);
            } else {
                // Renamed: the old key goes away.
                self.delete_record(&prev.record, prev.revision).await?;
                self.health.remove_service(&prev.record.service_id, &prev.record.key()).await;
            }
        }
        let revision = self.write_record(&record, known).await?;
        if record.checks.is_empty() {
            if previous.as_ref().is_some_and(|p| !p.record.checks.is_empty()) {
                self.health.remove_service(&record.service_id, &key).await;
            }
        } else {
            self.health.set_service(&record);
        }
        let receipt = RegistrationReceipt {
            service_id: record.service_id.clone(),
            service_name: record.service_name.clone(),
            key,
            revision,
            generation: self.generation.clone(),
            checks: record.checks.iter().map(|c| c.id.clone()).collect(),
        };
        tracing::info!(service = %record.service_id, name = %record.service_name, revision, "registered");
        self.local.lock().unwrap().insert(record.service_id.clone(), Local { record, revision });
        Ok(receipt)
    }

    async fn deregister_locked(&self, service_id: &str) -> Result<()> {
        self.check_fenced()?;
        let local = self
            .local
            .lock()
            .unwrap()
            .get(service_id)
            .cloned()
            .ok_or_else(|| RegistryError::NotFound(format!("service '{service_id}'")))?;
        self.delete_record(&local.record, local.revision).await?;
        self.health.remove_service(service_id, &local.record.key()).await;
        self.local.lock().unwrap().remove(service_id);
        tracing::info!(service = service_id, "deregistered");
        Ok(())
    }

    fn registration_of(record: &ServiceRecord) -> ServiceRegistration {
        ServiceRegistration {
            id: Some(record.service_id.clone()),
            name: record.service_name.clone(),
            address: Some(record.address.clone()),
            port: record.port,
            protocol: Some(record.protocol.clone()),
            tags: record.tags.clone(),
            metadata: record.metadata.clone(),
            checks: record.checks.clone(),
        }
    }

    fn view(&self, l: &Local) -> ServiceInstance {
        self.catalog.local_view(&l.record.key()).unwrap_or_else(|| ServiceInstance {
            namespace: l.record.namespace.clone(),
            region: l.record.region.clone(),
            service_id: l.record.service_id.clone(),
            service_name: l.record.service_name.clone(),
            address: l.record.address.clone(),
            port: l.record.port,
            protocol: l.record.protocol.clone(),
            tags: l.record.tags.clone(),
            metadata: l.record.metadata.clone(),
            status: lictor_types::HealthStatus::Unknown,
            checks: vec![],
            node_id: l.record.node_id.clone(),
            node_hostname: self.cfg.hostname.clone(),
            node_addresses: self.cfg.addresses.clone(),
            node_metadata: self.cfg.metadata.clone(),
            generation: l.record.generation.clone(),
            revision: l.revision,
        })
    }
}

async fn heartbeat_loop(inner: Weak<Inner>) {
    let (interval, cancel) = match inner.upgrade() {
        Some(i) => (i.cfg.heartbeat_interval, i.cancel.clone()),
        None => return,
    };
    let mut tick = tokio::time::interval_at(Instant::now() + interval, interval);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tick.tick() => {}
        }
        let Some(i) = inner.upgrade() else { return };
        if !i.heartbeat().await {
            i.fence();
            return;
        }
    }
}

async fn refresh_loop(inner: Weak<Inner>) {
    let (every, cancel) = match inner.upgrade() {
        Some(i) => (i.cfg.health_refresh(), i.cancel.clone()),
        None => return,
    };
    let mut tick = tokio::time::interval_at(Instant::now() + every, every);
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tick.tick() => {}
        }
        match inner.upgrade() {
            Some(i) => i.health.refresh_all(),
            None => return,
        }
    }
}

async fn deregister_loop(inner: Weak<Inner>, mut rx: tokio::sync::mpsc::UnboundedReceiver<String>) {
    let Some(cancel) = inner.upgrade().map(|i| i.cancel.clone()) else { return };
    loop {
        let id = tokio::select! {
            _ = cancel.cancelled() => return,
            id = rx.recv() => match id { Some(id) => id, None => return },
        };
        let Some(i) = inner.upgrade() else { return };
        let _guard = i.ops.lock().await;
        tracing::warn!(service = %id, "deregistering service: check critical for too long");
        if let Err(e) = i.deregister_locked(&id).await {
            tracing::warn!(service = %id, error = %e, "automatic deregistration failed");
        }
    }
}

async fn gc_loop(inner: Weak<Inner>, retention: Duration) {
    let Some(cancel) = inner.upgrade().map(|i| i.cancel.clone()) else { return };
    let every = (retention / 10).clamp(Duration::from_secs(1), Duration::from_secs(300));
    let mut orphaned: HashMap<String, Instant> = HashMap::new();
    let mut tick = tokio::time::interval(every);
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tick.tick() => {}
        }
        let Some(i) = inner.upgrade() else { return };
        let now = Instant::now();
        let candidates = i.catalog.services_where(|r| !i.catalog.node_alive(&r.namespace, &r.region, &r.node_id));
        let current: HashSet<String> = candidates.iter().map(|(r, _)| r.key()).collect();
        orphaned.retain(|k, _| current.contains(k));
        for (record, rev) in candidates {
            let key = record.key();
            let since = *orphaned.entry(key.clone()).or_insert(now);
            if now.duration_since(since) >= retention {
                match i.buckets.services.delete(&key, Some(rev)).await {
                    Ok(()) => {
                        tracing::info!(key, "removed registration of a node gone for longer than the retention period");
                        i.catalog.forget(Kind::Services, &key);
                        orphaned.remove(&key);
                    }
                    Err(StoreError::Conflict { .. }) => {
                        orphaned.remove(&key);
                    }
                    Err(e) => tracing::debug!(key, error = %e, "stale registration cleanup failed"),
                }
            }
        }
    }
}

pub(crate) fn watch_query(catalog: Arc<Catalog>, query: DiscoveryQuery) -> RegistryEventStream {
    use futures::StreamExt;
    let rx = catalog.subscribe();
    futures::stream::unfold(
        (catalog, query, rx, None::<Vec<ServiceInstance>>),
        |(catalog, query, mut rx, last)| async move {
            loop {
                rx.borrow_and_update();
                let current = catalog.discover(&query);
                if last.as_ref() != Some(&current) {
                    let event = lictor_types::RegistryEvent { index: catalog.index(), instances: current.clone() };
                    return Some((event, (catalog, query, rx, Some(current))));
                }
                if rx.changed().await.is_err() {
                    return None;
                }
            }
        },
    )
    .boxed()
}

#[async_trait]
impl Registry for Agent {
    async fn register(&self, service: ServiceRegistration) -> Result<RegistrationReceipt> {
        let _guard = self.inner.ops.lock().await;
        self.inner.register_locked(service).await
    }

    async fn deregister(&self, service_id: &str) -> Result<()> {
        let _guard = self.inner.ops.lock().await;
        self.inner.deregister_locked(service_id).await
    }

    async fn discover(&self, query: DiscoveryQuery) -> Result<Vec<ServiceInstance>> {
        Ok(self.inner.catalog.discover(&query))
    }

    async fn get_service(&self, service_id: &str) -> Result<Option<ServiceInstance>> {
        let local = self.inner.local.lock().unwrap().get(service_id).cloned();
        Ok(local.map(|l| self.inner.view(&l)))
    }

    async fn local_services(&self) -> Result<Vec<ServiceInstance>> {
        let local: Vec<Local> = self.inner.local.lock().unwrap().values().cloned().collect();
        let mut out: Vec<ServiceInstance> = local.iter().map(|l| self.inner.view(l)).collect();
        out.sort_by(|a, b| a.service_id.cmp(&b.service_id));
        Ok(out)
    }

    async fn local_checks(&self) -> Result<Vec<CheckView>> {
        Ok(self.local_services().await?.into_iter().flat_map(|s| s.checks).collect())
    }

    async fn list_services(&self, scope: RegistryScope) -> Result<Vec<ServiceSummary>> {
        Ok(self.inner.catalog.list_services(&scope))
    }

    async fn list_nodes(&self, scope: RegistryScope) -> Result<Vec<NodeInfo>> {
        Ok(self.inner.catalog.list_nodes(&scope))
    }

    async fn register_check(&self, check: HealthCheck) -> Result<String> {
        let _guard = self.inner.ops.lock().await;
        let record = self
            .inner
            .local
            .lock()
            .unwrap()
            .get(&check.service_id)
            .map(|l| l.record.clone())
            .ok_or_else(|| RegistryError::NotFound(format!("service '{}'", check.service_id)))?;
        let mut def: CheckDefinition = check.check;
        if def.id.is_empty() {
            let base = format!("service:{}", record.service_id);
            let used = |id: &str| record.checks.iter().any(|c| c.id == id);
            def.id = base.clone();
            let mut n = record.checks.len() + 1;
            while used(&def.id) {
                def.id = format!("{base}:{n}");
                n += 1;
            }
        }
        if record.checks.iter().any(|c| c.id == def.id) {
            return Err(RegistryError::Invalid(format!(
                "service '{}' already has check '{}'",
                record.service_id, def.id
            )));
        }
        let id = def.id.clone();
        let mut reg = Inner::registration_of(&record);
        reg.checks.push(def);
        self.inner.register_locked(reg).await?;
        Ok(id)
    }

    async fn deregister_check(&self, check_id: &str) -> Result<()> {
        let _guard = self.inner.ops.lock().await;
        let record = self
            .inner
            .local
            .lock()
            .unwrap()
            .values()
            .find(|l| l.record.checks.iter().any(|c| c.id == check_id))
            .map(|l| l.record.clone())
            .ok_or_else(|| RegistryError::NotFound(format!("check '{check_id}'")))?;
        let mut reg = Inner::registration_of(&record);
        reg.checks.retain(|c| c.id != check_id);
        self.inner.register_locked(reg).await.map(|_| ())
    }

    async fn update_health(&self, update: HealthUpdate) -> Result<()> {
        self.inner.check_fenced()?;
        self.inner.health.report(&update.check_id, update.status, update.output)
    }

    async fn watch(&self, query: DiscoveryQuery) -> Result<RegistryEventStream> {
        Ok(watch_query(self.inner.catalog.clone(), query))
    }

    async fn status(&self) -> Result<AgentStatus> {
        let i = &self.inner;
        let scope = RegistryScope { namespace: Some(i.cfg.namespace.clone()), region: Some(i.cfg.region.clone()) };
        let node = i.catalog.list_nodes(&scope).into_iter().find(|n| n.node_id == i.cfg.node_id);
        let (catalog_nodes, catalog_services) = i.catalog.counts();
        Ok(AgentStatus {
            version: crate::VERSION.into(),
            backend: i.store.backend().into(),
            connected: i.store.is_connected() && i.catalog.is_syncing(),
            catalog_ready: i.catalog.is_ready(),
            node,
            local_services: i.local.lock().unwrap().len(),
            local_checks: i.health.check_count(),
            catalog_nodes,
            catalog_services,
            catalog_index: i.catalog.index(),
        })
    }
}
