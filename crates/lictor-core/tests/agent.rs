//! Agent behavior against the in-memory store.
//!
//! Tests that depend on leases run with paused time, so a 30-second TTL takes no real time.

use futures::StreamExt;
use lictor_core::{Agent, AgentConfig, Observer, ReconcilePolicy, Registry, RegistryConfig, RegistryError};
use lictor_store::{BucketConfig, MemoryStore, Store, StoreError};
use lictor_types::{
    CheckDefinition, CheckKind, DiscoveryQuery, HealthCheck, HealthStatus, HealthUpdate, RegistryScope,
    ServiceRegistration,
};
use std::sync::Arc;
use std::time::Duration;

fn cfg(node: &str) -> AgentConfig {
    let mut c = AgentConfig::new(node);
    c.addresses = vec![format!("10.0.0.{}", node.len())];
    c
}

async fn agent(store: &MemoryStore, node: &str) -> Agent {
    Agent::start(Arc::new(store.clone()), cfg(node)).await.expect("agent starts")
}

fn web(port: u16) -> ServiceRegistration {
    let mut r = ServiceRegistration::new("web", port);
    r.tags = vec!["v2".into()];
    r
}

fn ttl_check(id: &str, ttl: u64) -> CheckDefinition {
    let mut c = CheckDefinition::new(CheckKind::Ttl { ttl: Duration::from_secs(ttl) });
    c.id = id.into();
    c
}

fn q(name: &str) -> DiscoveryQuery {
    DiscoveryQuery::service(name)
}

