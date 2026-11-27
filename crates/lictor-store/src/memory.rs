//! In-process [`Store`] used by tests and `lictor agent --dev`.
//!
//! It behaves like NATS JetStream KV where it matters to the core:
//! per-bucket monotonically increasing revisions, conditional writes, bucket TTL
//! (enforced lazily, with no watch event on expiry), and watch streams that end
//! when the store becomes unavailable. [`MemoryStore::set_available`] simulates outages.
//!
//! Expiry uses `tokio::time::Instant`, so tests can drive it with paused time.

use crate::{Bucket, BucketConfig, Entry, Result, Store, StoreError, WatchEvent, WatchStream};
use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tokio::sync::broadcast;
use tokio::time::Instant;
use tokio_stream::wrappers::BroadcastStream;

const WATCH_BUFFER: usize = 4096;

/// In-memory backend. Cloning shares the same data.
#[derive(Clone, Default)]
pub struct MemoryStore {
    shared: Arc<Shared>,
}

#[derive(Default)]
struct Shared {
    buckets: Mutex<HashMap<String, Arc<MemBucket>>>,
    unavailable: AtomicBool,
}

impl MemoryStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Simulate losing (`false`) or regaining (`true`) the backend.
    ///
    /// While unavailable every operation fails with [`StoreError::Unavailable`], and
    /// existing watch streams end, as they would on a NATS disconnect.
    pub fn set_available(&self, available: bool) {
        self.shared.unavailable.store(!available, Ordering::SeqCst);
        if !available {
            for b in self.shared.buckets.lock().unwrap().values() {
                let mut st = b.state.lock().unwrap();
                st.tx = broadcast::channel(WATCH_BUFFER).0;
            }
        }
    }
}

#[async_trait]
impl Store for MemoryStore {
    fn backend(&self) -> &'static str {
        "memory"
    }

    fn is_connected(&self) -> bool {
        !self.shared.unavailable.load(Ordering::SeqCst)
    }

    async fn bucket(&self, config: &BucketConfig, create: bool) -> Result<Arc<dyn Bucket>> {
        check_available(&self.shared)?;
        let mut buckets = self.shared.buckets.lock().unwrap();
        if let Some(b) = buckets.get(&config.name) {
            return Ok(b.clone());
        }
        if !create {
            return Err(StoreError::BucketNotFound(config.name.clone()));
        }
        let b = Arc::new(MemBucket {
            name: config.name.clone(),
            ttl: config.ttl,
            shared: Arc::downgrade(&self.shared),
            state: Mutex::new(State { revision: 0, slots: BTreeMap::new(), tx: broadcast::channel(WATCH_BUFFER).0 }),
        });
        buckets.insert(config.name.clone(), b.clone());
        Ok(b)
    }
}

fn check_available(shared: &Shared) -> Result<()> {
    if shared.unavailable.load(Ordering::SeqCst) {
        Err(StoreError::Unavailable("memory store is offline".into()))
    } else {
        Ok(())
    }
}

struct MemBucket {
    name: String,
    ttl: Option<Duration>,
    shared: std::sync::Weak<Shared>,
    state: Mutex<State>,
}

struct State {
    revision: u64,
    slots: BTreeMap<String, Slot>,
    tx: broadcast::Sender<WatchEvent>,
}

struct Slot {
    entry: Entry,
    written: Instant,
}

impl MemBucket {
    fn available(&self) -> Result<()> {
        match self.shared.upgrade() {
            Some(s) => check_available(&s),
            None => Err(StoreError::Unavailable("memory store dropped".into())),
        }
    }

    fn live<'a>(&self, slot: Option<&'a Slot>) -> Option<&'a Slot> {
        slot.filter(|s| self.ttl.is_none_or(|ttl| s.written.elapsed() < ttl))
    }

    /// Run `f` with the state locked, after dropping expired entries.
    fn with_state<T>(&self, f: impl FnOnce(&mut State) -> Result<T>) -> Result<T> {
        self.available()?;
        let mut st = self.state.lock().unwrap();
        if let Some(ttl) = self.ttl {
            st.slots.retain(|_, s| s.written.elapsed() < ttl);
        }
        f(&mut st)
    }

    fn write(st: &mut State, key: &str, value: Bytes) -> u64 {
        st.revision += 1;
        let entry = Entry { key: key.into(), value, revision: st.revision, timestamp: SystemTime::now() };
        let _ = st.tx.send(WatchEvent::Put(entry.clone()));
        st.slots.insert(key.into(), Slot { entry, written: Instant::now() });
        st.revision
    }
}

fn valid_key(key: &str) -> Result<()> {
    let ok = !key.is_empty()
        && !key.starts_with('.')
        && !key.ends_with('.')
        && !key.contains("..")
        && key.chars().all(|c| c.is_ascii_alphanumeric() || "-_=/.:".contains(c));
    if ok { Ok(()) } else { Err(StoreError::InvalidKey(key.into())) }
}

#[async_trait]
impl Bucket for MemBucket {
    fn name(&self) -> &str {
        &self.name
    }

    fn ttl(&self) -> Option<Duration> {
        self.ttl
    }

