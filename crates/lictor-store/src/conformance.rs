//! Behavior every [`Store`] implementation must have.
//!
//! A backend crate runs the whole suite with one macro call in its tests:
//!
//! ```ignore
//! lictor_store::store_conformance_tests!(async { Some(MyStore::connect().await) });
//! ```
//!
//! The expression yields `Option<impl Store>`; `None` skips the tests (for example
//! when an external server is not available). Each test uses fresh, randomly named buckets.

use crate::{Bucket, BucketConfig, Store, StoreError, WatchEvent, WatchStream};
use bytes::Bytes;
use futures::StreamExt;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

const STEP: Duration = Duration::from_secs(10);

/// Generate one `#[tokio::test]` per conformance check.
#[macro_export]
macro_rules! store_conformance_tests {
    ($make:expr) => {
        $crate::store_conformance_tests!(@each $make;
            put_get_and_revisions, create_conflicts_when_present, update_requires_current_revision,
            delete_semantics, keys_lists_live_keys, watch_delivers_state_then_changes,
            watch_empty_bucket_is_ready, ttl_expires_entries, buckets_reopen, invalid_keys_rejected,
            concurrent_creates_have_one_winner);
    };
    (@each $make:expr; $($name:ident),*) => {
        $(
            #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
            async fn $name() {
                let Some(store) = $make.await else {
                    eprintln!("skipping {}: store unavailable", stringify!($name));
                    return;
                };
                $crate::conformance::$name(&store).await;
            }
        )*
    };
}

async fn fresh(store: &dyn Store, ttl: Option<Duration>) -> Arc<dyn Bucket> {
    let mut cfg = BucketConfig::new(format!("CONFORMANCE_{:016x}", rand::random::<u64>()));
    cfg.ttl = ttl;
    store.bucket(&cfg, true).await.expect("create bucket")
}

async fn next(w: &mut WatchStream) -> WatchEvent {
    tokio::time::timeout(STEP, w.next())
        .await
        .expect("watch event within timeout")
        .expect("watch stream ended")
        .expect("watch event ok")
}

fn b(s: &'static str) -> Bytes {
    Bytes::from_static(s.as_bytes())
}

/// Writes return increasing revisions; reads see the latest value and a sane timestamp.
pub async fn put_get_and_revisions(store: &dyn Store) {
    let bk = fresh(store, None).await;
    let r1 = bk.put("a", b("1")).await.unwrap();
    let r2 = bk.put("b", b("2")).await.unwrap();
    let r3 = bk.put("a", b("3")).await.unwrap();
    assert!(r1 < r2 && r2 < r3, "revisions increase: {r1} {r2} {r3}");
    let e = bk.get("a").await.unwrap().expect("a exists");
    assert_eq!((e.key.as_str(), e.value.as_ref(), e.revision), ("a", b"3".as_ref(), r3));
    let age = SystemTime::now().duration_since(e.timestamp).unwrap_or_default();
    assert!(age < Duration::from_secs(60), "timestamp is recent");
    assert!(bk.get("missing").await.unwrap().is_none());
    let dotted = "ns.region.svc.node.id";
    bk.put(dotted, b("x")).await.unwrap();
    assert_eq!(bk.get(dotted).await.unwrap().unwrap().value, b("x"));
}

/// `create` succeeds only when the key has no current value.
pub async fn create_conflicts_when_present(store: &dyn Store) {
    let bk = fresh(store, None).await;
    bk.create("k", b("1")).await.unwrap();
    assert!(matches!(bk.create("k", b("2")).await, Err(StoreError::Conflict { .. })));
    assert_eq!(bk.get("k").await.unwrap().unwrap().value, b("1"));
    bk.delete("k", None).await.unwrap();
    bk.create("k", b("3")).await.expect("create after delete");
    assert_eq!(bk.get("k").await.unwrap().unwrap().value, b("3"));
}

/// `update` is a compare-and-set on the revision.
pub async fn update_requires_current_revision(store: &dyn Store) {
    let bk = fresh(store, None).await;
    let r1 = bk.put("k", b("1")).await.unwrap();
    let r2 = bk.update("k", b("2"), r1).await.unwrap();
    assert!(r2 > r1);
    assert!(matches!(bk.update("k", b("x"), r1).await, Err(StoreError::Conflict { .. })));
    assert_eq!(bk.get("k").await.unwrap().unwrap().value, b("2"));
    assert!(matches!(bk.update("absent", b("x"), 1).await, Err(StoreError::Conflict { .. })));
}

/// Conditional and unconditional deletes.
pub async fn delete_semantics(store: &dyn Store) {
    let bk = fresh(store, None).await;
    let r = bk.put("k", b("1")).await.unwrap();
    assert!(matches!(bk.delete("k", Some(r + 1000)).await, Err(StoreError::Conflict { .. })));
    assert!(bk.get("k").await.unwrap().is_some(), "failed delete leaves value");
    bk.delete("k", Some(r)).await.unwrap();
    assert!(bk.get("k").await.unwrap().is_none());
    bk.delete("never-existed", None).await.expect("deleting absent key is fine");
}

