//! DNS: `[tag.]name.service[.dc].lictor`, RFC 2782 `_name._tag.service.lictor`, `node.node.lictor`,
//! `hex.addr.dc.lictor`. A, AAAA and SRV, over UDP and TCP. The same names under `consul` (an
//! alias, like Consul's `alt_domain`) keep Consul clients working; each answer uses the domain the
//! question was asked in.
//!
//! The behavior follows Consul 1.22 as measured against a real agent (see `tests/consul_parity.rs`
//! and `docs/consul.md`): TTL 0 by default, passing and warning instances only, answers shuffled,
//! SRV targets `<hex-ip>.addr.<dc>.<domain>` with the address in the additional section, a CNAME
//! for services registered with a hostname, at most 3 answers over UDP without EDNS, NXDOMAIN with
//! the SOA for unknown names, and REFUSED for names outside the domain.

pub use crate::Local;
use hickory_proto::op::{Edns, Message, MessageType, Metadata, OpCode, Query, ResponseCode};
use hickory_proto::rr::rdata::{A, AAAA, CNAME, NS, SOA, SRV};
use hickory_proto::rr::{Name, RData, Record, RecordType};
use lictor_core::Registry;
use lictor_types::{DiscoveryQuery, HealthFilter, NodeInfo, RegistryScope, ServiceInstance};
use rand::seq::SliceRandom;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};

/// DNS settings. The defaults are Consul's.
#[derive(Debug, Clone)]
pub struct DnsConfig {
    /// The primary domain, `lictor` by default.
    pub domain: String,
    /// A second domain answered the same way, `consul` by default (for Consul clients).
    pub alt_domain: Option<String>,
    /// TTL of service records, in seconds.
    pub service_ttl: u32,
    /// TTL of node records, in seconds.
    pub node_ttl: u32,
    /// Only passing instances (by default, warning instances are returned too).
    pub only_passing: bool,
    /// Most A/AAAA/SRV answers in a UDP response to a query without EDNS.
    pub udp_answer_limit: usize,
}

impl Default for DnsConfig {
    fn default() -> Self {
        DnsConfig {
            domain: "lictor".into(),
            alt_domain: Some("consul".into()),
            service_ttl: 0,
            node_ttl: 0,
            only_passing: false,
            udp_answer_limit: 3,
        }
    }
}

/// Which transport a query came over: UDP responses are size-limited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Udp,
    Tcp,
}

pub struct DnsServer {
    registry: Arc<dyn Registry>,
    cfg: DnsConfig,
    local: Local,
    /// The primary domain first, then the alias.
    domains: Vec<Name>,
}

/// What a name inside the domain asks for.
#[derive(Debug, PartialEq, Eq)]
enum Lookup {
    Apex,
    Service {
        name: String,
        tag: Option<String>,
        dc: Option<String>,
        ns: Option<String>,
    },
    Node {
        name: String,
        dc: Option<String>,
    },
    Addr(IpAddr),
    /// Prepared queries, Connect and the like: not supported, answered NXDOMAIN.
    Unknown,
}

const TYPES: [&str; 7] = ["service", "node", "addr", "query", "connect", "ingress", "virtual"];

/// Parse the labels left of the domain (lowercased).
fn parse(labels: &[String]) -> Lookup {
    if labels.is_empty() {
        return Lookup::Apex;
    }
    // The rightmost type label followed by a valid locality (`.dc1`, or `.prod.ns.dc1.dc` pairs).
    for i in (0..labels.len()).rev() {
        if !TYPES.contains(&labels[i].as_str()) {
            continue;
        }
        let Some((dc, ns)) = locality(&labels[i + 1..]) else { continue };
        let prefix = &labels[..i];
        return match labels[i].as_str() {
            "service" => service(prefix, dc, ns),
            "node" if !prefix.is_empty() => Lookup::Node { name: prefix.join("."), dc },
            "addr" if prefix.len() == 1 => addr(&prefix[0]).map(Lookup::Addr).unwrap_or(Lookup::Unknown),
            _ => Lookup::Unknown,
        };
    }
    Lookup::Unknown
}

