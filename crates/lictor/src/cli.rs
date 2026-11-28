//! Command-line interface, declared with clyops (one registration per line).
//!
//! Every option's variable name doubles as its environment variable, so
//! `--nats-url` can also be set with `LICTOR_NATS_URL`. The `--config` file
//! (clyops format, `key = value` per line) can set any option of the command run.

use clyops::Cli;

pub const DEFAULT_HTTP_ADDR: &str = "127.0.0.1:7171";

fn filters(c: &mut Cli) {
    c.opt_array("LICTOR_TAG", "tag", "t", "Only instances with this tag (repeatable; all must match)").group("Filters");
    c.opt_array("LICTOR_META", "meta", "m", "Only instances with this metadata, key=value (repeatable)")
        .group("Filters");
    c.opt("LICTOR_FILTER_NODE", "node", "", "optional", "Only instances on this node").group("Filters");
    c.opt("LICTOR_HEALTHY", "healthy", "", "flag", "Only passing instances (same as --health passing)")
        .group("Filters");
    c.opt("LICTOR_HEALTH", "health", "", "any", "Health filter")
        .rule("choice:any,passing,not-critical")
        .group("Filters");
    scope(c);
}

fn scope(c: &mut Cli) {
    c.opt("LICTOR_SCOPE_NAMESPACE", "namespace", "n", "optional", "Only this namespace").group("Filters");
    c.opt("LICTOR_SCOPE_REGION", "region", "r", "optional", "Only this region").group("Filters");
}

fn check_options(c: &mut Cli, prefix: &str, group: &str) {
    let p = |s: &str| if prefix.is_empty() { s.to_string() } else { format!("{prefix}-{s}") };
    let v = |s: &str| format!("LICTOR_CHECK_{}", s.to_uppercase().replace('-', "_"));
    c.opt(&v("tcp"), &p("tcp"), "", "optional", "TCP check: host:port to connect to").group(group);
    c.opt(&v("http"), &p("http"), "", "optional", "HTTP check: URL that must answer 2xx").rule("url").group(group);
    c.opt(&v("ttl"), &p("ttl"), "", "optional", "TTL check: critical unless updated within this duration (e.g. 30s)")
        .group(group);
    c.opt(&v("interval"), &p("interval"), "", "optional", "Check interval (TCP/HTTP), e.g. 10s").group(group);
    c.opt(&v("timeout"), &p("timeout"), "", "optional", "Check timeout (TCP/HTTP), e.g. 3s").group(group);
    c.opt(&v("method"), &p("method"), "", "GET", "HTTP method").group(group);
    c.opt(&v("tls-skip-verify"), &p("tls-skip-verify"), "", "flag", "HTTP check: accept invalid TLS certificates")
        .group(group);
    c.opt(
        &v("deregister-after"),
        &p("deregister-after"),
        "",
        "optional",
        "Deregister the service after being critical this long",
    )
    .group(group);
    c.opt(&v("initial-status"), &p("initial-status"), "", "critical", "Status before the first result")
        .rule("choice:passing,warning,critical,unknown")
        .group(group);
    c.opt(&v("failures"), &p("failures"), "", "1", "Consecutive failures before critical")
        .rule("int:1-100")
        .group(group);
    c.opt(&v("successes"), &p("successes"), "", "1", "Consecutive successes before passing")
        .rule("int:1-100")
        .group(group);
}

