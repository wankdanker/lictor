//! What each command does. Every command works against `dyn Registry`: the
//! local agent over HTTP by default, or the NATS catalog with `--direct`.

use crate::client::HttpRegistry;
use crate::output::{self, Format};
use clyops::Values;
use futures::StreamExt;
use lictor_core::{Observer, Registry, RegistryConfig, RegistryError};
use lictor_store_nats::{NatsOptions, NatsStore};
use lictor_types::{
    CheckDefinition, CheckKind, DiscoveryQuery, HealthCheck, HealthFilter, HealthStatus, HealthUpdate, RegistryScope,
    ServiceRegistration, parse_duration,
};
use std::collections::BTreeMap;
use std::io::Write;
use std::sync::Arc;

/// A failed command: message, stable code and process exit status.
#[derive(Debug)]
pub struct CliError {
    pub code: &'static str,
    pub message: String,
    pub exit: u8,
}

pub const EXIT_ERROR: u8 = 1;
pub const EXIT_NOT_FOUND: u8 = 2;
pub const EXIT_UNAVAILABLE: u8 = 3;
pub const EXIT_FENCED: u8 = 4;

impl CliError {
    pub fn invalid(message: impl Into<String>) -> Self {
        CliError { code: "invalid", message: message.into(), exit: EXIT_ERROR }
    }
    pub fn not_found(message: impl Into<String>) -> Self {
        CliError { code: "not_found", message: message.into(), exit: EXIT_NOT_FOUND }
    }
}

impl From<RegistryError> for CliError {
    fn from(e: RegistryError) -> Self {
        let code = e.code();
        let exit = match code {
            "not_found" => EXIT_NOT_FOUND,
            "unavailable" | "not_ready" => EXIT_UNAVAILABLE,
            "fenced" => EXIT_FENCED,
            _ => EXIT_ERROR,
        };
        let message = match &e {
            RegistryError::Unsupported(_) => format!("{e} (this command needs the local agent; drop --direct)"),
            _ => e.to_string(),
        };
        CliError { code, message, exit }
    }
}

pub type CmdResult = Result<String, CliError>;

pub fn opt(v: &Values, var: &str) -> Option<String> {
    let s = v.str(var);
    (!s.is_empty()).then(|| s.to_string())
}

pub fn duration(v: &Values, var: &str, what: &str) -> Result<Option<std::time::Duration>, CliError> {
    opt(v, var).map(|s| parse_duration(&s).map_err(|e| CliError::invalid(format!("{what}: {e}")))).transpose()
}

pub fn nats_options(v: &Values) -> NatsOptions {
    NatsOptions {
        servers: v.str("LICTOR_NATS_URL").split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        name: Some(format!("lictor {}", crate::sys::hostname())),
        credentials_file: opt(v, "LICTOR_NATS_CREDS").map(Into::into),
        nkey_file: opt(v, "LICTOR_NATS_NKEY").map(Into::into),
        user: opt(v, "LICTOR_NATS_USER"),
        password: opt(v, "LICTOR_NATS_PASSWORD"),
        token: opt(v, "LICTOR_NATS_TOKEN"),
        tls_ca: opt(v, "LICTOR_NATS_TLS_CA").map(Into::into),
        tls_cert: opt(v, "LICTOR_NATS_TLS_CERT").map(Into::into),
        tls_key: opt(v, "LICTOR_NATS_TLS_KEY").map(Into::into),
        require_tls: v.bool("LICTOR_NATS_TLS_REQUIRED"),
        connect_timeout: None,
        retry_on_initial_connect: false,
    }
}

async fn registry(v: &Values) -> Result<Arc<dyn Registry>, CliError> {
    if v.bool("LICTOR_DIRECT") {
        let store = NatsStore::connect(&nats_options(v)).await.map_err(RegistryError::from)?;
        let cfg = RegistryConfig { bucket_prefix: v.str("LICTOR_BUCKET_PREFIX").to_string(), ..Default::default() };
        let observer = Observer::start(Arc::new(store), cfg).await.map_err(|e| match e {
            RegistryError::Store(lictor_store::StoreError::BucketNotFound(b)) => CliError {
                code: "not_found",
                message: format!("bucket '{b}' does not exist yet; start an agent first (or check --bucket-prefix)"),
                exit: EXIT_NOT_FOUND,
            },
            other => other.into(),
        })?;
        Ok(Arc::new(observer))
    } else {
        Ok(Arc::new(HttpRegistry::new(v.str("LICTOR_HTTP_ADDR"), opt(v, "LICTOR_HTTP_TOKEN"))))
    }
}

fn scope(v: &Values) -> RegistryScope {
    RegistryScope { namespace: opt(v, "LICTOR_SCOPE_NAMESPACE"), region: opt(v, "LICTOR_SCOPE_REGION") }
}

