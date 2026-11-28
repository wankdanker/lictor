//! End-to-end CLI tests against `lictor agent --dev` (no NATS needed; runs on every OS).

mod common;
use common::{AgentProc, eventually, run, run_stdin};

#[test]
fn full_lifecycle_text_and_json() {
    let a = AgentProc::start(&["--dev"]);
    let r = a
        .run(&[
            "services",
            "register",
            "--name",
            "web",
            "--port",
            "8080",
            "-t",
            "v2",
            "-m",
            "shard=4",
            "--check-ttl",
            "30s",
        ])
        .ok();
    assert!(r.stdout.contains("Registered service 'web'") && r.stdout.contains("service:web"), "{}", r.stdout);

    // A TTL check starts critical: not returned by healthy queries.
    let healthy = a.run(&["discover", "web", "--healthy", "-o", "json"]).ok().json();
    assert_eq!(healthy.as_array().unwrap().len(), 0);
    let all = a.run(&["discover", "web", "-o", "json"]).ok().json();
    assert_eq!(all[0]["status"], "critical");

    a.run(&["checks", "pass", "service:web", "--note", "fine"]).ok();
    let healthy = a.run(&["discover", "web", "--healthy", "-o", "json"]).ok().json();
    assert_eq!(healthy[0]["port"], 8080);
    assert_eq!(healthy[0]["metadata"]["shard"], "4");
    assert_eq!(healthy[0]["checks"][0]["output"], "fine");

    let text = a.run(&["discover", "web"]).ok().stdout;
    assert!(text.starts_with("SERVICE"), "{text}");
    assert!(text.contains("passing"));

    a.run(&["checks", "warn", "service:web"]).ok();
    assert_eq!(
        a.run(&["discover", "web", "--health", "not-critical", "-o", "json"]).ok().json()[0]["status"],
        "warning"
    );
    a.run(&["checks", "fail", "service:web"]).ok();
    assert_eq!(
        a.run(&["discover", "web", "--health", "not-critical", "-o", "json"]).ok().json().as_array().unwrap().len(),
        0
    );

    let list = a.run(&["services", "list", "-o", "json"]).ok().json();
    assert_eq!(list[0]["name"], "web");
    assert_eq!(list[0]["critical"], 1);
    assert_eq!(a.run(&["nodes", "list", "-o", "json"]).ok().json()[0]["node_id"], "dev");
    assert!(a.run(&["services", "inspect", "web"]).ok().stdout.contains("Checks:"));
    assert_eq!(a.run(&["checks", "list", "-o", "json"]).ok().json()[0]["id"], "service:web");
    assert_eq!(a.run(&["services", "local", "-o", "json"]).ok().json()[0]["service_id"], "web");
    assert!(a.run(&["health", "status"]).ok().stdout.contains("CRITICAL"));
    assert!(a.run(&["health", "status", "web"]).ok().stdout.contains("critical"));

    let status = a.run(&["status", "-o", "json"]).ok().json();
    assert_eq!(status["backend"], "memory");
    assert_eq!(status["local_services"], 1);

    a.run(&["services", "deregister", "web"]).ok();
    assert_eq!(a.run(&["discover", "web", "-o", "json"]).ok().json().as_array().unwrap().len(), 0);
}

