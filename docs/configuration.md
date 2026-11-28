# Configuration

## Where settings come from

Every option can be set four ways. The first one found wins:

1. the command line: `--nats-url nats://n1:4222`
2. the config file given with `--config` (or `LICTOR_CONFIG`)
3. an environment variable: `LICTOR_NATS_URL=nats://n1:4222`
4. the default

### The config file

One `option = value` per line, using the long option names. `#` starts a comment and
`@include other.conf` includes another file. The packaged agent reads `/etc/lictor/lictor.conf`:

```ini
nats-url = tls://nats-01:4222,tls://nats-02:4222,tls://nats-03:4222
nats-creds = /etc/lictor/nats.creds
namespace = production
region = us-east
advertise = 10.10.1.25
node-meta = provider=aws,rack=r12
config-dir = /etc/lictor/conf.d
replicas = 3
```

The file may hold options of any command; each command uses the ones it knows and ignores the rest,
so one file serves both `lictor agent` and the client commands. Array options (`--tag`) take one value from a file.

### Environment variables

Each option's variable is listed by `lictor <command> --help-json-schema` (`variableName`).
The ones you are most likely to use:

| Variable | Option |
|---|---|
| `LICTOR_HTTP_ADDR` | `--http-addr` |
| `LICTOR_HTTP_TOKEN` | `--token` |
| `LICTOR_OUTPUT` | `--output` |
| `LICTOR_CONFIG` | `--config` |
| `LICTOR_NATS_URL`, `LICTOR_NATS_CREDS`, `LICTOR_NATS_NKEY`, `LICTOR_NATS_USER`, `LICTOR_NATS_PASSWORD`, `LICTOR_NATS_TOKEN` | NATS connection |
| `LICTOR_NATS_TLS_CA`, `LICTOR_NATS_TLS_CERT`, `LICTOR_NATS_TLS_KEY`, `LICTOR_NATS_TLS_REQUIRED` | NATS TLS |
| `LICTOR_NODE_ID`, `LICTOR_DATA_DIR`, `LICTOR_NAMESPACE`, `LICTOR_REGION`, `LICTOR_ADVERTISE`, `LICTOR_CONFIG_DIR` | Agent identity |
| `LICTOR_LOG_LEVEL`, `LICTOR_LOG_FORMAT` | Logging (`RUST_LOG` overrides the level) |
| `NO_COLOR` | Disable colored logs |

## Agent options

### Identity

| Option | Default | Meaning |
|---|---|---|
| `--node-id` | generated | Persistent node identifier. Generated once (`<hostname>-<4 hex>`) and saved in `<data-dir>/node-id`. |
| `--data-dir` | `/var/lib/lictor` (root), else `~/.local/share/lictor` | Where the node id lives. |
| `--namespace` | `default` | Namespace of this node and its services. |
| `--region` | `dc1` | Region; Consul calls this the datacenter. |
| `--hostname` | system host name | Advertised host name. |
| `--advertise` | primary interface address | Comma-separated addresses; the first is the default service address. |
| `--node-meta` | none | `key=value,key=value`. `os` and `arch` are added automatically. |

Identifiers (namespace, region, node id, service names and ids) use letters, digits, `_` and `-`.
Tags are free text up to 255 characters (spaces allowed, as in Consul; no control characters).

### Behavior

| Option | Default | Meaning |
|---|---|---|
| `--dev` | off | In-memory store, nothing persisted, no NATS. |
| `--config-dir` | none | Register every `*.toml` / `*.json` definition in this directory at start. |
| `--leave-on-shutdown` | `true` | On a clean stop, delete the membership so services vanish immediately. |
| `--reconcile` | `adopt` | Registrations from this node's previous run: `adopt`, `drop` or `keep` (inactive). |
| `--http-addr` | `127.0.0.1:7171` | Where the HTTP API listens. |
| `--token` | none | Require `Authorization: Bearer <token>` on every API request. |
| `--ui` | `true` | Serve the [web UI](ui.md) at `/ui`. |

### Consul compatibility

See [consul.md](consul.md). Both listeners use Consul's ports; `disabled` turns one off.

| Option | Default | Meaning |
|---|---|---|
| `--dns-addr` | `127.0.0.1:8600` | Consul-compatible DNS, UDP and TCP. |
| `--dns-domain` | `consul` | The domain answered (`web.service.consul`). |
| `--dns-service-ttl`, `--dns-node-ttl` | `0s` | TTL of service and node records (Consul's default is 0). |
| `--dns-only-passing` | `false` | Leave warning instances out of answers (critical ones always are). |
| `--dns-udp-answer-limit` | `3` | Most answers over UDP when the query has no EDNS. |
| `--consul-http-addr` | `127.0.0.1:8500` | Consul-compatible catalog and health API (read-only), for Prometheus and other clients. |

### Registry and leases

| Option | Default | Meaning |
|---|---|---|
| `--bucket-prefix` | `LICTOR` | Buckets are `<prefix>_NODES`, `<prefix>_SERVICES`, `<prefix>_HEALTH`. Separate registries can share a NATS account. |
| `--replicas` | `1` | Replicas of buckets this agent creates. Use `3` on a NATS cluster. |
| `--create-buckets` | `true` | Create missing buckets. Turn off when agents lack JetStream admin rights. |
| `--heartbeat-interval` | `10s` | Membership refresh. Must be shorter than the membership TTL. |
| `--membership-ttl` | `30s` | Lease length for new buckets. Failure detection takes from TTL - interval to TTL. |
| `--health-ttl` | `30s` | Health record lifetime for new buckets; health is refreshed every TTL / 3. |
| `--clock-skew` | `2s` | Tolerated clock difference to the NATS servers. |
| `--stale-retention` | `72h` | Delete registrations of nodes gone this long. `0` disables. |

TTLs are properties of the buckets and are fixed by whoever creates them. Agents that start
later use the buckets' TTLs (and refuse to start if their heartbeat would be too slow).

### Health checks

| Option | Default | Meaning |
|---|---|---|
| `--default-check-interval` | `10s` | For checks that set no interval. |
| `--default-check-timeout` | `3s` | For checks that set no timeout. |
| `--max-concurrent-checks` | `128` | Checks running at once. |

Durations are written `500ms`, `10s`, `1m30s`, `2h`, or a bare number of seconds.

## Service definitions

Used by `--config-dir` and `lictor services register --file`. TOML or JSON, one service or several.

```toml
# One service: fields at the top level.
id = "web-1"                   # default: the name
name = "web"
address = "10.0.1.5"           # default: the node's advertise address
port = 8080
protocol = "http"              # a free-form label, default "tcp"
tags = ["v2", "primary"]
metadata = { version = "2.1.0", shard = "4" }

[[checks]]
id = "web-http"                # default: service:<id>
name = "HTTP health"
type = "http"                  # tcp | http | ttl
url = "http://127.0.0.1:8080/health"
method = "GET"
headers = { Authorization = "Bearer abc" }
tls_skip_verify = false
interval = "10s"
timeout = "2s"
failures_before_critical = 3
successes_before_passing = 2
initial_status = "critical"    # passing | warning | critical | unknown
deregister_critical_after = "30m"
notes = "Owned by the web team"

[[checks]]
type = "tcp"
address = "127.0.0.1:8080"

[[checks]]
type = "ttl"
ttl = "30s"
```

Several services in one file:

```toml
[[service]]
name = "web"
port = 8080

[[service]]
name = "worker"
[[service.checks]]
type = "ttl"
ttl = "1m"
```

JSON uses the same fields: a service object, an array of them, or `{"services": [...]}`. Unknown fields are errors.
