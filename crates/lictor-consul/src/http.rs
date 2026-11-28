//! The read side of Consul's HTTP API: what service discovery clients (Prometheus
//! `consul_sd_configs`, consul-template, Fabio, ...) use to find services.
//!
//! | Endpoint | |
//! |---|---|
//! | `GET /v1/catalog/services` | `{name: [tags]}` |
//! | `GET /v1/catalog/service/:name` | Instances, flattened (`ServiceAddress`, `ServicePort`, ...) |
//! | `GET /v1/health/service/:name` | `[{Node, Service, Checks}]`; `?passing`, `?tag` |
//! | `GET /v1/catalog/nodes`, `/v1/catalog/datacenters` | |
//! | `GET /v1/agent/self`, `/v1/status/leader`, `/v1/status/peers` | Agent identity |
//!
//! Catalog and health endpoints take `?dc=`, `?tag=` (repeatable), `?node-meta=key:value` and
//! blocking queries (`?index=&wait=`), and send `X-Consul-Index`, `X-Consul-KnownLeader` and
//! `X-Consul-LastContact` like Consul (the Go client requires them). Shapes follow Consul 1.22.

use crate::Local;
use axum::Router;
use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use lictor_core::Registry;
use lictor_types::{CheckView, DiscoveryQuery, HealthFilter, HealthStatus, NodeInfo, RegistryScope, ServiceInstance};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone)]
struct Ctx {
    registry: Arc<dyn Registry>,
    local: Arc<Local>,
}

/// Routes for the Consul HTTP adapter.
pub fn router(registry: Arc<dyn Registry>, local: Local) -> Router {
    Router::new()
        .route("/v1/agent/self", get(agent_self))
        .route("/v1/status/leader", get(leader))
        .route("/v1/status/peers", get(peers))
        .route("/v1/catalog/datacenters", get(datacenters))
        .route("/v1/catalog/nodes", get(catalog_nodes))
        .route("/v1/catalog/services", get(catalog_services))
        .route("/v1/catalog/service/{name}", get(catalog_service))
        .route("/v1/health/service/{name}", get(health_service))
        .fallback(|| async {
            text(
                StatusCode::NOT_FOUND,
                "Not found: Lictor serves the read side of Consul's catalog and health API (see docs/consul.md)",
            )
        })
        .with_state(Ctx { registry, local: Arc::new(local) })
}

fn text(status: StatusCode, body: &str) -> Response {
    (status, [(header::CONTENT_TYPE, "text/plain; charset=utf-8")], body.to_string()).into_response()
}

/// An error answer: a status and a plain-text message, as Consul sends them.
struct Reject(StatusCode, String);

impl Reject {
    fn new(status: StatusCode, message: &str) -> Self {
        Reject(status, message.to_string())
    }
}

impl IntoResponse for Reject {
    fn into_response(self) -> Response {
        text(self.0, &self.1)
    }
}

/// Query parameters, keeping repeated keys (`tag=a&tag=b`).
struct Params(Vec<(String, String)>);

impl Params {
    fn parse(raw: &Option<String>) -> Self {
        Params(raw.as_deref().map(|q| form_urlencoded::parse(q.as_bytes()).into_owned().collect()).unwrap_or_default())
    }
    fn get(&self, key: &str) -> Option<&str> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
    fn all(&self, key: &str) -> impl Iterator<Item = &str> {
        self.0.iter().filter(move |(k, _)| k == key).map(|(_, v)| v.as_str())
    }
    fn has(&self, key: &str) -> bool {
        self.0.iter().any(|(k, _)| k == key)
    }
}

/// A Go duration (`5m`, `30s`, `1m30s`, `500ms`). Consul rejects bare numbers.
fn go_duration(text: &str) -> Option<Duration> {
    let mut total = 0f64;
    let mut rest = text.trim();
    if rest.is_empty() {
        return None;
    }
    while !rest.is_empty() {
        let digits = rest.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(rest.len());
        let (num, tail) = rest.split_at(digits);
        let unit_len = tail.find(|c: char| c.is_ascii_digit() || c == '.').unwrap_or(tail.len());
        let (unit, tail) = tail.split_at(unit_len);
        let n: f64 = num.parse().ok()?;
        let factor = match unit {
            "ns" => 1e-9,
            "us" | "µs" => 1e-6,
            "ms" => 1e-3,
            "s" => 1.0,
            "m" => 60.0,
            "h" => 3600.0,
            _ => return None,
        };
        total += n * factor;
        rest = tail;
    }
    Some(Duration::from_secs_f64(total))
}

