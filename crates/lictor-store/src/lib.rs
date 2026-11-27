//! Storage backend abstraction for Lictor.
//!
//! The registry core never talks to NATS directly. It needs a handful of
//! key/value primitives, described by the [`Store`] and [`Bucket`] traits:
//!
//! - **Revisions**: every write returns a revision that increases monotonically within a bucket.
//! - **Conditional writes**: [`Bucket::create`] fails if the key exists, [`Bucket::update`] and
//!   [`Bucket::delete`] can require the current revision. This is how agents fence each other.
//! - **Bucket TTL**: entries in a bucket with a TTL disappear when not rewritten in time.
//!   Expiry does not have to produce a watch event; the core enforces expiry itself.
//! - **Watch**: a stream that delivers the current value of every key, then [`WatchEvent::Ready`],
//!   then every change.
//! - **Server timestamps**: each entry carries the time the backend accepted it.
//!
//! Two implementations exist: [`MemoryStore`] (this crate; tests and `lictor agent --dev`)
//! and `lictor-store-nats` (NATS JetStream KV). A new backend should pass the suite in
//! [`conformance`] (enable the `conformance` feature).

use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::BoxStream;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

#[cfg(feature = "conformance")]
pub mod conformance;
mod memory;

pub use memory::MemoryStore;

/// Errors returned by store operations.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum StoreError {
    /// A conditional write lost: the key exists (create) or its revision changed (update/delete).
    #[error("conflicting write to '{key}'")]
    Conflict { key: String },
    /// The key is not valid for this backend.
    #[error("invalid key '{0}'")]
    InvalidKey(String),
    /// The backend cannot be reached or did not confirm the operation.
    #[error("store unavailable: {0}")]
    Unavailable(String),
    /// The bucket does not exist and was not created.
    #[error("bucket '{0}' not found")]
    BucketNotFound(String),
    /// Anything else.
    #[error("store error: {0}")]
    Other(String),
}

/// Result alias for store operations.
pub type Result<T, E = StoreError> = std::result::Result<T, E>;

/// How to open (or create) a bucket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BucketConfig {
    pub name: String,
    pub description: String,
    /// Entries expire this long after their last write. `None` keeps them forever.
    pub ttl: Option<Duration>,
    /// Copies kept by backends that replicate.
    pub replicas: usize,
}

impl BucketConfig {
    /// A bucket with no TTL and one replica.
    pub fn new(name: impl Into<String>) -> Self {
        BucketConfig { name: name.into(), description: String::new(), ttl: None, replicas: 1 }
    }

    /// Set the TTL.
    pub fn ttl(mut self, ttl: Duration) -> Self {
        self.ttl = Some(ttl);
        self
    }
}

/// A stored value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub key: String,
    pub value: Bytes,
    /// Revision of the write that produced this value.
    pub revision: u64,
    /// When the backend accepted the write.
    pub timestamp: SystemTime,
}

/// One item of a [`Bucket::watch`] stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchEvent {
    /// A key was written (or, before `Ready`, exists).
    Put(Entry),
    /// A key was deleted.
    Delete { key: String, revision: u64, timestamp: SystemTime },
    /// Every key that existed when the watch started has been delivered.
    Ready,
}

/// Stream returned by [`Bucket::watch`]. It ends (or yields an error) when the backend
/// connection is lost; callers re-watch to resynchronize.
pub type WatchStream = BoxStream<'static, Result<WatchEvent>>;

/// A storage backend: a factory for buckets.
#[async_trait]
pub trait Store: Send + Sync + 'static {
    /// Short backend name for status output, e.g. `nats` or `memory`.
    fn backend(&self) -> &'static str;

    /// Whether the backend is currently reachable.
    fn is_connected(&self) -> bool;

    /// Open a bucket, creating it when `create` is true and it does not exist.
    async fn bucket(&self, config: &BucketConfig, create: bool) -> Result<Arc<dyn Bucket>>;
}

/// A key/value bucket.
#[async_trait]
pub trait Bucket: Send + Sync + 'static {
    /// The bucket name.
    fn name(&self) -> &str;

    /// The bucket TTL, if any.
    fn ttl(&self) -> Option<Duration>;

    /// The current value of `key`, or `None` if absent, deleted or expired.
    async fn get(&self, key: &str) -> Result<Option<Entry>>;

    /// Write unconditionally. Returns the new revision.
    async fn put(&self, key: &str, value: Bytes) -> Result<u64>;

    /// Write only if `key` has no current value. Fails with [`StoreError::Conflict`] otherwise.
    async fn create(&self, key: &str, value: Bytes) -> Result<u64>;

    /// Write only if `key`'s current revision is `revision`. Fails with [`StoreError::Conflict`] otherwise.
    async fn update(&self, key: &str, value: Bytes, revision: u64) -> Result<u64>;

    /// Delete `key`; with `Some(revision)`, only if that is its current revision.
    /// Deleting an absent key without a revision succeeds.
    async fn delete(&self, key: &str, revision: Option<u64>) -> Result<()>;

    /// Every key with a current value.
    async fn keys(&self) -> Result<Vec<String>>;

    /// Current values, then [`WatchEvent::Ready`], then changes.
    async fn watch(&self) -> Result<WatchStream>;
}