#[test]
fn filters() {
    let a = AgentProc::start(&["--dev"]);
    a.run(&["services", "register", "--name", "web", "--id", "w1", "-t", "v1", "-m", "zone=a"]).ok();
    a.run(&["services", "register", "--name", "web", "--id", "w2", "-t", "v2", "-m", "zone=b"]).ok();
    let ids = |args: &[&str]| -> Vec<String> {
        let mut full = vec!["discover", "web", "-o", "json"];
        full.extend_from_slice(args);
        a.run(&full)
            .ok()
            .json()
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["service_id"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(ids(&[]), ["w1", "w2"]);
    assert_eq!(ids(&["-t", "v2"]), ["w2"]);
    assert_eq!(ids(&["-m", "zone=a"]), ["w1"]);
    assert_eq!(ids(&["--node", "dev", "-t", "v1"]), ["w1"]);
    assert!(ids(&["--node", "other"]).is_empty());
    assert!(ids(&["--region", "eu"]).is_empty());
    assert_eq!(ids(&["--namespace", "default", "--region", "dc1"]).len(), 2);
}

#[test]
fn export_prints_a_definition_that_registers_again() {
    let a = AgentProc::start(&["--dev"]);
    a.run(&["services", "register", "--name", "web", "--port", "8080", "-t", "v2", "--check-ttl", "30s"]).ok();
    let def = a.run(&["services", "export", "web"]).ok();
    let json: serde_json::Value = serde_json::from_str(&def.stdout).unwrap();
    assert_eq!(json["id"], "web");
    assert_eq!(json["port"], 8080);
    assert_eq!(json["checks"][0]["id"], "service:web");
    assert_eq!(json["checks"][0]["ttl"], "30s");

    a.run(&["services", "deregister", "web"]).ok();
    run_stdin(&a.addr, &["services", "register", "--file", "-"], &def.stdout).ok();
    assert_eq!(a.run(&["services", "export", "web"]).ok().stdout, def.stdout);
    assert_eq!(a.run(&["services", "export", "nope"]).code, 2);
}

#[test]
fn register_from_files_and_stdin() {
    let a = AgentProc::start(&["--dev"]);
    let dir = tempfile::tempdir().unwrap();
    let toml = dir.path().join("svc.toml");
    std::fs::write(
        &toml,
        r#"
[[service]]
name = "api"
port = 9000
tags = ["edge"]

[[service.checks]]
id = "api-ttl"
type = "ttl"
ttl = "1m"

[[service]]
name = "db"
port = 5432
"#,
    )
    .unwrap();
    let r = a.run(&["services", "register", "-f", toml.to_str().unwrap(), "-o", "json"]).ok().json();
    assert_eq!(r.as_array().unwrap().len(), 2);
    assert_eq!(r[0]["checks"][0], "api-ttl");

    let json = dir.path().join("svc.json");
    std::fs::write(&json, r#"{"name":"cache","port":6379,"meta":{"role":"primary"}}"#).unwrap();
    a.run(&["services", "register", "--file", json.to_str().unwrap()]).ok();
    run_stdin(&a.addr, &["services", "register", "--file", "-"], r#"{"name":"queue","port":5672}"#).ok();
    let names: Vec<String> = a
        .run(&["services", "list", "-o", "json"])
        .ok()
        .json()
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap().into())
        .collect();
    assert_eq!(names, ["api", "cache", "db", "queue"]);

    std::fs::write(&json, r#"{"name":"bad","prot":1}"#).unwrap();
    let r = a.run(&["services", "register", "--file", json.to_str().unwrap()]);
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("unknown field"), "{}", r.stderr);
}

#[test]
#[cfg_attr(windows, ignore = "clyops 0.13.1 resolves absolute Windows paths incorrectly; see docs/development.md")]
fn config_dir_services_are_registered_at_start() {
    let conf = tempfile::tempdir().unwrap();
    std::fs::write(conf.path().join("web.toml"), "name = \"web\"\nport = 80\n").unwrap();
    std::fs::write(conf.path().join("ignored.txt"), "nope").unwrap();
    let a = AgentProc::start(&["--dev", "--config-dir", conf.path().to_str().unwrap()]);
    assert_eq!(a.run(&["discover", "web", "-o", "json"]).ok().json()[0]["port"], 80);
}

#[test]
#[cfg_attr(windows, ignore = "clyops 0.13.1 resolves absolute Windows paths incorrectly; see docs/development.md")]
fn bad_config_dir_fails_the_agent() {
    let conf = tempfile::tempdir().unwrap();
    std::fs::write(conf.path().join("web.toml"), "name = \"bad name\"\n").unwrap();
    let r: common::Run = common::lictor("127.0.0.1:0")
        .args(["agent", "--dev", "--config-dir", conf.path().to_str().unwrap()])
        .output()
        .unwrap()
        .into();
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("web.toml"), "{}", r.stderr);
}

#[test]
fn exit_codes_and_error_output() {
    let a = AgentProc::start(&["--dev"]);
    let r = a.run(&["services", "deregister", "nope"]);
    assert_eq!(r.code, 2, "not found");
    assert!(r.stderr.contains("service 'nope' not found"), "{}", r.stderr);
    let r = a.run(&["services", "inspect", "nope", "-o", "json"]);
    assert_eq!(r.code, 2);
    let err: serde_json::Value = serde_json::from_str(r.stderr.trim()).unwrap();
    assert_eq!(err["error"]["code"], "not_found");

    let r = a.run(&["services", "register", "--name", "bad.name"]);
    assert_eq!(r.code, 1, "invalid");
    let r = a.run(&["checks", "pass", "nope"]);
    assert_eq!(r.code, 2);
    a.run(&["services", "register", "--name", "web", "--check-tcp", "127.0.0.1:1"]).ok();
    let r = a.run(&["checks", "pass", "service:web"]);
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("only TTL checks"), "{}", r.stderr);

    // Nothing listening.
    let r = run(common::NOTHING_LISTENS, &["status"]);
    assert_eq!(r.code, 3, "unavailable");
    assert!(r.stderr.contains("Is 'lictor agent' running?"), "{}", r.stderr);

    // Usage errors come from the parser.
    let r = run(&a.addr, &["discover"]);
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("Missing required positional argument"), "{}", r.stderr);
}

