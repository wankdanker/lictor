//! Consul compatibility, checked against the real thing (needs Docker; Linux):
//!
//! - `dns_and_http_match_consul`: the same services registered in Consul 1.22 and in a Lictor
//!   agent; the same DNS questions and HTTP reads asked of both; the answers compared.
//! - `prometheus_discovers_targets_through_lictor`: a real Prometheus with a `consul_sd_configs`
//!   job pointed at a Lictor agent finds the tagged services, with the relabeled labels.
//!
//! They pull images and take about a minute, so they run only with LICTOR_DOCKER_TESTS=1
//! (the "Consul compatibility" CI job sets it): `LICTOR_DOCKER_TESTS=1 cargo test --test consul_compat`.
#![cfg(target_os = "linux")]

mod common;
use common::AgentProc;
use hickory_proto::op::{Edns, Message, MessageType, OpCode, Query};
use hickory_proto::rr::{Name, RData, RecordType};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::process::Command;
use std::time::{Duration, Instant};

const CONSUL_IMAGE: &str = "hashicorp/consul:1.22.7";
const PROMETHEUS_IMAGE: &str = "prom/prometheus:v3.16.0";

fn docker_available() -> bool {
    Command::new("docker").args(["info", "--format", "{{.ServerVersion}}"]).output().is_ok_and(|o| o.status.success())
}

macro_rules! require_docker {
    () => {
        if std::env::var("LICTOR_DOCKER_TESTS").as_deref() != Ok("1") {
            eprintln!("skipping: set LICTOR_DOCKER_TESTS=1 to run the tests against Consul and Prometheus in Docker");
            return;
        }
        assert!(docker_available(), "LICTOR_DOCKER_TESTS=1 but Docker is not available");
    };
}

/// A container removed when dropped.
struct Container(String);

