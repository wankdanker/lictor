//! NATS JetStream KV backend for Lictor.
//!
//! Each Lictor bucket maps to one JetStream KV bucket with `history = 1`,
//! file storage, the configured replica count, and `max_age` set to the bucket TTL.
//!
//! ```no_run
//! # async fn demo() -> Result<(), Box<dyn std::error::Error>> {
//! use lictor_store::{BucketConfig, Store};
//! use lictor_store_nats::{NatsOptions, NatsStore};
//!
//! let store = NatsStore::connect(&NatsOptions::new("nats://127.0.0.1:4222")).await?;
//! let bucket = store.bucket(&BucketConfig::new("DEMO"), true).await?;
//! bucket.put("hello", "world".into()).await?;
//! # Ok(()) }
//! ```

use async_nats::jetstream::{self, kv};
use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use lictor_store::{Bucket, BucketConfig, Entry, Result, Store, StoreError, WatchEvent, WatchStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::mpsc;

/// How to connect to NATS.
#[derive(Debug, Clone, Default)]
pub struct NatsOptions {
    /// Server URLs, e.g. `nats://127.0.0.1:4222` or `tls://nats-01:4222`.
    pub servers: Vec<String>,
    /// Client name shown in server monitoring.
    pub name: Option<String>,
    /// `.creds` file (JWT + NKey seed).
    pub credentials_file: Option<PathBuf>,
    /// File holding an NKey seed (`SU...`).
    pub nkey_file: Option<PathBuf>,
    pub user: Option<String>,
    pub password: Option<String>,
    pub token: Option<String>,
    /// CA certificate (PEM) used to verify the server.
    pub tls_ca: Option<PathBuf>,
    /// Client certificate and key (PEM) for mutual TLS.
    pub tls_cert: Option<PathBuf>,
    pub tls_key: Option<PathBuf>,
    /// Require TLS even for `nats://` URLs.
    pub require_tls: bool,
    /// Initial connection timeout (default 5s).
    pub connect_timeout: Option<Duration>,
    /// Keep retrying the initial connection instead of failing.
    pub retry_on_initial_connect: bool,
}

impl NatsOptions {
    /// Options for one server with everything else at defaults.
    pub fn new(server: impl Into<String>) -> Self {
        NatsOptions { servers: vec![server.into()], ..Default::default() }
    }

    async fn connect_options(&self) -> std::result::Result<async_nats::ConnectOptions, String> {
        let mut o = async_nats::ConnectOptions::new()
            .connection_timeout(self.connect_timeout.unwrap_or(Duration::from_secs(5)))
            .max_reconnects(None);
        if let Some(path) = &self.credentials_file {
            o = o.credentials_file(path).await.map_err(|e| format!("reading credentials {}: {e}", path.display()))?;
        }
        if let Some(path) = &self.nkey_file {
            let seed = tokio::fs::read_to_string(path)
                .await
                .map_err(|e| format!("reading nkey seed {}: {e}", path.display()))?;
            o = o.nkey(seed.trim().to_string());
        }
        if let (Some(u), Some(p)) = (&self.user, &self.password) {
            o = o.user_and_password(u.clone(), p.clone());
        }
        if let Some(t) = &self.token {
            o = o.token(t.clone());
        }
        if let Some(ca) = &self.tls_ca {
            o = o.add_root_certificates(ca.clone());
        }
        match (&self.tls_cert, &self.tls_key) {
            (Some(c), Some(k)) => o = o.add_client_certificate(c.clone(), k.clone()),
            (None, None) => {}
            _ => return Err("tls cert and tls key must be given together".into()),
        }
        if self.require_tls {
            o = o.require_tls(true);
        }
        if let Some(n) = &self.name {
            o = o.name(n);
        }
        if self.retry_on_initial_connect {
            o = o.retry_on_initial_connect();
        }
        Ok(o)
    }
}

/// A [`Store`] backed by NATS JetStream KV.
#[derive(Clone)]
pub struct NatsStore {
    client: async_nats::Client,
    js: jetstream::Context,
}

impl NatsStore {
    /// Connect to NATS.
    pub async fn connect(options: &NatsOptions) -> Result<Self> {
        if options.servers.is_empty() {
            return Err(StoreError::Other("no NATS servers configured".into()));
        }
        let addrs: Vec<async_nats::ServerAddr> = options
            .servers
            .iter()
            .map(|s| s.parse().map_err(|e| StoreError::Other(format!("invalid NATS server '{s}': {e}"))))
            .collect::<Result<_>>()?;
        let client = options
            .connect_options()
            .await
            .map_err(StoreError::Other)?
            .connect(addrs)
            .await
            .map_err(|e| StoreError::Unavailable(format!("connecting to {}: {e}", options.servers.join(","))))?;
        Ok(Self::from_client(client))
    }

    /// Wrap an existing client.
    pub fn from_client(client: async_nats::Client) -> Self {
        let js = jetstream::new(client.clone());
        NatsStore { client, js }
    }

    /// The underlying client, for adapters that speak NATS directly.
    pub fn client(&self) -> &async_nats::Client {
        &self.client
    }
}

#[async_trait]
impl Store for NatsStore {
    fn backend(&self) -> &'static str {
        "nats"
    }

    fn is_connected(&self) -> bool {
        self.client.connection_state() == async_nats::connection::State::Connected
    }

    async fn bucket(&self, config: &BucketConfig, create: bool) -> Result<Arc<dyn Bucket>> {
        let kv = match self.js.get_key_value(&config.name).await {
            Ok(kv) => kv,
            Err(e) if !create => {
                return Err(if is_not_found(&e) {
                    StoreError::BucketNotFound(config.name.clone())
                } else {
                    StoreError::Unavailable(format!("opening bucket {}: {e}", config.name))
                });
            }
            Err(_) => {
                let cfg = kv::Config {
                    bucket: config.name.clone(),
                    description: config.description.clone(),
                    history: 1,
                    max_age: config.ttl.unwrap_or_default(),
                    num_replicas: config.replicas.max(1),
                    storage: jetstream::stream::StorageType::File,
                    ..Default::default()
                };
                match self.js.create_key_value(cfg).await {
                    Ok(kv) => kv,
                    // Another agent created it first.
                    Err(e) => self
                        .js
                        .get_key_value(&config.name)
                        .await
                        .map_err(|_| StoreError::Unavailable(format!("creating bucket {}: {e}", config.name)))?,
                }
            }
        };
        // Report the bucket's real TTL: whoever created it decides how long leases last.
        let actual = kv.status().await.ok().map(|s| s.info.config.max_age).filter(|d| !d.is_zero());
        if config.ttl.is_some() && actual != config.ttl {
            tracing::warn!(
                bucket = %config.name, configured = ?config.ttl, actual = ?actual,
                "bucket exists with a different TTL; using the bucket's"
            );
        }
        Ok(Arc::new(NatsBucket { name: config.name.clone(), ttl: actual, kv, js: self.js.clone() }))
    }
}