const DEFAULT_WAIT: Duration = Duration::from_secs(300);
const MAX_WAIT: Duration = Duration::from_secs(600);

impl Ctx {
    /// Run a blocking query: return at once without `?index`, else when the catalog moves past it
    /// or `?wait` (default 5m, at most 10m, plus up to 1/16 jitter like Consul) runs out.
    async fn block(&self, p: &Params) -> Result<u64, Reject> {
        let index = match p.get("index") {
            Some(i) => i.parse::<u64>().map_err(|_| Reject::new(StatusCode::BAD_REQUEST, "Invalid index"))?,
            None => 0,
        };
        let wait = match p.get("wait") {
            Some(w) => go_duration(w).ok_or_else(|| Reject::new(StatusCode::BAD_REQUEST, "Invalid wait time"))?,
            None => DEFAULT_WAIT,
        };
        let wait = if index == 0 {
            Duration::ZERO
        } else {
            let wait = wait.min(MAX_WAIT);
            wait + wait.mul_f64(rand::random::<f64>() / 16.0)
        };
        self.registry
            .wait_for_change(index, wait)
            .await
            .map_err(|e| Reject::new(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))
    }

    /// The datacenter asked for (`?dc=`, default this agent's), which must be one the catalog knows.
    async fn scope(&self, p: &Params) -> Result<RegistryScope, Reject> {
        let dc = p.get("dc").filter(|d| !d.is_empty()).map(str::to_string);
        if let Some(dc) = &dc
            && dc != &self.local.region
            && !self.datacenters().await.contains(dc)
        {
            return Err(Reject::new(StatusCode::INTERNAL_SERVER_ERROR, "No path to datacenter"));
        }
        Ok(RegistryScope {
            namespace: Some(self.local.namespace.clone()),
            region: Some(dc.unwrap_or_else(|| self.local.region.clone())),
        })
    }

    async fn datacenters(&self) -> BTreeSet<String> {
        let scope = RegistryScope { namespace: Some(self.local.namespace.clone()), region: None };
        let mut dcs: BTreeSet<String> =
            self.registry.list_nodes(scope).await.unwrap_or_default().into_iter().map(|n| n.region).collect();
        dcs.insert(self.local.region.clone());
        dcs
    }

    /// Instances of `name` matching the query's tags (case-insensitive), node metadata and health.
    async fn instances(&self, name: &str, p: &Params, scope: RegistryScope) -> Vec<ServiceInstance> {
        let health = if p.has("passing") && !matches!(p.get("passing"), Some("false" | "0")) {
            HealthFilter::Passing
        } else {
            HealthFilter::Any
        };
        let q = DiscoveryQuery { service: name.to_string(), scope, health, ..Default::default() };
        let mut found = self.registry.discover(q).await.unwrap_or_default();
        let tags: Vec<&str> = p.all("tag").collect();
        found.retain(|i| tags.iter().all(|t| i.tags.iter().any(|it| it.eq_ignore_ascii_case(t))));
        let meta: Vec<(&str, &str)> = p.all("node-meta").filter_map(|m| m.split_once(':')).collect();
        found.retain(|i| meta.iter().all(|(k, v)| i.node_metadata.get(*k).is_some_and(|x| x == v)));
        found
    }
}

/// A JSON response with Consul's query metadata headers (`index` for catalog and health reads).
fn reply(body: Value, index: Option<u64>, p: &Params) -> Response {
    let text = if p.has("pretty") { serde_json::to_string_pretty(&body) } else { serde_json::to_string(&body) };
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert("x-consul-default-acl-policy", HeaderValue::from_static("allow"));
    if let Some(index) = index {
        // Consul indexes start at 1; clients treat 0 as "not blocking".
        headers.insert("x-consul-index", HeaderValue::from(index.max(1)));
        headers.insert("x-consul-knownleader", HeaderValue::from_static("true"));
        headers.insert("x-consul-lastcontact", HeaderValue::from_static("0"));
        headers.insert("x-consul-effective-consistency", HeaderValue::from_static("leader"));
    }
    (StatusCode::OK, headers, text.unwrap_or_default()).into_response()
}

