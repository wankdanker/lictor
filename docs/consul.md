# Consul compatibility

Every agent answers Consul's DNS names and the read side of Consul's HTTP API from its local
catalog, so programs that discover services through Consul keep working when Consul goes away:

| | Default | |
|---|---|---|
| DNS | `127.0.0.1:8600` (UDP and TCP) | `web.service.consul`, `v2.web.service.consul`, SRV, nodes |
| HTTP | `127.0.0.1:8500` | Catalog and health reads with blocking queries: enough for Prometheus `consul_sd_configs` |

Services are registered with Lictor's own tools (`lictor services register`, `--config-dir` files,
the [native API](http-api.md) or the [web UI](ui.md)). Consul's registration endpoints are not
implemented.

The behavior below was measured against Consul 1.22.7, and `crates/lictor/tests/consul_compat.rs`
keeps it that way: it registers the same services in a real Consul and in Lictor and compares the
DNS and HTTP answers, then runs a real Prometheus against Lictor.

## Mapping

| Consul | Lictor |
|---|---|
| Datacenter | Region (`--region`, default `dc1`) |
| Node name | Node id (`--node-id`, by default the host name on first start) |
| Node address | The node's first `--advertise` address |
| Service address, port, tags, meta | The same fields of a [service definition](configuration.md#service-definitions) (`metadata` = `Meta`) |
| Check status | `passing`, `warning`, `critical`. A Lictor check with no result yet (`unknown`) is `critical`. |
| `serfHealth` node check | Present and passing for every live node (a node whose lease runs out disappears instead) |
| Namespaces (Enterprise) | Lictor namespaces: an agent answers for its own; DNS can ask for another with `.<ns>.ns` |

## DNS

```text
[tag.]<service>.service[.<datacenter>].consul      A, AAAA (and SRV)
_<service>._<tag>.service[.<datacenter>].consul    SRV, RFC 2782 style (_tcp means any tag)
<service>.service.<namespace>.ns.<datacenter>.dc.consul
<node>.node[.<datacenter>].consul                  A, AAAA
<hex-ip>.addr.<datacenter>.consul                  A, AAAA (the targets of SRV records)
consul                                             SOA, NS
```

```console
$ dig @127.0.0.1 -p 8600 +short web.service.consul
10.0.0.12
10.0.0.11
$ dig @127.0.0.1 -p 8600 +short v1.web.service.consul
10.0.0.12
$ dig @127.0.0.1 -p 8600 +short _web._prometheus.service.consul SRV
1 1 8080 0a00000b.addr.dc1.consul.
```

What is returned, as in Consul:

- **Healthy instances only:** passing and warning (`--dns-only-passing` drops warning).
  Critical instances, and instances of nodes whose lease ran out, are never returned.
- **Every instance, shuffled,** with TTL 0 by default (`--dns-service-ttl`, `--dns-node-ttl`).
- **SRV** records are `1 1 <port> <hex-ip>.addr.<dc>.consul.`, with the address in the additional
  section. A service registered with a host name instead of an IP gets that name as the target,
  and a **CNAME** for A queries.
- **UDP without EDNS:** at most 3 answers (`--dns-udp-answer-limit`), without the truncation flag.
  With EDNS, everything that fits the client's buffer; beyond that, answers are trimmed and the
  response is marked truncated so the client retries over TCP.
- **Names and tags are case-insensitive.**
- **Unknown names** (services without healthy instances, unknown datacenters, prepared queries)
  get NXDOMAIN with the SOA; a known name without records of the asked type gets an empty NOERROR.
- **Names outside the domain** get REFUSED: Lictor does not recurse. Point only the `consul`
  domain at it (below).

`--dns-domain` changes the domain (for example `lictor.internal`).

### Pointing the system resolver at it

**systemd-resolved** (most current Linux distributions), `/etc/systemd/resolved.conf.d/consul.conf`:

```ini
[Resolve]
DNS=127.0.0.1:8600
Domains=~consul
```

then `sudo systemctl restart systemd-resolved`. Only names under `consul` go to Lictor.

**dnsmasq**: `server=/consul/127.0.0.1#8600`.

**Docker containers** use the host's resolver configuration when it points at a reachable
address. For containers to reach the agent directly, listen on the Docker bridge too
(`--dns-addr 172.17.0.1:8600`) and pass `--dns 172.17.0.1` to the container, or run a local
resolver as above.

## HTTP

| Endpoint | |
|---|---|
| `GET /v1/catalog/services` | `{"web": ["v1", "v2"], ...}` |
| `GET /v1/catalog/service/<name>` | Instances, flattened (`ServiceAddress`, `ServicePort`, `ServiceTags`, `NodeMeta`, ...) |
| `GET /v1/health/service/<name>` | `[{Node, Service, Checks}]`, with the node's `serfHealth` check first |
| `GET /v1/catalog/nodes`, `/v1/catalog/datacenters` | |
| `GET /v1/agent/self`, `/v1/status/leader`, `/v1/status/peers` | This agent (`Config.Datacenter`, `Config.NodeName`) |

Parameters: `dc`, `tag` (repeatable, all must match, case-insensitive), `passing`, `node-meta`
(`key:value`, repeatable), `pretty`, and blocking queries: `index` and `wait` (default `5m`, at
most `10m`). Responses carry `X-Consul-Index`, `X-Consul-KnownLeader` and `X-Consul-LastContact`,
which Consul's Go client requires. `filter` expressions are rejected with 400 rather than ignored.

The index is the agent's catalog index. It moves only when what the catalog contains changes:
registrations, health changes, nodes joining or leaving. Heartbeats don't move it, so blocking
queries don't wake up for nothing.

The API is read-only and has no token (like Consul without ACLs). It listens on loopback by
default; listening elsewhere logs a warning, because it lists every service and node.

## Prometheus

A `consul_sd_configs` job works unchanged: point `server` at a Lictor agent.

```yaml
scrape_configs:
  - job_name: 'nomad-services'
    consul_sd_configs:
      - server: 'host.docker.internal:8500'   # a Lictor agent's --consul-http-addr
        datacenter: 'dc1'                     # a Lictor region
        tags: [prometheus]
    relabel_configs:
      - source_labels: [__meta_consul_service_metadata_shard]
        target_label: shard
      - source_labels: [__meta_consul_service]
        target_label: job
      - source_labels: [__meta_consul_node]
        target_label: instance
```

Register the services to scrape with the tag and metadata the relabel rules read:

```sh
lictor services register --name matcher-metrics --port 9102 \
    --tag prometheus --meta shard=2 --meta client=acme --meta match_set=ads
```

or in a `--config-dir` file:

```toml
name = "matcher-metrics"
port = 9102
tags = ["prometheus", "shard-2"]
metadata = { shard = "2", client = "acme", match_set = "ads" }
```

Every `__meta_consul_*` label Prometheus documents is filled in: `__meta_consul_service`,
`__meta_consul_service_id`, `__meta_consul_service_address`, `__meta_consul_service_port`,
`__meta_consul_service_metadata_<key>`, `__meta_consul_tags`, `__meta_consul_node`,
`__meta_consul_address`, `__meta_consul_dc`, `__meta_consul_health`, `__meta_consul_metadata_<key>`
(node metadata) and `__meta_consul_tagged_address_<name>`.

**Prometheus in Docker** reaching an agent on the host through `host.docker.internal` needs the
agent to listen where the container can connect, which loopback is not:
`--consul-http-addr 0.0.0.0:8500`, or the bridge address (`172.17.0.1:8500`). One agent can serve
the whole catalog; any agent will do.

## Running next to Consul

During a migration, Consul may still hold ports 8500 and 8600. The agent then refuses to start and
says which option to change. Move Lictor's listeners (`--consul-http-addr 127.0.0.1:8501
--dns-addr 127.0.0.1:8601`) or turn them off (`disabled`) until Consul is gone.

## Differences from Consul

- **No registration, KV, sessions, events, ACLs or Connect** over the Consul API. Register with
  Lictor's CLI, config files, native API or UI.
- **No prepared queries** (`.query.consul`) and **no `filter` expressions**.
- **No TXT records** with node metadata in DNS additional sections.
- **SRV targets** are always `<hex-ip>.addr` names for IP addresses. Consul uses the node's name
  when a service was registered without an address; Lictor always knows the address (it defaults
  to the node's), so both resolve to the same place.
- **Unknown health** (no result yet) counts as critical; Consul has no such state.
- **Indexes** are per agent and restart from 1 when the agent restarts. Clients handle this the way
  they handle a Consul server change: the next blocking query returns at once.
