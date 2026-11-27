use crate::health::HealthStatus;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

/// Which instances a discovery query returns, by health.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum HealthFilter {
    /// Every live instance, whatever its health.
    #[default]
    Any,
    /// Only passing instances.
    Passing,
    /// Passing or warning instances.
    NotCritical,
}

impl HealthFilter {
    /// Whether an instance in `status` satisfies the filter.
    pub fn accepts(self, status: HealthStatus) -> bool {
        match self {
            HealthFilter::Any => true,
            HealthFilter::Passing => status == HealthStatus::Passing,
            HealthFilter::NotCritical => matches!(status, HealthStatus::Passing | HealthStatus::Warning),
        }
    }
}

impl fmt::Display for HealthFilter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            HealthFilter::Any => "any",
            HealthFilter::Passing => "passing",
            HealthFilter::NotCritical => "not-critical",
        })
    }
}

impl FromStr for HealthFilter {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "any" | "" => Ok(HealthFilter::Any),
            "passing" | "healthy" => Ok(HealthFilter::Passing),
            "not-critical" => Ok(HealthFilter::NotCritical),
            other => Err(format!("unknown health filter '{other}' (expected any, passing or not-critical)")),
        }
    }
}

/// Restricts listings to a namespace and/or region. `None` matches all.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryScope {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
}

impl RegistryScope {
    /// Every namespace and region.
    pub fn all() -> Self {
        Self::default()
    }

    /// Whether a record in `namespace`/`region` is in scope.
    pub fn contains(&self, namespace: &str, region: &str) -> bool {
        self.namespace.as_deref().is_none_or(|n| n == namespace) && self.region.as_deref().is_none_or(|r| r == region)
    }
}

/// A request for the instances of one service.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveryQuery {
    /// Service name to look up.
    pub service: String,
    #[serde(default, flatten)]
    pub scope: RegistryScope,
    /// Every tag listed must be present on the instance.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Every pair listed must be present in the instance's metadata.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
    /// Only instances on this node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    #[serde(default)]
    pub health: HealthFilter,
}

impl DiscoveryQuery {
    /// A query for every live instance of `service`.
    pub fn service(service: impl Into<String>) -> Self {
        DiscoveryQuery { service: service.into(), ..Default::default() }
    }

    /// Only passing instances.
    pub fn passing(mut self) -> Self {
        self.health = HealthFilter::Passing;
        self
    }

    /// Require a tag.
    pub fn tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }

    /// Restrict to a namespace.
    pub fn namespace(mut self, ns: impl Into<String>) -> Self {
        self.scope.namespace = Some(ns.into());
        self
    }

    /// Restrict to a region.
    pub fn region(mut self, region: impl Into<String>) -> Self {
        self.scope.region = Some(region.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_accepts() {
        use HealthStatus::*;
        assert!(HealthFilter::Any.accepts(Critical));
        assert!(HealthFilter::Passing.accepts(Passing));
        assert!(!HealthFilter::Passing.accepts(Warning));
        assert!(HealthFilter::NotCritical.accepts(Warning));
        assert!(!HealthFilter::NotCritical.accepts(Unknown));
        assert!(!HealthFilter::NotCritical.accepts(Critical));
    }

    #[test]
    fn filter_parse_round_trip() {
        for f in [HealthFilter::Any, HealthFilter::Passing, HealthFilter::NotCritical] {
            assert_eq!(f.to_string().parse::<HealthFilter>().unwrap(), f);
        }
        assert_eq!("healthy".parse::<HealthFilter>().unwrap(), HealthFilter::Passing);
        assert!("bogus".parse::<HealthFilter>().is_err());
    }

    #[test]
    fn scope() {
        let s = RegistryScope { namespace: Some("prod".into()), region: None };
        assert!(s.contains("prod", "us-east"));
        assert!(!s.contains("dev", "us-east"));
        assert!(RegistryScope::all().contains("x", "y"));
    }

    #[test]
    fn query_builder_and_json() {
        let q = DiscoveryQuery::service("matcher").passing().tag("v2").region("us-east");
        let json = serde_json::to_value(&q).unwrap();
        assert_eq!(json["service"], "matcher");
        assert_eq!(json["region"], "us-east");
        assert_eq!(json["health"], "passing");
        let back: DiscoveryQuery = serde_json::from_value(json).unwrap();
        assert_eq!(back, q);
    }
}