/// `[]`, `[dc]`, or `[value, kind]...` with kind `dc`, `ns`, `ap` or `peer`.
fn locality(labels: &[String]) -> Option<(Option<String>, Option<String>)> {
    match labels {
        [] => Some((None, None)),
        [dc] => Some((Some(dc.clone()), None)),
        pairs if pairs.len() % 2 == 0 => {
            let (mut dc, mut ns) = (None, None);
            for pair in pairs.chunks(2) {
                match pair[1].as_str() {
                    "dc" => dc = Some(pair[0].clone()),
                    "ns" => ns = Some(pair[0].clone()),
                    "ap" | "peer" => {}
                    _ => return None,
                }
            }
            Some((dc, ns))
        }
        _ => None,
    }
}

fn service(prefix: &[String], dc: Option<String>, ns: Option<String>) -> Lookup {
    match prefix {
        [] => Lookup::Unknown,
        // RFC 2782: `_name._tag`; `_tcp` means any tag.
        [name, tag] if name.starts_with('_') && tag.starts_with('_') => {
            let tag = &tag[1..];
            let tag = (tag != "tcp").then(|| tag.to_string());
            Lookup::Service { name: name[1..].to_string(), tag, dc, ns }
        }
        [name] => Lookup::Service { name: name.clone(), tag: None, dc, ns },
        [tag @ .., name] => Lookup::Service { name: name.clone(), tag: Some(tag.join(".")), dc, ns },
    }
}

/// `0a00000b` → 10.0.0.11, 32 hex digits → an IPv6 address.
fn addr(hex: &str) -> Option<IpAddr> {
    match hex.len() {
        8 => u32::from_str_radix(hex, 16).ok().map(|v| IpAddr::V4(Ipv4Addr::from(v))),
        32 => u128::from_str_radix(hex, 16).ok().map(|v| IpAddr::V6(Ipv6Addr::from(v))),
        _ => None,
    }
}

fn hex(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(v4) => format!("{:08x}", u32::from(v4)),
        IpAddr::V6(v6) => format!("{:032x}", u128::from(v6)),
    }
}

fn ip_record(name: Name, ttl: u32, ip: IpAddr) -> Record {
    match ip {
        IpAddr::V4(v4) => Record::from_rdata(name, ttl, RData::A(A(v4))),
        IpAddr::V6(v6) => Record::from_rdata(name, ttl, RData::AAAA(AAAA(v6))),
    }
}

/// Whether an address answers a query of this type (other types are answered like A).
fn wanted(ip: IpAddr, qtype: RecordType) -> bool {
    match qtype {
        RecordType::AAAA => ip.is_ipv6(),
        RecordType::ANY => true,
        _ => ip.is_ipv4(),
    }
}

#[derive(Default)]
struct Answer {
    code: ResponseCode,
    answers: Vec<Record>,
    authorities: Vec<Record>,
    additionals: Vec<Record>,
    /// For SRV: the additional records that belong to each answer, so they can be trimmed together.
    srv_extras: Vec<Vec<Record>>,
}

impl DnsServer {
    pub fn new(registry: Arc<dyn Registry>, cfg: DnsConfig, local: Local) -> Result<Self, String> {
        let parse = |d: &str| {
            let d = d.trim_matches('.').to_ascii_lowercase();
            Name::from_ascii(format!("{d}.")).map_err(|e| format!("invalid DNS domain '{d}': {e}"))
        };
        let mut domains = vec![parse(&cfg.domain)?];
        if let Some(alt) = cfg.alt_domain.as_deref().filter(|d| !d.trim_matches('.').is_empty()) {
            let alt = parse(alt)?;
            if !domains.contains(&alt) {
                domains.push(alt);
            }
        }
        Ok(DnsServer { registry, cfg, local, domains })
    }

