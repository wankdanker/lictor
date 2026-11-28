use crate::health::{CheckDefinition, HealthStatus};
use crate::validate;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A request to register a service on the local agent.
///
/// This is the shape accepted by the CLI (`--file`), the native HTTP API and
/// `config-dir` files. Missing fields get defaults from the agent:
/// `id` defaults to `name`, `address` to the node's first address.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceRegistration {
    /// Unique on this node. Defaults to `name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub name: String,
    /// Defaults to the node's advertise address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    #[serde(default)]
    pub port: u16,
    /// Defaults to `tcp`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, alias = "meta", skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<CheckDefinition>,
}

impl ServiceRegistration {
    /// A registration with just a name and port.
    pub fn new(name: impl Into<String>, port: u16) -> Self {
        ServiceRegistration { name: name.into(), port, ..Default::default() }
    }

    /// The id the service will be registered under.
    pub fn effective_id(&self) -> &str {
        self.id.as_deref().unwrap_or(&self.name)
    }

    /// Validate every field.
    pub fn validate(&self) -> Result<(), String> {
        validate::ident("service name", &self.name)?;
        validate::ident("service id", self.effective_id())?;
        if let Some(p) = &self.protocol {
            validate::ident("protocol", p)?;
        }
        if let Some(a) = &self.address
            && a.chars().any(char::is_whitespace)
        {
            return Err(format!("address '{a}' must not contain whitespace"));
        }
        for t in &self.tags {
            validate::tag(t)?;
        }
        validate::metadata(&self.metadata)?;
        let mut seen = std::collections::HashSet::new();
        for c in &self.checks {
            c.validate()?;
            if !c.id.is_empty() && !seen.insert(c.id.as_str()) {
                return Err(format!("duplicate check id '{}'", c.id));
            }
        }
        Ok(())
    }
}

/// Returned once a registration has been persisted by the store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrationReceipt {
    pub service_id: String,
    pub service_name: String,
    /// Store key of the registration.
    pub key: String,
    /// Store revision of the write.
    pub revision: u64,
    pub generation: String,
    /// Check ids after defaults were applied.
    pub checks: Vec<String>,
}

/// One check of an instance, as returned by discovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckView {
    pub id: String,
    /// The service the check belongs to.
    pub service_id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub status: HealthStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub output: String,
}

/// A live service instance with its node and health.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceInstance {
    pub namespace: String,
    pub region: String,
    pub service_id: String,
    pub service_name: String,
    pub address: String,
    pub port: u16,
    pub protocol: String,
    pub tags: Vec<String>,
    pub metadata: BTreeMap<String, String>,
    pub status: HealthStatus,
    pub checks: Vec<CheckView>,
    pub node_id: String,
    pub node_hostname: String,
    pub node_addresses: Vec<String>,
    pub node_metadata: BTreeMap<String, String>,
    pub generation: String,
    /// Store revision of the registration.
    pub revision: u64,
}

/// Per-name rollup returned by service listings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceSummary {
    pub name: String,
    /// Live instances (node lease valid, generation current).
    pub instances: usize,
    pub passing: usize,
    pub warning: usize,
    pub critical: usize,
    pub unknown: usize,
    /// Union of instance tags, sorted.
    pub tags: Vec<String>,
}

/// A live node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeInfo {
    pub namespace: String,
    pub region: String,
    pub node_id: String,
    pub generation: String,
    pub hostname: String,
    pub addresses: Vec<String>,
    pub metadata: BTreeMap<String, String>,
    pub agent_version: String,
    /// Live services on the node.
    pub services: usize,
    /// Milliseconds until the membership lease expires without a heartbeat.
    pub lease_remaining_ms: u64,
}

/// A request to add a check to an already registered local service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthCheck {
    pub service_id: String,
    #[serde(flatten)]
    pub check: CheckDefinition,
}

/// A state reported by the application for a TTL check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthUpdate {
    pub check_id: String,
    pub status: HealthStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub output: String,
}

/// Emitted by a registry watch whenever the result of its query changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryEvent {
    /// Catalog index at the time of the change.
    pub index: u64,
    /// The full, current result of the query.
    pub instances: Vec<ServiceInstance>,
}

/// Operational state of an agent (or a direct, agentless client).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentStatus {
    pub version: String,
    /// `nats`, `memory`, ...
    pub backend: String,
    pub connected: bool,
    /// The local catalog has loaded the initial state of every bucket.
    pub catalog_ready: bool,
    /// Present when an agent is running (absent for direct clients).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<NodeInfo>,
    pub local_services: usize,
    pub local_checks: usize,
    pub catalog_nodes: usize,
    pub catalog_services: usize,
    /// Changes applied to the catalog since start; increases monotonically.
    pub catalog_index: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::health::CheckKind;

    #[test]
    fn registration_defaults_and_validation() {
        let r = ServiceRegistration::new("matcher", 8080);
        assert_eq!(r.effective_id(), "matcher");
        r.validate().unwrap();

        let mut r = ServiceRegistration::new("matcher", 8080);
        r.id = Some("bad.id".into());
        assert!(r.validate().is_err());

        let mut r = ServiceRegistration::new("matcher", 8080);
        r.tags = vec!["has space".into()];
        r.validate().unwrap();
        r.tags = vec!["bell\u{7}".into()];
        assert!(r.validate().is_err());

        let mut r = ServiceRegistration::new("matcher", 8080);
        let mut c = CheckDefinition::new(CheckKind::Tcp { address: "a:1".into() });
        c.id = "x".into();
        r.checks = vec![c.clone(), c];
        assert_eq!(r.validate().unwrap_err(), "duplicate check id 'x'");
    }

    #[test]
    fn registration_rejects_unknown_fields() {
        let err = serde_json::from_str::<ServiceRegistration>(r#"{"name":"a","prot":1}"#).unwrap_err();
        assert!(err.to_string().contains("unknown field"), "{err}");
        let r: ServiceRegistration = serde_json::from_str(r#"{"name":"a","meta":{"k":"v"}}"#).unwrap();
        assert_eq!(r.metadata["k"], "v");
    }

    #[test]
    fn health_check_flattens() {
        let hc: HealthCheck =
            serde_json::from_str(r#"{"service_id":"web","id":"ttl1","type":"ttl","ttl":"30s"}"#).unwrap();
        assert_eq!(hc.service_id, "web");
        assert_eq!(hc.check.id, "ttl1");
    }
}
