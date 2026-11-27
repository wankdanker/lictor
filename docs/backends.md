# Storage backends

The registry core never talks to NATS. It uses two small traits from `lictor-store`, so NATS
can be replaced by anything that can implement them: etcd, FoundationDB, Redis with care, a database, ...

```rust
#[async_trait]
pub trait Store: Send + Sync + 'static {
    fn backend(&self) -> &'static str;          // "nats", "memory", ...
    fn is_connected(&self) -> bool;
    async fn bucket(&self, config: &BucketConfig, create: bool) -> Result<Arc<dyn Bucket>>;
}

#[async_trait]
pub trait Bucket: Send + Sync + 'static {
    fn name(&self) -> &str;
    fn ttl(&self) -> Option<Duration>;
    async fn get(&self, key: &str) -> Result<Option<Entry>>;
    async fn put(&self, key: &str, value: Bytes) -> Result<u64>;
    async fn create(&self, key: &str, value: Bytes) -> Result<u64>;                 // only if absent
    async fn update(&self, key: &str, value: Bytes, revision: u64) -> Result<u64>;  // compare-and-set
    async fn delete(&self, key: &str, revision: Option<u64>) -> Result<()>;
    async fn keys(&self) -> Result<Vec<String>>;
    async fn watch(&self) -> Result<WatchStream>;                                   // state, Ready, changes
}
```

## What a backend must guarantee

| Property | Why the core needs it |
|---|---|
| Revisions increase monotonically within a bucket | Ordering events and ignoring replays. |
| `create` fails with `Conflict` if the key has a value | Exactly one agent claims a key. |
| `update` / `delete(Some(rev))` fail with `Conflict` on a revision mismatch | Generation fencing; safe cleanup. |
| Writes return only once durable | Registrations are acknowledged persistent writes. |
| Bucket TTL: entries disappear if not rewritten in time | Leases. No expiry event is required; the core enforces expiry itself. |
| Entries carry the backend's own timestamp | Lease age is measured from the server's clock, not the writer's. |
| `watch` yields every current value, then `Ready`, then changes | Catalog bootstrap and resynchronization. |
| A broken watch ends or yields an error | The core re-watches and sweeps what it missed. |
| Keys up to five `.`-separated segments of `[A-Za-z0-9_-]` | Key layout `namespace.region.service.node.id`. |

Values are opaque bytes (JSON records). The core needs no cross-key transactions.

## The conformance suite

`lictor-store` ships the tests every backend must pass, behind the `conformance` feature:

```toml
[dev-dependencies]
lictor-store = { version = "0.1", features = ["conformance"] }
```

```rust
// tests/conformance.rs
lictor_store::store_conformance_tests!(async { Some(MyStore::connect("...").await.unwrap()) });
```

The expression returns `Option<impl Store>`; return `None` to skip (for example when a server is not available).
The in-memory and NATS stores both run this suite in CI.

## Using a backend

```rust
use std::sync::Arc;
use lictor_core::{Agent, AgentConfig, Registry};

let store = Arc::new(MyStore::connect("...").await?);
let agent = Agent::start(store, AgentConfig::new("node-1")).await?;
```

To make it selectable in the `lictor` binary, add a crate next to `lictor-store-nats` and
a flag in `crates/lictor/src/agent_cmd.rs` that chooses it.

## The NATS backend

- One KV bucket per Lictor bucket: `history = 1`, file storage, `max_age` = TTL, configurable replicas.
- Revisions are stream sequences; timestamps are JetStream publish times.
- `watch` uses an ordered consumer with `LastPerSubject` delivery; `Ready` is emitted when the
  consumer has caught up (or immediately for an empty bucket).
- Supported servers: NATS 2.10 and newer (CI tests 2.10, 2.11, 2.12, 2.14 and 2.15).
- `lictor_store_nats::testing::NatsServer` (feature `testing`) starts a throwaway `nats-server` for tests.
