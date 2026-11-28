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

## Phase 2: Consul compatibility (next)

Each adapter is its own crate depending only on `lictor_core::Registry`, enabled by a flag.

**Characterization first.** A test harness that runs the same scenario against a pinned Consul
(in Docker) and against Lictor, normalizes non-contractual fields (timestamps, generated
indexes), and compares the responses. Golden fixtures are checked in.

- [ ] `adapter-consul-http` on `127.0.0.1:8500`:
  `PUT /v1/agent/service/register`, `PUT /v1/agent/service/deregister/:id`, `GET /v1/agent/services`,
  `GET /v1/agent/checks`, `PUT /v1/agent/check/register`, `PUT /v1/agent/check/{pass,warn,fail}/:id`,
  `GET /v1/catalog/services`, `GET /v1/catalog/service/:name`, `GET /v1/catalog/nodes`,
  `GET /v1/health/service/:name` (with `?passing`, `?tag`, `?dc`, `?filter` subset).
  Blocking queries (`?index=`, `X-Consul-Index`) on top of the catalog index and `watch`.
- [ ] `adapter-consul-dns` on `127.0.0.1:8600` (UDP and TCP, `hickory-server`):
  `name.service[.dc].consul`, `tag.name.service.consul`, RFC 2782 `_name._tag.service.consul`,
  `node.consul`; A, AAAA, SRV; Consul's TTLs, ordering (shuffling), negative answers and truncation.
- [ ] Mapping decisions to confirm against your Consul usage: region = datacenter; namespace not exposed
  (Consul OSS has none) or mapped to Enterprise namespaces; node checks (Consul `serfHealth`).
- [ ] Node-level checks (checks not attached to a service) for Consul parity.

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
