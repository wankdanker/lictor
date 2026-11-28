//! The local catalog: an in-memory materialized view of the three buckets.
//!
//! Every discovery query is answered from here without network or disk I/O.
//! Watch events from the store are applied with [`Catalog::apply`]. The catalog
//! tolerates duplicate, replayed and reordered events (revisions only move
//! forward per key), and makes no assumption that the buckets agree with each
//! other: an instance is returned only when its records are consistent.
//!
//! # Eligibility
//!
//! A service instance is *live* when:
//! 1. its registration exists,
//! 2. its node's membership lease is unexpired, and
//! 3. the registration's generation equals the node's current generation.
//!
//! Its health is the worst state among its checks, taken from a health record
//! that is unexpired and of the same generation. A service with checks but no
//! current health record is `unknown`. A service without checks is `passing`.

use crate::lease;
use lictor_store::WatchEvent;
use lictor_types::{
    CheckView, DiscoveryQuery, HealthRecord, HealthStatus, NodeInfo, NodeRecord, RegistryScope, SCHEMA_VERSION,
    ServiceInstance, ServiceRecord, ServiceSummary, keys,
};
use serde::de::DeserializeOwned;
use std::collections::{BTreeSet, HashMap};
use std::sync::RwLock;
use std::time::Duration;
use tokio::sync::watch;
use tokio::time::Instant;

/// Which bucket an event belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Nodes,
    Services,
    Health,
}

struct Slot<T> {
    value: T,
    revision: u64,
    deadline: Option<Instant>,
    epoch: u64,
}

struct Table<T> {
    slots: HashMap<String, Slot<T>>,
    /// Highest revision seen per key, including deletes; older events are ignored.
    last: HashMap<String, u64>,
    epoch: u64,
    ready: bool,
    syncing: bool,
}

impl<T> Default for Table<T> {
    fn default() -> Self {
        Table { slots: HashMap::new(), last: HashMap::new(), epoch: 0, ready: false, syncing: false }
    }
}

impl<T: PartialEq> Table<T> {
    /// Returns true if the visible state changed. Rewriting the same value (a heartbeat or a
    /// health refresh) only renews the lease: it is not a change, so watchers and blocking
    /// queries are not woken by every heartbeat in the cluster.
    fn put(&mut self, key: &str, value: T, revision: u64, deadline: Option<Instant>) -> bool {
        if let Some(&last) = self.last.get(key) {
            if revision < last {
                return false;
            }
            if revision == last {
                // A redelivery (e.g. after a resync): only mark it as seen in this epoch.
                if let Some(s) = self.slots.get_mut(key) {
                    s.epoch = self.epoch;
                }
                return false;
            }
        }
        self.last.insert(key.to_string(), revision);
        let changed = match self.slots.get(key) {
            // A lease that had run out (not yet swept) is invisible, so renewing it is a change.
            Some(old) => old.value != value || old.deadline.is_some_and(|d| d <= Instant::now()),
            None => true,
        };
        self.slots.insert(key.to_string(), Slot { value, revision, deadline, epoch: self.epoch });
        changed
    }
}

impl<T> Table<T> {
    fn delete(&mut self, key: &str, revision: u64) -> bool {
        if self.last.get(key).is_some_and(|&last| last >= revision) {
            return false;
        }
        self.last.insert(key.to_string(), revision);
        self.slots.remove(key).is_some()
    }

    /// Drop entries not seen since the current epoch began. Returns the removed keys.
    fn sweep(&mut self) -> Vec<String> {
        let epoch = self.epoch;
        let gone: Vec<String> = self.slots.iter().filter(|(_, s)| s.epoch < epoch).map(|(k, _)| k.clone()).collect();
        for k in &gone {
            self.slots.remove(k);
        }
        let slots = &self.slots;
        self.last.retain(|k, _| slots.contains_key(k));
        gone
    }

    /// Drop entries whose lease ran out. Returns the removed keys.
    fn expire(&mut self, now: Instant) -> Vec<String> {
        let gone: Vec<String> =
            self.slots.iter().filter(|(_, s)| s.deadline.is_some_and(|d| d <= now)).map(|(k, _)| k.clone()).collect();
        for k in &gone {
            self.slots.remove(k);
        }
        gone
    }

