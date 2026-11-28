//! The Consul DNS adapter against a real agent (in-memory store), over real UDP and TCP sockets.
//! Expected answers come from the same probes run against Consul 1.22 (see docs/consul.md).

use hickory_proto::op::{Edns, Message, MessageType, OpCode, Query, ResponseCode};
use hickory_proto::rr::{Name, RData, RecordType};
use lictor_consul::dns::{self, DnsConfig, DnsServer, Local};
use lictor_core::{Agent, AgentConfig, Registry};
use lictor_store::MemoryStore;
use lictor_types::{CheckDefinition, CheckKind, HealthStatus, HealthUpdate, ServiceRegistration};
use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn reg(id: &str, name: &str, address: &str, port: u16, tags: &[&str]) -> ServiceRegistration {
    ServiceRegistration {
        id: Some(id.into()),
        name: name.into(),
        address: Some(address.into()),
        port,
        tags: tags.iter().map(|t| t.to_string()).collect(),
        ..Default::default()
    }
}

fn ttl_check(id: &str) -> CheckDefinition {
    let mut c = CheckDefinition::new(CheckKind::Ttl { ttl: Duration::from_secs(300) });
    c.id = id.into();
    c
}

struct Env {
    agent: Agent,
    addr: SocketAddr,
    _server: tokio::task::JoinHandle<()>,
}

