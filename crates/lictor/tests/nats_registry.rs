//! Registry behavior on a real nats-server (library level, several agents in one process,
//! each with its own NATS connection, like separate hosts).
//!
//! Skipped when nats-server is not installed, unless LICTOR_REQUIRE_NATS=1.

use futures::StreamExt;
use lictor_core::{Agent, AgentConfig, Observer, Registry, RegistryConfig, RegistryError};
use lictor_store_nats::{NatsOptions, NatsStore, testing::NatsServer};
use lictor_types::{DiscoveryQuery, HealthStatus, HealthUpdate, RegistryScope, ServiceRegistration};
use std::sync::Arc;
use std::time::Duration;

async fn store(server: &NatsServer) -> Arc<NatsStore> {
    Arc::new(NatsStore::connect(&NatsOptions::new(server.url())).await.expect("connect"))
}

/// Short leases so expiry tests run in seconds.
fn cfg(node: &str) -> AgentConfig {
    let mut c = AgentConfig::new(node);
    c.registry.membership_ttl = Duration::from_secs(3);
    c.registry.health_ttl = Duration::from_secs(3);
    c.registry.clock_skew = Duration::ZERO;
    c.heartbeat_interval = Duration::from_millis(500);
    c.addresses = vec!["10.1.1.1".into()];
    c
}

async fn agent(server: &NatsServer, node: &str) -> Agent {
    Agent::start(store(server).await, cfg(node)).await.expect("agent starts")
}

