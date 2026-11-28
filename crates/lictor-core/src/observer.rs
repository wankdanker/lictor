//! A read-only participant: syncs the catalog but owns no node.
//!
//! Used by `lictor --direct` commands and by tools that only need discovery.

use crate::agent::watch_query;
use crate::catalog::Catalog;
use crate::config::RegistryConfig;
use crate::registry::{Registry, RegistryEventStream};
use crate::sync::{Buckets, start_catalog};
use crate::{RegistryError, Result};
use async_trait::async_trait;
use lictor_store::Store;
use lictor_types::{
    AgentStatus, CheckView, DiscoveryQuery, HealthCheck, HealthUpdate, NodeInfo, RegistrationReceipt, RegistryScope,
    ServiceInstance, ServiceRegistration, ServiceSummary,
};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Read-only access to the registry.
#[derive(Clone)]
pub struct Observer {
    store: Arc<dyn Store>,
    catalog: Arc<Catalog>,
    cancel: CancellationToken,
}

impl Observer {
    /// Load the catalog from `store` and keep it in sync until dropped or stopped.
    /// Buckets are never created by an observer.
    pub async fn start(store: Arc<dyn Store>, cfg: RegistryConfig) -> Result<Observer> {
        cfg.validate().map_err(RegistryError::Invalid)?;
        let cfg = RegistryConfig { create_buckets: false, ..cfg };
        let cancel = CancellationToken::new();
        let buckets = Buckets::open(&*store, &cfg).await?;
        let (catalog, _tasks) = start_catalog(&buckets, &cfg, &cancel).await?;
        Ok(Observer { store, catalog, cancel })
    }

    /// The local catalog.
    pub fn catalog(&self) -> &Arc<Catalog> {
        &self.catalog
    }

    /// Stop syncing.
    pub fn stop(&self) {
        self.cancel.cancel();
    }
}

fn read_only<T>() -> Result<T> {
    Err(RegistryError::Unsupported("this operation needs a running agent".into()))
}

#[async_trait]
impl Registry for Observer {
    async fn register(&self, _: ServiceRegistration) -> Result<RegistrationReceipt> {
        read_only()
    }
    async fn deregister(&self, _: &str) -> Result<()> {
        read_only()
    }
    async fn discover(&self, query: DiscoveryQuery) -> Result<Vec<ServiceInstance>> {
        Ok(self.catalog.discover(&query))
    }
    async fn get_service(&self, _: &str) -> Result<Option<ServiceInstance>> {
        read_only()
    }
    async fn service_definition(&self, _: &str) -> Result<Option<ServiceRegistration>> {
        read_only()
    }
    async fn local_services(&self) -> Result<Vec<ServiceInstance>> {
        read_only()
    }
    async fn local_checks(&self) -> Result<Vec<CheckView>> {
        read_only()
    }
    async fn list_services(&self, scope: RegistryScope) -> Result<Vec<ServiceSummary>> {
        Ok(self.catalog.list_services(&scope))
    }
    async fn list_nodes(&self, scope: RegistryScope) -> Result<Vec<NodeInfo>> {
        Ok(self.catalog.list_nodes(&scope))
    }
    async fn register_check(&self, _: HealthCheck) -> Result<String> {
        read_only()
    }
    async fn deregister_check(&self, _: &str) -> Result<()> {
        read_only()
    }
    async fn update_health(&self, _: HealthUpdate) -> Result<()> {
        read_only()
    }
    async fn watch(&self, query: DiscoveryQuery) -> Result<RegistryEventStream> {
        Ok(watch_query(self.catalog.clone(), query))
    }
    async fn status(&self) -> Result<AgentStatus> {
        let (catalog_nodes, catalog_services) = self.catalog.counts();
        Ok(AgentStatus {
            version: crate::VERSION.into(),
            backend: self.store.backend().into(),
            connected: self.store.is_connected() && self.catalog.is_syncing(),
            catalog_ready: self.catalog.is_ready(),
            node: None,
            local_services: 0,
            local_checks: 0,
            catalog_nodes,
            catalog_services,
            catalog_index: self.catalog.index(),
        })
    }
}