    /// Answer one DNS message.
    pub async fn answer(&self, request: &Message, transport: Transport) -> Message {
        let mut meta = Metadata::response_from_request(&request.metadata);
        let mut response = Message::new(request.metadata.id, MessageType::Response, request.metadata.op_code);
        if let Some(edns) = &request.edns {
            let mut e = Edns::new();
            e.set_max_payload(edns.max_payload().max(512));
            response.set_edns(e);
        }
        if request.metadata.op_code != OpCode::Query {
            meta.response_code = ResponseCode::NotImp;
            response.metadata = meta;
            return response;
        }
        let Some(query) = request.queries.first().cloned() else {
            meta.response_code = ResponseCode::FormErr;
            response.metadata = meta;
            return response;
        };
        response.add_query(query.clone());

        let lowered = query.name().to_lowercase();
        // The most specific domain the name is in (an alias may sit under the primary).
        let Some(domain) = self.domains.iter().filter(|d| d.zone_of(&lowered)).max_by_key(|d| d.num_labels()).cloned()
        else {
            meta.response_code = ResponseCode::Refused;
            response.metadata = meta;
            return response;
        };
        meta.authoritative = true;
        let skip = domain.num_labels() as usize;
        let labels: Vec<String> = lowered.iter().map(|l| String::from_utf8_lossy(l).into_owned()).collect();
        let labels = &labels[..labels.len() - skip];

        let mut a = match parse(labels) {
            Lookup::Apex => self.apex(&query, &domain),
            Lookup::Service { name, tag, dc, ns } => self.service(&query, &domain, &name, tag.as_deref(), dc, ns).await,
            Lookup::Node { name, dc } => self.node(&query, &name, dc).await,
            Lookup::Addr(ip) => {
                let mut a = Answer::default();
                if wanted(ip, query.query_type()) {
                    a.answers.push(ip_record(query.name().clone(), self.cfg.service_ttl, ip));
                }
                a
            }
            Lookup::Unknown => Answer { code: ResponseCode::NXDomain, ..Default::default() },
        };
        if a.answers.is_empty() && a.authorities.is_empty() {
            a.authorities.push(self.soa(&domain));
        }

        // Without EDNS, UDP answers are cut to the limit (silently, as Consul does).
        let limited = transport == Transport::Udp && request.edns.is_none();
        let srv = query.query_type() == RecordType::SRV;
        if limited && a.answers.len() > self.cfg.udp_answer_limit {
            a.answers.truncate(self.cfg.udp_answer_limit);
            a.srv_extras.truncate(self.cfg.udp_answer_limit);
        }
        if srv {
            a.additionals = a.srv_extras.iter().flatten().cloned().collect();
        }
        meta.response_code = a.code;
        response.metadata = meta;
        response.answers = a.answers;
        response.authorities = a.authorities;
        response.additionals = a.additionals;

        // Anything still too big for the client's UDP buffer is trimmed and flagged truncated.
        if transport == Transport::Udp {
            let max = request.edns.as_ref().map_or(512, |e| e.max_payload().max(512)) as usize;
            while response.to_vec().map(|b| b.len()).unwrap_or(0) > max && !response.answers.is_empty() {
                response.answers.pop();
                if srv {
                    a.srv_extras.pop();
                    response.additionals = a.srv_extras.iter().flatten().cloned().collect();
                }
                response.metadata.truncation = true;
            }
        }
        response
    }

