# Lictor

**A lightweight service registry on NATS JetStream.**

In ancient Rome, *lictors* walked ahead of the consul and carried out his authority.
Lictor does the same job for your services: it registers them, checks their health and
answers "where is `web`?". It is built to speak Consul's protocols (coming soon) without being Consul.

- **One small binary.** Agent, CLI and HTTP API in a single static executable (about 9 MB, about 12 MB of RAM).
- **No servers of its own.** NATS JetStream stores, replicates and distributes the registry.
  There is no leader election, no gossip, and agents never talk to each other.
- **Local answers.** Every agent keeps an in-memory copy of the catalog, so lookups never wait on the network.
- **A web UI included.** Every agent serves a management UI at `/ui`: browse the catalog, watch
  instances live, register and edit services, report checks.
- **Health checks built in.** TCP, HTTP(S) and TTL checks, with thresholds and auto-deregistration.
- **Safe by construction.** Membership leases expire on their own; a restarted or duplicated
  agent fences the old one, so stale data never wins.
- **Swappable backend.** The core talks to a small `Store` trait. NATS is one implementation;
  an in-memory store (used by tests and `--dev`) is another.

> **Status:** early. The core registry, health checks, CLI and native HTTP API work and are tested
> against real NATS servers. Consul-compatible HTTP and DNS come next. See the [roadmap](docs/roadmap.md).

## Try it in 30 seconds

No NATS needed: `--dev` keeps everything in memory.

```console
$ lictor agent --dev &
$ lictor services register --name web --port 8080 --tag v2 --check-ttl 30s
Registered service 'web' (id web, revision 1, checks: service:web)

$ lictor checks pass service:web
Check 'service:web' is now passing

$ lictor discover web --healthy
SERVICE  ID   NODE  ADDRESS         STATUS   TAGS  SCOPE
web      web  dev   127.0.0.1:8080  passing  v2    default/dc1

$ lictor discover web --healthy -o json     # every command speaks JSON too
```

Then open <http://127.0.0.1:7171/ui> for the [web UI](docs/ui.md).

## For real: with NATS

```console
$ nats-server -js &                         # or any NATS cluster with JetStream
$ lictor agent --nats-url nats://127.0.0.1:4222
```

No `nats-server` installed? `docker compose -f deploy/docker/compose.nats.yaml up -d` runs one with
JetStream on `127.0.0.1:4222`.

Run an agent on every host. Each one registers the services of its host, runs their checks,
and can discover services anywhere:

```console
host-a$ lictor services register --name api --port 9000 --check-http http://127.0.0.1:9000/health
host-b$ lictor discover api --healthy
host-b$ lictor nodes list
host-b$ lictor watch api                    # prints the instance list every time it changes
```

No agent on the machine? Read the registry straight from NATS:

```console
$ lictor --direct --nats-url nats://nats:4222 services list
```

## Try the whole stack with Docker

```console
$ docker compose -f deploy/docker/compose.yaml up -d --build
```

This starts NATS and two agents, all on localhost:

| Port | |
|---|---|
| `7171`, `7172` | Web UI and API of `agent-a` and `agent-b`: <http://127.0.0.1:7171/ui> (token `change-me`) |
| `4222` | NATS, for `lictor --direct` or another agent you run on the host |
| `8222` | NATS monitoring |

To open the UIs from another device on your network, start it with
`LICTOR_BIND=0.0.0.0 LICTOR_TOKEN=<a real token>` (NATS stays on localhost).

`agent-a` registers the example services in [`deploy/docker/example-services`](deploy/docker/example-services).
Try `lictor --http-addr 127.0.0.1:7172 --token change-me discover nats`.

## Install

Download a release for Linux (x86_64 / arm64, static), macOS or Windows from the
[releases page](https://github.com/wankdanker/lictor/releases), or:

| | |
|---|---|
| Debian / Ubuntu | `sudo apt install ./lictor_<version>_amd64.deb` (adds a `lictor` systemd unit and user) |
| RHEL / Fedora | `sudo dnf install ./lictor-<version>.x86_64.rpm` |
| Container | `docker run ghcr.io/wankdanker/lictor agent --nats-url nats://nats:4222` |
| From source | `cargo install --git https://github.com/wankdanker/lictor lictor` |

After installing the package, edit `/etc/lictor/lictor.conf` and run `sudo systemctl enable --now lictor`.

## Defining services in files

Put service definitions in a directory and point the agent at it with `--config-dir`:

```toml
# /etc/lictor/conf.d/web.toml
name = "web"
port = 8080
tags = ["v2"]
metadata = { version = "2.1.0" }

[[checks]]
type = "http"
url = "http://127.0.0.1:8080/health"
interval = "10s"
timeout = "2s"
deregister_critical_after = "10m"
```

The same file works with `lictor services register --file web.toml`.

## How it works

```
      host A                               host B
 ┌──────────────────┐               ┌──────────────────┐
 │ app ──► lictor   │               │ lictor ◄── app   │
 │        agent     │               │ agent            │
 │   catalog (RAM)  │               │   catalog (RAM)  │
 └────────┬─────────┘               └─────────┬────────┘
          │      outbound connections only    │
          ▼                                   ▼
     ┌─────────────────────────────────────────────┐
     │  NATS JetStream KV                          │
     │  LICTOR_NODES     membership leases (TTL)   │
     │  LICTOR_SERVICES  registrations             │
     │  LICTOR_HEALTH    health (TTL)              │
     └─────────────────────────────────────────────┘
```

An instance is discoverable only while its node's lease is alive and its registration belongs
to that node's current *generation*. Read [architecture.md](docs/architecture.md) for the full story.

## Documentation

| | |
|---|---|
| [CLI reference](docs/cli.md) | Every command, flag and exit code |
| [Configuration](docs/configuration.md) | Flags, `LICTOR_*` variables, the config file, service definitions |
| [HTTP API](docs/http-api.md) | The agent's native `/v1` API |
| [Web UI](docs/ui.md) | The management UI every agent serves at `/ui` |
| [Architecture](docs/architecture.md) | Data model, leases, generations, consistency, failure handling |
| [Storage backends](docs/backends.md) | The `Store` trait and how to replace NATS |
| [Development](docs/development.md) | Building, testing, CI and releasing |
| [Roadmap](docs/roadmap.md) | What is done and what is next |
| [Specification](docs/spec.md) | The original design document |

## License

MIT. See [LICENSE](LICENSE).
