//! Protocol-independent data model for the Lictor service registry.
//!
//! This crate holds the types every other Lictor crate shares: the records
//! stored in the backing key/value store, the queries the registry answers,
//! and the views it returns. It contains no Consul, DNS or HTTP specifics:
//! protocol adapters translate to and from these types.
//!
//! The three stored records are:
//!
//! - [`NodeRecord`]: an agent's membership lease (one per agent, expires).
//! - [`ServiceRecord`]: a persistent service declaration (no expiry).
//! - [`HealthRecord`]: the current health of one service instance (expires).
//!
//! An instance is discoverable only when all three agree; see
//! [`ServiceInstance`] and the eligibility rules in `lictor-core`.

mod duration;
mod health;
pub mod keys;
mod model;
mod query;
pub mod validate;
mod views;

pub use duration::{format_duration, parse_duration};
pub use health::{CheckDefinition, CheckKind, CheckState, HealthStatus};
pub use model::{HealthRecord, NodeRecord, SCHEMA_VERSION, ServiceRecord};
pub use query::{DiscoveryQuery, HealthFilter, RegistryScope};
pub use views::{
    AgentStatus, CheckView, HealthCheck, HealthUpdate, NodeInfo, RegistrationReceipt, RegistryEvent, ServiceInstance,
    ServiceRegistration, ServiceSummary,
};

/// Serde helpers for durations written as human-readable strings (`"10s"`, `"1m30s"`).
pub mod serde_duration {
    pub use crate::duration::serde_impl::{deserialize, serialize};
    /// The same, for `Option<Duration>`.
    pub mod option {
        pub use crate::duration::serde_impl::option::{deserialize, serialize};
    }
}