    fn soa(&self, domain: &Name) -> Record {
        let serial = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as u32).unwrap_or(0);
        let child = |l: &str| Name::from_ascii(l).unwrap().append_domain(domain).unwrap();
        let soa = SOA::new(child("ns"), child("hostmaster"), serial, 3600, 600, 86400, 0);
        Record::from_rdata(domain.clone(), 0, RData::SOA(soa))
    }

    fn ns_name(&self, domain: &Name) -> Name {
        Name::from_ascii(format!("{}.node.{}", self.local.node_id, self.local.region))
            .and_then(|n| n.append_domain(domain))
            .unwrap_or_else(|_| domain.clone())
    }

    fn apex(&self, query: &Query, domain: &Name) -> Answer {
        let mut a = Answer::default();
        let ns = Record::from_rdata(domain.clone(), 0, RData::NS(NS(self.ns_name(domain))));
        let glue = self.local.address.map(|ip| ip_record(self.ns_name(domain), 0, ip));
        match query.query_type() {
            RecordType::SOA => {
                a.answers.push(self.soa(domain));
                a.authorities.push(ns);
                a.additionals.extend(glue);
            }
            RecordType::NS => {
                a.answers.push(ns);
                a.additionals.extend(glue);
            }
            _ => {}
        }
        a
    }

    fn scope(&self, dc: Option<String>, ns: Option<String>) -> RegistryScope {
        RegistryScope {
            namespace: Some(ns.unwrap_or_else(|| self.local.namespace.clone())),
            region: Some(dc.unwrap_or_else(|| self.local.region.clone())),
        }
    }

    /// Instances of `name` (matched without regard to case, like the rest of DNS).
    async fn instances(&self, name: &str, tag: Option<&str>, scope: RegistryScope) -> Vec<ServiceInstance> {
        let health = if self.cfg.only_passing { HealthFilter::Passing } else { HealthFilter::NotCritical };
        let query = |service: String| DiscoveryQuery { service, scope: scope.clone(), health, ..Default::default() };
        let mut found = self.registry.discover(query(name.to_string())).await.unwrap_or_default();
        if found.is_empty() {
            // Registered as `Web`, asked for `web`?
            let names = self.registry.list_services(scope.clone()).await.unwrap_or_default();
            if let Some(s) = names.iter().find(|s| s.name != name && s.name.eq_ignore_ascii_case(name)) {
                found = self.registry.discover(query(s.name.clone())).await.unwrap_or_default();
            }
        }
        if let Some(tag) = tag {
            found.retain(|i| i.tags.iter().any(|t| t.eq_ignore_ascii_case(tag)));
        }
        found.shuffle(&mut rand::rng());
        found
    }

    async fn service(
        &self,
        query: &Query,
        domain: &Name,
        name: &str,
        tag: Option<&str>,
        dc: Option<String>,
        ns: Option<String>,
    ) -> Answer {
        let instances = self.instances(name, tag, self.scope(dc, ns)).await;
        if instances.is_empty() {
            return Answer { code: ResponseCode::NXDomain, ..Default::default() };
        }
        let ttl = self.cfg.service_ttl;
        let qname = query.name().clone();
        let qtype = query.query_type();
        let mut a = Answer::default();
        for inst in &instances {
            let ip = inst.address.parse::<IpAddr>().ok();
            if qtype == RecordType::SRV {
                let (target, extras) = match ip {
                    Some(ip) => {
                        let target = Name::from_ascii(format!("{}.addr.{}", hex(ip), inst.region))
                            .and_then(|n| n.append_domain(domain));
                        let Ok(target) = target else { continue };
                        (target.clone(), vec![ip_record(target, ttl, ip)])
                    }
                    None => match Name::from_ascii(format!("{}.", inst.address.trim_end_matches('.'))) {
                        Ok(host) => (host, vec![]),
                        Err(_) => continue,
                    },
                };
                a.answers.push(Record::from_rdata(qname.clone(), ttl, RData::SRV(SRV::new(1, 1, inst.port, target))));
                a.srv_extras.push(extras);
                continue;
            }
            match ip {
                Some(ip) if wanted(ip, qtype) => a.answers.push(ip_record(qname.clone(), ttl, ip)),
                Some(_) => {}
                // Registered with a hostname: Consul answers with a CNAME (one per response).
                None if !a.answers.iter().any(|r| r.record_type() == RecordType::CNAME) => {
                    if let Ok(host) = Name::from_ascii(format!("{}.", inst.address.trim_end_matches('.'))) {
                        a.answers.push(Record::from_rdata(qname.clone(), ttl, RData::CNAME(CNAME(host))));
                    }
                }
                None => {}
            }
        }
        a
    }

    async fn node(&self, query: &Query, name: &str, dc: Option<String>) -> Answer {
        let nodes: Vec<NodeInfo> = self.registry.list_nodes(self.scope(dc, None)).await.unwrap_or_default();
        let Some(node) = nodes.iter().find(|n| n.node_id.eq_ignore_ascii_case(name)) else {
            return Answer { code: ResponseCode::NXDomain, ..Default::default() };
        };
        let mut a = Answer::default();
        // Like a Consul node, a node answers with its first address.
        if let Some(ip) = node.addresses.first().and_then(|a| a.parse::<IpAddr>().ok())
            && wanted(ip, query.query_type())
        {
            a.answers.push(ip_record(query.name().clone(), self.cfg.node_ttl, ip));
        }
        a
    }

    /// Serve UDP and TCP until the futures are dropped.
    pub async fn serve(self: Arc<Self>, udp: UdpSocket, tcp: TcpListener) {
        let udp = Arc::new(udp);
        let tcp_server = self.clone();
        tokio::join!(self.serve_udp(udp), tcp_server.serve_tcp(tcp));
    }

    async fn serve_udp(self: Arc<Self>, socket: Arc<UdpSocket>) {
        let mut buf = vec![0u8; 65535];
        loop {
            let (n, peer) = match socket.recv_from(&mut buf).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::debug!(error = %e, "DNS UDP receive failed");
                    continue;
                }
            };
            let Ok(request) = Message::from_vec(&buf[..n]) else { continue };
            let (server, socket) = (self.clone(), socket.clone());
            tokio::spawn(async move {
                let response = server.answer(&request, Transport::Udp).await;
                if let Ok(bytes) = response.to_vec() {
                    let _ = socket.send_to(&bytes, peer).await;
                }
            });
        }
    }

    async fn serve_tcp(self: Arc<Self>, listener: TcpListener) {
        loop {
            let Ok((stream, _)) = listener.accept().await else { continue };
            let server = self.clone();
            tokio::spawn(async move {
                let _ = server.tcp_connection(stream).await;
            });
        }
    }

    async fn tcp_connection(&self, mut stream: TcpStream) -> io::Result<()> {
        loop {
            let mut len = [0u8; 2];
            // Idle connections are closed after 10 seconds.
            match tokio::time::timeout(Duration::from_secs(10), stream.read_exact(&mut len)).await {
                Ok(Ok(_)) => {}
                _ => return Ok(()),
            }
            let mut msg = vec![0u8; u16::from_be_bytes(len) as usize];
            stream.read_exact(&mut msg).await?;
            let Ok(request) = Message::from_vec(&msg) else { return Ok(()) };
            let bytes = self.answer(&request, Transport::Tcp).await.to_vec().map_err(io::Error::other)?;
            stream.write_all(&(bytes.len() as u16).to_be_bytes()).await?;
            stream.write_all(&bytes).await?;
        }
    }
}

