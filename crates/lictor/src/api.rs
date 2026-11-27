//! The agent's native HTTP API (`/v1/...`), JSON in and out.
//!
//! This is Lictor's own API, used by the CLI. Consul compatibility lives in a separate adapter.
//! See docs/http-api.md for the reference.

use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::{Path, RawQuery, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use futures::StreamExt;
use lictor_core::{Registry, RegistryError};
use lictor_types::{
    DiscoveryQuery, HealthCheck, HealthFilter, HealthStatus, HealthUpdate, RegistryScope, ServiceRegistration,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

type Reg = Arc<dyn Registry>;

/// Error body: `{"error": {"code": "not_found", "message": "..."}}`.
#[derive(Debug, Serialize, Deserialize)]
pub struct ErrorBody {
    pub error: ErrorDetail,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ErrorDetail {
    pub code: String,
    pub message: String,
}

pub struct ApiError(RegistryError);

impl From<RegistryError> for ApiError {
    fn from(e: RegistryError) -> Self {
        ApiError(e)
    }
}

pub fn status_for(code: &str) -> StatusCode {
    match code {
        "invalid" => StatusCode::BAD_REQUEST,
        "not_found" => StatusCode::NOT_FOUND,
        "conflict" => StatusCode::CONFLICT,
        "unsupported" => StatusCode::NOT_IMPLEMENTED,
        "fenced" | "not_ready" | "unavailable" => StatusCode::SERVICE_UNAVAILABLE,
        "unauthorized" => StatusCode::UNAUTHORIZED,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn error_response(code: &str, message: String) -> Response {
    let body = ErrorBody { error: ErrorDetail { code: code.into(), message } };
    (status_for(code), Json(body)).into_response()
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        error_response(self.0.code(), self.0.to_string())
    }
}

type ApiResult<T> = Result<Json<T>, ApiError>;

/// Parse a query string into pairs, keeping repeated keys.
fn pairs(raw: &Option<String>) -> Vec<(String, String)> {
    raw.as_deref().map(|q| form_urlencoded::parse(q.as_bytes()).into_owned().collect()).unwrap_or_default()
}

fn scope_of(pairs: &[(String, String)]) -> RegistryScope {
    let get = |k: &str| pairs.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone()).filter(|v| !v.is_empty());
    RegistryScope { namespace: get("namespace"), region: get("region") }
}

/// `?namespace=&region=&tag=a&tag=b&meta=k:v&node=&health=passing` (`?passing` also accepted).
pub fn query_of(service: String, raw: &Option<String>) -> Result<DiscoveryQuery, ApiError> {
    let pairs = pairs(raw);
    let mut q = DiscoveryQuery { service, scope: scope_of(&pairs), ..Default::default() };
    for (k, v) in &pairs {
        match k.as_str() {
            "tag" => q.tags.push(v.clone()),
            "meta" => {
                let (mk, mv) = v
                    .split_once(':')
                    .or_else(|| v.split_once('='))
                    .ok_or_else(|| ApiError(RegistryError::Invalid(format!("meta filter '{v}' must be key:value"))))?;
                q.metadata.insert(mk.into(), mv.into());
            }
            "node" if !v.is_empty() => q.node = Some(v.clone()),
            "health" => q.health = v.parse::<HealthFilter>().map_err(|e| ApiError(RegistryError::Invalid(e)))?,
            "passing" | "healthy" if v.is_empty() || v == "true" || v == "1" => q.health = HealthFilter::Passing,
            _ => {}
        }
    }
    Ok(q)
}

async fn status(State(r): State<Reg>) -> ApiResult<lictor_types::AgentStatus> {
    Ok(Json(r.status().await?))
}

async fn nodes(State(r): State<Reg>, RawQuery(q): RawQuery) -> ApiResult<Vec<lictor_types::NodeInfo>> {
    Ok(Json(r.list_nodes(scope_of(&pairs(&q))).await?))
}

async fn services(State(r): State<Reg>, RawQuery(q): RawQuery) -> ApiResult<Vec<lictor_types::ServiceSummary>> {
    Ok(Json(r.list_services(scope_of(&pairs(&q))).await?))
}

async fn discover(
    State(r): State<Reg>,
    Path(name): Path<String>,
    RawQuery(q): RawQuery,
) -> ApiResult<Vec<lictor_types::ServiceInstance>> {
    Ok(Json(r.discover(query_of(name, &q)?).await?))
}

async fn watch(State(r): State<Reg>, Path(name): Path<String>, RawQuery(q): RawQuery) -> Result<Response, ApiError> {
    let stream = r.watch(query_of(name, &q)?).await?;
    let body = stream.map(|event| {
        let mut line = serde_json::to_vec(&event).expect("serialize event");
        line.push(b'\n');
        Ok::<_, std::convert::Infallible>(line)
    });
    Ok(([(header::CONTENT_TYPE, "application/x-ndjson")], Body::from_stream(body)).into_response())
}

async fn local_services(State(r): State<Reg>) -> ApiResult<Vec<lictor_types::ServiceInstance>> {
    Ok(Json(r.local_services().await?))
}

async fn register(
    State(r): State<Reg>,
    Json(reg): Json<ServiceRegistration>,
) -> Result<(StatusCode, Json<lictor_types::RegistrationReceipt>), ApiError> {
    Ok((StatusCode::CREATED, Json(r.register(reg).await?)))
}

async fn get_service(State(r): State<Reg>, Path(id): Path<String>) -> ApiResult<lictor_types::ServiceInstance> {
    match r.get_service(&id).await? {
        Some(s) => Ok(Json(s)),
        None => Err(RegistryError::NotFound(format!("service '{id}'")).into()),
    }
}

async fn deregister(State(r): State<Reg>, Path(id): Path<String>) -> Result<StatusCode, ApiError> {
    r.deregister(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn local_checks(State(r): State<Reg>) -> ApiResult<Vec<lictor_types::CheckView>> {
    Ok(Json(r.local_checks().await?))
}

#[derive(Serialize, Deserialize)]
pub struct CheckCreated {
    pub check_id: String,
}

async fn register_check(
    State(r): State<Reg>,
    Json(c): Json<HealthCheck>,
) -> Result<(StatusCode, Json<CheckCreated>), ApiError> {
    Ok((StatusCode::CREATED, Json(CheckCreated { check_id: r.register_check(c).await? })))
}

async fn deregister_check(State(r): State<Reg>, Path(id): Path<String>) -> Result<StatusCode, ApiError> {
    r.deregister_check(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Body of `PUT /v1/agent/checks/{id}/{pass|warn|fail}` (optional).
#[derive(Default, Serialize, Deserialize)]
pub struct Note {
    #[serde(default)]
    pub output: String,
}

async fn update_check(
    State(r): State<Reg>,
    Path((id, verb)): Path<(String, String)>,
    body: Option<Json<Note>>,
) -> Result<StatusCode, ApiError> {
    let status = match verb.as_str() {
        "pass" => HealthStatus::Passing,
        "warn" => HealthStatus::Warning,
        "fail" => HealthStatus::Critical,
        other => {
            return Err(RegistryError::Invalid(format!("unknown check update '{other}' (pass, warn or fail)")).into());
        }
    };
    let output = body.map(|b| b.0.output).unwrap_or_default();
    r.update_health(HealthUpdate { check_id: id, status, output }).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn not_found() -> Response {
    error_response("not_found", "no such endpoint".into())
}

fn authorized(headers: &HeaderMap, token: &str) -> bool {
    let bearer =
        headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer "));
    let custom = headers.get("x-lictor-token").and_then(|v| v.to_str().ok());
    let given = bearer.or(custom).unwrap_or("");
    // Constant-time comparison.
    given.len() == token.len() && given.bytes().zip(token.bytes()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

/// The `/v1` router. With a token, every request must carry `Authorization: Bearer <token>`.
pub fn router(registry: Reg, token: Option<String>) -> Router {
    let app = Router::new()
        .route("/v1/status", get(status))
        .route("/v1/nodes", get(nodes))
        .route("/v1/services", get(services))
        .route("/v1/discover/{name}", get(discover))
        .route("/v1/watch/{name}", get(watch))
        .route("/v1/agent/services", get(local_services).post(register))
        .route("/v1/agent/services/{id}", get(get_service).delete(deregister))
        .route("/v1/agent/checks", get(local_checks).post(register_check))
        .route("/v1/agent/checks/{id}", axum::routing::delete(deregister_check))
        .route("/v1/agent/checks/{id}/{verb}", put(update_check))
        .fallback(not_found)
        .with_state(registry);
    match token {
        Some(token) => app.layer(middleware::from_fn(move |req: Request, next: Next| {
            let token = token.clone();
            async move {
                if authorized(req.headers(), &token) {
                    next.run(req).await
                } else {
                    error_response("unauthorized", "missing or wrong API token".into())
                }
            }
        })),
        None => app,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_discovery_queries() {
        let q =
            query_of("web".into(), &Some("tag=a&tag=b&meta=shard:4&node=n1&region=us&passing".into())).ok().unwrap();
        assert_eq!(q.tags, ["a", "b"]);
        assert_eq!(q.metadata["shard"], "4");
        assert_eq!(q.node.as_deref(), Some("n1"));
        assert_eq!(q.scope.region.as_deref(), Some("us"));
        assert_eq!(q.health, HealthFilter::Passing);
        let q = query_of("web".into(), &Some("health=not-critical".into())).ok().unwrap();
        assert_eq!(q.health, HealthFilter::NotCritical);
        assert!(query_of("web".into(), &Some("meta=novalue".into())).is_err());
        assert!(query_of("web".into(), &Some("health=great".into())).is_err());
        assert_eq!(query_of("web".into(), &None).ok().unwrap(), DiscoveryQuery::service("web"));
    }

    #[test]
    fn token_check() {
        let mut h = HeaderMap::new();
        assert!(!authorized(&h, "s3cret"));
        h.insert(header::AUTHORIZATION, "Bearer s3cret".parse().unwrap());
        assert!(authorized(&h, "s3cret"));
        assert!(!authorized(&h, "s3cre"));
        let mut h = HeaderMap::new();
        h.insert("x-lictor-token", "s3cret".parse().unwrap());
        assert!(authorized(&h, "s3cret"));
    }

    #[test]
    fn error_statuses() {
        assert_eq!(status_for("invalid"), StatusCode::BAD_REQUEST);
        assert_eq!(status_for("fenced"), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(status_for("???"), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
