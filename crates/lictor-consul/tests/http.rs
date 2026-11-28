//! The Consul HTTP adapter against a real agent (in-memory store). Shapes and headers follow the
//! responses captured from Consul 1.22 (see docs/consul.md).

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use lictor_consul::{Local, http};
use lictor_core::{Agent, AgentConfig, Registry};
use lictor_store::MemoryStore;
use lictor_types::{CheckDefinition, CheckKind, HealthStatus, HealthUpdate, ServiceRegistration};
use serde_json::Value;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tower::ServiceExt;

struct Env {
    agent: Agent,
    app: Router,
}

async fn env() -> Env {
    let mut c = AgentConfig::new("host-a");
    c.addresses = vec!["192.0.2.10".into()];
    c.metadata.insert("rack".into(), "r1".into());
    let agent = Agent::start(Arc::new(MemoryStore::new()), c).await.unwrap();
    let mut web = ServiceRegistration::new("web", 8080);
    web.id = Some("web-1".into());
    web.address = Some("10.0.0.11".into());
    web.tags = vec!["v2".into(), "prometheus".into()];
    web.metadata.insert("shard".into(), "4".into());
    web.metadata.insert("callsign".into(), "KXYZ".into());
    let mut ttl = CheckDefinition::new(CheckKind::Ttl { ttl: Duration::from_secs(300) });
    ttl.id = "web-ttl".into();
    web.checks = vec![ttl];
    agent.register(web).await.unwrap();
    let mut web2 = ServiceRegistration::new("web", 8081);
    web2.id = Some("web-2".into());
    web2.tags = vec!["v1".into()];
    agent.register(web2).await.unwrap();
    agent.register(ServiceRegistration::new("db", 5432)).await.unwrap();
    let local = Local {
        node_id: "host-a".into(),
        namespace: "default".into(),
        region: "dc1".into(),
        address: Some("192.0.2.10".parse().unwrap()),
    };
    let app = http::router(Arc::new(agent.clone()), local);
    Env { agent, app }
}

struct Reply {
    status: StatusCode,
    index: Option<u64>,
    headers: axum::http::HeaderMap,
    body: String,
}

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("{e}: {}", self.body))
    }
}

async fn get(app: &Router, uri: &str) -> Reply {
    let res = app.clone().oneshot(Request::get(uri).body(Body::empty()).unwrap()).await.unwrap();
    let status = res.status();
    let headers = res.headers().clone();
    let index = headers.get("x-consul-index").map(|v| v.to_str().unwrap().parse().unwrap());
    let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
    Reply { status, index, headers, body: String::from_utf8_lossy(&body).into_owned() }
}