#[test]
fn checks_register_and_deregister() {
    let a = AgentProc::start(&["--dev"]);
    a.run(&["services", "register", "--name", "web"]).ok();
    let r =
        a.run(&["checks", "register", "--service", "web", "--ttl", "30s", "--id", "beat", "-o", "json"]).ok().json();
    assert_eq!(r["check_id"], "beat");
    let r = a.run(&["checks", "register", "--service", "web", "--tcp", "127.0.0.1:9", "--interval", "1s"]).ok();
    assert!(r.stdout.contains("service:web"), "{}", r.stdout);
    assert_eq!(a.run(&["checks", "list", "-o", "json"]).ok().json().as_array().unwrap().len(), 2);
    a.run(&["checks", "deregister", "beat"]).ok();
    assert_eq!(a.run(&["checks", "list", "-o", "json"]).ok().json().as_array().unwrap().len(), 1);
    let r = a.run(&["checks", "register", "--service", "web", "--ttl", "soon"]);
    assert_eq!(r.code, 1);
}

#[test]
fn tcp_check_goes_passing() {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port().to_string();
    let a = AgentProc::start(&["--dev"]);
    a.run(&[
        "services",
        "register",
        "--name",
        "web",
        "--port",
        &port,
        "--check-tcp",
        &format!("127.0.0.1:{port}"),
        "--check-interval",
        "200ms",
    ])
    .ok();
    eventually("tcp check passes", || {
        a.run(&["discover", "web", "--healthy", "-o", "json"]).json().as_array().is_some_and(|v| v.len() == 1)
    });
}

#[test]
fn api_token_is_enforced() {
    let mut args = vec!["--dev"];
    args.push("--token");
    args.push("s3cret");
    // The agent and CLI share LICTOR_HTTP_TOKEN, so pass it explicitly to the agent only.
    let a = AgentProc::start(&args);
    let r = a.run(&["services", "list"]);
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("rejected the API token"), "{}", r.stderr);
    a.run(&["--token", "s3cret", "services", "list"]).ok();
    common::lictor(&a.addr)
        .env("LICTOR_HTTP_TOKEN", "s3cret")
        .args(["status"])
        .status()
        .unwrap()
        .success()
        .then_some(())
        .expect("env token works");
}

