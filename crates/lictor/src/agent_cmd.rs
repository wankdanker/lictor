//! `lictor agent`: the long-running process.

use crate::commands::{CliError, EXIT_FENCED, EXIT_UNAVAILABLE, duration, nats_options, opt};
use crate::sys;
use clyops::Values;
use lictor_core::{Agent, AgentConfig, Registry, RegistryConfig, RegistryError};
use lictor_store::{MemoryStore, Store};
use lictor_store_nats::NatsStore;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

fn required_duration(v: &Values, var: &str, what: &str) -> Result<std::time::Duration, CliError> {
    duration(v, var, what)?.ok_or_else(|| CliError::invalid(format!("{what} is required")))
}

/// Translate options into an agent configuration.
pub fn config(v: &Values, dev: bool) -> Result<AgentConfig, CliError> {
    let node_id = match opt(v, "LICTOR_NODE_ID") {
        Some(id) => id,
        None if dev => "dev".into(),
        None => {
            let dir = opt(v, "LICTOR_DATA_DIR").map(PathBuf::from).unwrap_or_else(sys::default_data_dir);
            sys::node_id(&dir).map_err(CliError::invalid)?
        }
    };
    let mut cfg = AgentConfig::new(node_id);
    cfg.namespace = v.str("LICTOR_NAMESPACE").to_string();
    cfg.region = v.str("LICTOR_REGION").to_string();
    cfg.hostname = opt(v, "LICTOR_HOSTNAME").unwrap_or_else(sys::hostname);
    cfg.addresses = match opt(v, "LICTOR_ADVERTISE") {
        Some(list) => list.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        None if dev => vec!["127.0.0.1".into()],
        None => vec![sys::primary_address()],
    };
    cfg.metadata = match opt(v, "LICTOR_NODE_META") {
        Some(m) => lictor_types::validate::parse_pairs(m.split(',')).map_err(CliError::invalid)?,
        None => BTreeMap::new(),
    };
    cfg.metadata.entry("os".into()).or_insert_with(|| std::env::consts::OS.into());
    cfg.metadata.entry("arch".into()).or_insert_with(|| std::env::consts::ARCH.into());
    cfg.registry = RegistryConfig {
        bucket_prefix: v.str("LICTOR_BUCKET_PREFIX").to_string(),
        replicas: v.int("LICTOR_REPLICAS") as usize,
        membership_ttl: required_duration(v, "LICTOR_MEMBERSHIP_TTL", "membership ttl")?,
        health_ttl: required_duration(v, "LICTOR_HEALTH_TTL", "health ttl")?,
        create_buckets: v.bool("LICTOR_CREATE_BUCKETS"),
        clock_skew: required_duration(v, "LICTOR_CLOCK_SKEW", "clock skew")?,
        ..Default::default()
    };
    cfg.heartbeat_interval = required_duration(v, "LICTOR_HEARTBEAT_INTERVAL", "heartbeat interval")?;
    cfg.check_interval = required_duration(v, "LICTOR_CHECK_DEFAULT_INTERVAL", "default check interval")?;
    cfg.check_timeout = required_duration(v, "LICTOR_CHECK_DEFAULT_TIMEOUT", "default check timeout")?;
    cfg.max_concurrent_checks = v.int("LICTOR_MAX_CONCURRENT_CHECKS") as usize;
    cfg.reconcile = v.str("LICTOR_RECONCILE").parse().map_err(CliError::invalid)?;
    let retention = required_duration(v, "LICTOR_STALE_RETENTION", "stale retention")?;
    cfg.stale_retention = (!retention.is_zero()).then_some(retention);
    cfg.validate().map_err(CliError::invalid)?;
    Ok(cfg)
}

async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        let mut term =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("signal handler");
        tokio::select! {
            _ = ctrl_c => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = ctrl_c.await;
    }
}

pub async fn run(v: &Values) -> Result<(), CliError> {
    let dev = v.bool("LICTOR_DEV");
    let cfg = config(v, dev)?;
    let addr: SocketAddr = v
        .str("LICTOR_HTTP_ADDR")
        .trim_start_matches("http://")
        .parse()
        .map_err(|e| CliError::invalid(format!("--http-addr must be ip:port: {e}")))?;
    let token = opt(v, "LICTOR_HTTP_TOKEN");
    if !addr.ip().is_loopback() && token.is_none() {
        tracing::warn!(%addr, "the HTTP API is exposed beyond loopback without --token; anyone who can reach it can register services");
    }

    let store: Arc<dyn Store> = if dev {
        tracing::warn!("development mode: in-memory store, nothing is persisted or shared with other agents");
        Arc::new(MemoryStore::new())
    } else {
        let options = nats_options(v);
        tracing::info!(servers = ?options.servers, "connecting to NATS");
        Arc::new(NatsStore::connect(&options).await.map_err(|e| CliError {
            code: "unavailable",
            message: e.to_string(),
            exit: EXIT_UNAVAILABLE,
        })?)
    };

    let agent = Agent::start(store, cfg).await?;

    if let Some(dir) = opt(v, "LICTOR_CONFIG_DIR") {
        let files = crate::confdir::read_dir(std::path::Path::new(&dir)).map_err(CliError::invalid)?;
        for (path, regs) in files {
            for reg in regs {
                let r = agent.register(reg).await.map_err(|e| CliError::invalid(format!("{}: {e}", path.display())))?;
                tracing::info!(service = %r.service_id, file = %path.display(), "registered from config dir");
            }
        }
    }

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| CliError::invalid(format!("cannot listen on {addr}: {e}")))?;
    // Log the bound address, so `--http-addr 127.0.0.1:0` reports the port it got.
    let bound = listener.local_addr().map(|a| a.to_string()).unwrap_or_else(|_| addr.to_string());
    tracing::info!(addr = %bound, "HTTP API listening");
    let registry: Arc<dyn Registry> = Arc::new(agent.clone());
    let mut app = crate::api::router(registry, token);
    if v.bool("LICTOR_UI") {
        // Added after the API's token layer: the UI's files are public, its API calls are not.
        app = app.merge(crate::ui::router());
        if crate::ui::embedded() {
            tracing::info!(url = %format!("http://{bound}{}", crate::ui::BASE), "web UI");
        } else {
            tracing::info!("web UI not built into this binary; see docs/ui.md");
        }
    }

    let stopped = agent.clone();
    let server = axum::serve(listener, app).with_graceful_shutdown(async move {
        tokio::select! {
            _ = shutdown_signal() => tracing::info!("shutting down"),
            _ = stopped.stopped() => {}
        }
    });
    if let Err(e) = server.await {
        tracing::error!(error = %e, "HTTP server failed");
    }

    if agent.is_fenced() {
        return Err(CliError {
            code: "fenced",
            message: RegistryError::Fenced(agent.config().node_id.clone()).to_string(),
            exit: EXIT_FENCED,
        });
    }
    agent.shutdown(v.bool("LICTOR_LEAVE_ON_SHUTDOWN")).await;
    Ok(())
}