/// Poll until `f` returns true (eventual consistency between agents).
async fn eventually<F, Fut>(what: &str, mut f: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while !f().await {
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for: {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn count(a: &Agent, q: DiscoveryQuery) -> usize {
    a.discover(q).await.unwrap().len()
}

macro_rules! server {
    () => {
        match NatsServer::start() {
            Some(s) => s,
            None => {
                eprintln!("skipping: nats-server not installed");
                return;
            }
        }
    };
}

#[tokio::test(flavor = "multi_thread")]
async fn agents_on_separate_connections_see_each_other() {
    let server = server!();
    let a = agent(&server, "a").await;
    let b = agent(&server, "b").await;
    a.register(ServiceRegistration::new("web", 1)).await.unwrap();
    b.register(ServiceRegistration::new("web", 2)).await.unwrap();
    for x in [&a, &b] {
        eventually("both instances", || async { count(x, DiscoveryQuery::service("web")).await == 2 }).await;
    }
    let nodes = a.list_nodes(RegistryScope::all()).await.unwrap();
    assert_eq!(nodes.len(), 2);
    let s = a.status().await.unwrap();
    assert_eq!(s.backend, "nats");
    assert!(s.connected && s.catalog_ready);
}

#[tokio::test(flavor = "multi_thread")]
async fn crashed_agent_disappears_after_lease() {
    let server = server!();
    let a = agent(&server, "a").await;
    let b = agent(&server, "b").await;
    a.register(ServiceRegistration::new("web", 1)).await.unwrap();
    eventually("b sees a", || async { count(&b, DiscoveryQuery::service("web")).await == 1 }).await;
    drop(a); // no leave: like a crash
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(count(&b, DiscoveryQuery::service("web")).await, 1, "still leased");
    eventually("lease expiry", || async { count(&b, DiscoveryQuery::service("web")).await == 0 }).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn graceful_leave_is_immediate() {
    let server = server!();
    let a = agent(&server, "a").await;
    let b = agent(&server, "b").await;
    a.register(ServiceRegistration::new("web", 1)).await.unwrap();
    eventually("b sees a", || async { count(&b, DiscoveryQuery::service("web")).await == 1 }).await;
    a.shutdown(true).await;
    let start = tokio::time::Instant::now();
    eventually("removal", || async { count(&b, DiscoveryQuery::service("web")).await == 0 }).await;
    assert!(start.elapsed() < Duration::from_secs(2), "faster than the 3s lease");
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_adopts_and_duplicate_is_fenced() {
    let server = server!();
    let first = agent(&server, "n1").await;
    first.register(ServiceRegistration::new("web", 1)).await.unwrap();
    let observer = agent(&server, "watcher").await;

    // A second process with the same node id takes over and fences the first.
    let second = agent(&server, "n1").await;
    tokio::time::timeout(Duration::from_secs(5), first.stopped()).await.expect("first agent fenced");
    assert!(first.is_fenced());
    assert!(matches!(first.register(ServiceRegistration::new("x", 1)).await, Err(RegistryError::Fenced(_))));
    let gen2 = second.generation().to_string();
    eventually("adopted under the new generation", || async {
        observer.discover(DiscoveryQuery::service("web")).await.unwrap().first().is_some_and(|i| i.generation == gen2)
    })
    .await;

    // A clean restart adopts again.
    second.shutdown(false).await;
    let third = agent(&server, "n1").await;
    assert_eq!(third.local_services().await.unwrap().len(), 1);
    let gen3 = third.generation().to_string();
    eventually("adopted again", || async {
        observer.discover(DiscoveryQuery::service("web")).await.unwrap().first().is_some_and(|i| i.generation == gen3)
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn ttl_health_propagates_and_expires() {
    let server = server!();
    let a = agent(&server, "a").await;
    let b = agent(&server, "b").await;
    let mut r = ServiceRegistration::new("web", 1);
    let mut c = lictor_types::CheckDefinition::new(lictor_types::CheckKind::Ttl { ttl: Duration::from_secs(2) });
    c.id = "hb".into();
    r.checks = vec![c];
    a.register(r).await.unwrap();
    eventually("critical first", || async {
        b.discover(DiscoveryQuery::service("web"))
            .await
            .unwrap()
            .first()
            .is_some_and(|i| i.status == HealthStatus::Critical)
    })
    .await;
    a.update_health(HealthUpdate { check_id: "hb".into(), status: HealthStatus::Passing, output: "ok".into() })
        .await
        .unwrap();
    eventually("passing on b", || async { count(&b, DiscoveryQuery::service("web").passing()).await == 1 }).await;
    eventually("ttl expiry on b", || async { count(&b, DiscoveryQuery::service("web").passing()).await == 0 }).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn nats_restart_serves_cache_and_recovers() {
    let mut server = server!();
    let a = agent(&server, "a").await;
    let b = agent(&server, "b").await;
    a.register(ServiceRegistration::new("web", 1)).await.unwrap();
    eventually("b sees a", || async { count(&b, DiscoveryQuery::service("web")).await == 1 }).await;

    server.stop();
    eventually("disconnected", || async { !b.status().await.unwrap().connected }).await;
    assert_eq!(count(&b, DiscoveryQuery::service("web")).await, 1, "cached record still served");
    assert!(a.register(ServiceRegistration::new("api", 2)).await.is_err(), "writes are not acknowledged");

    server.restart();
    eventually("reconnected", || async { b.status().await.unwrap().connected }).await;
    a.register(ServiceRegistration::new("api", 2)).await.unwrap();
    eventually("new writes visible", || async { count(&b, DiscoveryQuery::service("api")).await == 1 }).await;
    eventually("a still alive", || async { count(&b, DiscoveryQuery::service("web")).await == 1 }).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn watch_streams_changes_across_agents() {
    let server = server!();
    let a = agent(&server, "a").await;
    let b = agent(&server, "b").await;
    let mut w = b.watch(DiscoveryQuery::service("web")).await.unwrap();
    assert!(w.next().await.unwrap().instances.is_empty());
    a.register(ServiceRegistration::new("web", 1)).await.unwrap();
    let e = tokio::time::timeout(Duration::from_secs(10), w.next()).await.unwrap().unwrap();
    assert_eq!(e.instances.len(), 1);
    a.deregister("web").await.unwrap();
    let e = tokio::time::timeout(Duration::from_secs(10), w.next()).await.unwrap().unwrap();
    assert!(e.instances.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn observers_use_the_buckets_ttl() {
    let server = server!();
    let a = agent(&server, "a").await;
    a.register(ServiceRegistration::new("web", 1)).await.unwrap();
    // The observer's own (default 30s) TTL is ignored in favor of the bucket's 3s.
    let o = Observer::start(store(&server).await, RegistryConfig::default()).await.unwrap();
    assert_eq!(o.discover(DiscoveryQuery::service("web")).await.unwrap().len(), 1);
    let lease = o.list_nodes(RegistryScope::all()).await.unwrap()[0].lease_remaining_ms;
    assert!(lease <= 3_000, "lease {lease}ms comes from the bucket");
    assert!(matches!(o.register(ServiceRegistration::new("x", 1)).await, Err(RegistryError::Unsupported(_))));
    drop(a);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !o.discover(DiscoveryQuery::service("web")).await.unwrap().is_empty() {
        assert!(tokio::time::Instant::now() < deadline, "observer expires the lease");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn agent_rejects_heartbeat_slower_than_existing_bucket_ttl() {
    let server = server!();
    let _a = agent(&server, "a").await; // creates buckets with a 3s TTL
    let mut c = AgentConfig::new("b");
    c.heartbeat_interval = Duration::from_secs(10);
    let err = Agent::start(store(&server).await, c).await.err().unwrap();
    assert!(err.to_string().contains("not shorter than the existing nodes bucket TTL"), "{err}");
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_registrations_from_many_agents() {
    let server = server!();
    let mut agents = Vec::new();
    for n in 0..5 {
        agents.push(agent(&server, &format!("n{n}")).await);
    }
    let tasks: Vec<_> = agents
        .iter()
        .flat_map(|a| {
            (0..10).map(move |i| {
                let a = a.clone();
                tokio::spawn(async move {
                    let mut r = ServiceRegistration::new("web", 8000 + i);
                    r.id = Some(format!("web-{i}"));
                    a.register(r).await.unwrap();
                })
            })
        })
        .collect();
    for t in tasks {
        t.await.unwrap();
    }
    for a in &agents {
        eventually("all 50 instances", || async { count(a, DiscoveryQuery::service("web")).await == 50 }).await;
    }
}
