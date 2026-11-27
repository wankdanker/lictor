//! Keeps the catalog in sync with the store.
//!
//! One task per bucket watches it and applies events. When the watch fails or
//! ends (connection lost), the task waits with backoff and watches again, which
//! redelivers the current state; entries not redelivered are swept at `Ready`.
//! Meanwhile the catalog keeps serving what it has, and leases keep expiring locally.

use crate::catalog::{Catalog, Kind};
use futures::StreamExt;
use lictor_store::Bucket;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

const MIN_BACKOFF: Duration = Duration::from_millis(250);
const MAX_BACKOFF: Duration = Duration::from_secs(5);
const EXPIRY_TICK: Duration = Duration::from_millis(500);

/// Spawn the sync task for one bucket.
pub(crate) fn spawn_watch(
    bucket: Arc<dyn Bucket>,
    kind: Kind,
    catalog: Arc<Catalog>,
    cancel: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut backoff = MIN_BACKOFF;
        loop {
            catalog.begin_epoch(kind);
            match bucket.watch().await {
                Ok(mut stream) => loop {
                    let next = tokio::select! {
                        _ = cancel.cancelled() => return,
                        next = stream.next() => next,
                    };
                    match next {
                        Some(Ok(event)) => {
                            if matches!(event, lictor_store::WatchEvent::Ready) {
                                backoff = MIN_BACKOFF;
                                tracing::debug!(bucket = bucket.name(), "catalog synchronized");
                            }
                            catalog.apply(kind, event);
                        }
                        Some(Err(e)) => {
                            tracing::warn!(bucket = bucket.name(), error = %e, "watch failed; resynchronizing");
                            break;
                        }
                        None => {
                            tracing::warn!(bucket = bucket.name(), "watch ended; resynchronizing");
                            break;
                        }
                    }
                },
                Err(e) => tracing::warn!(bucket = bucket.name(), error = %e, "cannot watch bucket"),
            }
            catalog.set_syncing(kind, false);
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = tokio::time::sleep(backoff) => {}
            }
            backoff = (backoff * 2).min(MAX_BACKOFF);
        }
    })
}

/// Spawn the task that drops expired leases (stores need not send expiry events).
pub(crate) fn spawn_expiry(catalog: Arc<Catalog>, cancel: CancellationToken) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(EXPIRY_TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = tick.tick() => { catalog.expire(); }
            }
        }
    })
}

/// Wait until the catalog has loaded every bucket, or time out.
pub(crate) async fn wait_ready(catalog: &Catalog, timeout: Duration) -> crate::Result<()> {
    let mut rx = catalog.subscribe();
    let deadline = tokio::time::Instant::now() + timeout;
    while !catalog.is_ready() {
        if tokio::time::timeout_at(deadline, rx.changed()).await.is_err() {
            return Err(crate::RegistryError::NotReady(format!("catalog did not load within {timeout:?}")));
        }
    }
    Ok(())
}

/// The three buckets of a registry.
#[derive(Clone)]
pub(crate) struct Buckets {
    pub nodes: Arc<dyn Bucket>,
    pub services: Arc<dyn Bucket>,
    pub health: Arc<dyn Bucket>,
}

impl Buckets {
    pub(crate) async fn open(store: &dyn lictor_store::Store, cfg: &crate::RegistryConfig) -> crate::Result<Self> {
        use lictor_store::BucketConfig;
        let make = |suffix: &str, ttl: Option<Duration>, description: &str| BucketConfig {
            name: cfg.bucket_name(suffix),
            description: description.into(),
            ttl,
            replicas: cfg.replicas,
        };
        let nodes = make("NODES", Some(cfg.membership_ttl), "Lictor node membership leases");
        let services = make("SERVICES", None, "Lictor service registrations");
        let health = make("HEALTH", Some(cfg.health_ttl), "Lictor service health");
        Ok(Buckets {
            nodes: store.bucket(&nodes, cfg.create_buckets).await?,
            services: store.bucket(&services, cfg.create_buckets).await?,
            health: store.bucket(&health, cfg.create_buckets).await?,
        })
    }
}

/// Create a catalog, start syncing it from `buckets`, and wait for the initial load.
pub(crate) async fn start_catalog(
    buckets: &Buckets,
    cfg: &crate::RegistryConfig,
    cancel: &CancellationToken,
) -> crate::Result<(Arc<Catalog>, Vec<JoinHandle<()>>)> {
    // Leases are interpreted with the buckets' actual TTLs, which may differ from this
    // participant's settings if another agent created the buckets.
    let catalog = Arc::new(Catalog::new(crate::catalog::Leases {
        node_ttl: buckets.nodes.ttl().unwrap_or(cfg.membership_ttl),
        health_ttl: buckets.health.ttl().unwrap_or(cfg.health_ttl),
        skew: cfg.clock_skew,
    }));
    let tasks = vec![
        spawn_watch(buckets.nodes.clone(), Kind::Nodes, catalog.clone(), cancel.clone()),
        spawn_watch(buckets.services.clone(), Kind::Services, catalog.clone(), cancel.clone()),
        spawn_watch(buckets.health.clone(), Kind::Health, catalog.clone(), cancel.clone()),
        spawn_expiry(catalog.clone(), cancel.clone()),
    ];
    if let Err(e) = wait_ready(&catalog, cfg.sync_timeout).await {
        cancel.cancel();
        return Err(e);
    }
    Ok((catalog, tasks))
}