/// A stable UUID-shaped node id (Consul nodes have one; Lictor nodes are named).
fn node_uuid(namespace: &str, region: &str, node: &str) -> String {
    let half = |seed: u8| {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (seed, namespace, region, node).hash(&mut h);
        h.finish()
    };
    let v = (u128::from(half(1)) << 64) | u128::from(half(2));
    let s = format!("{v:032x}");
    format!("{}-{}-{}-{}-{}", &s[..8], &s[8..12], &s[12..16], &s[16..20], &s[20..])
}

fn tagged_node_addresses(address: &str) -> Value {
    let family = if address.contains(':') { "ipv6" } else { "ipv4" };
    json!({ "lan": address, format!("lan_{family}"): address, "wan": address, format!("wan_{family}"): address })
}

fn tagged_service_addresses(address: &str, port: u16) -> Value {
    let family = if address.contains(':') { "ipv6" } else { "ipv4" };
    let a = json!({ "Address": address, "Port": port });
    json!({ format!("lan_{family}"): a.clone(), format!("wan_{family}"): a })
}

/// Consul has no "unknown": a check without a result is critical there.
fn status(s: HealthStatus) -> &'static str {
    match s {
        HealthStatus::Passing => "passing",
        HealthStatus::Warning => "warning",
        HealthStatus::Unknown | HealthStatus::Critical => "critical",
    }
}

fn node_json(n: &NodeInfo) -> Value {
    let address = n.addresses.first().cloned().unwrap_or_default();
    json!({
        "ID": node_uuid(&n.namespace, &n.region, &n.node_id),
        "Node": n.node_id,
        "Address": address,
        "Datacenter": n.region,
        "TaggedAddresses": tagged_node_addresses(&address),
        "Meta": n.metadata,
        "CreateIndex": 1,
        "ModifyIndex": 1,
    })
}

fn instance_node_address(i: &ServiceInstance) -> String {
    i.node_addresses.first().cloned().unwrap_or_else(|| i.address.clone())
}

fn instance_node(i: &ServiceInstance) -> Value {
    let address = instance_node_address(i);
    json!({
        "ID": node_uuid(&i.namespace, &i.region, &i.node_id),
        "Node": i.node_id,
        "Address": address,
        "Datacenter": i.region,
        "TaggedAddresses": tagged_node_addresses(&address),
        "Meta": i.node_metadata,
        "CreateIndex": 1,
        "ModifyIndex": 1,
    })
}

fn proxy() -> Value {
    json!({ "Mode": "", "MeshGateway": {}, "Expose": {} })
}

fn check_json(i: &ServiceInstance, c: &CheckView) -> Value {
    json!({
        "Node": i.node_id,
        "CheckID": c.id,
        "Name": c.name,
        "Status": status(c.status),
        "Notes": "",
        "Output": c.output,
        "ServiceID": i.service_id,
        "ServiceName": i.service_name,
        "ServiceTags": i.tags,
        "Type": c.kind,
        "Interval": "",
        "Timeout": "",
        "ExposedPort": 0,
        "Definition": {},
        "CreateIndex": i.revision,
        "ModifyIndex": i.revision,
    })
}

/// The node's own check, as Consul reports it: a live Lictor node is "alive and reachable".
fn serf_health(i: &ServiceInstance) -> Value {
    json!({
        "Node": i.node_id,
        "CheckID": "serfHealth",
        "Name": "Serf Health Status",
        "Status": "passing",
        "Notes": "",
        "Output": "Agent alive and reachable",
        "ServiceID": "",
        "ServiceName": "",
        "ServiceTags": [],
        "Type": "",
        "Interval": "",
        "Timeout": "",
        "ExposedPort": 0,
        "Definition": {},
        "CreateIndex": 1,
        "ModifyIndex": 1,
    })
}

