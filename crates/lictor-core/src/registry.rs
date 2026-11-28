use crate::Result;
use crate::catalog::Catalog;
use async_trait::async_trait;
use futures::stream::BoxStream;
use lictor_types::{
    AgentStatus, CheckView, DiscoveryQuery, HealthCheck, HealthUpdate, NodeInfo, RegistrationReceipt, RegistryEvent,
    RegistryScope, ServiceInstance, ServiceRegistration, ServiceSummary,
};

/// Stream of query results returned by [`Registry::watch`].
pub type RegistryEventStream = BoxStream<'static, RegistryEvent>;

/// The registry interface protocol adapters are written against.
///
/// Reads are answered from the local in-memory catalog and never touch the
/// network. Writes return once the store has persisted them. Methods that
/// change local state need an [`crate::Agent`]; an [`crate::Observer`] returns
/// [`crate::RegistryError::Unsupported`] for them.
#[async_trait]
pub trait Registry: Send + Sync {
    /// Register (or re-register) a service on this agent's node.
    async fn register(&self, service: ServiceRegistration) -> Result<RegistrationReceipt>;

    /// Remove a service registered on this agent.
    async fn deregister(&self, service_id: &str) -> Result<()>;

    /// Live instances matching the query.
    async fn discover(&self, query: DiscoveryQuery) -> Result<Vec<ServiceInstance>>;

    /// A service registered on this agent, whether or not it is currently discoverable.
    async fn get_service(&self, service_id: &str) -> Result<Option<ServiceInstance>>;

    /// The stored definition of a service registered on this agent, with every default
    /// applied (id, address, protocol and check ids). Re-registering it changes nothing.
    async fn service_definition(&self, service_id: &str) -> Result<Option<ServiceRegistration>>;

    /// Services registered on this agent.
    async fn local_services(&self) -> Result<Vec<ServiceInstance>>;

    /// Checks of the services registered on this agent.
    async fn local_checks(&self) -> Result<Vec<CheckView>>;

    /// One summary per service name with live instances in scope.
    async fn list_services(&self, scope: RegistryScope) -> Result<Vec<ServiceSummary>>;

    /// Live nodes in scope.
    async fn list_nodes(&self, scope: RegistryScope) -> Result<Vec<NodeInfo>>;

    /// Add a check to a local service. Returns the check id.
    async fn register_check(&self, check: HealthCheck) -> Result<String>;

    /// Remove a check from a local service.
    async fn deregister_check(&self, check_id: &str) -> Result<()>;

    /// Report the state of a TTL check.
    async fn update_health(&self, update: HealthUpdate) -> Result<()>;

    /// The current result of `query`, then a new result every time it changes.
    async fn watch(&self, query: DiscoveryQuery) -> Result<RegistryEventStream>;

    /// Operational status.
    async fn status(&self) -> Result<AgentStatus>;

    /// Wait until the catalog changes after `index` (a value of [`AgentStatus::catalog_index`]),
    /// or until `timeout` passes. Returns the catalog index at that point. For long polling
    /// (Consul-style blocking queries): the index moves only when the catalog's content does.
    async fn wait_for_change(&self, index: u64, timeout: std::time::Duration) -> Result<u64>;
}

/// [`Registry::wait_for_change`] on top of a catalog.
pub(crate) async fn wait_for_change(catalog: &Catalog, index: u64, timeout: std::time::Duration) -> u64 {
    let mut changes = catalog.subscribe();
    let _ = tokio::time::timeout(timeout, changes.wait_for(|i| *i > index)).await;
    catalog.index()
}