    fn live(&self, key: &str, now: Instant) -> Option<&Slot<T>> {
        self.slots.get(key).filter(|s| s.deadline.is_none_or(|d| d > now))
    }
}

#[derive(Default)]
struct State {
    nodes: Table<NodeRecord>,
    services: Table<ServiceRecord>,
    health: Table<HealthRecord>,
    /// Service name -> service keys.
    by_name: HashMap<String, BTreeSet<String>>,
    index: u64,
}

/// Lease settings the catalog needs.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Leases {
    pub node_ttl: Duration,
    pub health_ttl: Duration,
    pub skew: Duration,
}

/// The in-memory catalog. Shared between the sync tasks, the agent and adapters.
pub struct Catalog {
    leases: Leases,
    state: RwLock<State>,
    changes: watch::Sender<u64>,
}

fn decode<T: DeserializeOwned>(kind: Kind, key: &str, bytes: &[u8]) -> Option<T> {
    #[derive(serde::Deserialize)]
    struct Version {
        #[serde(default)]
        schema_version: u32,
    }
    match serde_json::from_slice::<Version>(bytes) {
        Ok(v) if v.schema_version > SCHEMA_VERSION => {
            tracing::debug!(?kind, key, version = v.schema_version, "ignoring record with newer schema");
            return None;
        }
        Err(e) => {
            tracing::warn!(?kind, key, error = %e, "ignoring malformed record");
            return None;
        }
        _ => {}
    }
    serde_json::from_slice(bytes).map_err(|e| tracing::warn!(?kind, key, error = %e, "ignoring malformed record")).ok()
}

impl Catalog {
    pub(crate) fn new(leases: Leases) -> Self {
        Catalog { leases, state: RwLock::new(State::default()), changes: watch::channel(0).0 }
    }

    /// Whether all three buckets have completed their initial load.
    pub fn is_ready(&self) -> bool {
        let st = self.state.read().unwrap();
        st.nodes.ready && st.services.ready && st.health.ready
    }

    /// Whether all three buckets currently have a working watch.
    pub fn is_syncing(&self) -> bool {
        let st = self.state.read().unwrap();
        st.nodes.syncing && st.services.syncing && st.health.syncing
    }

    /// Increases on every visible change.
    pub fn index(&self) -> u64 {
        self.state.read().unwrap().index
    }