async fn eventually<F: Future<Output = bool>>(mut f: impl FnMut() -> F) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !f().await {
        assert!(Instant::now() < deadline, "timed out");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The catalog index once background health publishing has settled.
async fn settled_index(app: &Router) -> u64 {
    let mut last = get(app, "/v1/catalog/services").await.index.unwrap();
    loop {
        tokio::time::sleep(Duration::from_millis(300)).await;
        let now = get(app, "/v1/catalog/services").await.index.unwrap();
        if now == last {
            return now;
        }
        last = now;
    }
}

#[tokio::test]
async fn health_service_has_the_shape_prometheus_reads() {
    let e = env().await;
    let r = get(&e.app, "/v1/health/service/web?dc=dc1&tag=prometheus").await;
    assert_eq!(r.status, StatusCode::OK);
    // The Go client fails without these.
    assert_eq!(r.headers["x-consul-knownleader"], "true");
    assert_eq!(r.headers["x-consul-lastcontact"], "0");
    assert!(r.index.unwrap() >= 1);
    let v = r.json();
    assert_eq!(v.as_array().unwrap().len(), 1, "only the instance with every tag asked for");
    let entry = &v[0];
    assert_eq!(entry["Node"]["Node"], "host-a");
    assert_eq!(entry["Node"]["Address"], "192.0.2.10");
    assert_eq!(entry["Node"]["Datacenter"], "dc1");
    assert_eq!(entry["Node"]["Meta"]["rack"], "r1");
    assert_eq!(entry["Node"]["TaggedAddresses"]["lan_ipv4"], "192.0.2.10");
    let s = &entry["Service"];
    assert_eq!((s["ID"].as_str(), s["Service"].as_str(), s["Port"].as_u64()), (Some("web-1"), Some("web"), Some(8080)));
    assert_eq!(s["Address"], "10.0.0.11");
    assert_eq!(s["Tags"], serde_json::json!(["v2", "prometheus"]));
    assert_eq!(s["Meta"]["shard"], "4");
    assert_eq!(s["Meta"]["callsign"], "KXYZ");
    assert_eq!(s["Weights"]["Passing"], 1);
    // The node's own check first, then the service's; a new TTL check is critical.
    let checks = entry["Checks"].as_array().unwrap();
    assert_eq!(checks[0]["CheckID"], "serfHealth");
    assert_eq!(checks[0]["Status"], "passing");
    assert_eq!(checks[1]["CheckID"], "web-ttl");
    assert_eq!(checks[1]["Status"], "critical");
    assert_eq!(checks[1]["Type"], "ttl");
    assert_eq!(checks[1]["ServiceID"], "web-1");
    // Every key Consul sends is there.
    let keys = |v: &Value| v.as_object().unwrap().keys().cloned().collect::<Vec<_>>().join(",");
    assert_eq!(keys(entry), "Checks,Node,Service");
    assert_eq!(keys(&entry["Node"]), "Address,CreateIndex,Datacenter,ID,Meta,ModifyIndex,Node,TaggedAddresses");
    assert_eq!(
        keys(s),
        "Address,Connect,CreateIndex,EnableTagOverride,ID,Meta,ModifyIndex,PeerName,Port,Proxy,Service,TaggedAddresses,Tags,Weights"
    );
}

#[tokio::test]
async fn passing_and_tag_filters() {
    let e = env().await;
    assert_eq!(get(&e.app, "/v1/health/service/web").await.json().as_array().unwrap().len(), 2);
    // web-1's TTL check is critical: ?passing leaves web-2 (no checks) only.
    let v = get(&e.app, "/v1/health/service/web?passing=1").await.json();
    assert_eq!(v.as_array().unwrap().len(), 1);
    assert_eq!(v[0]["Service"]["ID"], "web-2");
    e.agent
        .update_health(HealthUpdate { check_id: "web-ttl".into(), status: HealthStatus::Passing, output: "ok".into() })
        .await
        .unwrap();
    // Health results are published in the background.
    eventually(|| async { get(&e.app, "/v1/health/service/web?passing").await.json().as_array().unwrap().len() == 2 })
        .await;
    // Tags: every one listed, case-insensitive.
    assert_eq!(get(&e.app, "/v1/health/service/web?tag=V2&tag=prometheus").await.json().as_array().unwrap().len(), 1);
    assert_eq!(get(&e.app, "/v1/health/service/web?tag=v2&tag=v1").await.json().as_array().unwrap().len(), 0);
    // Node metadata.
    assert_eq!(get(&e.app, "/v1/health/service/web?node-meta=rack:r1").await.json().as_array().unwrap().len(), 2);
    assert_eq!(get(&e.app, "/v1/health/service/web?node-meta=rack:r2").await.json().as_array().unwrap().len(), 0);
    // Unknown services are an empty list, like Consul.
    let r = get(&e.app, "/v1/health/service/nope").await;
    assert_eq!((r.status, r.body.as_str()), (StatusCode::OK, "[]"));
}

#[tokio::test]
async fn catalog_endpoints() {
    let e = env().await;
    let v = get(&e.app, "/v1/catalog/services").await.json();
    assert_eq!(v, serde_json::json!({ "db": [], "web": ["prometheus", "v1", "v2"] }));
    let v = get(&e.app, "/v1/catalog/service/web?tag=v1").await.json();
    assert_eq!(v.as_array().unwrap().len(), 1);
    assert_eq!(v[0]["ServiceID"], "web-2");
    assert_eq!(v[0]["ServicePort"], 8081);
    assert_eq!(v[0]["ServiceAddress"], "192.0.2.10", "defaulted to the node's address");
    assert_eq!(v[0]["Node"], "host-a");
    assert_eq!(v[0]["NodeMeta"]["rack"], "r1");
    let v = get(&e.app, "/v1/catalog/nodes").await.json();
    assert_eq!(v[0]["Node"], "host-a");
    assert_eq!(v[0]["Address"], "192.0.2.10");
    assert_eq!(get(&e.app, "/v1/catalog/datacenters").await.json(), serde_json::json!(["dc1"]));
}

#[tokio::test]
async fn agent_identity() {
    let e = env().await;
    let r = get(&e.app, "/v1/agent/self").await;
    assert!(r.index.is_none());
    let v = r.json();
    // Prometheus reads Config.Datacenter when its job does not name one.
    assert_eq!(v["Config"]["Datacenter"], "dc1");
    assert_eq!(v["Config"]["NodeName"], "host-a");
    assert_eq!(v["Member"]["Addr"], "192.0.2.10");
    assert_eq!(get(&e.app, "/v1/status/leader").await.json(), "192.0.2.10:8300");
}

#[tokio::test]
async fn errors_like_consul() {
    let e = env().await;
    let r = get(&e.app, "/v1/health/service/web?dc=dc9").await;
    assert_eq!((r.status, r.body.as_str()), (StatusCode::INTERNAL_SERVER_ERROR, "No path to datacenter"));
    assert_eq!(get(&e.app, "/v1/catalog/services?index=1&wait=5").await.status, StatusCode::BAD_REQUEST);
    assert_eq!(get(&e.app, "/v1/catalog/services?index=x").await.status, StatusCode::BAD_REQUEST);
    let r = get(&e.app, "/v1/health/service/web?filter=Service.Port%3D%3D80").await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(r.body.contains("not supported"), "{}", r.body);
    assert_eq!(get(&e.app, "/v1/kv/foo").await.status, StatusCode::NOT_FOUND);
    // ?pretty is honored.
    assert!(get(&e.app, "/v1/catalog/datacenters?pretty").await.body.contains('\n'));
}

#[tokio::test]
async fn blocking_queries() {
    let e = env().await;
    let first = settled_index(&e.app).await;
    // Already past: immediate.
    let t = Instant::now();
    get(&e.app, &format!("/v1/health/service/web?index={}&wait=10s", first - 1)).await;
    assert!(t.elapsed() < Duration::from_secs(2));
    // Nothing changes: waits out ?wait (plus up to 1/16 jitter).
    let t = Instant::now();
    let r = get(&e.app, &format!("/v1/health/service/web?index={first}&wait=500ms")).await;
    assert!(t.elapsed() >= Duration::from_millis(500), "{:?}", t.elapsed());
    assert_eq!(r.index, Some(first));
    // A change wakes it, with the new index.
    let app = e.app.clone();
    let waiter =
        tokio::spawn(async move { get(&app, &format!("/v1/health/service/web?index={first}&wait=30s")).await });
    tokio::time::sleep(Duration::from_millis(200)).await;
    let mut web3 = ServiceRegistration::new("web", 8083);
    web3.id = Some("web-3".into());
    e.agent.register(web3).await.unwrap();
    let r = tokio::time::timeout(Duration::from_secs(5), waiter).await.expect("woken").unwrap();
    assert!(r.index.unwrap() > first);
    assert_eq!(r.json().as_array().unwrap().len(), 3);
}
