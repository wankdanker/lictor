//! End-to-end CLI tests: several `lictor agent` processes sharing a real nats-server.
//!
//! Skipped when nats-server is not installed, unless LICTOR_REQUIRE_NATS=1.

mod common;
use common::{AgentProc, eventually, run};
use lictor_store_nats::testing::NatsServer;

macro_rules! server {
    () => {
        match NatsServer::start() {
            Some(s) => s,
            None => {
                eprintln!("skipping: nats-server not installed");
                return;
            }
        }
    };
}

/// Agent flags for fast leases.
fn agent_args<'a>(server: &'a str, node: &'a str) -> Vec<&'a str> {
    vec![
        "--nats-url",
        server,
        "--node-id",
        node,
        "--advertise",
        "10.9.9.9",
        "--heartbeat-interval",
        "500ms",
        "--membership-ttl",
        "3s",
        "--health-ttl",
        "3s",
        "--clock-skew",
        "0s",
    ]
}

fn count(addr: &str, args: &[&str]) -> usize {
    let r = run(addr, args);
    if r.code != 0 {
        return usize::MAX;
    }
    r.json().as_array().map(|a| a.len()).unwrap_or(usize::MAX)
}

#[test]
fn two_hosts_register_and_discover() {
    let server = server!();
    let url = server.url();
    let a = AgentProc::start(&agent_args(&url, "host-a"));
    let b = AgentProc::start(&agent_args(&url, "host-b"));
    a.run(&["services", "register", "--name", "web", "--port", "8080", "-t", "v2"]).ok();
    b.run(&["services", "register", "--name", "web", "--port", "8081"]).ok();

    eventually("both instances on b", || count(&b.addr, &["discover", "web", "-o", "json"]) == 2);
    eventually("tag filter on a", || count(&a.addr, &["discover", "web", "-t", "v2", "-o", "json"]) == 1);
    let found = b.run(&["discover", "web", "-o", "json"]).ok().json();
    assert_eq!(found[0]["address"], "10.9.9.9");
    assert_eq!(found[0]["node_id"], "host-a");
    assert_eq!(b.run(&["nodes", "list", "-o", "json"]).ok().json().as_array().unwrap().len(), 2);

    // Read-only access without any agent, straight from NATS.
    let direct = run("127.0.0.1:1", &["--direct", "--nats-url", &url, "discover", "web", "-o", "json"]).ok().json();
    assert_eq!(direct.as_array().unwrap().len(), 2);
    let status = run("127.0.0.1:1", &["--direct", "--nats-url", &url, "status", "-o", "json"]).ok().json();
    assert_eq!(status["backend"], "nats");
    assert!(status["node"].is_null());
    let r = run("127.0.0.1:1", &["--direct", "--nats-url", &url, "services", "register", "--name", "x"]);
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("needs the local agent"), "{}", r.stderr);

    let st = a.run(&["status", "-o", "json"]).ok().json();
    assert_eq!(st["backend"], "nats");
    assert_eq!(st["node"]["node_id"], "host-a");
}

#[test]
fn crash_expires_and_clean_exit_leaves() {
    let server = server!();
    let url = server.url();
    let mut a = AgentProc::start(&agent_args(&url, "host-a"));
    let mut b = AgentProc::start(&agent_args(&url, "host-b"));
    let c = AgentProc::start(&agent_args(&url, "host-c"));
    a.run(&["services", "register", "--name", "web"]).ok();
    b.run(&["services", "register", "--name", "api"]).ok();
    eventually("c sees both", || {
        count(&c.addr, &["discover", "web", "-o", "json"]) == 1
            && count(&c.addr, &["discover", "api", "-o", "json"]) == 1
    });

    // SIGKILL: no chance to leave, so the lease has to run out.
    let _ = a.child.kill();
    let _ = a.wait();
    eventually("crashed node expires", || count(&c.addr, &["discover", "web", "-o", "json"]) == 0);

    #[cfg(unix)]
    {
        let start = std::time::Instant::now();
        assert_eq!(b.terminate(), 0);
        eventually("clean exit removes membership", || count(&c.addr, &["discover", "api", "-o", "json"]) == 0);
        assert!(start.elapsed() < std::time::Duration::from_secs(3), "faster than the lease");
    }
    #[cfg(not(unix))]
    let _ = &mut b;
}

#[test]
fn node_id_persists_and_registrations_are_adopted() {
    let server = server!();
    let url = server.url();
    let args = [
        "--nats-url",
        url.as_str(),
        "--heartbeat-interval",
        "500ms",
        "--membership-ttl",
        "3s",
        "--health-ttl",
        "3s",
        "--leave-on-shutdown=false",
    ];
    let mut a = AgentProc::start(&args);
    a.run(&["services", "register", "--name", "web", "--port", "1"]).ok();
    let node_id = std::fs::read_to_string(a.data_path().join("node-id")).unwrap().trim().to_string();
    let first_gen = a.run(&["status", "-o", "json"]).ok().json()["node"]["generation"].as_str().unwrap().to_string();

    let _ = a.child.kill();
    let _ = a.wait();
    let data = std::mem::replace(&mut a.data_dir, tempfile::tempdir().unwrap());
    drop(a);

    let a2 = AgentProc::start_in(data, &args);
    let st = a2.run(&["status", "-o", "json"]).ok().json();
    assert_eq!(st["node"]["node_id"], node_id.as_str(), "same node id from the data dir");
    assert_ne!(st["node"]["generation"], first_gen.as_str(), "new generation");
    let local = a2.run(&["services", "local", "-o", "json"]).ok().json();
    assert_eq!(local[0]["service_id"], "web", "adopted");
    assert_eq!(count(&a2.addr, &["discover", "web", "-o", "json"]), 1);
}

#[test]
fn duplicate_node_id_fences_the_older_process() {
    let server = server!();
    let url = server.url();
    let mut old = AgentProc::start(&agent_args(&url, "same"));
    let new = AgentProc::start(&agent_args(&url, "same"));
    assert_eq!(old.wait(), 4, "fenced agents exit with code 4:\n{}", old.log_text());
    assert!(old.log_text().contains("fenced"), "{}", old.log_text());
    new.run(&["status"]).ok();
}

#[test]
fn health_checks_over_nats() {
    let server = server!();
    let url = server.url();
    let a = AgentProc::start(&agent_args(&url, "host-a"));
    let b = AgentProc::start(&agent_args(&url, "host-b"));
    a.run(&["services", "register", "--name", "web", "--check-ttl", "2s"]).ok();
    eventually("visible, not healthy", || {
        count(&b.addr, &["discover", "web", "-o", "json"]) == 1
            && count(&b.addr, &["discover", "web", "--healthy", "-o", "json"]) == 0
    });
    a.run(&["checks", "pass", "service:web"]).ok();
    eventually("healthy on b", || count(&b.addr, &["discover", "web", "--healthy", "-o", "json"]) == 1);
    eventually("TTL lapses on b", || count(&b.addr, &["discover", "web", "--healthy", "-o", "json"]) == 0);
}

#[test]
fn agent_reports_unreachable_nats() {
    let r: common::Run = common::lictor("127.0.0.1:0")
        .args(["agent", "--nats-url", "nats://127.0.0.1:1", "--node-id", "x"])
        .output()
        .unwrap()
        .into();
    assert_eq!(r.code, 3, "{}", r.stderr);
    assert!(r.stderr.contains("127.0.0.1:1"), "{}", r.stderr);
}