    /// Notified with the new index on every change.
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.changes.subscribe()
    }

    fn bump(&self, st: &mut State) {
        st.index += 1;
        self.changes.send_replace(st.index);
    }

    /// Start a new sync epoch for a bucket (called before each (re)watch).
    pub(crate) fn begin_epoch(&self, kind: Kind) {
        let mut st = self.state.write().unwrap();
        match kind {
            Kind::Nodes => st.nodes.epoch += 1,
            Kind::Services => st.services.epoch += 1,
            Kind::Health => st.health.epoch += 1,
        }
    }

    pub(crate) fn set_syncing(&self, kind: Kind, syncing: bool) {
        let mut st = self.state.write().unwrap();
        match kind {
            Kind::Nodes => st.nodes.syncing = syncing,
            Kind::Services => st.services.syncing = syncing,
            Kind::Health => st.health.syncing = syncing,
        }
    }

    /// Apply one watch event (or a local write-through) to the catalog.
    pub fn apply(&self, kind: Kind, event: WatchEvent) {
        let mut st = self.state.write().unwrap();
        let changed = match event {
            WatchEvent::Ready => {
                let st = &mut *st;
                let gone = match kind {
                    Kind::Nodes => {
                        st.nodes.ready = true;
                        st.nodes.syncing = true;
                        st.nodes.sweep()
                    }
                    Kind::Services => {
                        st.services.ready = true;
                        st.services.syncing = true;
                        let gone = st.services.sweep();
                        for k in &gone {
                            unindex(&mut st.by_name, k);
                        }
                        gone
                    }
                    Kind::Health => {
                        st.health.ready = true;
                        st.health.syncing = true;
                        st.health.sweep()
                    }
                };
                if !gone.is_empty() {
                    tracing::debug!(?kind, removed = gone.len(), "resync removed entries");
                }
                true
            }
            WatchEvent::Put(e) => {
                let leases = self.leases;
                match kind {
                    Kind::Nodes => {
                        decode::<NodeRecord>(kind, &e.key, &e.value).filter(|r| r.key() == e.key).is_some_and(|r| {
                            let d = lease::deadline(e.timestamp, leases.node_ttl, leases.skew);
                            st.nodes.put(&e.key, r, e.revision, Some(d))
                        })
                    }
                    Kind::Services => {
                        match decode::<ServiceRecord>(kind, &e.key, &e.value).filter(|r| r.key() == e.key) {
                            Some(r) => {
                                let name = r.service_name.clone();
                                let changed = st.services.put(&e.key, r, e.revision, None);
                                if changed {
                                    st.by_name.entry(name).or_default().insert(e.key.clone());
                                }
                                changed
                            }
                            None => false,
                        }
                    }
                    Kind::Health => decode::<HealthRecord>(kind, &e.key, &e.value).is_some_and(|r| {
                        let d = lease::deadline(e.timestamp, leases.health_ttl, leases.skew);
                        st.health.put(&e.key, r, e.revision, Some(d))
                    }),
                }
            }
            WatchEvent::Delete { key, revision, .. } => match kind {
                Kind::Nodes => st.nodes.delete(&key, revision),
                Kind::Services => {
                    let changed = st.services.delete(&key, revision);
                    if changed {
                        unindex(&mut st.by_name, &key);
                    }
                    changed
                }
                Kind::Health => st.health.delete(&key, revision),
            },
        };
        if changed {
            self.bump(&mut st);
        }
    }

    /// Remove a key after a local delete whose revision is unknown. The watch later
    /// delivers the real delete; stale puts stay ignored because the last revision is kept.
    pub(crate) fn forget(&self, kind: Kind, key: &str) {
        let mut st = self.state.write().unwrap();
        let st = &mut *st;
        let removed = match kind {
            Kind::Nodes => st.nodes.slots.remove(key).is_some(),
            Kind::Services => {
                let r = st.services.slots.remove(key).is_some();
                unindex(&mut st.by_name, key);
                r
            }
            Kind::Health => st.health.slots.remove(key).is_some(),
        };
        if removed {
            st.index += 1;
            self.changes.send_replace(st.index);
        }
    }

    /// Remove expired leases. Called periodically; queries also check deadlines themselves.
    pub fn expire(&self) -> usize {
        let now = Instant::now();
        let mut st = self.state.write().unwrap();
        let n = st.nodes.expire(now).len() + st.health.expire(now).len();
        if n > 0 {
            tracing::debug!(expired = n, "leases expired");
            self.bump(&mut st);
        }
        n
    }

    /// Every registration (live or not) whose key matches `filter`.
    pub(crate) fn services_where(&self, filter: impl Fn(&ServiceRecord) -> bool) -> Vec<(ServiceRecord, u64)> {
        let st = self.state.read().unwrap();
        st.services.slots.values().filter(|s| filter(&s.value)).map(|s| (s.value.clone(), s.revision)).collect()
    }

    /// Whether the node of a service record is currently alive (any generation).
    pub(crate) fn node_alive(&self, namespace: &str, region: &str, node: &str) -> bool {
        let st = self.state.read().unwrap();
        st.nodes.live(&keys::node(namespace, region, node), Instant::now()).is_some()
    }

    /// Instances matching `query`.
    pub fn discover(&self, query: &DiscoveryQuery) -> Vec<ServiceInstance> {
        let now = Instant::now();
        let st = self.state.read().unwrap();
        let Some(keys) = st.by_name.get(&query.service) else { return Vec::new() };
        let mut out: Vec<ServiceInstance> = keys
            .iter()
            .filter_map(|k| st.services.slots.get(k))
            .filter(|s| {
                let r = &s.value;
                query.scope.contains(&r.namespace, &r.region)
                    && query.node.as_deref().is_none_or(|n| n == r.node_id)
                    && query.tags.iter().all(|t| r.tags.contains(t))
                    && query.metadata.iter().all(|(k, v)| r.metadata.get(k) == Some(v))
            })
            .filter_map(|s| instance(&st, s, now, true))
            .filter(|i| query.health.accepts(i.status))
            .collect();
        out.sort_by(|a, b| {
            (&a.namespace, &a.region, &a.node_id, &a.service_id).cmp(&(
                &b.namespace,
                &b.region,
                &b.node_id,
                &b.service_id,
            ))
        });
        out
    }

    /// The view of one registration regardless of eligibility (for an agent's own services).
    pub(crate) fn local_view(&self, key: &str) -> Option<ServiceInstance> {
        let st = self.state.read().unwrap();
        st.services.slots.get(key).and_then(|s| instance(&st, s, Instant::now(), false))
    }

    /// One summary per service name with live instances in scope, sorted by name.
    pub fn list_services(&self, scope: &RegistryScope) -> Vec<ServiceSummary> {
        let now = Instant::now();
        let st = self.state.read().unwrap();
        let mut out = Vec::new();
        let mut names: Vec<&String> = st.by_name.keys().collect();
        names.sort();
        for name in names {
            let mut sum = ServiceSummary {
                name: name.clone(),
                instances: 0,
                passing: 0,
                warning: 0,
                critical: 0,
                unknown: 0,
                tags: Vec::new(),
            };
            let mut tags = BTreeSet::new();
            for s in st.by_name[name].iter().filter_map(|k| st.services.slots.get(k)) {
                if !scope.contains(&s.value.namespace, &s.value.region) {
                    continue;
                }
                let Some(i) = instance(&st, s, now, true) else { continue };
                sum.instances += 1;
                match i.status {
                    HealthStatus::Passing => sum.passing += 1,
                    HealthStatus::Warning => sum.warning += 1,
                    HealthStatus::Critical => sum.critical += 1,
                    HealthStatus::Unknown => sum.unknown += 1,
                }
                tags.extend(i.tags);
            }
            if sum.instances > 0 {
                sum.tags = tags.into_iter().collect();
                out.push(sum);
            }
        }
        out
    }

    /// Live nodes in scope, sorted by namespace, region and id.
    pub fn list_nodes(&self, scope: &RegistryScope) -> Vec<NodeInfo> {
        let now = Instant::now();
        let st = self.state.read().unwrap();
        let mut services: HashMap<String, usize> = HashMap::new();
        for s in st.services.slots.values() {
            if instance(&st, s, now, true).is_some() {
                let r = &s.value;
                *services.entry(keys::node(&r.namespace, &r.region, &r.node_id)).or_default() += 1;
            }
        }
        let mut out: Vec<NodeInfo> = st
            .nodes
            .slots
            .iter()
            .filter(|(_, s)| s.deadline.is_none_or(|d| d > now))
            .filter(|(_, s)| scope.contains(&s.value.namespace, &s.value.region))
            .map(|(k, s)| {
                let n = &s.value;
                NodeInfo {
                    namespace: n.namespace.clone(),
                    region: n.region.clone(),
                    node_id: n.node_id.clone(),
                    generation: n.generation.clone(),
                    hostname: n.hostname.clone(),
                    addresses: n.addresses.clone(),
                    metadata: n.metadata.clone(),
                    agent_version: n.agent_version.clone(),
                    services: services.get(k).copied().unwrap_or(0),
                    lease_remaining_ms: s.deadline.map(|d| (d - now).as_millis() as u64).unwrap_or(0),
                }
            })
            .collect();
        out.sort_by(|a, b| (&a.namespace, &a.region, &a.node_id).cmp(&(&b.namespace, &b.region, &b.node_id)));
        out
    }

    /// Counts for status output: (live nodes, registrations).
    pub fn counts(&self) -> (usize, usize) {
        let now = Instant::now();
        let st = self.state.read().unwrap();
        let nodes = st.nodes.slots.values().filter(|s| s.deadline.is_none_or(|d| d > now)).count();
        (nodes, st.services.slots.len())
    }
}