impl Container {
    fn run(args: &[&str]) -> Container {
        let out = Command::new("docker").args(["run", "-d", "--rm"]).args(args).output().expect("docker run");
        assert!(out.status.success(), "docker run failed: {}", String::from_utf8_lossy(&out.stderr));
        Container(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    /// The host address a container port is published on.
    fn port(&self, port: &str) -> SocketAddr {
        let out = Command::new("docker").args(["port", &self.0, port]).output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        text.lines().find_map(|l| l.trim().parse().ok()).unwrap_or_else(|| panic!("no published {port}: {text}"))
    }

    fn logs(&self) -> String {
        let out = Command::new("docker").args(["logs", "--tail", "50", &self.0]).output().unwrap();
        format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
    }
}

impl Drop for Container {
    fn drop(&mut self) {
        let _ = Command::new("docker").args(["rm", "-f", &self.0]).output();
    }
}

fn eventually<T>(what: &str, timeout: Duration, mut f: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn rt() -> tokio::runtime::Runtime {
    // reqwest is built without a default TLS provider; the binary installs ring at startup.
    let _ = rustls::crypto::ring::default_provider().install_default();
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
}

struct HttpReply {
    status: u16,
    headers: reqwest::header::HeaderMap,
    body: Value,
}

fn http(method: reqwest::Method, url: &str, body: Option<Value>) -> HttpReply {
    rt().block_on(async {
        let mut req = reqwest::Client::new().request(method, url).timeout(Duration::from_secs(30));
        if let Some(b) = body {
            req = req.json(&b);
        }
        let res = req.send().await.unwrap_or_else(|e| panic!("{url}: {e}"));
        let status = res.status().as_u16();
        let headers = res.headers().clone();
        let text = res.text().await.unwrap();
        HttpReply { status, headers, body: serde_json::from_str(&text).unwrap_or(Value::String(text)) }
    })
}

fn get(url: &str) -> HttpReply {
    http(reqwest::Method::GET, url, None)
}

/// One DNS question over UDP.
fn dns(server: SocketAddr, name: &str, qtype: RecordType, edns: bool) -> Message {
    rt().block_on(async {
        let mut m = Message::new(7, MessageType::Query, OpCode::Query);
        m.metadata.recursion_desired = true;
        m.add_query(Query::query(Name::from_ascii(name).unwrap(), qtype));
        if edns {
            let mut e = Edns::new();
            e.set_max_payload(1232);
            m.set_edns(e);
        }
        let sock = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        sock.send_to(&m.to_vec().unwrap(), server).await.unwrap();
        let mut buf = vec![0u8; 65535];
        let n = tokio::time::timeout(Duration::from_secs(5), sock.recv(&mut buf)).await.expect("DNS reply").unwrap();
        Message::from_vec(&buf[..n]).unwrap()
    })
}

/// The parts of a DNS response that must match: code, flags, and record sets (answers are
/// shuffled; SOA serials are timestamps; Consul adds TXT node metadata to the additional section).
fn normalize(m: &Message) -> Value {
    let rdata = |d: &RData| match d {
        RData::SOA(s) => format!("{} {} * {} {} {} {}", s.mname, s.rname, s.refresh, s.retry, s.expire, s.minimum),
        other => other.to_string(),
    };
    let set = |records: &[hickory_proto::rr::Record], keep: &dyn Fn(RecordType) -> bool| -> BTreeSet<String> {
        records
            .iter()
            .filter(|r| keep(r.record_type()))
            .map(|r| format!("{} {} {} {}", r.name.to_ascii().to_lowercase(), r.ttl, r.record_type(), rdata(&r.data)))
            .collect()
    };
    json!({
        "code": m.metadata.response_code.to_string(),
        "aa": m.metadata.authoritative,
        "answers": set(&m.answers, &|_| true),
        "authorities": set(&m.authorities, &|_| true),
        "additionals": set(&m.additionals, &|t| matches!(t, RecordType::A | RecordType::AAAA)),
    })
}

/// The reference services, as Consul agent registrations. `checks` start in the given state.
fn services() -> Vec<Value> {
    let svc = |id: &str, name: &str, addr: &str, port: u16, tags: &[&str], meta: Value, check: Option<&str>| {
        json!({ "ID": id, "Name": name, "Address": addr, "Port": port, "Tags": tags, "Meta": meta,
                "Check": check.map(|status| json!({ "TTL": "300s", "Status": status })) })
    };
    vec![
        svc("web-1", "web", "10.0.0.11", 8080, &["v2", "prometheus"], json!({ "shard": "4", "dma": "Denver" }), None),
        svc("web-2", "web", "10.0.0.12", 8081, &["v1"], json!({}), None),
        svc("web-3", "web", "10.0.0.13", 8082, &["v2", "dma-New York"], json!({}), None),
        svc("web-4", "web", "10.0.0.14", 8084, &["v2"], json!({}), None),
        svc("v6", "v6", "fd00::1", 9, &[], json!({}), None),
        svc("db", "db", "db.example.com", 5432, &[], json!({}), Some("passing")),
        svc("warny", "warny", "10.0.0.20", 1, &[], json!({}), Some("warning")),
        svc("down", "down", "10.0.0.30", 1, &[], json!({}), Some("critical")),
    ]
}

/// The same registration in Lictor's native format (check id as Consul names it: `service:<id>`).
fn as_lictor(c: &Value) -> Value {
    let mut r = json!({
        "id": c["ID"], "name": c["Name"], "address": c["Address"], "port": c["Port"],
        "tags": c["Tags"], "metadata": c["Meta"],
    });
    if let Some(status) = c["Check"]["Status"].as_str() {
        r["checks"] = json!([{ "id": format!("service:{}", c["ID"].as_str().unwrap()), "type": "ttl", "ttl": "300s", "initial_status": status }]);
    }
    r
}

struct Pair {
    _consul: Container,
    _agent: AgentProc,
    consul_http: String,
    consul_dns: SocketAddr,
    lictor_http: String,
    lictor_dns: SocketAddr,
}

fn start_pair() -> Pair {
    let consul = Container::run(&[
        "-p",
        "127.0.0.1::8500",
        "-p",
        "127.0.0.1::8600/udp",
        "-p",
        "127.0.0.1::8600/tcp",
        CONSUL_IMAGE,
        "agent",
        "-dev",
        "-client",
        "0.0.0.0",
        "-node",
        "host-a",
        "-datacenter",
        "dc1",
    ]);
    let consul_http = format!("http://{}", consul.port("8500/tcp"));
    let consul_dns = consul.port("8600/udp");
    eventually("Consul to elect itself", Duration::from_secs(60), || {
        let ok = rt()
            .block_on(async { reqwest::get(format!("{consul_http}/v1/status/leader")).await.ok()?.text().await.ok() });
        ok.filter(|l| l.len() > 2).map(|_| ())
    });

    let agent = AgentProc::start(&["--dev", "--node-id", "host-a"]);
    let lictor_http = format!("http://{}", agent.logged_addr("Consul HTTP API listening"));
    let lictor_dns: SocketAddr = agent.logged_addr("Consul DNS listening").parse().unwrap();

    for s in services() {
        let r = http(reqwest::Method::PUT, &format!("{consul_http}/v1/agent/service/register"), Some(s.clone()));
        assert_eq!(r.status, 200, "Consul registration of {s}: {:?}\n{}", r.body, consul.logs());
        let r = http(reqwest::Method::POST, &format!("http://{}/v1/agent/services", agent.addr), Some(as_lictor(&s)));
        assert_eq!(r.status, 201, "Lictor registration of {s}: {:?}", r.body);
    }
    // Both have applied every registration and initial check state.
    for (http_base, what) in [(&consul_http, "Consul"), (&lictor_http, "Lictor")] {
        eventually(&format!("{what} to show warny as warning"), Duration::from_secs(20), || {
            let v = get(&format!("{http_base}/v1/health/service/warny")).body;
            let statuses: Vec<String> =
                v[0]["Checks"].as_array()?.iter().map(|c| c["Status"].as_str().unwrap_or("").to_string()).collect();
            statuses.contains(&"warning".to_string()).then_some(())
        });
    }
    Pair { _consul: consul, _agent: agent, consul_http, consul_dns, lictor_http, lictor_dns }
}

#[test]
fn dns_and_http_match_consul() {
    require_docker!();
    let p = start_pair();

    // -- DNS -------------------------------------------------------------------------------
    let questions: &[(&str, RecordType)] = &[
        ("web.service.consul.", RecordType::A),
        ("web.service.dc1.consul.", RecordType::A),
        ("v1.web.service.consul.", RecordType::A),
        ("V2.WEB.Service.Consul.", RecordType::A),
        ("web.service.consul.", RecordType::SRV),
        ("_web._v2.service.consul.", RecordType::SRV),
        ("_web._tcp.service.consul.", RecordType::SRV),
        ("web.service.consul.", RecordType::AAAA),
        ("db.service.consul.", RecordType::A),
        ("db.service.consul.", RecordType::SRV),
        ("warny.service.consul.", RecordType::A),
        ("down.service.consul.", RecordType::A),
        ("v6.service.consul.", RecordType::AAAA),
        ("v6.service.consul.", RecordType::A),
        ("nope.service.consul.", RecordType::A),
        ("web.service.dc2.consul.", RecordType::A),
        ("host-a.node.consul.", RecordType::A),
        ("0a00000b.addr.dc1.consul.", RecordType::A),
        ("consul.", RecordType::SOA),
        ("consul.", RecordType::NS),
        ("example.com.", RecordType::A),
    ];
    let mut differences = Vec::new();
    for (name, qtype) in questions {
        let consul = normalize(&dns(p.consul_dns, name, *qtype, true));
        let lictor = normalize(&dns(p.lictor_dns, name, *qtype, true));
        if consul != lictor {
            differences.push(format!(
                "{name} {qtype}\n  consul: {}\n  lictor: {}",
                serde_json::to_string(&consul).unwrap(),
                serde_json::to_string(&lictor).unwrap()
            ));
        }
    }
    // Without EDNS, both stop at 3 of web's 4 instances.
    for server in [p.consul_dns, p.lictor_dns] {
        assert_eq!(dns(server, "web.service.consul.", RecordType::A, false).answers.len(), 3, "{server}");
    }
    assert!(differences.is_empty(), "DNS answers differ from Consul:\n{}", differences.join("\n"));

    // -- HTTP ------------------------------------------------------------------------------
    // Every response of a catalog or health read carries what the Go client parses.
    for base in [&p.consul_http, &p.lictor_http] {
        let r = get(&format!("{base}/v1/health/service/web"));
        for h in ["x-consul-index", "x-consul-knownleader", "x-consul-lastcontact"] {
            assert!(r.headers.contains_key(h), "{base}: no {h}");
        }
    }

    // catalog/services, without Consul's own service; tag order is not meaningful.
    let services = |base: &str| -> BTreeMap<String, BTreeSet<String>> {
        let v = get(&format!("{base}/v1/catalog/services")).body;
        v.as_object()
            .unwrap()
            .iter()
            .filter(|(k, _)| *k != "consul")
            .map(|(k, tags)| {
                (k.clone(), tags.as_array().unwrap().iter().map(|t| t.as_str().unwrap().to_string()).collect())
            })
            .collect()
    };
    assert_eq!(services(&p.lictor_http), services(&p.consul_http), "catalog/services");

    // health/service: the fields discovery clients read, entry by entry.
    let keys = |v: &Value| v.as_object().map(|o| o.keys().cloned().collect::<BTreeSet<_>>()).unwrap_or_default();
    let summary = |base: &str, query: &str| -> Vec<Value> {
        let v = get(&format!("{base}/v1/health/service/{query}")).body;
        let mut entries: Vec<Value> = v.as_array().unwrap().iter().map(|e| {
            let tags: BTreeSet<&str> = e["Service"]["Tags"].as_array().unwrap().iter().map(|t| t.as_str().unwrap()).collect();
            let checks: Vec<(&str, &str)> = e["Checks"].as_array().unwrap().iter()
                .map(|c| (c["CheckID"].as_str().unwrap(), c["Status"].as_str().unwrap())).collect();
            json!({
                "node": [e["Node"]["Node"], e["Node"]["Datacenter"], e["Node"]["Address"]],
                "service": [e["Service"]["ID"], e["Service"]["Service"], e["Service"]["Address"], e["Service"]["Port"], e["Service"]["Meta"]],
                "tags": tags, "checks": checks,
                "keys": [keys(e), keys(&e["Node"]), keys(&e["Service"])],
            })
        }).collect();
        entries.sort_by_key(|e| e["service"][0].as_str().unwrap_or("").to_string());
        entries
    };
    for query in [
        "web?dc=dc1&tag=prometheus",
        "web",
        "web?tag=v2&tag=dma-New%20York",
        "web?passing",
        "db?passing",
        "warny",
        "warny?passing",
        "down",
        "down?passing",
        "nope",
    ] {
        assert_eq!(summary(&p.lictor_http, query), summary(&p.consul_http, query), "health/service/{query}");
    }

    // catalog/service: same keys and values discovery clients use.
    let flat = |base: &str| -> Vec<Value> {
        let v = get(&format!("{base}/v1/catalog/service/web?tag=v1")).body;
        v.as_array()
            .unwrap()
            .iter()
            .map(|e| {
                json!([keys(e), e["ServiceID"], e["ServiceAddress"], e["ServicePort"], e["Node"], e["Datacenter"]])
            })
            .collect()
    };
    assert_eq!(flat(&p.lictor_http), flat(&p.consul_http), "catalog/service");

    // Errors and identity.
    assert_eq!(
        get(&format!("{}/v1/health/service/web?dc=dc9", p.lictor_http)).status,
        get(&format!("{}/v1/health/service/web?dc=dc9", p.consul_http)).status
    );
    assert_eq!(get(&format!("{}/v1/agent/self", p.lictor_http)).body["Config"]["Datacenter"], "dc1");
    assert_eq!(
        get(&format!("{}/v1/catalog/datacenters", p.lictor_http)).body,
        get(&format!("{}/v1/catalog/datacenters", p.consul_http)).body
    );

    // Blocking: an index from the future waits out ?wait on both.
    for base in [&p.consul_http, &p.lictor_http] {
        let t = Instant::now();
        get(&format!("{base}/v1/catalog/services?index=999999999&wait=1s"));
        assert!(t.elapsed() >= Duration::from_millis(900), "{base} returned after {:?}", t.elapsed());
    }
}

#[test]
fn prometheus_discovers_targets_through_lictor() {
    require_docker!();
    let agent = AgentProc::start(&["--dev", "--node-id", "host-a"]);
    let consul_http = agent.logged_addr("Consul HTTP API listening");
    let register = |body: Value| {
        let r = http(reqwest::Method::POST, &format!("http://{}/v1/agent/services", agent.addr), Some(body));
        assert_eq!(r.status, 201, "{:?}", r.body);
    };
    register(json!({
        "id": "matcher-metrics-2", "name": "matcher-metrics", "address": "127.0.0.1", "port": 9102,
        "tags": ["prometheus", "shard-2"],
        "metadata": { "shard": "2", "client": "acme", "match_set": "ads", "config": "green", "dma": "Denver", "callsign": "KXYZ" },
    }));
    register(json!({ "name": "api", "address": "127.0.0.1", "port": 9000, "tags": ["api"] }));

    // The scrape job from the existing Consul setup, pointed at Lictor.
    let dir = tempfile::tempdir().unwrap();
    let config = format!(
        r#"
global:
  scrape_interval: 15s
scrape_configs:
  - job_name: 'nomad-services'
    consul_sd_configs:
      - server: '{consul_http}'
        datacenter: 'dc1'
        tags:
          - prometheus
    relabel_configs:
      - source_labels: [__meta_consul_service_metadata_dma]
        target_label: dma
      - source_labels: [__meta_consul_service_metadata_tag]
        target_label: tag
      - source_labels: [__meta_consul_service_metadata_callsign]
        target_label: callsign
      - source_labels: [__meta_consul_service_metadata_config]
        target_label: config
      - source_labels: [__meta_consul_service_metadata_client]
        target_label: client
      - source_labels: [__meta_consul_service_metadata_match_set]
        target_label: match_set
      - source_labels: [__meta_consul_service_metadata_shard]
        target_label: shard
      - source_labels: [__meta_consul_service]
        target_label: job
      - source_labels: [__meta_consul_node]
        target_label: instance
"#
    );
    std::fs::write(dir.path().join("prometheus.yml"), config).unwrap();
    // Readable by the container's nobody user.
    let _ = Command::new("chmod").args(["-R", "a+rX", dir.path().to_str().unwrap()]).status();
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let mount = format!("{}:/etc/prometheus:ro", dir.path().display());
    let listen = format!("--web.listen-address=127.0.0.1:{port}");
    let prometheus = Container::run(&[
        "--network",
        "host",
        "-v",
        &mount,
        PROMETHEUS_IMAGE,
        "--config.file=/etc/prometheus/prometheus.yml",
        &listen,
    ]);
    let api = format!("http://127.0.0.1:{port}/api/v1/targets?state=active");

    let targets = || -> Vec<Value> {
        rt().block_on(async { reqwest::get(&api).await.ok()?.json::<Value>().await.ok() })
            .and_then(|v| v["data"]["activeTargets"].as_array().cloned())
            .unwrap_or_default()
    };
    let target = eventually("Prometheus to discover matcher-metrics", Duration::from_secs(60), || {
        targets().into_iter().find(|t| t["labels"]["job"] == "matcher-metrics")
    });
    let labels = &target["labels"];
    assert_eq!(labels["instance"], "host-a", "{target}");
    for (k, v) in [
        ("shard", "2"),
        ("client", "acme"),
        ("match_set", "ads"),
        ("config", "green"),
        ("dma", "Denver"),
        ("callsign", "KXYZ"),
    ] {
        assert_eq!(labels[k], v, "{k}: {target}");
    }
    let discovered = &target["discoveredLabels"];
    assert_eq!(discovered["__address__"], "127.0.0.1:9102");
    assert_eq!(discovered["__meta_consul_dc"], "dc1");
    assert_eq!(discovered["__meta_consul_service_id"], "matcher-metrics-2");
    assert!(discovered["__meta_consul_tags"].as_str().unwrap().contains(",prometheus,"), "{discovered}");
    assert!(targets().iter().all(|t| t["labels"]["job"] != "api"), "untagged services are not scraped");
    drop(target);

    // Blocking queries carry changes to Prometheus: a deregistered service stops being a target.
    let r = http(reqwest::Method::DELETE, &format!("http://{}/v1/agent/services/matcher-metrics-2", agent.addr), None);
    assert_eq!(r.status, 204);
    eventually("Prometheus to drop the target", Duration::from_secs(60), || {
        targets().iter().all(|t| t["labels"]["job"] != "matcher-metrics").then_some(())
    });
    drop(prometheus);
}
