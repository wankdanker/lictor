//! Consul-compatible adapters for Lictor: DNS (`web.service.consul`) and the read side of the HTTP
//! API that service discovery clients use (catalog, health, blocking queries), which is enough
//! for Prometheus `consul_sd_configs`.
//!
//! Both adapters are clients of [`lictor_core::Registry`], like the CLI and the native API: they
//! answer from the agent's local catalog. Lictor regions are Consul datacenters, and an agent
//! answers for its own namespace (Consul OSS has no namespaces) unless a DNS name asks for
//! another with `.ns`. See `docs/consul.md`.

pub mod dns;
pub mod http;

use std::net::IpAddr;

/// The agent the adapters run in.
#[derive(Debug, Clone)]
pub struct Local {
    pub node_id: String,
    pub namespace: String,
    /// Its region: the default datacenter.
    pub region: String,
    /// Its advertised address: the domain's name server address in DNS, the leader in HTTP.
    pub address: Option<IpAddr>,
}