fn unindex(by_name: &mut HashMap<String, BTreeSet<String>>, key: &str) {
    if let Some(name) = keys::parse_service(key).map(|k| k.service.to_string())
        && let Some(set) = by_name.get_mut(&name)
    {
        set.remove(key);
        if set.is_empty() {
            by_name.remove(&name);
        }
    }
}

/// Build the view of one registration. With `require_live`, returns `None` unless
/// the node lease is valid and generations match.
fn instance(st: &State, s: &Slot<ServiceRecord>, now: Instant, require_live: bool) -> Option<ServiceInstance> {
    let r = &s.value;
    let node = st
        .nodes
        .live(&keys::node(&r.namespace, &r.region, &r.node_id), now)
        .map(|n| &n.value)
        .filter(|n| n.generation == r.generation);
    if require_live && node.is_none() {
        return None;
    }
    let health = st
        .health
        .live(&r.key(), now)
        .map(|h| &h.value)
        .filter(|h| h.generation == r.generation && h.service_id == r.service_id && h.node_id == r.node_id);
    let checks: Vec<CheckView> = r
        .checks
        .iter()
        .map(|c| {
            let state = health.and_then(|h| h.checks.get(&c.id));
            CheckView {
                id: c.id.clone(),
                service_id: r.service_id.clone(),
                name: if c.name.is_empty() { c.id.clone() } else { c.name.clone() },
                kind: c.kind.type_name().into(),
                status: state.map(|s| s.status).unwrap_or(HealthStatus::Unknown),
                output: state.map(|s| s.output.clone()).unwrap_or_default(),
            }
        })
        .collect();
    let status = if node.is_none() && checks.is_empty() {
        HealthStatus::Unknown
    } else {
        HealthStatus::aggregate(checks.iter().map(|c| c.status))
    };
    Some(ServiceInstance {
        namespace: r.namespace.clone(),
        region: r.region.clone(),
        service_id: r.service_id.clone(),
        service_name: r.service_name.clone(),
        address: r.address.clone(),
        port: r.port,
        protocol: r.protocol.clone(),
        tags: r.tags.clone(),
        metadata: r.metadata.clone(),
        status,
        checks,
        node_id: r.node_id.clone(),
        node_hostname: node.map(|n| n.hostname.clone()).unwrap_or_default(),
        node_addresses: node.map(|n| n.addresses.clone()).unwrap_or_default(),
        node_metadata: node.map(|n| n.metadata.clone()).unwrap_or_default(),
        generation: r.generation.clone(),
        revision: s.revision,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lictor_store::Entry;
    use lictor_types::{CheckDefinition, CheckKind, CheckState, HealthFilter};
    use std::collections::BTreeMap;
    use std::time::SystemTime;

    const TTL: Duration = Duration::from_secs(30);

    fn catalog() -> Catalog {
        Catalog::new(Leases { node_ttl: TTL, health_ttl: TTL, skew: Duration::ZERO })
    }

    fn node(id: &str, generation: &str) -> NodeRecord {
        NodeRecord {
            schema_version: 1,
            node_id: id.into(),
            generation: generation.into(),
            namespace: "prod".into(),
            region: "us-east".into(),
            hostname: format!("{id}.example"),
            addresses: vec!["10.0.0.1".into()],
            metadata: BTreeMap::new(),
            agent_version: "test".into(),
        }
    }

    fn svc(name: &str, id: &str, node: &str, generation: &str) -> ServiceRecord {
        ServiceRecord {
            schema_version: 1,
            service_id: id.into(),
            service_name: name.into(),
            node_id: node.into(),
            generation: generation.into(),
            namespace: "prod".into(),
            region: "us-east".into(),
            address: "10.0.0.1".into(),
            port: 8080,
            protocol: "tcp".into(),
            tags: vec!["v2".into()],
            metadata: BTreeMap::from([("shard".into(), "4".into())]),
            checks: vec![],
        }
    }

    fn with_check(mut s: ServiceRecord, id: &str) -> ServiceRecord {
        let mut c = CheckDefinition::new(CheckKind::Tcp { address: "10.0.0.1:8080".into() });
        c.id = id.into();
        s.checks.push(c);
        s
    }

    fn health(s: &ServiceRecord, status: HealthStatus) -> HealthRecord {
        HealthRecord {
            schema_version: 1,
            node_id: s.node_id.clone(),
            generation: s.generation.clone(),
            service_id: s.service_id.clone(),
            status,
            checks: s
                .checks
                .iter()
                .map(|c| (c.id.clone(), CheckState { status, output: String::new(), duration_ms: 1 }))
                .collect(),
        }
    }

    fn put<T: serde::Serialize>(c: &Catalog, kind: Kind, key: String, v: &T, rev: u64) {
        put_at(c, kind, key, v, rev, SystemTime::now());
    }

    fn put_at<T: serde::Serialize>(c: &Catalog, kind: Kind, key: String, v: &T, rev: u64, ts: SystemTime) {
        c.apply(
            kind,
            WatchEvent::Put(Entry { key, value: serde_json::to_vec(v).unwrap().into(), revision: rev, timestamp: ts }),
        );
    }

    fn del(c: &Catalog, kind: Kind, key: String, rev: u64) {
        c.apply(kind, WatchEvent::Delete { key, revision: rev, timestamp: SystemTime::now() });
    }

    fn q(name: &str) -> DiscoveryQuery {
        DiscoveryQuery::service(name)
    }

    #[tokio::test(start_paused = true)]
    async fn heartbeats_renew_leases_without_counting_as_changes() {
        let c = catalog();
        let s = svc("web", "web1", "n1", "g1");
        put(&c, Kind::Services, s.key(), &s, 1);
        let n = node("n1", "g1");
        put(&c, Kind::Nodes, n.key(), &n, 1);
        let idx = c.index();

        // Heartbeats rewrite the same record: the lease moves, the index does not.
        for rev in 2..5 {
            tokio::time::advance(TTL / 2).await;
            put(&c, Kind::Nodes, n.key(), &n, rev);
        }
        assert_eq!(c.index(), idx, "a heartbeat is not a change");
        assert_eq!(c.discover(&q("web")).len(), 1, "still alive well past the first lease");

        // A changed record is a change.
        let mut moved = n.clone();
        moved.addresses = vec!["10.0.0.2".into()];
        put(&c, Kind::Nodes, n.key(), &moved, 5);
        assert_eq!(c.index(), idx + 1);

        // So is reviving a lease that ran out before the sweep removed it.
        tokio::time::advance(TTL + Duration::from_secs(1)).await;
        assert!(c.discover(&q("web")).is_empty());
        put(&c, Kind::Nodes, n.key(), &moved, 6);
        assert_eq!(c.index(), idx + 2);
        assert_eq!(c.discover(&q("web")).len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn instance_needs_live_node_with_same_generation() {
        let c = catalog();
        let s = svc("web", "web1", "n1", "g1");
        put(&c, Kind::Services, s.key(), &s, 1);
        assert!(c.discover(&q("web")).is_empty(), "no node yet");

        put(&c, Kind::Nodes, node("n1", "g0").key(), &node("n1", "g0"), 1);
        assert!(c.discover(&q("web")).is_empty(), "stale generation");

        put(&c, Kind::Nodes, node("n1", "g1").key(), &node("n1", "g1"), 2);
        let found = c.discover(&q("web"));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].status, HealthStatus::Passing, "no checks means passing");
        assert_eq!(found[0].node_hostname, "n1.example");

        tokio::time::advance(TTL + Duration::from_secs(1)).await;
        assert!(c.discover(&q("web")).is_empty(), "node lease expired");
    }

    #[tokio::test(start_paused = true)]
    async fn health_must_be_current_and_same_generation() {
        let c = catalog();
        let s = with_check(svc("web", "web1", "n1", "g1"), "tcp");
        put(&c, Kind::Nodes, node("n1", "g1").key(), &node("n1", "g1"), 1);
        put(&c, Kind::Services, s.key(), &s, 1);
        let i = &c.discover(&q("web"))[0];
        assert_eq!(i.status, HealthStatus::Unknown);
        assert_eq!(i.checks[0].status, HealthStatus::Unknown);
        assert!(c.discover(&q("web").passing()).is_empty());

        let mut old = health(&s, HealthStatus::Passing);
        old.generation = "g0".into();
        put(&c, Kind::Health, s.key(), &old, 1);
        assert_eq!(c.discover(&q("web"))[0].status, HealthStatus::Unknown, "old generation ignored");

        put(&c, Kind::Health, s.key(), &health(&s, HealthStatus::Passing), 2);
        assert_eq!(c.discover(&q("web").passing()).len(), 1);

        put(&c, Kind::Health, s.key(), &health(&s, HealthStatus::Warning), 3);
        assert!(c.discover(&q("web").passing()).is_empty());
        let mut nc = q("web");
        nc.health = HealthFilter::NotCritical;
        assert_eq!(c.discover(&nc).len(), 1);

        // Keep the node alive but let health expire.
        tokio::time::advance(Duration::from_secs(20)).await;
        put(&c, Kind::Nodes, node("n1", "g1").key(), &node("n1", "g1"), 2);
        tokio::time::advance(Duration::from_secs(15)).await;
        assert_eq!(c.discover(&q("web"))[0].status, HealthStatus::Unknown, "health lease expired");
    }

    #[tokio::test(start_paused = true)]
    async fn filters_by_tag_metadata_node_and_scope() {
        let c = catalog();
        for (n, id) in [("n1", "a"), ("n2", "b")] {
            put(&c, Kind::Nodes, node(n, "g").key(), &node(n, "g"), 1);
            let mut s = svc("web", id, n, "g");
            if n == "n2" {
                s.tags = vec!["v1".into()];
                s.metadata.insert("shard".into(), "5".into());
            }
            put(&c, Kind::Services, s.key(), &s, if n == "n1" { 1 } else { 2 });
        }
        assert_eq!(c.discover(&q("web")).len(), 2);
        assert_eq!(c.discover(&q("web").tag("v2"))[0].service_id, "a");
        let mut m = q("web");
        m.metadata.insert("shard".into(), "5".into());
        assert_eq!(c.discover(&m)[0].service_id, "b");
        let mut n = q("web");
        n.node = Some("n2".into());
        assert_eq!(c.discover(&n)[0].service_id, "b");
        assert_eq!(c.discover(&q("web").namespace("prod").region("us-east")).len(), 2);
        assert!(c.discover(&q("web").region("eu-west")).is_empty());
        assert!(c.discover(&q("nope")).is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn duplicate_and_stale_events_are_ignored() {
        let c = catalog();
        let s = svc("web", "a", "n1", "g");
        put(&c, Kind::Nodes, node("n1", "g").key(), &node("n1", "g"), 1);
        put(&c, Kind::Services, s.key(), &s, 5);
        let idx = c.index();
        put(&c, Kind::Services, s.key(), &s, 5);
        assert_eq!(c.index(), idx, "redelivery is not a change");
        let mut older = s.clone();
        older.port = 1;
        put(&c, Kind::Services, s.key(), &older, 3);
        assert_eq!(c.discover(&q("web"))[0].port, 8080, "older revision ignored");
        del(&c, Kind::Services, s.key(), 4);
        assert_eq!(c.discover(&q("web")).len(), 1, "stale delete ignored");
        del(&c, Kind::Services, s.key(), 6);
        assert!(c.discover(&q("web")).is_empty());
        put(&c, Kind::Services, s.key(), &s, 5);
        assert!(c.discover(&q("web")).is_empty(), "replayed put after delete ignored");
    }

    #[tokio::test(start_paused = true)]
    async fn replayed_old_heartbeat_does_not_revive_node() {
        let c = catalog();
        let s = svc("web", "a", "n1", "g");
        put(&c, Kind::Services, s.key(), &s, 1);
        let old = SystemTime::now() - Duration::from_secs(120);
        put_at(&c, Kind::Nodes, node("n1", "g").key(), &node("n1", "g"), 1, old);
        assert!(c.discover(&q("web")).is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn resync_sweeps_entries_missed_while_disconnected() {
        let c = catalog();
        let a = svc("web", "a", "n1", "g");
        let b = svc("web", "b", "n1", "g");
        put(&c, Kind::Nodes, node("n1", "g").key(), &node("n1", "g"), 1);
        put(&c, Kind::Services, a.key(), &a, 1);
        put(&c, Kind::Services, b.key(), &b, 2);
        c.apply(Kind::Services, WatchEvent::Ready);
        assert_eq!(c.discover(&q("web")).len(), 2);

        // Reconnect: only `a` still exists in the store.
        c.begin_epoch(Kind::Services);
        put(&c, Kind::Services, a.key(), &a, 1);
        c.apply(Kind::Services, WatchEvent::Ready);
        let found = c.discover(&q("web"));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].service_id, "a");
    }

    #[tokio::test(start_paused = true)]
    async fn ignores_malformed_newer_schema_and_mismatched_keys() {
        let c = catalog();
        c.apply(
            Kind::Services,
            WatchEvent::Put(Entry {
                key: "a.b.c.d.e".into(),
                value: "not json".into(),
                revision: 1,
                timestamp: SystemTime::now(),
            }),
        );
        let mut s = svc("web", "a", "n1", "g");
        s.schema_version = 99;
        put(&c, Kind::Services, s.key(), &s, 2);
        let s = svc("web", "a", "n1", "g");
        put(&c, Kind::Services, "wrong.key.web.n1.a".into(), &s, 3);
        put(&c, Kind::Nodes, node("n1", "g").key(), &node("n1", "g"), 1);
        assert!(c.discover(&q("web")).is_empty());
        assert_eq!(c.counts(), (1, 0));
    }

    #[tokio::test(start_paused = true)]
    async fn summaries_and_nodes() {
        let c = catalog();
        put(&c, Kind::Nodes, node("n1", "g").key(), &node("n1", "g"), 1);
        put(&c, Kind::Nodes, node("n2", "g").key(), &node("n2", "g"), 2);
        let a = with_check(svc("web", "a", "n1", "g"), "c");
        let mut b = svc("web", "b", "n2", "g");
        b.tags = vec!["v1".into()];
        let d = svc("db", "d", "n2", "g");
        put(&c, Kind::Services, a.key(), &a, 1);
        put(&c, Kind::Services, b.key(), &b, 2);
        put(&c, Kind::Services, d.key(), &d, 3);
        put(&c, Kind::Health, a.key(), &health(&a, HealthStatus::Critical), 1);

        let sums = c.list_services(&RegistryScope::all());
        assert_eq!(sums.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["db", "web"]);
        let web = &sums[1];
        assert_eq!((web.instances, web.passing, web.critical), (2, 1, 1));
        assert_eq!(web.tags, ["v1", "v2"]);

        let nodes = c.list_nodes(&RegistryScope::all());
        assert_eq!(nodes.len(), 2);
        assert_eq!((nodes[0].node_id.as_str(), nodes[0].services), ("n1", 1));
        assert_eq!(nodes[1].services, 2);
        assert!(nodes[0].lease_remaining_ms > 29_000);
        assert!(c.list_nodes(&RegistryScope { namespace: Some("dev".into()), region: None }).is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn expire_removes_and_notifies() {
        let c = catalog();
        let mut rx = c.subscribe();
        put(&c, Kind::Nodes, node("n1", "g").key(), &node("n1", "g"), 1);
        assert!(rx.has_changed().unwrap());
        rx.borrow_and_update();
        assert_eq!(c.expire(), 0);
        tokio::time::advance(TTL).await;
        assert_eq!(c.expire(), 1);
        assert!(rx.has_changed().unwrap());
        assert_eq!(c.counts(), (0, 0));
    }

    #[tokio::test(start_paused = true)]
    async fn local_view_ignores_eligibility() {
        let c = catalog();
        let s = with_check(svc("web", "a", "n1", "g"), "c");
        put(&c, Kind::Services, s.key(), &s, 1);
        let v = c.local_view(&s.key()).unwrap();
        assert_eq!(v.status, HealthStatus::Unknown);
        assert!(v.node_hostname.is_empty());
    }

    #[test]
    fn readiness_requires_all_buckets() {
        let c = catalog();
        assert!(!c.is_ready());
        c.apply(Kind::Nodes, WatchEvent::Ready);
        c.apply(Kind::Services, WatchEvent::Ready);
        assert!(!c.is_ready());
        c.apply(Kind::Health, WatchEvent::Ready);
        assert!(c.is_ready() && c.is_syncing());
        c.set_syncing(Kind::Health, false);
        assert!(c.is_ready() && !c.is_syncing());
    }
}
