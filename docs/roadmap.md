# Roadmap

## Phase 0: foundation (done)

- [x] Cargo workspace, five crates, MIT license.
- [x] CI: fmt, clippy, docs, tests on Linux (NATS 2.10 to 2.15), macOS and Windows, MSRV, coverage, packaging smoke test.
- [x] Release pipeline: static Linux x86_64/arm64 (`.tar.gz`, `.deb`, `.rpm`), macOS x86_64/arm64, Windows x86_64, multi-arch container image.
- [x] systemd unit, default config, packaging scripts, Dockerfiles, compose quickstart.

## Phase 1: core registry and CLI (done)

- [x] Data model, validation, key layout ([spec §6](spec.md)).
- [x] `Store` trait with in-memory and NATS JetStream KV backends, shared conformance suite.
- [x] Catalog: materialized view, eligibility, duplicate/replay/reorder tolerance, resync sweeps, local lease expiry.
- [x] Agent: membership heartbeats, generation fencing, reconciliation (`adopt`/`drop`/`keep`), stale cleanup, graceful leave.
- [x] Health engine: TCP, HTTP(S), TTL; thresholds; concurrency limit; deregister-after-critical.
- [x] Observer: read-only access straight from NATS (`--direct`).
- [x] Native HTTP API with bearer-token auth; streaming watch.
- [x] CLI covering every operation, text and JSON output, stable exit codes, config file, env vars, shell completion.

## Web UI (done)

- [x] Management UI embedded in the binary at `/ui` (SvelteKit): overview, catalog, live instance view, nodes, register/edit/deregister services, add/remove/report checks, token prompt. See [ui.md](ui.md).
- [x] `GET /v1/agent/services/{id}/definition` and `lictor services export` for editing stored definitions.

## Phase 2: Consul compatibility (done for discovery)

`lictor-consul`, a client of `lictor_core::Registry` like the CLI. See [consul.md](consul.md).

- [x] Characterization against Consul 1.22.7 in Docker: the same services registered in both, DNS
  and HTTP answers compared (`crates/lictor/tests/consul_compat.rs`, CI job "Consul compatibility").
- [x] Consul DNS on `127.0.0.1:8600`, UDP and TCP: `[tag.]name.service[.dc].consul`, RFC 2782
  `_name._tag.service.consul`, `.ns`/`.dc` locality, `node.consul`, `addr.consul`; A, AAAA, SRV,
  CNAME for host names, SOA and NS; Consul's TTLs, shuffling, UDP answer limit, truncation and
  negative answers.
- [x] Consul HTTP reads on `127.0.0.1:8500`: catalog services, service, nodes and datacenters;
  health service (`?passing`, `?tag`, `?dc`, `?node-meta`); agent self; blocking queries on a
  catalog index that moves only on real changes.
- [x] Prometheus `consul_sd_configs` verified with a real Prometheus (targets, relabeled metadata,
  removal on deregistration).
- [x] Mapping: region = datacenter; an agent answers for its namespace; node `serfHealth` check.
- [ ] Consul registration endpoints (`/v1/agent/service/register`, check updates), if a client
  that cannot use the CLI or native API needs them.
- [ ] `filter` expressions (a subset), prepared queries, node TXT records in DNS.
- [ ] Node-level checks (checks not attached to a service).

## Phase 3: native adapters and operations

- [ ] Native DNS adapter with configurable naming (`name.svc.<ns>.internal`).
- [ ] Native NATS request/reply adapter (`registry.v1.*`) with explicit ownership routing.
- [ ] Prometheus metrics ([spec §15](spec.md)).
- [ ] NATS permission templates so each agent can write only its own keys; namespace-scoped reads.
- [ ] Failure-injection tests: network partitions between agents and NATS cluster members, clock skew, JetStream quorum loss.
- [ ] Benchmarks against the [spec §17](spec.md) targets (100k instances, 1k agents); per-node health aggregation if needed.
- [ ] Script and gRPC checks.

## Later, as needed

Prometheus HTTP SD, Kubernetes sync, Envoy xDS, Eureka, a selected etcd subset ([spec §9.5](spec.md)).
