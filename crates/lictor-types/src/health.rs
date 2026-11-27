use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;
use std::time::Duration;

/// Health of a check or of a service instance.
///
/// Ordered from best to worst: `Passing < Warning < Unknown < Critical`.
/// The health of an instance is the worst state among its checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    /// The service is healthy.
    Passing,
    /// The service is degraded but serving.
    Warning,
    /// Health has not been established (no current health record).
    #[default]
    Unknown,
    /// The service is unhealthy.
    Critical,
}

impl HealthStatus {
    /// All states, best first.
    pub const ALL: [HealthStatus; 4] =
        [HealthStatus::Passing, HealthStatus::Warning, HealthStatus::Unknown, HealthStatus::Critical];

    /// The lowercase name: `passing`, `warning`, `unknown` or `critical`.
    pub fn as_str(self) -> &'static str {
        match self {
            HealthStatus::Passing => "passing",
            HealthStatus::Warning => "warning",
            HealthStatus::Unknown => "unknown",
            HealthStatus::Critical => "critical",
        }
    }

    /// The worst of several states; `Passing` when there are none.
    pub fn aggregate<I: IntoIterator<Item = HealthStatus>>(states: I) -> HealthStatus {
        states.into_iter().max().unwrap_or(HealthStatus::Passing)
    }
}

impl fmt::Display for HealthStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for HealthStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s.to_ascii_lowercase().as_str() {
            "passing" | "pass" | "ok" => Ok(HealthStatus::Passing),
            "warning" | "warn" => Ok(HealthStatus::Warning),
            "unknown" => Ok(HealthStatus::Unknown),
            "critical" | "fail" => Ok(HealthStatus::Critical),
            other => Err(format!("unknown health status '{other}' (expected passing, warning, unknown or critical)")),
        }
    }
}

/// How a check is performed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum CheckKind {
    /// Passing when a TCP connection to `address` (`host:port`) succeeds.
    Tcp {
        /// `host:port` to connect to.
        address: String,
    },
    /// Passing on a 2xx response, warning on 429, critical otherwise.
    Http {
        /// `http://` or `https://` URL to request.
        url: String,
        /// HTTP method, `GET` by default.
        #[serde(default = "default_method")]
        method: String,
        /// Extra request headers.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        headers: BTreeMap<String, String>,
        /// Accept invalid TLS certificates.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        tls_skip_verify: bool,
    },
    /// The application reports its own health; critical if it does not report within `ttl`.
    Ttl {
        /// How long a reported state stays valid.
        #[serde(with = "crate::serde_duration")]
        ttl: Duration,
    },
}

fn default_method() -> String {
    "GET".into()
}

impl CheckKind {
    /// The type name: `tcp`, `http` or `ttl`.
    pub fn type_name(&self) -> &'static str {
        match self {
            CheckKind::Tcp { .. } => "tcp",
            CheckKind::Http { .. } => "http",
            CheckKind::Ttl { .. } => "ttl",
        }
    }
}

/// Definition of one health check attached to a service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckDefinition {
    /// Unique within the agent. Defaults to `service:<service-id>` (or `service:<service-id>:<n>`).
    #[serde(default)]
    pub id: String,
    /// Human-readable name. Defaults to the id.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// How the check runs.
    #[serde(flatten)]
    pub kind: CheckKind,
    /// Time between runs (TCP and HTTP). Defaults to the agent's `check-interval`.
    #[serde(default, with = "crate::serde_duration::option", skip_serializing_if = "Option::is_none")]
    pub interval: Option<Duration>,
    /// Time allowed for one run (TCP and HTTP). Defaults to the agent's `check-timeout`.
    #[serde(default, with = "crate::serde_duration::option", skip_serializing_if = "Option::is_none")]
    pub timeout: Option<Duration>,
    /// Consecutive failures needed before the check turns critical.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub failures_before_critical: u32,
    /// Consecutive successes needed before the check turns passing.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub successes_before_passing: u32,
    /// State before the first result. Defaults to critical, so new instances are not
    /// returned by healthy queries until they have proven themselves.
    #[serde(default = "critical")]
    pub initial_status: HealthStatus,
    /// Deregister the service once this check has been critical for this long.
    #[serde(default, with = "crate::serde_duration::option", skip_serializing_if = "Option::is_none")]
    pub deregister_critical_after: Option<Duration>,
    /// Free-form notes for operators.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
}

fn one() -> u32 {
    1
}
fn is_one(v: &u32) -> bool {
    *v == 1
}
fn critical() -> HealthStatus {
    HealthStatus::Critical
}

impl CheckDefinition {
    /// A check of the given kind with every other field at its default.
    pub fn new(kind: CheckKind) -> Self {
        CheckDefinition {
            id: String::new(),
            name: String::new(),
            kind,
            interval: None,
            timeout: None,
            failures_before_critical: 1,
            successes_before_passing: 1,
            initial_status: HealthStatus::Critical,
            deregister_critical_after: None,
            notes: String::new(),
        }
    }