/// Bind UDP and TCP on the same address. With port 0, both get the same free port.
pub async fn bind(addr: SocketAddr) -> io::Result<(UdpSocket, TcpListener)> {
    let mut last = None;
    for _ in 0..10 {
        let udp = UdpSocket::bind(addr).await?;
        match TcpListener::bind(udp.local_addr()?).await {
            Ok(tcp) => return Ok((udp, tcp)),
            // The port is free for UDP but taken for TCP: with port 0, try another.
            Err(e) if addr.port() == 0 => last = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or_else(|| io::Error::other("no free port")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(s: &str) -> Vec<String> {
        s.split('.').filter(|l| !l.is_empty()).map(str::to_string).collect()
    }

    fn svc(name: &str, tag: Option<&str>, dc: Option<&str>) -> Lookup {
        Lookup::Service { name: name.into(), tag: tag.map(Into::into), dc: dc.map(Into::into), ns: None }
    }

    #[test]
    fn parses_consul_names() {
        assert_eq!(parse(&labels("")), Lookup::Apex);
        assert_eq!(parse(&labels("web.service")), svc("web", None, None));
        assert_eq!(parse(&labels("v2.web.service")), svc("web", Some("v2"), None));
        assert_eq!(parse(&labels("web.service.dc1")), svc("web", None, Some("dc1")));
        assert_eq!(parse(&labels("v2.web.service.dc1")), svc("web", Some("v2"), Some("dc1")));
        assert_eq!(parse(&labels("web.service.dc1.dc")), svc("web", None, Some("dc1")));
        assert_eq!(
            parse(&labels("web.service.prod.ns.dc1.dc")),
            Lookup::Service { name: "web".into(), tag: None, dc: Some("dc1".into()), ns: Some("prod".into()) }
        );
        assert_eq!(parse(&labels("_web._v2.service")), svc("web", Some("v2"), None));
        assert_eq!(parse(&labels("_web._tcp.service.dc1")), svc("web", None, Some("dc1")));
        // A tag that looks like a type label.
        assert_eq!(parse(&labels("service.web.service")), svc("web", Some("service"), None));
        assert_eq!(parse(&labels("host-a.node")), Lookup::Node { name: "host-a".into(), dc: None });
        assert_eq!(parse(&labels("host-a.node.dc1")), Lookup::Node { name: "host-a".into(), dc: Some("dc1".into()) });
        assert_eq!(parse(&labels("0a00000b.addr.dc1")), Lookup::Addr("10.0.0.11".parse().unwrap()));
        assert_eq!(
            parse(&labels("fd000000000000000000000000000001.addr.dc1")),
            Lookup::Addr("fd00::1".parse().unwrap())
        );
        for unknown in ["service", "web", "q.query", "web.connect", "zz.addr.dc1", "web.service.a.b.c"] {
            assert_eq!(parse(&labels(unknown)), Lookup::Unknown, "{unknown}");
        }
    }

    #[test]
    fn hex_addresses_round_trip() {
        for ip in ["10.0.0.11", "fd00::1", "::1"] {
            let ip: IpAddr = ip.parse().unwrap();
            assert_eq!(addr(&hex(ip)), Some(ip));
        }
        assert_eq!(hex("10.0.0.11".parse().unwrap()), "0a00000b");
    }
}