fn query(v: &Values) -> Result<DiscoveryQuery, CliError> {
    let metadata = lictor_types::validate::parse_pairs(v.strs("LICTOR_META")).map_err(CliError::invalid)?;
    let mut health: HealthFilter = v.str("LICTOR_HEALTH").parse().map_err(CliError::invalid)?;
    if v.bool("LICTOR_HEALTHY") {
        health = HealthFilter::Passing;
    }
    Ok(DiscoveryQuery {
        service: v.str("name").to_string(),
        scope: scope(v),
        tags: v.strs("LICTOR_TAG").into_iter().map(String::from).collect(),
        metadata,
        node: opt(v, "LICTOR_FILTER_NODE"),
        health,
    })
}

/// Build check definitions from `--check-*` (prefix "check") or plain (prefix "") options.
fn checks_from(v: &Values) -> Result<Vec<CheckDefinition>, CliError> {
    let mut kinds = Vec::new();
    if let Some(addr) = opt(v, "LICTOR_CHECK_TCP") {
        kinds.push(CheckKind::Tcp { address: addr });
    }
    if let Some(url) = opt(v, "LICTOR_CHECK_HTTP") {
        kinds.push(CheckKind::Http {
            url,
            method: v.str("LICTOR_CHECK_METHOD").to_uppercase(),
            headers: BTreeMap::new(),
            tls_skip_verify: v.bool("LICTOR_CHECK_TLS_SKIP_VERIFY"),
        });
    }
    if let Some(ttl) = duration(v, "LICTOR_CHECK_TTL", "ttl")? {
        kinds.push(CheckKind::Ttl { ttl });
    }
    let interval = duration(v, "LICTOR_CHECK_INTERVAL", "check interval")?;
    let timeout = duration(v, "LICTOR_CHECK_TIMEOUT", "check timeout")?;
    let deregister = duration(v, "LICTOR_CHECK_DEREGISTER_AFTER", "deregister after")?;
    let initial: HealthStatus = v.str("LICTOR_CHECK_INITIAL_STATUS").parse().map_err(CliError::invalid)?;
    kinds
        .into_iter()
        .map(|kind| {
            let mut c = CheckDefinition::new(kind);
            c.interval = interval;
            c.timeout = timeout;
            c.deregister_critical_after = deregister;
            c.initial_status = initial;
            c.failures_before_critical = v.int("LICTOR_CHECK_FAILURES") as u32;
            c.successes_before_passing = v.int("LICTOR_CHECK_SUCCESSES") as u32;
            c.validate().map_err(CliError::invalid)?;
            Ok(c)
        })
        .collect()
}

fn registration(v: &Values) -> Result<Vec<ServiceRegistration>, CliError> {
    if let Some(file) = opt(v, "LICTOR_SERVICE_FILE") {
        return crate::confdir::read_file(&file).map_err(CliError::invalid);
    }
    let reg = ServiceRegistration {
        id: opt(v, "LICTOR_SERVICE_ID"),
        name: v.str("LICTOR_SERVICE_NAME").to_string(),
        address: opt(v, "LICTOR_SERVICE_ADDRESS"),
        port: v.int("LICTOR_SERVICE_PORT") as u16,
        protocol: opt(v, "LICTOR_SERVICE_PROTOCOL"),
        tags: v.strs("LICTOR_SERVICE_TAG").into_iter().map(String::from).collect(),
        metadata: lictor_types::validate::parse_pairs(v.strs("LICTOR_SERVICE_META")).map_err(CliError::invalid)?,
        checks: checks_from(v)?,
    };
    reg.validate().map_err(CliError::invalid)?;
    Ok(vec![reg])
}

fn render<T: serde::Serialize + ?Sized>(f: Format, value: &T, text: impl FnOnce() -> String) -> String {
    match f {
        Format::Json => output::json(value),
        Format::Text => text(),
    }
}

