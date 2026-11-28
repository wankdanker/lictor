//! A [`Registry`] that talks to an agent's native HTTP API. Used by the CLI.

use crate::api::{CheckCreated, ErrorBody, Note};
use async_trait::async_trait;
use futures::StreamExt;
use lictor_core::{Registry, RegistryError, RegistryEventStream, Result};
use lictor_store::StoreError;
use lictor_types::{
    AgentStatus, CheckView, DiscoveryQuery, HealthCheck, HealthFilter, HealthStatus, HealthUpdate, NodeInfo,
    RegistrationReceipt, RegistryEvent, RegistryScope, ServiceInstance, ServiceRegistration, ServiceSummary,
};
use serde::de::DeserializeOwned;

pub struct HttpRegistry {
    base: String,
    token: Option<String>,
    http: reqwest::Client,
}

fn encode(s: &str) -> String {
    form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

fn scope_query(scope: &RegistryScope) -> Vec<(String, String)> {
    let mut q = Vec::new();
    if let Some(n) = &scope.namespace {
        q.push(("namespace".into(), n.clone()));
    }
    if let Some(r) = &scope.region {
        q.push(("region".into(), r.clone()));
    }
    q
}

fn discovery_query(query: &DiscoveryQuery) -> Vec<(String, String)> {
    let mut q = scope_query(&query.scope);
    q.extend(query.tags.iter().map(|t| ("tag".to_string(), t.clone())));
    q.extend(query.metadata.iter().map(|(k, v)| ("meta".to_string(), format!("{k}:{v}"))));
    if let Some(n) = &query.node {
        q.push(("node".into(), n.clone()));
    }
    if query.health != HealthFilter::Any {
        q.push(("health".into(), query.health.to_string()));
    }
    q
}

fn from_code(code: &str, message: String) -> RegistryError {
    match code {
        "invalid" => RegistryError::Invalid(message),
        "not_found" => RegistryError::NotFound(message),
        "conflict" => RegistryError::Conflict(message),
        "fenced" => RegistryError::Fenced(message),
        "unsupported" => RegistryError::Unsupported(message),
        "not_ready" => RegistryError::NotReady(message),
        "unavailable" => RegistryError::Store(StoreError::Unavailable(message)),
        _ => RegistryError::Store(StoreError::Other(message)),
    }
}

/// Error messages from the server already include their prefix ("invalid request: ..."); strip it
/// so re-wrapping in the same variant does not repeat it.
fn strip_prefix(code: &str, message: String) -> String {
    let prefix = match code {
        "invalid" => "invalid request: ",
        "conflict" => "conflict: ",
        "unsupported" => "unsupported: ",
        "not_ready" => "not ready: ",
        "unavailable" => "store unavailable: ",
        _ => "",
    };
    match code {
        "not_found" => message.strip_suffix(" not found").map(str::to_string).unwrap_or(message),
        _ => message.strip_prefix(prefix).map(str::to_string).unwrap_or(message),
    }
}

impl HttpRegistry {
    /// `addr` is `host:port` or a full `http(s)://` URL.
    pub fn new(addr: &str, token: Option<String>) -> Self {
        let base = if addr.starts_with("http://") || addr.starts_with("https://") {
            addr.trim_end_matches('/').to_string()
        } else {
            format!("http://{addr}")
        };
        let _ = rustls::crypto::ring::default_provider().install_default();
        let http = reqwest::Client::builder()
            .user_agent(concat!("lictor-cli/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("HTTP client");
        HttpRegistry { base, token, http }
    }

    fn unreachable(&self, e: reqwest::Error) -> RegistryError {
        RegistryError::Store(StoreError::Unavailable(format!(
            "cannot reach the agent at {} ({}). Is 'lictor agent' running? Use --direct to read from NATS instead",
            self.base,
            e.without_url()
        )))
    }

    fn request(&self, method: reqwest::Method, path: &str, query: &[(String, String)]) -> reqwest::RequestBuilder {
        let mut url = format!("{}{path}", self.base);
        if !query.is_empty() {
            let qs: Vec<String> = query.iter().map(|(k, v)| format!("{}={}", encode(k), encode(v))).collect();
            url.push('?');
            url.push_str(&qs.join("&"));
        }
        let mut req = self.http.request(method, url);
        if let Some(t) = &self.token {
            req = req.bearer_auth(t);
        }
        req
    }

    async fn send(&self, req: reqwest::RequestBuilder) -> Result<reqwest::Response> {
        let resp = req.send().await.map_err(|e| self.unreachable(e))?;
        if resp.status().is_success() {
            return Ok(resp);
        }
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        Err(match serde_json::from_str::<ErrorBody>(&text) {
            Ok(b) if b.error.code == "unauthorized" => {
                RegistryError::Invalid("the agent rejected the API token (set --token or LICTOR_HTTP_TOKEN)".into())
            }
            Ok(b) => from_code(&b.error.code, strip_prefix(&b.error.code, b.error.message)),
            Err(_) => RegistryError::Store(StoreError::Other(format!("agent answered {status}: {text}"))),
        })
    }

    async fn json<T: DeserializeOwned>(&self, req: reqwest::RequestBuilder) -> Result<T> {
        let resp = self.send(req).await?;
        resp.json::<T>().await.map_err(|e| RegistryError::Store(StoreError::Other(format!("bad response: {e}"))))
    }

    async fn get<T: DeserializeOwned>(&self, path: &str, query: &[(String, String)]) -> Result<T> {
        self.json(self.request(reqwest::Method::GET, path, query)).await
    }
}

#[async_trait]
impl Registry for HttpRegistry {
    async fn register(&self, service: ServiceRegistration) -> Result<RegistrationReceipt> {
        self.json(self.request(reqwest::Method::POST, "/v1/agent/services", &[]).json(&service)).await
    }

    async fn deregister(&self, service_id: &str) -> Result<()> {
        let path = format!("/v1/agent/services/{}", encode(service_id));
        self.send(self.request(reqwest::Method::DELETE, &path, &[])).await.map(|_| ())
    }

    async fn discover(&self, query: DiscoveryQuery) -> Result<Vec<ServiceInstance>> {
        self.get(&format!("/v1/discover/{}", encode(&query.service)), &discovery_query(&query)).await
    }

    async fn get_service(&self, service_id: &str) -> Result<Option<ServiceInstance>> {
        match self.get(&format!("/v1/agent/services/{}", encode(service_id)), &[]).await {
            Ok(s) => Ok(Some(s)),
            Err(RegistryError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    async fn service_definition(&self, service_id: &str) -> Result<Option<ServiceRegistration>> {
        match self.get(&format!("/v1/agent/services/{}/definition", encode(service_id)), &[]).await {
            Ok(s) => Ok(Some(s)),
            Err(RegistryError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    async fn local_services(&self) -> Result<Vec<ServiceInstance>> {
        self.get("/v1/agent/services", &[]).await
    }

    async fn local_checks(&self) -> Result<Vec<CheckView>> {
        self.get("/v1/agent/checks", &[]).await
    }

    async fn list_services(&self, scope: RegistryScope) -> Result<Vec<ServiceSummary>> {
        self.get("/v1/services", &scope_query(&scope)).await
    }

    async fn list_nodes(&self, scope: RegistryScope) -> Result<Vec<NodeInfo>> {
        self.get("/v1/nodes", &scope_query(&scope)).await
    }

    async fn register_check(&self, check: HealthCheck) -> Result<String> {
        let c: CheckCreated =
            self.json(self.request(reqwest::Method::POST, "/v1/agent/checks", &[]).json(&check)).await?;
        Ok(c.check_id)
    }

    async fn deregister_check(&self, check_id: &str) -> Result<()> {
        let path = format!("/v1/agent/checks/{}", encode(check_id));
        self.send(self.request(reqwest::Method::DELETE, &path, &[])).await.map(|_| ())
    }

    async fn update_health(&self, update: HealthUpdate) -> Result<()> {
        let verb = match update.status {
            HealthStatus::Passing => "pass",
            HealthStatus::Warning => "warn",
            _ => "fail",
        };
        let path = format!("/v1/agent/checks/{}/{verb}", encode(&update.check_id));
        let req = self.request(reqwest::Method::PUT, &path, &[]).json(&Note { output: update.output });
        self.send(req).await.map(|_| ())
    }

    async fn watch(&self, query: DiscoveryQuery) -> Result<RegistryEventStream> {
        let path = format!("/v1/watch/{}", encode(&query.service));
        let resp = self.send(self.request(reqwest::Method::GET, &path, &discovery_query(&query))).await?;
        let bytes = resp.bytes_stream();
        let lines = futures::stream::unfold((bytes, Vec::<u8>::new()), |(mut bytes, mut buf)| async move {
            loop {
                if let Some(pos) = buf.iter().position(|b| *b == b'\n') {
                    let line: Vec<u8> = buf.drain(..=pos).collect();
                    if let Ok(event) = serde_json::from_slice::<RegistryEvent>(&line) {
                        return Some((event, (bytes, buf)));
                    }
                    continue;
                }
                match bytes.next().await {
                    Some(Ok(chunk)) => buf.extend_from_slice(&chunk),
                    _ => return None,
                }
            }
        });
        Ok(lines.boxed())
    }

    async fn wait_for_change(&self, _: u64, _: std::time::Duration) -> Result<u64> {
        Err(RegistryError::Unsupported("blocking on catalog changes needs a local agent or --direct".into()))
    }

    async fn status(&self) -> Result<AgentStatus> {
        self.get("/v1/status", &[]).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_urls() {
        assert_eq!(HttpRegistry::new("127.0.0.1:7171", None).base, "http://127.0.0.1:7171");
        assert_eq!(HttpRegistry::new("https://agent:7171/", None).base, "https://agent:7171");
    }

    #[test]
    fn error_round_trip_strips_prefixes() {
        let e = RegistryError::Invalid("bad".into());
        let back = from_code(e.code(), strip_prefix(e.code(), e.to_string()));
        assert_eq!(back, e);
        let e = RegistryError::NotFound("service 'x'".into());
        assert_eq!(from_code(e.code(), strip_prefix(e.code(), e.to_string())), e);
    }

    #[test]
    fn query_encoding() {
        let mut q = DiscoveryQuery::service("web").tag("v 2").region("us");
        q.metadata.insert("k".into(), "v".into());
        q.health = HealthFilter::Passing;
        let pairs = discovery_query(&q);
        assert!(pairs.contains(&("tag".into(), "v 2".into())));
        assert!(pairs.contains(&("meta".into(), "k:v".into())));
        assert!(pairs.contains(&("health".into(), "passing".into())));
        assert_eq!(encode("a b/c"), "a+b%2Fc");
    }
}