    async fn get(&self, key: &str) -> Result<Option<Entry>> {
        valid_key(key)?;
        self.with_state(|st| Ok(self.live(st.slots.get(key)).map(|s| s.entry.clone())))
    }

    async fn put(&self, key: &str, value: Bytes) -> Result<u64> {
        valid_key(key)?;
        self.with_state(|st| Ok(Self::write(st, key, value)))
    }

    async fn create(&self, key: &str, value: Bytes) -> Result<u64> {
        valid_key(key)?;
        self.with_state(|st| {
            if self.live(st.slots.get(key)).is_some() {
                return Err(StoreError::Conflict { key: key.into() });
            }
            Ok(Self::write(st, key, value))
        })
    }

    async fn update(&self, key: &str, value: Bytes, revision: u64) -> Result<u64> {
        valid_key(key)?;
        self.with_state(|st| {
            match self.live(st.slots.get(key)) {
                Some(s) if s.entry.revision == revision => {}
                _ => return Err(StoreError::Conflict { key: key.into() }),
            }
            Ok(Self::write(st, key, value))
        })
    }

    async fn delete(&self, key: &str, revision: Option<u64>) -> Result<()> {
        valid_key(key)?;
        self.with_state(|st| {
            let current = self.live(st.slots.get(key)).map(|s| s.entry.revision);
            if let Some(expected) = revision
                && current != Some(expected)
            {
                return Err(StoreError::Conflict { key: key.into() });
            }
            if current.is_none() {
                return Ok(());
            }
            st.slots.remove(key);
            st.revision += 1;
            let _ =
                st.tx.send(WatchEvent::Delete { key: key.into(), revision: st.revision, timestamp: SystemTime::now() });
            Ok(())
        })
    }

    async fn keys(&self) -> Result<Vec<String>> {
        self.with_state(|st| Ok(st.slots.keys().cloned().collect()))
    }

    async fn watch(&self) -> Result<WatchStream> {
        let (snapshot, rx) = self.with_state(|st| {
            let mut snapshot: Vec<Entry> = st.slots.values().map(|s| s.entry.clone()).collect();
            snapshot.sort_by_key(|e| e.revision);
            Ok((snapshot, st.tx.subscribe()))
        })?;
        let initial = futures::stream::iter(
            snapshot.into_iter().map(|e| Ok(WatchEvent::Put(e))).chain(std::iter::once(Ok(WatchEvent::Ready))),
        );
        let live = BroadcastStream::new(rx)
            .map(|r| r.map_err(|_| StoreError::Unavailable("watcher fell behind; resynchronize".into())));
        Ok(initial.chain(live).boxed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    async fn bucket(ttl: Option<Duration>) -> (MemoryStore, Arc<dyn Bucket>) {
        let s = MemoryStore::new();
        let mut cfg = BucketConfig::new("B");
        cfg.ttl = ttl;
        let b = s.bucket(&cfg, true).await.unwrap();
        (s, b)
    }

    #[tokio::test]
    async fn missing_bucket_without_create() {
        let s = MemoryStore::new();
        let err = s.bucket(&BucketConfig::new("nope"), false).await.err().unwrap();
        assert_eq!(err, StoreError::BucketNotFound("nope".into()));
    }

    #[tokio::test(start_paused = true)]
    async fn ttl_expires_entries_without_events() {
        let (_s, b) = bucket(Some(Duration::from_secs(30))).await;
        let mut w = b.watch().await.unwrap();
        assert_eq!(w.next().await.unwrap().unwrap(), WatchEvent::Ready);
        let rev = b.put("a", Bytes::from_static(b"1")).await.unwrap();
        assert!(matches!(w.next().await.unwrap().unwrap(), WatchEvent::Put(_)));
        tokio::time::advance(Duration::from_secs(29)).await;
        assert!(b.get("a").await.unwrap().is_some());
        tokio::time::advance(Duration::from_secs(2)).await;
        assert!(b.get("a").await.unwrap().is_none());
        assert!(b.keys().await.unwrap().is_empty());
        // Expired keys can be created again, and an update against the old revision conflicts.
        assert!(matches!(b.update("a", Bytes::new(), rev).await, Err(StoreError::Conflict { .. })));
        b.create("a", Bytes::new()).await.unwrap();
    }

    #[tokio::test]
    async fn outage_fails_ops_and_ends_watches() {
        let (s, b) = bucket(None).await;
        let mut w = b.watch().await.unwrap();
        assert_eq!(w.next().await.unwrap().unwrap(), WatchEvent::Ready);
        s.set_available(false);
        assert!(!s.is_connected());
        assert!(w.next().await.is_none(), "watch ends on outage");
        assert!(matches!(b.put("a", Bytes::new()).await, Err(StoreError::Unavailable(_))));
        assert!(b.watch().await.is_err());
        s.set_available(true);
        b.put("a", Bytes::new()).await.unwrap();
    }

    #[tokio::test]
    async fn rejects_invalid_keys() {
        let (_s, b) = bucket(None).await;
        for k in ["", ".a", "a.", "a..b", "a b", "a*", "a>"] {
            assert!(matches!(b.put(k, Bytes::new()).await, Err(StoreError::InvalidKey(_))), "{k:?}");
        }
    }
}