#[tokio::test(start_paused = true)]
async fn register_and_discover() {
    let store = MemoryStore::new();
    let a = agent(&store, "node1").await;
    let receipt = a.register(web(8080)).await.unwrap();
    assert_eq!(receipt.service_id, "web");
    assert_eq!(receipt.key, "default.dc1.web.node1.web");
    assert_eq!(receipt.generation, a.generation());

    let found = a.discover(q("web")).await.unwrap();
    assert_eq!(found.len(), 1);
    let i = &found[0];
    assert_eq!((i.address.as_str(), i.port, i.status), ("10.0.0.5", 8080, HealthStatus::Passing));
    assert_eq!(i.node_id, "node1");
    assert_eq!(i.protocol, "tcp");
    assert!(a.discover(q("web").tag("v1")).await.unwrap().is_empty());
    assert_eq!(a.discover(q("web").tag("v2")).await.unwrap().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn defaults_for_ids_and_check_names() {
    let store = MemoryStore::new();
    let a = agent(&store, "n1").await;
    let mut r = ServiceRegistration::new("api", 1);
    r.address = Some("192.168.1.9".into());
    r.checks = vec![
        CheckDefinition::new(CheckKind::Ttl { ttl: Duration::from_secs(30) }),
        CheckDefinition::new(CheckKind::Ttl { ttl: Duration::from_secs(30) }),
    ];
    let receipt = a.register(r).await.unwrap();
    assert_eq!(receipt.checks, ["service:api:1", "service:api:2"]);
    let s = a.get_service("api").await.unwrap().unwrap();
    assert_eq!(s.address, "192.168.1.9");
    assert_eq!(s.checks[0].name, "Service 'api' check");

    let mut one = ServiceRegistration::new("db", 5432);
    one.checks = vec![CheckDefinition::new(CheckKind::Ttl { ttl: Duration::from_secs(30) })];
    assert_eq!(a.register(one).await.unwrap().checks, ["service:db"]);
}

#[tokio::test(start_paused = true)]
async fn reregistering_replaces_and_renaming_moves() {
    let store = MemoryStore::new();
    let a = agent(&store, "n1").await;
    let r1 = a.register(web(1)).await.unwrap();
    let r2 = a.register(web(2)).await.unwrap();
    assert!(r2.revision > r1.revision);
    let found = a.discover(q("web")).await.unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].port, 2);

    let mut renamed = ServiceRegistration::new("api", 3);
    renamed.id = Some("web".into());
    a.register(renamed).await.unwrap();
    assert!(a.discover(q("web")).await.unwrap().is_empty());
    assert_eq!(a.discover(q("api")).await.unwrap()[0].service_id, "web");
    assert_eq!(a.local_services().await.unwrap().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn deregister() {
    let store = MemoryStore::new();
    let a = agent(&store, "n1").await;
    a.register(web(1)).await.unwrap();
    a.deregister("web").await.unwrap();
    assert!(a.discover(q("web")).await.unwrap().is_empty());
    assert!(a.local_services().await.unwrap().is_empty());
    assert!(matches!(a.deregister("web").await, Err(RegistryError::NotFound(_))));
    let services = store.bucket(&BucketConfig::new("LICTOR_SERVICES"), false).await.unwrap();
    assert!(services.keys().await.unwrap().is_empty(), "removed from the store too");
}

#[tokio::test(start_paused = true)]
async fn validation_errors() {
    let store = MemoryStore::new();
    let a = agent(&store, "n1").await;
    let bad = ServiceRegistration::new("bad.name", 1);
    assert!(matches!(a.register(bad).await, Err(RegistryError::Invalid(_))));

    let mut r = web(1);
    r.checks = vec![ttl_check("shared", 30)];
    a.register(r).await.unwrap();
    let mut other = ServiceRegistration::new("api", 2);
    other.checks = vec![ttl_check("shared", 30)];
    let err = a.register(other).await.unwrap_err();
    assert!(err.to_string().contains("already used by service 'web'"), "{err}");

    let err = Agent::start(Arc::new(store.clone()), cfg("bad node")).await.err().unwrap();
    assert!(matches!(err, RegistryError::Invalid(_)));
}

#[tokio::test(start_paused = true)]
async fn ttl_checks() {
    let store = MemoryStore::new();
    let a = agent(&store, "n1").await;
    let mut r = web(1);
    r.checks = vec![ttl_check("hb", 10)];
    a.register(r).await.unwrap();
    tokio::task::yield_now().await;
    assert_eq!(a.discover(q("web")).await.unwrap()[0].status, HealthStatus::Critical, "initial status");
    assert!(a.discover(q("web").passing()).await.unwrap().is_empty());

    let update = |s| HealthUpdate { check_id: "hb".into(), status: s, output: "all good".into() };
    a.update_health(update(HealthStatus::Passing)).await.unwrap();
    tokio::task::yield_now().await;
    let i = &a.discover(q("web").passing()).await.unwrap()[0];
    assert_eq!(i.checks[0].output, "all good");

    a.update_health(update(HealthStatus::Warning)).await.unwrap();
    tokio::task::yield_now().await;
    assert_eq!(a.discover(q("web")).await.unwrap()[0].status, HealthStatus::Warning);

    a.update_health(update(HealthStatus::Passing)).await.unwrap();
    tokio::time::sleep(Duration::from_secs(11)).await;
    let i = &a.discover(q("web")).await.unwrap()[0];
    assert_eq!(i.status, HealthStatus::Critical, "TTL expired");
    assert!(i.checks[0].output.contains("TTL expired"));

    let missing = HealthUpdate { check_id: "nope".into(), status: HealthStatus::Passing, output: String::new() };
    assert!(matches!(a.update_health(missing).await, Err(RegistryError::NotFound(_))));
}

#[tokio::test]
async fn tcp_checks_follow_the_listener() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let store = MemoryStore::new();
    let a = agent(&store, "n1").await;
    let mut r = web(addr.port());
    let mut c = CheckDefinition::new(CheckKind::Tcp { address: addr.to_string() });
    c.interval = Some(Duration::from_millis(100));
    c.timeout = Some(Duration::from_millis(500));
    r.checks = vec![c];
    a.register(r).await.unwrap();

    let wait_for = |want: HealthStatus| {
        let a = a.clone();
        async move {
            for _ in 0..100 {
                if a.get_service("web").await.unwrap().unwrap().status == want {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            panic!("status never became {want}");
        }
    };
    wait_for(HealthStatus::Passing).await;
    drop(listener);
    wait_for(HealthStatus::Critical).await;
    let checks = a.local_checks().await.unwrap();
    assert!(checks[0].output.starts_with("TCP connect"), "{}", checks[0].output);
}

#[tokio::test(start_paused = true)]
async fn add_and_remove_checks() {
    let store = MemoryStore::new();
    let a = agent(&store, "n1").await;
    a.register(web(1)).await.unwrap();
    let id = a
        .register_check(HealthCheck {
            service_id: "web".into(),
            check: CheckDefinition::new(CheckKind::Ttl { ttl: Duration::from_secs(30) }),
        })
        .await
        .unwrap();
    assert_eq!(id, "service:web");
    let id2 = a
        .register_check(HealthCheck {
            service_id: "web".into(),
            check: CheckDefinition::new(CheckKind::Ttl { ttl: Duration::from_secs(30) }),
        })
        .await
        .unwrap();
    assert_eq!(id2, "service:web:2");
    assert_eq!(a.local_checks().await.unwrap().len(), 2);
    assert_eq!(a.status().await.unwrap().local_checks, 2);

    a.deregister_check("service:web").await.unwrap();
    let checks = a.local_checks().await.unwrap();
    assert_eq!(checks.len(), 1);
    assert_eq!(checks[0].id, "service:web:2");
    assert!(matches!(a.deregister_check("service:web").await, Err(RegistryError::NotFound(_))));
    let orphan = HealthCheck { service_id: "nope".into(), check: ttl_check("x", 30) };
    assert!(matches!(a.register_check(orphan).await, Err(RegistryError::NotFound(_))));

    a.deregister_check("service:web:2").await.unwrap();
    assert_eq!(a.get_service("web").await.unwrap().unwrap().status, HealthStatus::Passing, "no checks again");
}

#[tokio::test(start_paused = true)]
async fn agents_see_each_other() {
    let store = MemoryStore::new();
    let a = agent(&store, "n1").await;
    let b = agent(&store, "n2").await;
    a.register(web(1)).await.unwrap();
    b.register(web(2)).await.unwrap();
    tokio::task::yield_now().await;
    for x in [&a, &b] {
        let found = x.discover(q("web")).await.unwrap();
        assert_eq!(found.iter().map(|i| i.node_id.as_str()).collect::<Vec<_>>(), ["n1", "n2"]);
    }
    let nodes = a.list_nodes(RegistryScope::all()).await.unwrap();
    assert_eq!(nodes.len(), 2);
    assert!(nodes.iter().all(|n| n.services == 1));
    let sums = b.list_services(RegistryScope::all()).await.unwrap();
    assert_eq!((sums[0].name.as_str(), sums[0].instances), ("web", 2));
    assert_eq!(a.local_services().await.unwrap().len(), 1, "local is only our own");
}

#[tokio::test(start_paused = true)]
async fn wait_for_change_blocks_until_the_catalog_changes() {
    let store = MemoryStore::new();
    let a = agent(&store, "node1").await;
    let now = a.status().await.unwrap().catalog_index;
    assert_eq!(a.wait_for_change(now - 1, Duration::from_secs(60)).await.unwrap(), now, "already past");

    // Nothing changes (heartbeats included): the wait runs to its timeout.
    let start = tokio::time::Instant::now();
    assert_eq!(a.wait_for_change(now, Duration::from_secs(60)).await.unwrap(), now);
    assert!(start.elapsed() >= Duration::from_secs(60));

    // A registration wakes it.
    let b = a.clone();
    let waiter = tokio::spawn(async move { b.wait_for_change(now, Duration::from_secs(600)).await.unwrap() });
    tokio::time::sleep(Duration::from_secs(5)).await;
    a.register(web(80)).await.unwrap();
    let woke = tokio::time::timeout(Duration::from_secs(1), waiter).await.expect("woken").unwrap();
    assert!(woke > now);
}

/// The stored definition has every default filled in, and registering it again is a no-op.
#[tokio::test(start_paused = true)]
async fn service_definition_round_trips() {
    let store = MemoryStore::new();
    let a = agent(&store, "node1").await;
    let mut reg = web(8080);
    reg.metadata.insert("shard".into(), "4".into());
    reg.checks.push(ttl_check("", 30));
    a.register(reg).await.unwrap();

    let def = a.service_definition("web").await.unwrap().expect("definition");
    assert_eq!(def.id.as_deref(), Some("web"));
    assert_eq!(def.address.as_deref(), Some("10.0.0.5"));
    assert_eq!(def.protocol.as_deref(), Some("tcp"));
    assert_eq!(def.tags, ["v2"]);
    assert_eq!(def.metadata["shard"], "4");
    assert_eq!(def.checks.len(), 1);
    assert_eq!(def.checks[0].id, "service:web");
    assert!(matches!(def.checks[0].kind, CheckKind::Ttl { ttl } if ttl == Duration::from_secs(30)));

    let before = a.get_service("web").await.unwrap().unwrap();
    a.register(def.clone()).await.unwrap();
    assert_eq!(a.service_definition("web").await.unwrap(), Some(def));
    let after = a.get_service("web").await.unwrap().unwrap();
    assert_eq!((after.address, after.port, after.tags), (before.address, before.port, before.tags));
    assert!(a.service_definition("nope").await.unwrap().is_none());
}

#[tokio::test(start_paused = true)]
async fn graceful_leave_hides_services_immediately() {
    let store = MemoryStore::new();
    let a = agent(&store, "n1").await;
    let b = agent(&store, "n2").await;
    a.register(web(1)).await.unwrap();
    tokio::task::yield_now().await;
    assert_eq!(b.discover(q("web")).await.unwrap().len(), 1);
    a.shutdown(true).await;
    tokio::task::yield_now().await;
    assert!(b.discover(q("web")).await.unwrap().is_empty());
    assert_eq!(b.list_nodes(RegistryScope::all()).await.unwrap().len(), 1);
}

/// A heartbeat that lands after the agent last recorded its revision must not stop it leaving.
#[tokio::test(start_paused = true)]
async fn graceful_leave_survives_a_concurrent_membership_write() {
    let store = MemoryStore::new();
    let a = agent(&store, "n1").await;
    let b = agent(&store, "n2").await;
    a.register(web(1)).await.unwrap();
    let nodes = store.bucket(&BucketConfig::new("LICTOR_NODES"), false).await.unwrap();
    let key = nodes.keys().await.unwrap().into_iter().find(|k| k.ends_with(".n1")).expect("n1 membership");
    let current = nodes.get(&key).await.unwrap().unwrap();
    nodes.update(&key, current.value, current.revision).await.unwrap();

    a.shutdown(true).await;
    assert!(nodes.get(&key).await.unwrap().is_none(), "membership removed despite the stale revision");
    tokio::task::yield_now().await;
    assert!(b.discover(q("web")).await.unwrap().is_empty());
}

/// Leaving never deletes a membership record that a newer generation has taken over.
#[tokio::test(start_paused = true)]
async fn graceful_leave_keeps_a_newer_generations_membership() {
    let store = MemoryStore::new();
    let old = agent(&store, "n1").await;
    let new = agent(&store, "n1").await;
    old.shutdown(true).await;
    let nodes = store.bucket(&BucketConfig::new("LICTOR_NODES"), false).await.unwrap();
    let key = nodes.keys().await.unwrap().into_iter().find(|k| k.ends_with(".n1")).expect("n1 membership");
    let rec: serde_json::Value = serde_json::from_slice(&nodes.get(&key).await.unwrap().unwrap().value).unwrap();
    assert_eq!(rec["generation"], new.generation());
}

#[tokio::test(start_paused = true)]
async fn crashed_agent_expires_after_membership_ttl() {
    let store = MemoryStore::new();
    let a = agent(&store, "n1").await;
    let b = agent(&store, "n2").await;
    a.register(web(1)).await.unwrap();
    drop(a);
    tokio::time::sleep(Duration::from_secs(20)).await;
    assert_eq!(b.discover(q("web")).await.unwrap().len(), 1, "still within the lease");
    tokio::time::sleep(Duration::from_secs(15)).await;
    assert!(b.discover(q("web")).await.unwrap().is_empty(), "lease expired");
}

#[tokio::test(start_paused = true)]
async fn heartbeats_keep_the_node_alive() {
    let store = MemoryStore::new();
    let a = agent(&store, "n1").await;
    a.register(web(1)).await.unwrap();
    tokio::time::sleep(Duration::from_secs(300)).await;
    assert_eq!(a.discover(q("web")).await.unwrap().len(), 1);
    let me = a.status().await.unwrap().node.unwrap();
    assert!(me.lease_remaining_ms > 15_000, "{}", me.lease_remaining_ms);
}

#[tokio::test(start_paused = true)]
async fn restart_adopts_previous_registrations() {
    let store = MemoryStore::new();
    let a = agent(&store, "n1").await;
    let mut r = web(1);
    r.checks = vec![ttl_check("hb", 30)];
    a.register(r).await.unwrap();
    let old_gen = a.generation().to_string();
    a.shutdown(false).await;

    let a2 = agent(&store, "n1").await;
    assert_ne!(a2.generation(), old_gen);
    let found = a2.discover(q("web")).await.unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].generation, a2.generation());
    assert_eq!(a2.local_services().await.unwrap().len(), 1);
    // The adopted TTL check runs again and accepts updates.
    a2.update_health(HealthUpdate { check_id: "hb".into(), status: HealthStatus::Passing, output: String::new() })
        .await
        .unwrap();
}

#[tokio::test(start_paused = true)]
async fn restart_with_drop_and_keep_policies() {
    for policy in [ReconcilePolicy::Drop, ReconcilePolicy::Keep] {
        let store = MemoryStore::new();
        let a = agent(&store, "n1").await;
        a.register(web(1)).await.unwrap();
        a.shutdown(false).await;

        let mut c = cfg("n1");
        c.reconcile = policy;
        let a2 = Agent::start(Arc::new(store.clone()), c).await.unwrap();
        assert!(a2.discover(q("web")).await.unwrap().is_empty(), "{policy:?}: not discoverable");
        assert!(a2.local_services().await.unwrap().is_empty());
        let services = store.bucket(&BucketConfig::new("LICTOR_SERVICES"), false).await.unwrap();
        let left = services.keys().await.unwrap().len();
        assert_eq!(left, if policy == ReconcilePolicy::Keep { 1 } else { 0 }, "{policy:?}");
        a2.register(web(1)).await.unwrap();
        assert_eq!(a2.discover(q("web")).await.unwrap().len(), 1, "{policy:?}: re-registration activates");
    }
}

#[tokio::test(start_paused = true)]
async fn duplicate_node_id_fences_the_older_agent() {
    let store = MemoryStore::new();
    let old = agent(&store, "n1").await;
    old.register(web(1)).await.unwrap();
    let new = agent(&store, "n1").await;
    tokio::time::timeout(Duration::from_secs(15), old.stopped()).await.expect("old agent stops");
    assert!(old.is_fenced());
    assert!(matches!(old.register(web(2)).await, Err(RegistryError::Fenced(_))));
    tokio::time::sleep(Duration::from_secs(60)).await;
    assert!(!new.is_fenced());
    let found = new.discover(q("web")).await.unwrap();
    assert_eq!(found.len(), 1, "the new generation adopted the service");
    assert_eq!(found[0].generation, new.generation());
}

#[tokio::test(start_paused = true)]
async fn critical_checks_can_deregister_their_service() {
    let store = MemoryStore::new();
    let a = agent(&store, "n1").await;
    let mut r = web(1);
    let mut c = ttl_check("hb", 5);
    c.deregister_critical_after = Some(Duration::from_secs(10));
    r.checks = vec![c];
    a.register(r).await.unwrap();
    tokio::time::sleep(Duration::from_secs(30)).await;
    assert!(a.local_services().await.unwrap().is_empty(), "deregistered");
}

#[tokio::test(start_paused = true)]
async fn stale_registrations_are_cleaned_up() {
    let store = MemoryStore::new();
    let a = agent(&store, "n1").await;
    a.register(web(1)).await.unwrap();
    drop(a);
    let mut c = cfg("n2");
    c.stale_retention = Some(Duration::from_secs(120));
    let _b = Agent::start(Arc::new(store.clone()), c).await.unwrap();
    let services = store.bucket(&BucketConfig::new("LICTOR_SERVICES"), false).await.unwrap();
    tokio::time::sleep(Duration::from_secs(60)).await;
    assert_eq!(services.keys().await.unwrap().len(), 1, "kept within retention");
    tokio::time::sleep(Duration::from_secs(200)).await;
    assert!(services.keys().await.unwrap().is_empty(), "removed after retention");
}

#[tokio::test(start_paused = true)]
async fn store_outage_serves_cache_then_recovers() {
    let store = MemoryStore::new();
    let a = agent(&store, "n1").await;
    let b = agent(&store, "n2").await;
    a.register(web(1)).await.unwrap();
    tokio::task::yield_now().await;

    store.set_available(false);
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(!b.status().await.unwrap().connected);
    assert_eq!(b.discover(q("web")).await.unwrap().len(), 1, "cached and unexpired");
    let err = a.register(web(2)).await.unwrap_err();
    assert!(matches!(err, RegistryError::Store(StoreError::Unavailable(_))), "{err}");

    store.set_available(true);
    tokio::time::sleep(Duration::from_secs(12)).await;
    assert!(b.status().await.unwrap().connected);
    a.register(web(3)).await.unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(b.discover(q("web")).await.unwrap()[0].port, 3);
}

#[tokio::test(start_paused = true)]
async fn outage_longer_than_the_lease_expires_cached_nodes() {
    let store = MemoryStore::new();
    let a = agent(&store, "n1").await;
    let b = agent(&store, "n2").await;
    a.register(web(1)).await.unwrap();
    store.set_available(false);
    tokio::time::sleep(Duration::from_secs(40)).await;
    assert!(b.discover(q("web")).await.unwrap().is_empty(), "leases expire locally without the store");
    store.set_available(true);
    tokio::time::sleep(Duration::from_secs(15)).await;
    assert_eq!(b.discover(q("web")).await.unwrap().len(), 1, "back after recovery");
}

#[tokio::test(start_paused = true)]
async fn watch_emits_on_changes() {
    let store = MemoryStore::new();
    let a = agent(&store, "n1").await;
    let mut w = a.watch(q("web")).await.unwrap();
    assert!(w.next().await.unwrap().instances.is_empty());
    a.register(web(1)).await.unwrap();
    let e = w.next().await.unwrap();
    assert_eq!(e.instances.len(), 1);
    a.register(ServiceRegistration::new("other", 1)).await.unwrap();
    a.deregister("web").await.unwrap();
    let e = tokio::time::timeout(Duration::from_secs(5), w.next()).await.unwrap().unwrap();
    assert!(e.instances.is_empty(), "unrelated changes are not emitted");
}

#[tokio::test(start_paused = true)]
async fn observer_reads_but_cannot_write() {
    let store = MemoryStore::new();
    let err = Observer::start(Arc::new(store.clone()), RegistryConfig::default()).await.err().unwrap();
    assert!(matches!(err, RegistryError::Store(StoreError::BucketNotFound(_))), "observers never create buckets");

    let a = agent(&store, "n1").await;
    a.register(web(1)).await.unwrap();
    let o = Observer::start(Arc::new(store.clone()), RegistryConfig::default()).await.unwrap();
    assert_eq!(o.discover(q("web")).await.unwrap().len(), 1);
    assert_eq!(o.list_nodes(RegistryScope::all()).await.unwrap().len(), 1);
    assert!(matches!(o.register(web(2)).await, Err(RegistryError::Unsupported(_))));
    let s = o.status().await.unwrap();
    assert!(s.node.is_none() && s.catalog_ready);
    assert_eq!((s.catalog_nodes, s.catalog_services), (1, 1));
}

#[tokio::test(start_paused = true)]
async fn status_reports_identity_and_counts() {
    let store = MemoryStore::new();
    let a = agent(&store, "n1").await;
    a.register(web(1)).await.unwrap();
    let s = a.status().await.unwrap();
    assert_eq!(s.backend, "memory");
    assert!(s.connected && s.catalog_ready);
    let node = s.node.unwrap();
    assert_eq!((node.node_id.as_str(), node.generation.as_str()), ("n1", a.generation()));
    assert_eq!((s.local_services, s.catalog_nodes, s.catalog_services), (1, 1, 1));
    assert!(s.catalog_index > 0);
}

#[tokio::test(start_paused = true)]
async fn namespaces_and_regions_are_isolated_scopes() {
    let store = MemoryStore::new();
    let mut c = cfg("n1");
    c.namespace = "prod".into();
    c.region = "us-east".into();
    let a = Agent::start(Arc::new(store.clone()), c).await.unwrap();
    let b = agent(&store, "n2").await;
    a.register(web(1)).await.unwrap();
    b.register(web(2)).await.unwrap();
    tokio::task::yield_now().await;
    assert_eq!(a.discover(q("web")).await.unwrap().len(), 2, "no scope: everything");
    let prod = a.discover(q("web").namespace("prod")).await.unwrap();
    assert_eq!((prod.len(), prod[0].port), (1, 1));
    assert_eq!(a.discover(q("web").region("dc1")).await.unwrap()[0].port, 2);
    let scope = RegistryScope { namespace: Some("prod".into()), region: None };
    assert_eq!(a.list_nodes(scope).await.unwrap().len(), 1);
}