/// Build the whole CLI.
#[rustfmt::skip]
pub fn build() -> Cli {
    let mut cli = Cli::new();
    cli.name("lictor");
    cli.description("Lictor: a lightweight service registry on NATS JetStream.\nRun 'lictor agent' on every host, then register, discover and check services.");
    cli.epilog("Run 'lictor <command> --help' for a command's options. Every option can also be set from its environment variable (shown as the variable name in --help-json-schema) or from the --config file.");

    cli.opt("LICTOR_OUTPUT", "output", "o", "text", "Output format").rule("choice:text,json").group("Global");
    cli.opt("LICTOR_HTTP_ADDR", "http-addr", "a", DEFAULT_HTTP_ADDR, "Agent HTTP API address (the agent listens here; commands connect here)").group("Global");
    cli.opt("LICTOR_HTTP_TOKEN", "token", "", "optional", "Bearer token for the agent HTTP API").rule("secret").group("Global");
    cli.opt("LICTOR_DIRECT", "direct", "", "flag", "Read the registry straight from NATS instead of a local agent (read-only commands)").group("Global");
    cli.opt("LICTOR_CONFIG", "config", "c", "optional", "Config file (lines of option = value)").rule("file:readable").group("Global");
    cli.opt("LICTOR_LOG_LEVEL", "log-level", "", "info", "Log level or tracing filter (e.g. debug, lictor_core=trace)").group("Global");
    cli.opt("LICTOR_LOG_FORMAT", "log-format", "", "text", "Log format").rule("choice:text,json").group("Global");
    cli.config("config", "");

    cli.opt("LICTOR_NATS_URL", "nats-url", "", "nats://127.0.0.1:4222", "NATS server URL(s), comma-separated").group("NATS");
    cli.opt("LICTOR_NATS_CREDS", "nats-creds", "", "optional", "NATS credentials (.creds) file").rule("file:readable").group("NATS");
    cli.opt("LICTOR_NATS_NKEY", "nats-nkey", "", "optional", "File holding an NKey seed").rule("file:readable").group("NATS");
    cli.opt("LICTOR_NATS_USER", "nats-user", "", "optional", "NATS user name").group("NATS");
    cli.opt("LICTOR_NATS_PASSWORD", "nats-password", "", "optional", "NATS password").rule("secret").group("NATS");
    cli.opt("LICTOR_NATS_TOKEN", "nats-token", "", "optional", "NATS token").rule("secret").group("NATS");
    cli.opt("LICTOR_NATS_TLS_CA", "nats-tls-ca", "", "optional", "CA certificate (PEM) for the NATS server").rule("file:readable").group("NATS");
    cli.opt("LICTOR_NATS_TLS_CERT", "nats-tls-cert", "", "optional", "Client certificate (PEM) for mutual TLS").rule("file:readable").group("NATS");
    cli.opt("LICTOR_NATS_TLS_KEY", "nats-tls-key", "", "optional", "Client key (PEM) for mutual TLS").rule("file:readable").group("NATS");
    cli.opt("LICTOR_NATS_TLS_REQUIRED", "nats-tls-required", "", "flag", "Require TLS to NATS").group("NATS");
    cli.opt("LICTOR_BUCKET_PREFIX", "bucket-prefix", "", "LICTOR", "Prefix of the JetStream KV bucket names").group("NATS");
    cli.requires("nats-tls-cert", &["nats-tls-key"]);
    cli.requires("nats-tls-key", &["nats-tls-cert"]);

    // -- agent ---------------------------------------------------------------
    let agent = cli.command("agent", "Run the agent (the long-running process on every host)");
    agent.opt("LICTOR_DEV", "dev", "", "flag", "Development mode: in-memory store, no NATS needed, nothing persisted").group("Agent");
    agent.opt("LICTOR_NODE_ID", "node-id", "", "optional", "Node identifier (default: generated once and saved in the data dir)").group("Agent");
    agent.opt("LICTOR_DATA_DIR", "data-dir", "", "optional", "Where the node id is kept (default: /var/lib/lictor as root, else ~/.local/share/lictor)").group("Agent");
    agent.opt("LICTOR_NAMESPACE", "namespace", "n", "default", "Namespace of this node").group("Agent");
    agent.opt("LICTOR_REGION", "region", "r", "dc1", "Region (Consul: datacenter) of this node").group("Agent");
    agent.opt("LICTOR_HOSTNAME", "hostname", "", "optional", "Host name to advertise (default: system host name)").group("Agent");
    agent.opt("LICTOR_ADVERTISE", "advertise", "", "optional", "Address(es) to advertise, comma-separated (default: primary interface)").group("Agent");
    agent.opt("LICTOR_NODE_META", "node-meta", "", "optional", "Node metadata, comma-separated key=value pairs").group("Agent");
    agent.opt("LICTOR_CONFIG_DIR", "config-dir", "", "optional", "Directory of service definitions (*.toml, *.json) to register at start").rule("dir:exists").group("Agent");
    agent.opt("LICTOR_UI", "ui", "", "true", "Serve the web UI at /ui on the HTTP API address").rule("bool").group("Agent");
    agent.opt("LICTOR_LEAVE_ON_SHUTDOWN", "leave-on-shutdown", "", "true", "Remove this node's membership on a clean shutdown").rule("bool").group("Agent");
    agent.opt("LICTOR_RECONCILE", "reconcile", "", "adopt", "What to do with registrations from this node's previous run").rule("choice:adopt,drop,keep").group("Agent");
    agent.opt("LICTOR_REPLICAS", "replicas", "", "1", "Replicas for buckets this agent creates (use 3 in a cluster)").rule("int:1-5").group("Registry");
    agent.opt("LICTOR_CREATE_BUCKETS", "create-buckets", "", "true", "Create missing buckets (needs JetStream admin rights)").rule("bool").group("Registry");
    agent.opt("LICTOR_HEARTBEAT_INTERVAL", "heartbeat-interval", "", "10s", "How often membership is refreshed").group("Registry");
    agent.opt("LICTOR_MEMBERSHIP_TTL", "membership-ttl", "", "30s", "How long a node lives without heartbeats").group("Registry");
    agent.opt("LICTOR_HEALTH_TTL", "health-ttl", "", "30s", "How long health stays valid without a refresh").group("Registry");
    agent.opt("LICTOR_CLOCK_SKEW", "clock-skew", "", "2s", "Tolerated clock difference to the NATS servers").group("Registry");
    agent.opt("LICTOR_STALE_RETENTION", "stale-retention", "", "72h", "Delete registrations of nodes gone this long (0 disables)").group("Registry");
    agent.opt("LICTOR_CHECK_DEFAULT_INTERVAL", "default-check-interval", "", "10s", "Interval for checks that set none").group("Health");
    agent.opt("LICTOR_CHECK_DEFAULT_TIMEOUT", "default-check-timeout", "", "3s", "Timeout for checks that set none").group("Health");
    agent.opt("LICTOR_MAX_CONCURRENT_CHECKS", "max-concurrent-checks", "", "128", "Checks allowed to run at once").rule("int:1-100000").group("Health");

    // -- status / version ----------------------------------------------------
    cli.command("status", "Show the agent's (or with --direct, the catalog's) status").effects(&["read-only"]);
    cli.command("version", "Print the version").effects(&["read-only"]);

    // -- nodes ---------------------------------------------------------------
    let nodes = cli.command("nodes", "Inspect nodes");
    let list = nodes.command("list", "List live nodes");
    scope(list);
    list.effects(&["read-only"]);

    // -- services ------------------------------------------------------------
    let services = cli.command("services", "Register, remove and inspect services");
    let list = services.command("list", "List services with live instances");
    scope(list);
    list.effects(&["read-only"]);
    let inspect = services.command("inspect", "Show every live instance of a service, with checks");
    inspect.arg("name", "Service name", "", "");
    filters(inspect);
    inspect.effects(&["read-only"]);
    services.command("local", "List the services registered on this agent").effects(&["read-only"]);
    let reg = services.command("register", "Register a service on this agent");
    reg.opt("LICTOR_SERVICE_FILE", "file", "f", "optional", "Service definition file (JSON or TOML; '-' for stdin)").group("Service");
    reg.opt("LICTOR_SERVICE_NAME", "name", "", "optional", "Service name").group("Service");
    reg.opt("LICTOR_SERVICE_ID", "id", "", "optional", "Service id, unique on this node (default: the name)").group("Service");
    reg.opt("LICTOR_SERVICE_ADDRESS", "address", "", "optional", "Address (default: the node's advertise address)").group("Service");
    reg.opt("LICTOR_SERVICE_PORT", "port", "p", "0", "Port").rule("int:0-65535").group("Service");
    reg.opt("LICTOR_SERVICE_PROTOCOL", "protocol", "", "tcp", "Protocol label").group("Service");
    reg.opt_array("LICTOR_SERVICE_TAG", "tag", "t", "Tag (repeatable)").group("Service");
    reg.opt_array("LICTOR_SERVICE_META", "meta", "m", "Metadata key=value (repeatable)").group("Service");
    check_options(reg, "check", "Check");
    reg.one_of(&["file", "name"]);
    reg.exclusive(&["file", "name"]);
    reg.effects(&["idempotent"]);
    let export = services.command("export", "Print the stored definition of a local service (a file for 'register --file')");
    export.arg("id", "Service id", "", "");
    export.effects(&["read-only"]);
    let dereg = services.command("deregister", "Remove a service registered on this agent");
    dereg.arg("id", "Service id", "", "");
    dereg.effects(&["destructive", "idempotent"]);

    // -- discover / watch ----------------------------------------------------
    let discover = cli.command("discover", "Find live instances of a service");
    discover.arg("name", "Service name", "", "");
    filters(discover);
    discover.effects(&["read-only"]);
    let watch = cli.command("watch", "Print the instances of a service every time they change (until interrupted)");
    watch.arg("name", "Service name", "", "");
    filters(watch);
    watch.effects(&["read-only"]);

    // -- health --------------------------------------------------------------
    let health = cli.command("health", "Health overview");
    let hs = health.command("status", "Health of every service, or of each instance of one service");
    hs.arg_variadic("name", "Service name (omit for every service)", "");
    scope(hs);
    hs.effects(&["read-only"]);

    // -- checks --------------------------------------------------------------
    let checks = cli.command("checks", "Manage the health checks of this agent's services");
    checks.command("list", "List the checks of this agent's services").effects(&["read-only"]);
    let cr = checks.command("register", "Add a check to a service registered on this agent");
    cr.opt("LICTOR_CHECK_SERVICE", "service", "s", "", "Service id the check belongs to").group("Check");
    cr.opt("LICTOR_CHECK_ID", "id", "", "optional", "Check id (default: service:<service-id>)").group("Check");
    cr.opt("LICTOR_CHECK_NAME", "name", "", "optional", "Check name").group("Check");
    check_options(cr, "", "Check");
    cr.one_of(&["tcp", "http", "ttl"]);
    cr.exclusive(&["tcp", "http", "ttl"]);
    let cd = checks.command("deregister", "Remove a check");
    cd.arg("id", "Check id", "", "");
    cd.effects(&["destructive", "idempotent"]);
    for (word, what) in [("pass", "passing"), ("warn", "warning"), ("fail", "critical")] {
        let c = checks.command(word, &format!("Report a TTL check as {what}"));
        c.arg("id", "Check id", "", "");
        c.opt("LICTOR_CHECK_NOTE", "note", "", "optional", "Output to record with the update");
        c.effects(&["idempotent"]);
    }
    cli
}