pub async fn run(path: &[String], v: &Values) -> CmdResult {
    let f = Format::parse(v.str("LICTOR_OUTPUT"));
    let words: Vec<&str> = path.iter().map(String::as_str).collect();
    if words == ["version"] {
        let version = env!("CARGO_PKG_VERSION");
        return Ok(render(f, &serde_json::json!({ "version": version }), || format!("lictor {version}\n")));
    }
    let reg = registry(v).await?;
    match words.as_slice() {
        ["status"] => {
            let s = reg.status().await?;
            let addr = (!v.bool("LICTOR_DIRECT")).then(|| v.str("LICTOR_HTTP_ADDR"));
            Ok(render(f, &s, || output::status(&s, addr)))
        }
        ["nodes", "list"] => {
            let n = reg.list_nodes(scope(v)).await?;
            Ok(render(f, &n, || output::nodes(&n)))
        }
        ["services", "list"] => {
            let s = reg.list_services(scope(v)).await?;
            Ok(render(f, &s, || output::summaries(&s)))
        }
        ["services", "inspect"] => {
            let q = query(v)?;
            let found = reg.discover(q.clone()).await?;
            if found.is_empty() {
                return Err(CliError::not_found(format!(
                    "service '{}' has no live instances matching the filters",
                    q.service
                )));
            }
            Ok(render(f, &found, || output::instance_details(&found, "")))
        }
        ["services", "local"] => {
            let s = reg.local_services().await?;
            Ok(render(f, &s, || output::instances(&s)))
        }
        ["services", "register"] => {
            let mut receipts = Vec::new();
            for r in registration(v)? {
                receipts.push(reg.register(r).await?);
            }
            Ok(render(f, &receipts, || {
                receipts
                    .iter()
                    .map(|r| {
                        let checks = if r.checks.is_empty() {
                            String::new()
                        } else {
                            format!(", checks: {}", r.checks.join(", "))
                        };
                        format!(
                            "Registered service '{}' (id {}, revision {}{checks})\n",
                            r.service_name, r.service_id, r.revision
                        )
                    })
                    .collect()
            }))
        }
        ["services", "export"] => {
            let id = v.str("id");
            let def =
                reg.service_definition(id).await?.ok_or_else(|| RegistryError::NotFound(format!("service '{id}'")))?;
            // Always JSON: the output is meant to be saved and passed back to `register --file x.json`.
            Ok(format!("{}\n", serde_json::to_string_pretty(&def).expect("serialize definition")))
        }
        ["services", "deregister"] => {
            let id = v.str("id");
            reg.deregister(id).await?;
            Ok(render(f, &serde_json::json!({ "deregistered": id }), || format!("Deregistered service '{id}'\n")))
        }
        ["discover"] => {
            let found = reg.discover(query(v)?).await?;
            Ok(render(f, &found, || output::instances(&found)))
        }
        ["watch"] => watch(reg, query(v)?, f).await,
        ["health", "status"] => match v.strs("name").first().map(|s| s.to_string()) {
            Some(name) => {
                let mut q = DiscoveryQuery::service(name.clone());
                q.scope = scope(v);
                let found = reg.discover(q).await?;
                if found.is_empty() {
                    return Err(CliError::not_found(format!("service '{name}' has no live instances")));
                }
                Ok(render(f, &found, || output::instance_details(&found, "")))
            }
            None => {
                let s = reg.list_services(scope(v)).await?;
                Ok(render(f, &s, || output::summaries(&s)))
            }
        },
        ["checks", "list"] => {
            let c = reg.local_checks().await?;
            Ok(render(f, &c, || output::checks(&c)))
        }
        ["checks", "register"] => {
            let mut check =
                checks_from(v)?.pop().ok_or_else(|| CliError::invalid("give one of --tcp, --http or --ttl"))?;
            check.id = opt(v, "LICTOR_CHECK_ID").unwrap_or_default();
            check.name = opt(v, "LICTOR_CHECK_NAME").unwrap_or_default();
            check.validate().map_err(CliError::invalid)?;
            let service_id = v.str("LICTOR_CHECK_SERVICE").to_string();
            let id = reg.register_check(HealthCheck { service_id: service_id.clone(), check }).await?;
            Ok(render(f, &serde_json::json!({ "check_id": id, "service_id": service_id }), || {
                format!("Registered check '{id}' on service '{service_id}'\n")
            }))
        }
        ["checks", "deregister"] => {
            let id = v.str("id");
            reg.deregister_check(id).await?;
            Ok(render(f, &serde_json::json!({ "deregistered": id }), || format!("Deregistered check '{id}'\n")))
        }
        ["checks", verb @ ("pass" | "warn" | "fail")] => {
            let status = match *verb {
                "pass" => HealthStatus::Passing,
                "warn" => HealthStatus::Warning,
                _ => HealthStatus::Critical,
            };
            let id = v.str("id").to_string();
            let output = opt(v, "LICTOR_CHECK_NOTE").unwrap_or_default();
            reg.update_health(HealthUpdate { check_id: id.clone(), status, output }).await?;
            Ok(render(f, &serde_json::json!({ "check_id": id, "status": status }), || {
                format!("Check '{id}' is now {status}\n")
            }))
        }
        other => Err(CliError::invalid(format!("unknown command '{}'", other.join(" ")))),
    }
}

async fn watch(reg: Arc<dyn Registry>, q: DiscoveryQuery, f: Format) -> CmdResult {
    let mut events = reg.watch(q.clone()).await?;
    let mut stdout = std::io::stdout();
    loop {
        let event = tokio::select! {
            e = events.next() => e,
            _ = tokio::signal::ctrl_c() => break,
        };
        let Some(event) = event else {
            return Err(CliError {
                code: "unavailable",
                message: "watch ended: lost the connection".into(),
                exit: EXIT_UNAVAILABLE,
            });
        };
        let text = match f {
            Format::Json => format!("{}\n", serde_json::to_string(&event).expect("serialize")),
            Format::Text => format!(
                "--- {} instance(s) of '{}' (index {}) ---\n{}",
                event.instances.len(),
                q.service,
                event.index,
                output::instances(&event.instances)
            ),
        };
        if stdout.write_all(text.as_bytes()).and_then(|_| stdout.flush()).is_err() {
            break;
        }
    }
    Ok(String::new())
}