/// An agent like the Consul reference setup: node host-a in dc1 with web (5 instances, tagged),
/// db (hostname address, TTL check), warny (warning), v6 (IPv6) and big (51 instances).
async fn env(cfg: DnsConfig) -> Env {
    let mut c = AgentConfig::new("host-a");
    c.addresses = vec!["127.0.0.1".into()];
    let agent = Agent::start(Arc::new(MemoryStore::new()), c).await.unwrap();
    for r in [
        reg("web-1", "web", "10.0.0.11", 8080, &["v2", "prometheus"]),
        reg("web-2", "web", "10.0.0.12", 8081, &["v1"]),
        reg("web-3", "web", "127.0.0.1", 8082, &["v2"]),
        reg("web-4", "web", "10.0.0.14", 8084, &["v2"]),
        reg("web-5", "web", "10.0.0.15", 8085, &["v2"]),
        reg("v6", "v6", "fd00::1", 9, &[]),
    ] {
        agent.register(r).await.unwrap();
    }
    let mut db = reg("db", "db", "db.example.com", 5432, &[]);
    db.checks = vec![ttl_check("service:db")];
    agent.register(db).await.unwrap();
    let mut warny = reg("warny", "warny", "10.0.0.20", 1, &[]);
    warny.checks = vec![ttl_check("warny")];
    agent.register(warny).await.unwrap();
    agent
        .update_health(HealthUpdate { check_id: "warny".into(), status: HealthStatus::Warning, output: String::new() })
        .await
        .unwrap();
    for i in 10..61u16 {
        agent.register(reg(&format!("big-{i}"), "big", &format!("10.1.0.{i}"), 9000 + i, &[])).await.unwrap();
    }

    let local = Local {
        node_id: "host-a".into(),
        namespace: "default".into(),
        region: "dc1".into(),
        address: Some("127.0.0.1".parse().unwrap()),
    };
    let server = Arc::new(DnsServer::new(Arc::new(agent.clone()), cfg, local).unwrap());
    let (udp, tcp) = dns::bind("127.0.0.1:0".parse().unwrap()).await.unwrap();
    let addr = udp.local_addr().unwrap();
    let handle = tokio::spawn(server.serve(udp, tcp));
    // Health results are published in the background: wait for warny's warning to land.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while agent.discover(lictor_types::DiscoveryQuery::service("warny")).await.unwrap()[0].status
        != HealthStatus::Warning
    {
        assert!(std::time::Instant::now() < deadline, "warny never turned warning");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    Env { agent, addr, _server: handle }
}

async fn eventually(e: &Env, name: &str, ok: impl Fn(&Message) -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !ok(&udp(e.addr, &query(name, RecordType::A, true)).await) {
        assert!(std::time::Instant::now() < deadline, "timed out waiting for {name}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn query(name: &str, qtype: RecordType, edns: bool) -> Message {
    let mut m = Message::new(4242, MessageType::Query, OpCode::Query);
    m.metadata.recursion_desired = true;
    m.add_query(Query::query(Name::from_ascii(name).unwrap(), qtype));
    if edns {
        let mut e = Edns::new();
        e.set_max_payload(1232);
        m.set_edns(e);
    }
    m
}

async fn udp(addr: SocketAddr, msg: &Message) -> Message {
    let sock = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    sock.send_to(&msg.to_vec().unwrap(), addr).await.unwrap();
    let mut buf = vec![0u8; 65535];
    let n = tokio::time::timeout(Duration::from_secs(5), sock.recv(&mut buf)).await.expect("DNS reply").unwrap();
    Message::from_vec(&buf[..n]).unwrap()
}

async fn tcp(addr: SocketAddr, msg: &Message) -> Message {
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    let bytes = msg.to_vec().unwrap();
    s.write_all(&(bytes.len() as u16).to_be_bytes()).await.unwrap();
    s.write_all(&bytes).await.unwrap();
    let mut len = [0u8; 2];
    s.read_exact(&mut len).await.unwrap();
    let mut buf = vec![0u8; u16::from_be_bytes(len) as usize];
    s.read_exact(&mut buf).await.unwrap();
    Message::from_vec(&buf).unwrap()
}

async fn ask(e: &Env, name: &str, qtype: RecordType) -> Message {
    udp(e.addr, &query(name, qtype, true)).await
}

/// Answer data as text, sorted (answers are shuffled).
fn data(m: &Message) -> BTreeSet<String> {
    m.answers.iter().map(|r| r.data.to_string()).collect()
}

fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[tokio::test]
async fn service_a_records_with_tags_and_datacenters() {
    let e = env(DnsConfig::default()).await;
    let all = set(&["10.0.0.11", "10.0.0.12", "127.0.0.1", "10.0.0.14", "10.0.0.15"]);
    let m = ask(&e, "web.service.consul.", RecordType::A).await;
    assert_eq!(m.metadata.response_code, ResponseCode::NoError);
    assert!(m.metadata.authoritative && m.metadata.recursion_desired && !m.metadata.recursion_available);
    assert_eq!(data(&m), all);
    assert!(m.answers.iter().all(|r| r.ttl == 0), "Consul's default TTL is 0");
    assert_eq!(data(&ask(&e, "web.service.dc1.consul.", RecordType::A).await), all);
    assert_eq!(data(&ask(&e, "web.service.dc1.dc.consul.", RecordType::A).await), all);
    assert_eq!(data(&ask(&e, "v1.web.service.consul.", RecordType::A).await), set(&["10.0.0.12"]));
    assert_eq!(data(&ask(&e, "prometheus.web.service.consul.", RecordType::A).await), set(&["10.0.0.11"]));
    // Names and tags are case-insensitive; answers keep the case asked.
    let m = ask(&e, "V1.WEB.Service.Consul.", RecordType::A).await;
    assert_eq!(data(&m), set(&["10.0.0.12"]));
    assert_eq!(m.answers[0].name.to_ascii(), "V1.WEB.Service.Consul.");
    // Other record types are answered like A, as Consul does.
    assert_eq!(data(&ask(&e, "web.service.consul.", RecordType::TXT).await), all);
}

#[tokio::test]
async fn srv_records_point_at_addr_names_with_glue() {
    let e = env(DnsConfig::default()).await;
    let m = ask(&e, "web.service.consul.", RecordType::SRV).await;
    assert_eq!(
        data(&m),
        set(&[
            "1 1 8080 0a00000b.addr.dc1.consul.",
            "1 1 8081 0a00000c.addr.dc1.consul.",
            "1 1 8082 7f000001.addr.dc1.consul.",
            "1 1 8084 0a00000e.addr.dc1.consul.",
            "1 1 8085 0a00000f.addr.dc1.consul.",
        ])
    );
    let glue: BTreeSet<String> = m.additionals.iter().map(|r| format!("{} {}", r.name, r.data)).collect();
    assert!(glue.contains("0a00000b.addr.dc1.consul. 10.0.0.11"), "{glue:?}");
    assert_eq!(glue.len(), 5);

    // RFC 2782 names; `_tcp` means any tag.
    assert_eq!(ask(&e, "_web._v2.service.consul.", RecordType::SRV).await.answers.len(), 4);
    assert_eq!(ask(&e, "_web._tcp.service.consul.", RecordType::SRV).await.answers.len(), 5);
    // The addr names resolve.
    assert_eq!(data(&ask(&e, "0a00000b.addr.dc1.consul.", RecordType::A).await), set(&["10.0.0.11"]));
}

#[tokio::test]
async fn health_decides_what_is_returned() {
    let e = env(DnsConfig::default()).await;
    // db's TTL check starts critical: not returned.
    let m = ask(&e, "db.service.consul.", RecordType::A).await;
    assert_eq!(m.metadata.response_code, ResponseCode::NXDomain);
    assert_eq!(m.authorities[0].record_type(), RecordType::SOA);
    e.agent
        .update_health(HealthUpdate {
            check_id: "service:db".into(),
            status: HealthStatus::Passing,
            output: String::new(),
        })
        .await
        .unwrap();
    eventually(&e, "db.service.consul.", |m| !m.answers.is_empty()).await;
    // A hostname address answers with a CNAME, and SRV targets the hostname.
    let m = ask(&e, "db.service.consul.", RecordType::A).await;
    assert_eq!(m.answers.len(), 1);
    assert!(matches!(&m.answers[0].data, RData::CNAME(c) if c.0.to_ascii() == "db.example.com."));
    assert_eq!(data(&ask(&e, "db.service.consul.", RecordType::SRV).await), set(&["1 1 5432 db.example.com."]));
    // Warning instances are returned by default...
    assert_eq!(data(&ask(&e, "warny.service.consul.", RecordType::A).await), set(&["10.0.0.20"]));
    // ...but not with only_passing.
    let strict = env(DnsConfig { only_passing: true, ..Default::default() }).await;
    assert_eq!(
        ask(&strict, "warny.service.consul.", RecordType::A).await.metadata.response_code,
        ResponseCode::NXDomain
    );
}

#[tokio::test]
async fn negative_answers_and_other_names() {
    let e = env(DnsConfig::default()).await;
    for name in ["nope.service.consul.", "web.service.dc2.consul.", "q.query.consul.", "nobody.node.consul."] {
        let m = ask(&e, name, RecordType::A).await;
        assert_eq!(m.metadata.response_code, ResponseCode::NXDomain, "{name}");
        let soa = m.authorities[0].data.to_string();
        assert!(soa.starts_with("ns.consul. hostmaster.consul. ") && soa.ends_with(" 3600 600 86400 0"), "{soa}");
    }
    // The name exists, the type does not: NOERROR, empty, with the SOA.
    for (name, t) in [("v6.service.consul.", RecordType::A), ("web.service.consul.", RecordType::AAAA)] {
        let m = ask(&e, name, t).await;
        assert_eq!((m.metadata.response_code, m.answers.len()), (ResponseCode::NoError, 0), "{name}");
        assert_eq!(m.authorities[0].record_type(), RecordType::SOA);
    }
    assert_eq!(data(&ask(&e, "v6.service.consul.", RecordType::AAAA).await), set(&["fd00::1"]));
    // Outside the domain: refused, not authoritative.
    let m = ask(&e, "example.com.", RecordType::A).await;
    assert_eq!(m.metadata.response_code, ResponseCode::Refused);
    assert!(!m.metadata.authoritative && m.answers.is_empty());
}

#[tokio::test]
async fn nodes_and_the_domain_apex() {
    let e = env(DnsConfig::default()).await;
    assert_eq!(data(&ask(&e, "host-a.node.consul.", RecordType::A).await), set(&["127.0.0.1"]));
    assert_eq!(data(&ask(&e, "host-a.node.dc1.consul.", RecordType::A).await), set(&["127.0.0.1"]));
    let m = ask(&e, "consul.", RecordType::SOA).await;
    assert_eq!(m.answers[0].record_type(), RecordType::SOA);
    assert_eq!(m.authorities[0].data.to_string(), "host-a.node.dc1.consul.");
    assert_eq!(m.additionals[0].data.to_string(), "127.0.0.1");
    let m = ask(&e, "consul.", RecordType::NS).await;
    assert_eq!(data(&m), set(&["host-a.node.dc1.consul."]));
}

#[tokio::test]
async fn udp_limits_and_tcp() {
    let e = env(DnsConfig::default()).await;
    // Without EDNS, UDP answers stop at 3 (without the TC flag, as Consul does).
    let m = udp(e.addr, &query("big.service.consul.", RecordType::A, false)).await;
    assert_eq!(m.answers.len(), 3);
    assert!(!m.metadata.truncation);
    let m = udp(e.addr, &query("big.service.consul.", RecordType::SRV, false)).await;
    assert_eq!((m.answers.len(), m.additionals.len()), (3, 3), "glue trimmed with its answers");
    // With EDNS, everything that fits.
    assert_eq!(udp(e.addr, &query("big.service.consul.", RecordType::A, true)).await.answers.len(), 51);
    // Too big even for EDNS: trimmed to fit and flagged truncated.
    let m = udp(e.addr, &query("big.service.consul.", RecordType::SRV, true)).await;
    assert!(m.metadata.truncation && m.answers.len() < 51 && m.to_vec().unwrap().len() <= 1232);
    // TCP has no limit.
    assert_eq!(tcp(e.addr, &query("big.service.consul.", RecordType::SRV, false)).await.answers.len(), 51);
    // Shuffled: some order other than the first one shows up quickly.
    let first: Vec<String> =
        ask(&e, "web.service.consul.", RecordType::A).await.answers.iter().map(|r| r.data.to_string()).collect();
    let mut shuffled = false;
    for _ in 0..20 {
        let next: Vec<String> =
            ask(&e, "web.service.consul.", RecordType::A).await.answers.iter().map(|r| r.data.to_string()).collect();
        shuffled |= next != first;
    }
    assert!(shuffled, "answers are shuffled");
}

#[tokio::test]
async fn lictor_is_the_primary_domain_and_consul_an_alias() {
    let e = env(DnsConfig::default()).await;
    // The same answers under both domains...
    for name in ["v1.web.service.lictor.", "v1.web.service.consul."] {
        assert_eq!(data(&ask(&e, name, RecordType::A).await), set(&["10.0.0.12"]), "{name}");
    }
    // ...each in the domain it was asked in.
    let m = ask(&e, "_web._v1.service.lictor.", RecordType::SRV).await;
    assert_eq!(data(&m), set(&["1 1 8081 0a00000c.addr.dc1.lictor."]));
    assert_eq!(m.additionals[0].name.to_ascii(), "0a00000c.addr.dc1.lictor.");
    assert_eq!(
        data(&ask(&e, "_web._v1.service.consul.", RecordType::SRV).await),
        set(&["1 1 8081 0a00000c.addr.dc1.consul."])
    );
    assert_eq!(data(&ask(&e, "0a00000c.addr.dc1.lictor.", RecordType::A).await), set(&["10.0.0.12"]));
    assert_eq!(data(&ask(&e, "host-a.node.lictor.", RecordType::A).await), set(&["127.0.0.1"]));
    let soa = |m: &Message| m.authorities[0].data.to_string();
    assert!(soa(&ask(&e, "nope.service.lictor.", RecordType::A).await).starts_with("ns.lictor. hostmaster.lictor. "));
    assert!(soa(&ask(&e, "nope.service.consul.", RecordType::A).await).starts_with("ns.consul. hostmaster.consul. "));
    assert_eq!(data(&ask(&e, "lictor.", RecordType::NS).await), set(&["host-a.node.dc1.lictor."]));
    assert_eq!(data(&ask(&e, "consul.", RecordType::NS).await), set(&["host-a.node.dc1.consul."]));
}

#[tokio::test]
async fn custom_domains_and_no_alias() {
    let e = env(DnsConfig { domain: "lictor.internal".into(), alt_domain: None, ..Default::default() }).await;
    assert_eq!(data(&ask(&e, "v1.web.service.lictor.internal.", RecordType::A).await), set(&["10.0.0.12"]));
    assert_eq!(ask(&e, "web.service.consul.", RecordType::A).await.metadata.response_code, ResponseCode::Refused);
    assert_eq!(ask(&e, "web.service.lictor.", RecordType::A).await.metadata.response_code, ResponseCode::Refused);

    // An alias under the primary: the most specific domain wins.
    let e =
        env(DnsConfig { domain: "internal".into(), alt_domain: Some("lictor.internal".into()), ..Default::default() })
            .await;
    let m = ask(&e, "web.service.lictor.internal.", RecordType::SRV).await;
    assert!(data(&m).iter().all(|t| t.ends_with(".addr.dc1.lictor.internal.")), "{:?}", data(&m));
    assert_eq!(ask(&e, "web.service.internal.", RecordType::SRV).await.answers.len(), 5);
}

#[tokio::test]
async fn deregistered_services_disappear() {
    let e = env(DnsConfig::default()).await;
    e.agent.deregister("web-2").await.unwrap();
    assert_eq!(ask(&e, "v1.web.service.consul.", RecordType::A).await.metadata.response_code, ResponseCode::NXDomain);
    assert_eq!(ask(&e, "web.service.consul.", RecordType::A).await.answers.len(), 4);
}
