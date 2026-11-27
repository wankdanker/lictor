//! Registry semantics for Lictor.
//!
//! This crate turns a generic key/value [`Store`](lictor_store::Store) into a
//! service registry. It knows nothing about NATS, Consul, DNS or HTTP.
//!
//! - [`Catalog`]: the in-memory materialized view every query is answered from.
//! - [`Agent`]: owns a node's membership lease, its registrations and their health checks.
//! - [`Observer`]: a read-only view of the catalog, for tools that are not agents.
//! - [`Registry`]: the interface both implement, and that protocol adapters depend on.
//!
//! ```no_run
//! # async fn demo() -> Result<(), Box<dyn std::error::Error>> {
//! use std::sync::Arc;
//! use lictor_core::{Agent, AgentConfig, Registry};
//! use lictor_store::MemoryStore;
//! use lictor_types::{DiscoveryQuery, ServiceRegistration};
//!
//! let agent = Agent::start(Arc::new(MemoryStore::new()), AgentConfig::new("node-1")).await?;
//! agent.register(ServiceRegistration::new("web", 8080)).await?;
//! let found = agent.discover(DiscoveryQuery::service("web")).await?;
//! assert_eq!(found.len(), 1);
//! # Ok(()) }
//! ```

mod agent;
mod catalog;
mod config;
mod error;
mod health;
mod lease;
mod observer;
mod registry;
mod sync;

pub use agent::Agent;
pub use catalog::{Catalog, Kind};
pub use config::{AgentConfig, ReconcilePolicy, RegistryConfig};
pub use error::{RegistryError, Result};
pub use observer::Observer;
pub use registry::{Registry, RegistryEventStream};

/// Version of this crate, reported by agents.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