#[cfg(test)]
mod tests {
    use super::*;
    use clyops::Parsed;

    fn parse(args: &[&str]) -> (Cli, Parsed) {
        let mut cli = build();
        cli.env(Vec::<(String, String)>::new());
        let p = cli.parse(args);
        (cli, p)
    }

    #[test]
    fn global_options_work_at_any_depth() {
        let (cli, p) = parse(&["services", "list", "-o", "json"]);
        assert_eq!(p, Parsed::Ok);
        assert_eq!(cli.command_path(), ["services", "list"]);
        assert_eq!(cli.values().str("LICTOR_OUTPUT"), "json");
        let (cli, p) = parse(&["--output=json", "discover", "web", "--healthy", "-t", "v1", "-t", "v2"]);
        assert_eq!(p, Parsed::Ok);
        assert!(cli.values().bool("LICTOR_HEALTHY"));
        assert_eq!(cli.values().strs("LICTOR_TAG"), ["v1", "v2"]);
        assert_eq!(cli.values().str("name"), "web");
    }

    #[test]
    fn rejects_bad_input() {
        assert!(matches!(parse(&["discover"]).1, Parsed::Error { .. }), "name is required");
        assert!(matches!(parse(&["-o", "xml", "status"]).1, Parsed::Error { .. }));
        assert!(matches!(parse(&["services", "register"]).1, Parsed::Error { .. }), "file or name");
        assert!(matches!(parse(&["services", "register", "--name", "a", "--file", "x"]).1, Parsed::Error { .. }));
        assert!(matches!(parse(&["services", "register", "--name", "a", "--port", "70000"]).1, Parsed::Error { .. }));
        assert!(matches!(parse(&["checks", "register", "--service", "a"]).1, Parsed::Error { .. }), "one kind");
        assert!(matches!(
            parse(&["checks", "register", "--service", "a", "--tcp", "h:1", "--ttl", "5s"]).1,
            Parsed::Error { .. }
        ));
        assert!(matches!(parse(&["nope"]).1, Parsed::Error { .. }));
    }

    #[test]
    fn help_and_schema_render() {
        let mut cli = build();
        assert!(cli.usage().contains("lictor"));
        let schema = cli.json_schema();
        assert!(schema.contains("\"clyops\""));
        assert!(schema.contains("discover"));
    }

    #[test]
    fn agent_defaults() {
        let (cli, p) = parse(&["agent", "--dev"]);
        assert_eq!(p, Parsed::Ok);
        let v = cli.values();
        assert!(v.bool("LICTOR_DEV"));
        assert_eq!(v.str("LICTOR_NAMESPACE"), "default");
        assert_eq!(v.str("LICTOR_REGION"), "dc1");
        assert_eq!(v.int("LICTOR_REPLICAS"), 1);
        assert!(v.bool("LICTOR_LEAVE_ON_SHUTDOWN"));
        assert_eq!(v.str("LICTOR_HTTP_ADDR"), DEFAULT_HTTP_ADDR);
    }
}