fn is_not_found(e: &dyn std::error::Error) -> bool {
    let mut cur: Option<&dyn std::error::Error> = Some(e);
    while let Some(err) = cur {
        let text = err.to_string().to_ascii_lowercase();
        if text.contains("not found") || text.contains("10059") {
            return true;
        }
        cur = err.source();
    }
    false
}

struct NatsBucket {
    name: String,
    ttl: Option<Duration>,
    kv: kv::Store,
    js: jetstream::Context,
}

/// Messages currently in the bucket's stream (fresh from the server; `kv::Store::status` is cached).
async fn message_count(js: &jetstream::Context, bucket: &str) -> Result<u64> {
    js.get_stream(format!("KV_{bucket}"))
        .await
        .map(|s| s.cached_info().state.messages)
        .map_err(|e| StoreError::Unavailable(format!("bucket status: {e}")))
}

fn ts(t: time::OffsetDateTime) -> SystemTime {
    t.into()
}

fn valid_key(key: &str) -> Result<()> {
    let ok = !key.is_empty()
        && !key.starts_with('.')
        && !key.ends_with('.')
        && !key.contains("..")
        && key.chars().all(|c| c.is_ascii_alphanumeric() || "-_=/.:".contains(c));
    if ok { Ok(()) } else { Err(StoreError::InvalidKey(key.into())) }
}

fn update_err(key: &str, e: kv::UpdateError) -> StoreError {
    match e.kind() {
        kv::UpdateErrorKind::WrongLastRevision => StoreError::Conflict { key: key.into() },
        kv::UpdateErrorKind::InvalidKey => StoreError::InvalidKey(key.into()),
        kv::UpdateErrorKind::TimedOut => StoreError::Unavailable(format!("writing '{key}': timed out")),
        kv::UpdateErrorKind::Other => StoreError::Unavailable(format!("writing '{key}': {e}")),
    }
}

#[async_trait]
impl Bucket for NatsBucket {
    fn name(&self) -> &str {
        &self.name
    }

    fn ttl(&self) -> Option<Duration> {
        self.ttl
    }

