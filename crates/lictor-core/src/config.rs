use std::collections::BTreeMap;
use std::str::FromStr;
use std::time::Duration;

/// Settings shared by every participant (agents and observers): where the
/// registry lives in the store and how leases are interpreted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryConfig {
    /// Bucket names are `{prefix}_NODES`, `{prefix}_SERVICES` and `{prefix}_HEALTH`.
    pub bucket_prefix: String,
    /// Replicas for newly created buckets (use 3 in a production NATS cluster).
    pub replicas: usize,
    /// How long a node stays alive without a heartbeat.
    pub membership_ttl: Duration,
    /// How long a health record stays valid without a refresh.
    pub health_ttl: Duration,
    /// Create missing buckets (needs JetStream admin permissions).
    pub create_buckets: bool,
    /// Tolerated clock difference between this host and the store's server.
    pub clock_skew: Duration,
    /// How long to wait for the initial catalog load.
    pub sync_timeout: Duration,
}

impl Default for RegistryConfig {
    fn default() -> Self {
        RegistryConfig {
            bucket_prefix: "LICTOR".into(),
            replicas: 1,
            membership_ttl: Duration::from_secs(30),
            health_ttl: Duration::from_secs(30),
            create_buckets: true,
            clock_skew: Duration::from_secs(2),
            sync_timeout: Duration::from_secs(30),
        }
    }
}

impl RegistryConfig {
    pub(crate) fn bucket_name(&self, suffix: &str) -> String {
        format!("{}_{suffix}", self.bucket_prefix)
    }

    /// Validate durations and the prefix.
    pub fn validate(&self) -> Result<(), String> {
        lictor_types::validate::ident("bucket prefix", &self.bucket_prefix)?;
        if self.membership_ttl < Duration::from_secs(1) || self.health_ttl < Duration::from_secs(1) {
            return Err("membership and health TTLs must be at least 1s".into());
        }
        if self.replicas == 0 || self.replicas > 5 {
            return Err("replicas must be between 1 and 5".into());
        }
        Ok(())
    }
}

/// What a restarted agent does with registrations left by its previous run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReconcilePolicy {
    /// Take them over under the new generation and resume their checks.
    #[default]
    Adopt,
    /// Delete them; only registrations made again (e.g. from config files) survive.
    Drop,
    /// Leave them inactive (not discoverable) until registered again.
    Keep,
}

impl FromStr for ReconcilePolicy {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "adopt" => Ok(Self::Adopt),
            "drop" => Ok(Self::Drop),
            "keep" => Ok(Self::Keep),
            other => Err(format!("unknown reconcile policy '{other}' (expected adopt, drop or keep)")),
        }
    }
}

/// Everything an [`crate::Agent`] needs.
#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub registry: RegistryConfig,
    pub namespace: String,
    pub region: String,
    /// Persistent node identifier.
    pub node_id: String,
    pub hostname: String,
    /// Advertised addresses; the first is the default service address.
    pub addresses: Vec<String>,
    pub metadata: BTreeMap<String, String>,
    pub heartbeat_interval: Duration,
    /// How often unchanged health is rewritten. Defaults to a third of the health TTL.
    pub health_refresh: Option<Duration>,
    pub check_interval: Duration,
    pub check_timeout: Duration,
    pub max_concurrent_checks: usize,
    pub reconcile: ReconcilePolicy,
    /// Registrations whose node has been gone this long are deleted. `None` disables cleanup.
    pub stale_retention: Option<Duration>,
    /// Reported in the node record.
    pub agent_version: String,
}

impl AgentConfig {
    /// Defaults for a node: namespace `default`, region `dc1`.
    pub fn new(node_id: impl Into<String>) -> Self {
        let node_id = node_id.into();
        AgentConfig {
            registry: RegistryConfig::default(),
            namespace: "default".into(),
            region: "dc1".into(),
            hostname: node_id.clone(),
            node_id,
            addresses: vec!["127.0.0.1".into()],
            metadata: BTreeMap::new(),
            heartbeat_interval: Duration::from_secs(10),
            health_refresh: None,
            check_interval: Duration::from_secs(10),
            check_timeout: Duration::from_secs(3),
            max_concurrent_checks: 128,
            reconcile: ReconcilePolicy::Adopt,
            stale_retention: Some(Duration::from_secs(72 * 3600)),
            agent_version: crate::VERSION.into(),
        }
    }

    pub(crate) fn health_refresh(&self) -> Duration {
        self.health_refresh.unwrap_or(self.registry.health_ttl / 3)
    }

    /// Validate every field.
    pub fn validate(&self) -> Result<(), String> {
        use lictor_types::validate::{ident, metadata};
        self.registry.validate()?;
        ident("namespace", &self.namespace)?;
        ident("region", &self.region)?;
        ident("node id", &self.node_id)?;
        metadata(&self.metadata)?;
        if self.hostname.is_empty() {
            return Err("hostname must not be empty".into());
        }
        if self.heartbeat_interval.is_zero() || self.heartbeat_interval >= self.registry.membership_ttl {
            return Err(format!(
                "heartbeat interval ({:?}) must be shorter than the membership TTL ({:?})",
                self.heartbeat_interval, self.registry.membership_ttl
            ));
        }
        if self.health_refresh() >= self.registry.health_ttl || self.health_refresh().is_zero() {
            return Err("health refresh must be shorter than the health TTL".into());
        }
        if self.max_concurrent_checks == 0 {
            return Err("max concurrent checks must be at least 1".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        AgentConfig::new("node-1").validate().unwrap();
        assert_eq!(AgentConfig::new("n").health_refresh(), Duration::from_secs(10));
    }

    #[test]
    fn rejects_bad_settings() {
        let mut c = AgentConfig::new("node.1");
        assert!(c.validate().is_err());
        c = AgentConfig::new("n");
        c.heartbeat_interval = Duration::from_secs(30);
        assert!(c.validate().unwrap_err().contains("heartbeat"));
        c = AgentConfig::new("n");
        c.registry.replicas = 0;
        assert!(c.validate().is_err());
        c = AgentConfig::new("n");
        c.health_refresh = Some(Duration::from_secs(60));
        assert!(c.validate().is_err());
        c = AgentConfig::new("n");
        c.registry.bucket_prefix = "has space".into();
        assert!(c.validate().is_err());
    }

    #[test]
    fn reconcile_parse() {
        assert_eq!("drop".parse::<ReconcilePolicy>().unwrap(), ReconcilePolicy::Drop);
        assert!("maybe".parse::<ReconcilePolicy>().is_err());
    }
}