/// `keys` lists keys with current values only.
pub async fn keys_lists_live_keys(store: &dyn Store) {
    let bk = fresh(store, None).await;
    assert!(bk.keys().await.unwrap().is_empty());
    bk.put("a.x", b("1")).await.unwrap();
    bk.put("b.y", b("1")).await.unwrap();
    bk.put("c.z", b("1")).await.unwrap();
    bk.delete("b.y", None).await.unwrap();
    let mut keys = bk.keys().await.unwrap();
    keys.sort();
    assert_eq!(keys, vec!["a.x", "c.z"]);
}

/// A watch delivers existing values, `Ready`, then puts and deletes as they happen.
pub async fn watch_delivers_state_then_changes(store: &dyn Store) {
    let bk = fresh(store, None).await;
    bk.put("x", b("1")).await.unwrap();
    bk.put("y", b("1")).await.unwrap();
    bk.put("x", b("2")).await.unwrap();
    bk.put("gone", b("1")).await.unwrap();
    bk.delete("gone", None).await.unwrap();

    let mut w = bk.watch().await.unwrap();
    let mut seen = std::collections::BTreeMap::new();
    loop {
        match next(&mut w).await {
            WatchEvent::Put(e) => {
                seen.insert(e.key.clone(), e.value.clone());
            }
            WatchEvent::Delete { key, .. } => {
                seen.remove(&key);
            }
            WatchEvent::Ready => break,
        }
    }
    assert_eq!(seen.len(), 2, "state before ready: {seen:?}");
    assert_eq!(seen["x"], b("2"));

    let rz = bk.put("z", b("9")).await.unwrap();
    match next(&mut w).await {
        WatchEvent::Put(e) => assert_eq!((e.key.as_str(), e.revision), ("z", rz)),
        other => panic!("expected put, got {other:?}"),
    }
    bk.delete("x", None).await.unwrap();
    match next(&mut w).await {
        WatchEvent::Delete { key, revision, .. } => {
            assert_eq!(key, "x");
            assert!(revision > rz, "delete revision {revision} > {rz}");
        }
        other => panic!("expected delete, got {other:?}"),
    }
}

/// An empty bucket's watch is ready immediately.
pub async fn watch_empty_bucket_is_ready(store: &dyn Store) {
    let bk = fresh(store, None).await;
    let mut w = bk.watch().await.unwrap();
    assert_eq!(next(&mut w).await, WatchEvent::Ready);
    bk.put("a", b("1")).await.unwrap();
    assert!(matches!(next(&mut w).await, WatchEvent::Put(_)));
}

/// Entries in a TTL bucket disappear when not rewritten, and can then be created again.
pub async fn ttl_expires_entries(store: &dyn Store) {
    let bk = fresh(store, Some(Duration::from_secs(1))).await;
    assert_eq!(bk.ttl(), Some(Duration::from_secs(1)));
    bk.put("k", b("1")).await.unwrap();
    assert!(bk.get("k").await.unwrap().is_some());
    let deadline = tokio::time::Instant::now() + STEP;
    while bk.get("k").await.unwrap().is_some() {
        assert!(tokio::time::Instant::now() < deadline, "entry expired within {STEP:?}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    bk.create("k", b("2")).await.expect("create after expiry");
}

/// Opening a bucket twice sees the same data; opening a missing bucket without create fails.
pub async fn buckets_reopen(store: &dyn Store) {
    let bk = fresh(store, None).await;
    bk.put("k", b("1")).await.unwrap();
    let again = store.bucket(&BucketConfig::new(bk.name()), false).await.unwrap();
    assert_eq!(again.get("k").await.unwrap().unwrap().value, b("1"));
    let missing = BucketConfig::new(format!("MISSING_{:016x}", rand::random::<u64>()));
    assert!(matches!(store.bucket(&missing, false).await, Err(StoreError::BucketNotFound(_))));
}

/// Keys with spaces or wildcards are rejected.
pub async fn invalid_keys_rejected(store: &dyn Store) {
    let bk = fresh(store, None).await;
    for k in ["has space", "wild*", "tail>", ""] {
        assert!(matches!(bk.put(k, b("1")).await, Err(StoreError::InvalidKey(_))), "{k:?} rejected");
    }
}

/// Many concurrent creates of one key: exactly one wins.
pub async fn concurrent_creates_have_one_winner(store: &dyn Store) {
    let bk = fresh(store, None).await;
    let tasks: Vec<_> = (0..16)
        .map(|i| {
            let bk = bk.clone();
            tokio::spawn(async move { bk.create("race", Bytes::from(i.to_string())).await })
        })
        .collect();
    let mut wins = 0;
    for t in tasks {
        match t.await.unwrap() {
            Ok(_) => wins += 1,
            Err(StoreError::Conflict { .. }) => {}
            Err(e) => panic!("unexpected error {e}"),
        }
    }
    assert_eq!(wins, 1);
}