/// Consul has tagged addresses only for services registered with an IP address.
fn is_ip(address: &str) -> bool {
    address.parse::<std::net::IpAddr>().is_ok()
}

fn health_entry(i: &ServiceInstance) -> Value {
    let mut checks = vec![serf_health(i)];
    checks.extend(i.checks.iter().map(|c| check_json(i, c)));
    let mut entry = json!({
        "Node": instance_node(i),
        "Service": {
            "ID": i.service_id,
            "Service": i.service_name,
            "Tags": i.tags,
            "Address": i.address,
            "TaggedAddresses": tagged_service_addresses(&i.address, i.port),
            "Meta": i.metadata,
            "Port": i.port,
            "Weights": { "Passing": 1, "Warning": 1 },
            "EnableTagOverride": false,
            "Proxy": proxy(),
            "Connect": {},
            "PeerName": "",
            "CreateIndex": i.revision,
            "ModifyIndex": i.revision,
        },
        "Checks": checks,
    });
    if !is_ip(&i.address) {
        entry["Service"].as_object_mut().unwrap().remove("TaggedAddresses");
    }
    entry
}

fn catalog_entry(i: &ServiceInstance) -> Value {
    let address = instance_node_address(i);
    let mut entry = json!({
        "ID": node_uuid(&i.namespace, &i.region, &i.node_id),
        "Node": i.node_id,
        "Address": address,
        "Datacenter": i.region,
        "TaggedAddresses": tagged_node_addresses(&address),
        "NodeMeta": i.node_metadata,
        "ServiceKind": "",
        "ServiceID": i.service_id,
        "ServiceName": i.service_name,
        "ServiceTags": i.tags,
        "ServiceAddress": i.address,
        "ServiceTaggedAddresses": tagged_service_addresses(&i.address, i.port),
        "ServiceWeights": { "Passing": 1, "Warning": 1 },
        "ServiceMeta": i.metadata,
        "ServicePort": i.port,
        "ServicePorts": null,
        "ServiceSocketPath": "",
        "ServiceEnableTagOverride": false,
        "ServiceProxy": proxy(),
        "ServiceConnect": {},
        "ServiceLocality": null,
        "CreateIndex": i.revision,
        "ModifyIndex": i.revision,
    });
    if !is_ip(&i.address) {
        entry.as_object_mut().unwrap().remove("ServiceTaggedAddresses");
    }
    entry
}

async fn agent_self(State(c): State<Ctx>, RawQuery(q): RawQuery) -> Response {
    let l = &c.local;
    let addr = l.address.map(|a| a.to_string()).unwrap_or_default();
    let version = c.registry.status().await.map(|s| s.version).unwrap_or_default();
    let body = json!({
        "Config": {
            "Datacenter": l.region,
            "PrimaryDatacenter": l.region,
            "NodeName": l.node_id,
            "NodeID": node_uuid(&l.namespace, &l.region, &l.node_id),
            "Server": false,
            "Version": version,
            "Revision": "lictor",
        },
        "Member": {
            "Name": l.node_id,
            "Addr": addr,
            "Port": 8301,
            "Tags": { "dc": l.region, "role": "node", "lictor": version },
            "Status": 1,
        },
        "Meta": {},
        "Coord": null,
        "Stats": {},
        "DebugConfig": {},
    });
    reply(body, None, &Params::parse(&q))
}

fn leader_addr(c: &Ctx) -> String {
    format!("{}:8300", c.local.address.map(|a| a.to_string()).unwrap_or_else(|| "127.0.0.1".into()))
}

async fn leader(State(c): State<Ctx>, RawQuery(q): RawQuery) -> Response {
    reply(json!(leader_addr(&c)), None, &Params::parse(&q))
}

async fn peers(State(c): State<Ctx>, RawQuery(q): RawQuery) -> Response {
    reply(json!([leader_addr(&c)]), None, &Params::parse(&q))
}

async fn datacenters(State(c): State<Ctx>, RawQuery(q): RawQuery) -> Response {
    // This agent's datacenter first (Consul sorts by distance), then the rest by name.
    let mut dcs: Vec<String> = c.datacenters().await.into_iter().filter(|d| d != &c.local.region).collect();
    dcs.insert(0, c.local.region.clone());
    reply(json!(dcs), None, &Params::parse(&q))
}

