//! Runs the shared store conformance suite against a real nats-server.
use async_trait::async_trait;
use lictor_store::{Bucket, BucketConfig, Result, Store};
use lictor_store_nats::{NatsOptions, NatsStore, testing::NatsServer};
use std::sync::Arc;

/// A store that keeps its server alive for the duration of the test.
struct Served {
    store: NatsStore,
    _server: NatsServer,
}

#[async_trait]
impl Store for Served {
    fn backend(&self) -> &'static str {
        self.store.backend()
    }
    fn is_connected(&self) -> bool {
        self.store.is_connected()
    }
    async fn bucket(&self, config: &BucketConfig, create: bool) -> Result<Arc<dyn Bucket>> {
        self.store.bucket(config, create).await
    }
}

async fn served() -> Option<Served> {
    let server = NatsServer::start()?;
    let store = NatsStore::connect(&NatsOptions::new(server.url())).await.expect("connect");
    Some(Served { store, _server: server })
}

lictor_store::store_conformance_tests!(served());

#[tokio::test]
async fn reports_connection_state_and_bad_urls() {
    let Some(s) = served().await else { return };
    assert!(s.is_connected());
    assert_eq!(s.backend(), "nats");
    let err = NatsStore::connect(&NatsOptions { servers: vec![], ..Default::default() }).await.err().unwrap();
    assert!(err.to_string().contains("no NATS servers"));
    let err = NatsStore::connect(&NatsOptions::new("nats://127.0.0.1:1")).await.err().unwrap();
    assert!(matches!(err, lictor_store::StoreError::Unavailable(_)), "{err}");
}

#[tokio::test]
async fn tls_options_must_pair() {
    let o = NatsOptions { tls_cert: Some("c.pem".into()), ..NatsOptions::new("nats://127.0.0.1:1") };
    let err = NatsStore::connect(&o).await.err().unwrap();
    assert!(err.to_string().contains("together"), "{err}");
}
