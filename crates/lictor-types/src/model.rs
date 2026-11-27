use crate::health::{CheckDefinition, CheckState, HealthStatus};
use crate::keys;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Version of the stored record format. Readers ignore records with a newer version.
pub const SCHEMA_VERSION: u32 = 1;

fn schema_version() -> u32 {
    SCHEMA_VERSION
}

/// Membership lease of one agent, stored in the nodes bucket and refreshed every heartbeat.
///
/// The bucket's TTL makes the lease expire if the agent stops refreshing it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeRecord {
    #[serde(default = "schema_version")]
    pub schema_version: u32,
    /// Persistent node identifier.
    pub node_id: String,
    /// Random identifier chosen at each agent start. Fences older runs of the same node.
    pub generation: String,
    pub namespace: String,
    pub region: String,
    pub hostname: String,
    /// Addresses other services can use to reach this node; the first is the default.
    #[serde(default)]
    pub addresses: Vec<String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
    /// Version of the agent that wrote the record.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub agent_version: String,
}

impl NodeRecord {
    /// Store key: `namespace.region.node`.
    pub fn key(&self) -> String {
        keys::node(&self.namespace, &self.region, &self.node_id)
    }
}

/// A persistent service declaration, stored in the services bucket.
///
/// Written once at registration; it is not rewritten by heartbeats.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceRecord {
    #[serde(default = "schema_version")]
    pub schema_version: u32,
    /// Unique on its node.
    pub service_id: String,
    pub service_name: String,
    pub node_id: String,
    /// Generation of the agent that owns the registration. Must match the node's.
    pub generation: String,
    pub namespace: String,
    pub region: String,
    pub address: String,
    pub port: u16,
    #[serde(default = "tcp")]
    pub protocol: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
    /// Full check definitions, so a restarted agent can resume them.
    #[serde(default)]
    pub checks: Vec<CheckDefinition>,
}

fn tcp() -> String {
    "tcp".into()
}

impl ServiceRecord {
    /// Store key: `namespace.region.service.node.id`.
    pub fn key(&self) -> String {
        keys::service(&self.namespace, &self.region, &self.service_name, &self.node_id, &self.service_id)
    }
}

/// Current health of one service instance, stored in the health bucket.
///
/// The owning agent refreshes it periodically; the bucket's TTL makes stale health expire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthRecord {
    #[serde(default = "schema_version")]
    pub schema_version: u32,
    pub node_id: String,
    pub generation: String,
    pub service_id: String,
    /// The worst state among `checks`.
    pub status: HealthStatus,
    #[serde(default)]
    pub checks: BTreeMap<String, CheckState>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_examples_parse() {
        // The examples from the specification (docs/spec.md, section 6).
        let node: NodeRecord = serde_json::from_str(
            r#"{"schema_version":1,"node_id":"node123","generation":"a8f9c201","namespace":"production",
                "region":"us-east","hostname":"matcher-01","addresses":["10.10.1.25"],
                "metadata":{"provider":"aws","architecture":"amd64"}}"#,
        )
        .unwrap();
        assert_eq!(node.key(), "production.us-east.node123");

        let svc: ServiceRecord = serde_json::from_str(
            r#"{"schema_version":1,"service_id":"instance01","service_name":"matcher","node_id":"node123",
                "generation":"a8f9c201","namespace":"production","region":"us-east","address":"10.10.1.25",
                "port":8080,"protocol":"tcp","tags":["v2","audio"],"metadata":{"shard":"4","version":"2.1.0"}}"#,
        )
        .unwrap();
        assert_eq!(svc.key(), "production.us-east.matcher.node123.instance01");

        let health: HealthRecord = serde_json::from_str(
            r#"{"schema_version":1,"node_id":"node123","generation":"a8f9c201","service_id":"instance01",
                "status":"passing","checks":{"tcp-main":{"status":"passing","duration_ms":2}}}"#,
        )
        .unwrap();
        assert_eq!(health.status, HealthStatus::Passing);
        assert_eq!(health.checks["tcp-main"].duration_ms, 2);
    }

    #[test]
    fn defaults_fill_missing_fields() {
        let svc: ServiceRecord = serde_json::from_str(
            r#"{"service_id":"a","service_name":"b","node_id":"n","generation":"g",
                "namespace":"ns","region":"r","address":"","port":0}"#,
        )
        .unwrap();
        assert_eq!(svc.schema_version, SCHEMA_VERSION);
        assert_eq!(svc.protocol, "tcp");
        assert!(svc.tags.is_empty() && svc.checks.is_empty());
    }
}