#[test]
#[cfg_attr(windows, ignore = "clyops 0.13.1 resolves absolute Windows paths incorrectly; see docs/development.md")]
fn config_file_sets_options() {
    let a = AgentProc::start(&["--dev"]);
    let dir = tempfile::tempdir().unwrap();
    let conf = dir.path().join("lictor.conf");
    std::fs::write(&conf, format!("# CLI defaults\nhttp-addr = {}\noutput = json\n", a.addr)).unwrap();
    // Point the CLI at a wrong address; the config file corrects it.
    let r = run("127.0.0.1:1", &["--config", conf.to_str().unwrap(), "status"]);
    // Environment has lower precedence than config, so the file wins over LICTOR_HTTP_ADDR.
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.json()["backend"], "memory");
}

#[test]
fn version_help_schema_and_completion() {
    let r = run("127.0.0.1:1", &["--version"]).ok();
    assert!(r.stdout.starts_with("lictor "));
    let r = run("127.0.0.1:1", &["version", "-o", "json"]).ok();
    assert_eq!(r.json()["version"], env!("CARGO_PKG_VERSION"));
    let r = run("127.0.0.1:1", &["--help"]).ok();
    assert!(r.stdout.contains("discover") && r.stdout.contains("agent"));
    let r = run("127.0.0.1:1", &["agent", "--help"]).ok();
    assert!(r.stdout.contains("--membership-ttl"));
    let r = run("127.0.0.1:1", &["--help-json-schema"]).ok();
    assert!(r.json()["clyops"].is_number());
    let r = run("127.0.0.1:1", &["--completion", "bash"]).ok();
    assert!(r.stdout.contains("lictor"));
}

#[test]
fn watch_prints_ndjson() {
    use std::io::{BufRead, BufReader};
    let a = AgentProc::start(&["--dev"]);
    let mut child = common::lictor(&a.addr)
        .args(["watch", "web", "-o", "json"])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let first: serde_json::Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
    assert_eq!(first["instances"].as_array().unwrap().len(), 0);
    a.run(&["services", "register", "--name", "web", "--port", "1"]).ok();
    let second: serde_json::Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
    assert_eq!(second["instances"][0]["port"], 1);
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn direct_mode_needs_nats_and_says_so() {
    let r = run("127.0.0.1:1", &["--direct", "--nats-url", "nats://127.0.0.1:1", "services", "list"]);
    assert_eq!(r.code, 3, "{}", r.stderr);
}

#[cfg(unix)]
#[test]
fn sigterm_shuts_down_cleanly() {
    let mut a = AgentProc::start(&["--dev"]);
    assert_eq!(a.terminate(), 0);
    assert!(a.log_text().contains("left the registry"), "{}", a.log_text());
}

/// A bare HTTP GET; returns the status code and the body.
fn http_get(addr: &str, path: &str) -> (u16, String) {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(addr).unwrap();
    write!(s, "GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n").unwrap();
    let mut response = String::new();
    s.read_to_string(&mut response).unwrap();
    let status = response.split_whitespace().nth(1).unwrap().parse().unwrap();
    let body = response.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    (status, body)
}

#[test]
fn web_ui_is_served_unless_disabled() {
    // With a token: the UI's files are public, the API is not.
    let a = AgentProc::start(&["--dev", "--token", "s3cret"]);
    let (status, body) = http_get(&a.addr, "/ui/services/web");
    // 200 with the app, or 404 with build instructions when the UI was not built into this binary.
    match status {
        200 => assert!(body.contains("_app/"), "{body}"),
        404 => assert!(body.contains("npm --prefix ui run build"), "{body}"),
        other => panic!("unexpected status {other}: {body}"),
    }
    assert_eq!(http_get(&a.addr, "/").0, 307);
    assert_eq!(http_get(&a.addr, "/v1/status").0, 401);

    let b = AgentProc::start(&["--dev", "--ui=false"]);
    let (status, body) = http_get(&b.addr, "/ui/services/web");
    assert_eq!(status, 404);
    assert!(body.contains("no such endpoint"), "{body}");
}