    /// Validate fields that do not depend on agent defaults.
    pub fn validate(&self) -> Result<(), String> {
        if !self.id.is_empty() {
            check_id(&self.id)?;
        }
        if self.failures_before_critical == 0 || self.successes_before_passing == 0 {
            return Err(format!("check '{}': thresholds must be at least 1", self.id));
        }
        for (what, d) in [("interval", self.interval), ("timeout", self.timeout)] {
            if d.is_some_and(|d| d < Duration::from_millis(100)) {
                return Err(format!("check '{}': {what} must be at least 100ms", self.id));
            }
        }
        match &self.kind {
            CheckKind::Tcp { address } => {
                if address.rsplit_once(':').and_then(|(_, p)| p.parse::<u16>().ok()).is_none() {
                    return Err(format!("check '{}': tcp address '{address}' must be host:port", self.id));
                }
            }
            CheckKind::Http { url, method, .. } => {
                if !(url.starts_with("http://") || url.starts_with("https://")) {
                    return Err(format!("check '{}': url '{url}' must start with http:// or https://", self.id));
                }
                if method.is_empty() || !method.chars().all(|c| c.is_ascii_uppercase()) {
                    return Err(format!("check '{}': method '{method}' must be an uppercase HTTP method", self.id));
                }
            }
            CheckKind::Ttl { ttl } => {
                if *ttl < Duration::from_secs(1) {
                    return Err(format!("check '{}': ttl must be at least 1s", self.id));
                }
            }
        }
        Ok(())
    }
}

/// Check ids may contain `:` (as in `service:web`) in addition to identifier characters.
pub(crate) fn check_id(id: &str) -> Result<(), String> {
    crate::validate::ident("check id", &id.replace(':', "_"))
}

/// The latest result of one check, as stored in a [`crate::HealthRecord`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckState {
    pub status: HealthStatus,
    /// Output of the last run (truncated), or the note sent with a TTL update.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub output: String,
    /// How long the last run took.
    #[serde(default)]
    pub duration_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordering_and_aggregate() {
        use HealthStatus::*;
        assert!(Passing < Warning && Warning < Unknown && Unknown < Critical);
        assert_eq!(HealthStatus::aggregate([]), Passing);
        assert_eq!(HealthStatus::aggregate([Passing, Warning]), Warning);
        assert_eq!(HealthStatus::aggregate([Passing, Unknown, Warning]), Unknown);
        assert_eq!(HealthStatus::aggregate([Critical, Passing]), Critical);
    }

    #[test]
    fn parse_status() {
        assert_eq!("PASS".parse::<HealthStatus>().unwrap(), HealthStatus::Passing);
        assert_eq!("warn".parse::<HealthStatus>().unwrap(), HealthStatus::Warning);
        assert_eq!("fail".parse::<HealthStatus>().unwrap(), HealthStatus::Critical);
        assert!("sick".parse::<HealthStatus>().is_err());
    }

    #[test]
    fn check_definition_json_shape() {
        let json = r#"{"id":"tcp-main","type":"tcp","address":"10.0.0.1:8080","interval":"5s"}"#;
        let c: CheckDefinition = serde_json::from_str(json).unwrap();
        assert_eq!(c.kind, CheckKind::Tcp { address: "10.0.0.1:8080".into() });
        assert_eq!(c.interval, Some(Duration::from_secs(5)));
        assert_eq!(c.failures_before_critical, 1);
        assert_eq!(c.initial_status, HealthStatus::Critical);
        c.validate().unwrap();
        let back: CheckDefinition = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn http_and_ttl_defaults() {
        let c: CheckDefinition = serde_json::from_str(r#"{"type":"http","url":"http://x/health"}"#).unwrap();
        assert!(matches!(&c.kind, CheckKind::Http { method, .. } if method == "GET"));
        let c: CheckDefinition = serde_json::from_str(r#"{"type":"ttl","ttl":"30s"}"#).unwrap();
        assert_eq!(c.kind, CheckKind::Ttl { ttl: Duration::from_secs(30) });
        assert!(serde_json::from_str::<CheckDefinition>(r#"{"type":"carrier-pigeon"}"#).is_err());
    }

    #[test]
    fn validation() {
        let tcp = |a: &str| CheckDefinition::new(CheckKind::Tcp { address: a.into() });
        tcp("localhost:80").validate().unwrap();
        tcp("[::1]:80").validate().unwrap();
        assert!(tcp("localhost").validate().is_err());
        assert!(tcp("localhost:http").validate().is_err());
        let mut c = tcp("h:1");
        c.id = "service:web:1".into();
        c.validate().unwrap();
        c.id = "bad id".into();
        assert!(c.validate().is_err());
        let mut c = tcp("h:1");
        c.failures_before_critical = 0;
        assert!(c.validate().is_err());
        let mut c = tcp("h:1");
        c.interval = Some(Duration::from_millis(10));
        assert!(c.validate().is_err());
        let http = |u: &str| {
            CheckDefinition::new(CheckKind::Http {
                url: u.into(),
                method: "GET".into(),
                headers: BTreeMap::new(),
                tls_skip_verify: false,
            })
        };
        http("https://x/").validate().unwrap();
        assert!(http("ftp://x/").validate().is_err());
        assert!(CheckDefinition::new(CheckKind::Ttl { ttl: Duration::from_millis(5) }).validate().is_err());
    }
}