    async fn get(&self, key: &str) -> Result<Option<Entry>> {
        valid_key(key)?;
        let entry = self.kv.entry(key).await.map_err(|e| StoreError::Unavailable(format!("reading '{key}': {e}")))?;
        Ok(entry.and_then(|e| match e.operation {
            kv::Operation::Put => {
                Some(Entry { key: e.key, value: e.value, revision: e.revision, timestamp: ts(e.created) })
            }
            _ => None,
        }))
    }

    async fn put(&self, key: &str, value: Bytes) -> Result<u64> {
        valid_key(key)?;
        self.kv.put(key, value).await.map_err(|e| match e.kind() {
            kv::PutErrorKind::InvalidKey => StoreError::InvalidKey(key.into()),
            _ => StoreError::Unavailable(format!("writing '{key}': {e}")),
        })
    }

    async fn create(&self, key: &str, value: Bytes) -> Result<u64> {
        valid_key(key)?;
        self.kv.create(key, value).await.map_err(|e| match e.kind() {
            kv::CreateErrorKind::AlreadyExists => StoreError::Conflict { key: key.into() },
            kv::CreateErrorKind::InvalidKey => StoreError::InvalidKey(key.into()),
            _ => StoreError::Unavailable(format!("creating '{key}': {e}")),
        })
    }

    async fn update(&self, key: &str, value: Bytes, revision: u64) -> Result<u64> {
        valid_key(key)?;
        self.kv.update(key, value, revision).await.map_err(|e| update_err(key, e))
    }

    async fn delete(&self, key: &str, revision: Option<u64>) -> Result<()> {
        valid_key(key)?;
        if revision.is_none() && self.get(key).await?.is_none() {
            return Ok(());
        }
        self.kv.delete_expect_revision(key, revision).await.map_err(|e| update_err(key, e))
    }

    async fn keys(&self) -> Result<Vec<String>> {
        let mut keys = self.kv.keys().await.map_err(|e| StoreError::Unavailable(format!("listing keys: {e}")))?;
        let mut out = Vec::new();
        while let Some(k) = keys.next().await {
            out.push(k.map_err(|e| StoreError::Unavailable(format!("listing keys: {e}")))?);
        }
        Ok(out)
    }

    async fn watch(&self) -> Result<WatchStream> {
        let count = message_count(&self.js, &self.name).await?;
        let mut watch = self
            .kv
            .watch_with_history(">")
            .await
            .map_err(|e| StoreError::Unavailable(format!("watching {}: {e}", self.name)))?;
        let (tx, rx) = mpsc::channel::<Result<WatchEvent>>(1024);
        let (js, name) = (self.js.clone(), self.name.clone());
        let mut ready = count == 0;
        if ready {
            let _ = tx.send(Ok(WatchEvent::Ready)).await;
        }
        tokio::spawn(async move {
            loop {
                // While waiting for the initial state, re-check for a bucket that emptied
                // (e.g. every entry expired) between the status call and the watch.
                let next = if ready {
                    watch.next().await
                } else {
                    match tokio::time::timeout(Duration::from_secs(2), watch.next()).await {
                        Ok(n) => n,
                        Err(_) => {
                            if message_count(&js, &name).await.is_ok_and(|n| n == 0) {
                                ready = true;
                                if tx.send(Ok(WatchEvent::Ready)).await.is_err() {
                                    return;
                                }
                            }
                            continue;
                        }
                    }
                };
                let event = match next {
                    None => return,
                    Some(Err(e)) => {
                        let _ = tx.send(Err(StoreError::Unavailable(format!("watch failed: {e}")))).await;
                        return;
                    }
                    Some(Ok(e)) => e,
                };
                let caught_up = !ready && event.delta == 0;
                let item = match event.operation {
                    kv::Operation::Put => WatchEvent::Put(Entry {
                        key: event.key,
                        value: event.value,
                        revision: event.revision,
                        timestamp: ts(event.created),
                    }),
                    kv::Operation::Delete | kv::Operation::Purge => {
                        WatchEvent::Delete { key: event.key, revision: event.revision, timestamp: ts(event.created) }
                    }
                };
                if tx.send(Ok(item)).await.is_err() {
                    return;
                }
                if caught_up {
                    ready = true;
                    if tx.send(Ok(WatchEvent::Ready)).await.is_err() {
                        return;
                    }
                }
            }
        });
        Ok(tokio_stream::wrappers::ReceiverStream::new(rx).boxed())
    }
}

#[cfg(feature = "testing")]
pub mod testing;