/// Shared prologue of catalog and health reads: reject what is not supported, block, pick the scope.
async fn read(c: &Ctx, p: &Params) -> Result<(u64, RegistryScope), Reject> {
    if p.get("filter").is_some_and(|f| !f.is_empty()) {
        return Err(Reject::new(
            StatusCode::BAD_REQUEST,
            "filter expressions (?filter=) are not supported by Lictor; use ?tag= and ?node-meta=",
        ));
    }
    let scope = c.scope(p).await?;
    let index = c.block(p).await?;
    Ok((index, scope))
}

async fn catalog_nodes(State(c): State<Ctx>, RawQuery(q): RawQuery) -> Response {
    let p = Params::parse(&q);
    let (index, scope) = match read(&c, &p).await {
        Ok(r) => r,
        Err(e) => return e.into_response(),
    };
    let meta: Vec<(&str, &str)> = p.all("node-meta").filter_map(|m| m.split_once(':')).collect();
    let mut nodes = c.registry.list_nodes(scope).await.unwrap_or_default();
    nodes.retain(|n| meta.iter().all(|(k, v)| n.metadata.get(*k).is_some_and(|x| x == v)));
    nodes.sort_by(|a, b| a.node_id.cmp(&b.node_id));
    reply(Value::Array(nodes.iter().map(node_json).collect()), Some(index), &p)
}

async fn catalog_services(State(c): State<Ctx>, RawQuery(q): RawQuery) -> Response {
    let p = Params::parse(&q);
    let (index, scope) = match read(&c, &p).await {
        Ok(r) => r,
        Err(e) => return e.into_response(),
    };
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    if p.has("node-meta") {
        // Node metadata lives on instances: look at them.
        for s in c.registry.list_services(scope.clone()).await.unwrap_or_default() {
            let found = c.instances(&s.name, &p, scope.clone()).await;
            if !found.is_empty() {
                let tags: BTreeSet<String> = found.into_iter().flat_map(|i| i.tags).collect();
                out.insert(s.name, tags.into_iter().collect());
            }
        }
    } else {
        for s in c.registry.list_services(scope).await.unwrap_or_default() {
            out.insert(s.name, s.tags);
        }
    }
    reply(json!(out), Some(index), &p)
}

async fn catalog_service(State(c): State<Ctx>, Path(name): Path<String>, RawQuery(q): RawQuery) -> Response {
    let p = Params::parse(&q);
    let (index, scope) = match read(&c, &p).await {
        Ok(r) => r,
        Err(e) => return e.into_response(),
    };
    let found = c.instances(&name, &p, scope).await;
    reply(Value::Array(found.iter().map(catalog_entry).collect()), Some(index), &p)
}

async fn health_service(State(c): State<Ctx>, Path(name): Path<String>, RawQuery(q): RawQuery) -> Response {
    let p = Params::parse(&q);
    let (index, scope) = match read(&c, &p).await {
        Ok(r) => r,
        Err(e) => return e.into_response(),
    };
    let found = c.instances(&name, &p, scope).await;
    reply(Value::Array(found.iter().map(health_entry).collect()), Some(index), &p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_go_durations() {
        assert_eq!(go_duration("5m"), Some(Duration::from_secs(300)));
        assert_eq!(go_duration("1m30s"), Some(Duration::from_secs(90)));
        assert_eq!(go_duration("500ms"), Some(Duration::from_millis(500)));
        assert_eq!(go_duration("1.5s"), Some(Duration::from_millis(1500)));
        for bad in ["", "1", "5x", "s", "m5"] {
            assert_eq!(go_duration(bad), None, "{bad}");
        }
    }

    #[test]
    fn node_ids_are_stable_uuids() {
        let a = node_uuid("default", "dc1", "host-a");
        assert_eq!(a, node_uuid("default", "dc1", "host-a"));
        assert_ne!(a, node_uuid("default", "dc1", "host-b"));
        assert_eq!(a.len(), 36);
        assert_eq!(a.matches('-').count(), 4);
    }
}
