# **Technical specification: NATS-backed distributed service registry**

Draft v0.1 · Proposed architecture · Rust / NATS JetStream

> **About this document.** This is the original design specification Lictor is built from.
> It is kept as written; where the implementation made a concrete choice, it is listed here.
>
> | Spec | Lictor |
> |---|---|
> | Buckets `REGISTRY_NODES`, `REGISTRY_SERVICES`, `REGISTRY_HEALTH` | `LICTOR_NODES`, `LICTOR_SERVICES`, `LICTOR_HEALTH` (prefix set by `--bucket-prefix`) |
> | 3 replicas by default | 1 by default so a single `nats-server` works out of the box; set `--replicas 3` in a cluster |
> | Service record lists check *names* (`health_checks`) | It stores full check definitions (`checks`), so a restarted agent can resume them |
> | `registry-*` crates (11) | 5 crates: `lictor-types`, `lictor-store`, `lictor-store-nats`, `lictor-core`, `lictor` |
> | NATS-specific core | The core talks to a `Store` trait; NATS JetStream KV is one implementation, an in-memory store another (see [backends.md](backends.md)) |
> | TOML agent config | Agent settings use flags / `LICTOR_*` env vars / a `key = value` config file (CLYOPS); service definitions use TOML or JSON files in `--config-dir` |
> | CLI named `registry` | `lictor` (same subcommands: `nodes list`, `services list`, `services inspect`, `discover --healthy`, `health status`, `status`) |
> | Default datacenter | Namespace `default`, region `dc1` (the region is Consul's datacenter) |
> | Health states | Ordered `passing < warning < unknown < critical`; an instance's health is its worst check |
>
> Status: sections 2-8, 12-13 and the CLI of section 15 are implemented (phase 0 and 1 of [roadmap.md](roadmap.md)).
> The adapters of section 9, metrics (15) and characterization against Consul (16) are next.


## **1. Overview**

Develop a lightweight, open-source, distributed service registration and discovery system built on NATS JetStream.

The system shall provide a protocol-independent registry core responsible for service registration, node membership, health monitoring, state synchronization, and service discovery.

Compatibility with existing service discovery systems shall be implemented through independently enabled adapters, rather than being intrinsic to the registry core.

The initial compatibility target is a subset of Consul, allowing existing infrastructure and applications to migrate without significant modification.

**The system is not intended to reimplement Consul. It is intended to provide a general-purpose service registry that can speak Consul's protocols.**

## **2. Design principles**

- **NATS-native:** Delegate distributed state management, persistence, replication, and communication to NATS JetStream.
- **Decentralized agents:** No additional centralized registry server, leader election, or agent-to-agent gossip protocol.
- **Pluggable compatibility:** External protocols are adapters over a common registry interface.
- **Local discovery:** Serve discovery queries from an in-memory catalog without requiring network round trips.
- **Minimal connectivity requirements:** Agents initiate outbound connections to NATS; direct agent-to-agent connectivity is unnecessary for registry operations.
- **Lightweight runtime:** Implement the agent in asynchronous Rust.
- **Behavioral compatibility:** Establish compatibility through characterization tests against existing reference implementations.

## **3. System architecture**

All protocol adapters interact with the registry interface. Only the core manages registry state through NATS.

Each agent is a self-contained runtime that may run on a virtual machine, bare-metal server, container, or Kubernetes pod.

## **4. Deployment topology**

Agents may exist across different networks, cloud providers, and geographic locations without requiring connectivity to one another for registration and discovery.

Actual application traffic between discovered services must still have appropriate network connectivity.

## **5. Core registry interface**

The core shall expose a typed asynchronous Rust interface. Compatibility adapters shall depend on this interface rather than directly manipulating JetStream buckets.

Illustrative interface:

```
#[async_trait]
pub trait Registry: Send + Sync {
    async fn register(
        &self,
        service: ServiceRegistration,
    ) -> Result<RegistrationReceipt>;

    async fn deregister(
        &self,
        id: &ServiceId,
    ) -> Result<()>;

    async fn discover(
        &self,
        query: DiscoveryQuery,
    ) -> Result<Vec<ServiceInstance>>;

    async fn get_service(
        &self,
        id: &ServiceId,
    ) -> Result<Option<ServiceInstance>>;

    async fn list_services(
        &self,
        scope: RegistryScope,
    ) -> Result<Vec<ServiceSummary>>;

    async fn list_nodes(
        &self,
        scope: RegistryScope,
    ) -> Result<Vec<Node>>;

    async fn register_check(
        &self,
        check: HealthCheck,
    ) -> Result<()>;

    async fn update_health(
        &self,
        update: HealthUpdate,
    ) -> Result<()>;

    async fn watch(
        &self,
        query: DiscoveryQuery,
    ) -> Result<RegistryEventStream>;
}
```

The core shall not contain Consul-specific request structures, DNS record types, HTTP response codes, or other protocol-specific representations.

## **6. JetStream storage architecture**

JetStream KV provides persistent values, revisions, change subscriptions, expiration, and conditional updates. These mechanisms form the foundation of the registry.





### **6.1 Bucket configuration**

| Bucket              | Purpose             | Expiration | Replicas |
| ------------------- | ------------------- | ---------- | -------- |
| `REGISTRY_NODES`    | Node membership     | 30s        | 3        |
| `REGISTRY_SERVICES` | Service definitions | None       | 3        |
| `REGISTRY_HEALTH`   | Health status       | 30s        | 3        |

These are proposed configurable defaults, not fixed requirements.

Use file-backed storage initially. Maintain only the latest value per key unless historical retention is explicitly enabled.

### **6.2 Node membership**

**Bucket:** `REGISTRY_NODES`

**Key:** `production.us-east.node123`

```
{
  "schema_version": 1,
  "node_id": "node123",
  "generation": "a8f9c201",
  "namespace": "production",
  "region": "us-east",
  "hostname": "matcher-01",
  "addresses": ["10.10.1.25"],
  "metadata": {
    "provider": "aws",
    "architecture": "amd64"
  }
}
```

Each agent shall possess a persistent node identifier and acquire a new generation identifier upon startup.

The agent shall refresh its membership every 10 seconds by default. A node whose membership expires becomes unavailable for discovery.

Conditional updates shall fence older agent generations from overwriting the membership of a newer generation.

### **6.3 Service registrations**

**Bucket:** `REGISTRY_SERVICES`

**Key:** `production.us-east.matcher.node123.instance01`

```
{
  "schema_version": 1,
  "service_id": "instance01",
  "service_name": "matcher",
  "node_id": "node123",
  "generation": "a8f9c201",
  "namespace": "production",
  "region": "us-east",
  "address": "10.10.1.25",
  "port": 8080,
  "protocol": "tcp",
  "tags": ["v2", "audio"],
  "metadata": {
    "shard": "4",
    "version": "2.1.0"
  },
  "health_checks": ["tcp-main"]
}
```

Service registrations shall be persistent declarations that do not require continuous rewriting.

An agent hosting hundreds of services can therefore maintain node membership using a single heartbeat.

A restarted agent must explicitly reconcile its previous service registrations before activating them under its new generation.

Expired or abandoned registrations shall be garbage-collected according to a configurable retention policy, using conditional deletion to avoid races with newly registered instances.

### **6.4 Health status**

**Bucket:** `REGISTRY_HEALTH`

**Key:** `production.us-east.matcher.node123.instance01`

```
{
  "schema_version": 1,
  "node_id": "node123",
  "generation": "a8f9c201",
  "service_id": "instance01",
  "status": "passing",
  "checks": {
    "tcp-main": {
      "status": "passing",
      "duration_ms": 2
    }
  }
}
```

Health status is independent of service registration.

Supported native health states:

| State    | Meaning                         |
| -------- | ------------------------------- |
| Passing  | Service is healthy              |
| Warning  | Service is degraded             |
| Critical | Service is unhealthy            |
| Unknown  | Health has not been established |

The health engine publishes state changes immediately and periodically refreshes unchanged status to maintain its expiration deadline.

### **6.5 Discovery eligibility**

A service is eligible for healthy discovery when all the following conditions are satisfied.

Service registration exists

Node membership is unexpired

Service and node generations match

Required health checks are current

Health status satisfies query policy

Return service instance

Protocol adapters may apply different health filtering policies to preserve compatibility with their reference implementations.

## **7. Local catalog and synchronization**

Every agent shall maintain an in-memory materialized view of the relevant JetStream buckets.

The catalog shall be indexed by service name, node identifier, namespace, region, tags, metadata, and health status.

### **Initialization**

1. Connect to NATS.
2. Establish KV watchers that deliver initial values and subsequent changes.
3. Populate the local catalog.
4. Validate membership, generations, and health expiration.
5. Reconcile locally owned registrations.
6. Mark the agent ready to serve requests.

### **Runtime synchronization**

Updates shall be applied incrementally as they arrive from JetStream.

The synchronization engine must account for duplicate events, delayed events, reconnections, expired records, and independent ordering across buckets.

There is no assumption of atomicity across buckets. Temporarily inconsistent records shall remain ineligible until their dependencies are valid.

### **Discovery query path**

Standard discovery queries shall not perform synchronous network or disk operations.

This implies eventual consistency for ordinary discovery reads. Successful registration writes shall be acknowledged only after JetStream confirms persistence.

## **8. Health monitoring engine**

The core shall implement a general-purpose health monitoring subsystem.

Initially supported check types:

| Check        | Description                          | Priority |
| ------------ | ------------------------------------ | -------- |
| TCP          | Verify connection establishment      | MVP      |
| HTTP / HTTPS | Validate response status             | MVP      |
| TTL          | Receive application heartbeat        | MVP      |
| gRPC         | Standard gRPC health protocol        | Future   |
| External     | Receive health from external systems | Future   |
| Script       | Execute a local health command       | Optional |

Health check configuration shall include interval, timeout, failure threshold, recovery threshold, initial state, and optional deregistration policy.

Checks shall execute asynchronously with configurable concurrency limits. Executable script checks shall be disabled by default.

## **9. Pluggable protocol architecture**

Adapters shall be independently enabled and configured.

They shall translate external requests into core registry operations and convert core responses into protocol-specific representations.

```
#[async_trait]
pub trait RegistryAdapter: Send + Sync {
    fn name(&self) -> &'static str;

    async fn start(
        &self,
        registry: Arc<dyn Registry>,
        shutdown: CancellationToken,
    ) -> Result<()>;
}
```

Initially, adapters should be compiled-in Rust crates enabled through runtime configuration. Dynamically loaded shared libraries or WASM plugins are not required.

This avoids introducing ABI stability, dynamic loading, and plugin isolation complexity prematurely.

### **9.1 Consul HTTP adapter**

The initial implementation shall support a selected subset of the Consul HTTP API.

Consul's agent endpoints distinguish local registration from global catalog and health queries. The adapter must preserve this distinction.





| Endpoint                               | Function               |
| -------------------------------------- | ---------------------- |
| `PUT /v1/agent/service/register`       | Register service       |
| `PUT /v1/agent/service/deregister/:id` | Remove service         |
| `GET /v1/agent/services`               | List local services    |
| `GET /v1/agent/checks`                 | List local checks      |
| `PUT /v1/agent/check/register`         | Register check         |
| `PUT /v1/agent/check/pass/:id`         | Update TTL check       |
| `PUT /v1/agent/check/fail/:id`         | Update TTL check       |
| `GET /v1/catalog/services`             | List catalog services  |
| `GET /v1/catalog/service/:name`        | List service instances |
| `GET /v1/catalog/nodes`                | List nodes             |
| `GET /v1/health/service/:name`         | Query service health   |

The exact compatibility surface shall be established by auditing existing infrastructure usage.

### **9.2 Consul DNS adapter**

Support Consul-style service and node resolution.

```
matcher.service.consul
v2.matcher.service.consul
matcher.service.us-east.consul
matcher-01.node.consul
```

Supported DNS record types shall initially include A, AAAA, and SRV, with both UDP and TCP DNS transport.

The adapter shall reproduce reference behavior for record filtering, TTLs, datacenter selection, response ordering, negative responses, and truncation.

### **9.3 Native DNS adapter**

A protocol-independent DNS interface with configurable naming conventions.

```
matcher.svc.production.internal
matcher.us-east.svc.internal
matcher-01.node.internal
```

The native DNS interface shall not inherit Consul-specific naming restrictions or assumptions unless explicitly configured.

### **9.4 Native NATS adapter**

Applications with existing NATS connectivity may interact directly with the registry through a request/reply interface.

Illustrative subjects:

```
registry.v1.register
registry.v1.deregister
registry.v1.discover
registry.v1.health.update
registry.v1.events.>
```

Requests must have a defined ownership and routing mechanism so that registration is processed by the responsible agent rather than an arbitrary subscriber.

This would allow applications to register and discover services without requiring a locally installed agent, provided their membership and health lifecycle is explicitly managed.

### **9.5 Future compatibility adapters**

| Interface          | Potential application                             |
| ------------------ | ------------------------------------------------- |
| etcd               | Selected KV-based discovery clients               |
| Eureka             | Spring Cloud applications                         |
| Kubernetes         | Import or synchronize Services and EndpointSlices |
| Envoy xDS          | Dynamic proxy configuration                       |
| Prometheus HTTP SD | Dynamic monitoring targets                        |
| ZooKeeper          | Selected service discovery conventions            |

Full etcd compatibility would be substantially more complex than a Consul discovery adapter. etcd exposes specific transaction, revision, lease, and watch consistency guarantees that do not map directly to JetStream KV.





An etcd adapter should therefore target particular service discovery clients, rather than promise to be a general-purpose replacement for etcd.

## **10. Rust workspace organization**

```
service-registry/
├── Cargo.toml
├── crates/
│   ├── registry-types/
│   ├── registry-core/
│   ├── registry-nats/
│   ├── registry-catalog/
│   ├── registry-health/
│   ├── registry-agent/
│   ├── registry-cli/
│   ├── adapter-consul-http/
│   ├── adapter-consul-dns/
│   ├── adapter-native-dns/
│   └── adapter-native-nats/
├── tests/
│   ├── characterization/
│   ├── compatibility/
│   ├── integration/
│   ├── failure/
│   └── benchmarks/
├── deploy/
│   ├── docker/
│   ├── systemd/
│   └── kubernetes/
└── docs/
```

Candidate Rust dependencies:

| Library          | Purpose            |
| ---------------- | ------------------ |
| `tokio`          | Async runtime      |
| `async-nats`     | NATS and JetStream |
| `serde`          | Serialization      |
| `axum`           | HTTP adapters      |
| `hickory-server` | DNS server         |
| `tracing`        | Structured logging |
| `prometheus`     | Metrics            |
| `clap`           | CLI                |

Dependency versions and compatibility shall be validated during implementation.

## **11. Agent configuration**

Example configuration using TOML:

```
[node]
id_file = "/var/lib/registry/node-id"
namespace = "production"
region = "us-east"
advertise_address = "10.10.1.25"

[nats]
servers = [
    "tls://nats-01.example.net:4222",
    "tls://nats-02.example.net:4222",
    "tls://nats-03.example.net:4222"
]
credentials = "/etc/registry/nats.creds"

[registry]
bucket_prefix = "REGISTRY"
replicas = 3
heartbeat_interval = "10s"
membership_ttl = "30s"
health_ttl = "30s"
stale_policy = "expire"

[health]
max_concurrent_checks = 128
default_interval = "10s"
default_timeout = "3s"

[adapters.consul_http]
enabled = true
listen = "127.0.0.1:8500"

[adapters.consul_dns]
enabled = true
listen = "127.0.0.1:8600"
domain = "consul"

[adapters.native_dns]
enabled = false
domain = "internal"

[adapters.native_nats]
enabled = false

[metrics]
enabled = true
listen = "127.0.0.1:9090"
```

## **12. Failure handling**

The agent shall distinguish between local availability, NATS connectivity, and catalog freshness.

| Failure                    | Expected behavior                                        |
| -------------------------- | -------------------------------------------------------- |
| Service crashes            | Health check marks service unhealthy                     |
| Agent crashes              | Membership expires; services become ineligible           |
| VM crashes                 | Membership expires                                       |
| NATS connection lost       | Continue serving unexpired cached records                |
| JetStream quorum lost      | Reject unconfirmed writes; serve eligible cached records |
| Agent reconnects           | Reconcile catalog and restart synchronization            |
| Duplicate node ID          | Fence the previous generation                            |
| Missed expiration event    | Local expiration removes discovery eligibility           |
| Stale service registration | Remains inactive until explicitly reconciled             |

### **Example node failure**

Illustrative membership lease timing

Membership TTL (seconds)

15 seconds

30 seconds

60 seconds

Heartbeat interval

5 seconds

10 seconds

15 seconds

Nominal failure detection

# **20–30s**

Heartbeats per node / minute

# **6**

Assumes regular successful heartbeats before failure. Health checks may detect failure sooner. DNS caches and synchronization delays may extend client-observed removal time.

Agents must enforce expiration independently of receiving JetStream deletion events.

Lease calculation should account for authoritative message timestamps, bounded clock skew, and monotonic elapsed time. Replayed heartbeat events must not restart expired leases.

## **13. Consistency model**

The system shall explicitly provide:

- Eventually consistent catalog reads.
- Acknowledged persistent writes.
- Conditional single-key updates.
- Idempotent registration and deregistration.
- Generation-based ownership fencing.
- Conservative handling of incomplete catalog state.

The system shall not claim globally linearizable discovery, atomic multi-bucket transactions, or exactly-once event delivery.

Adapters requiring stronger semantics must either implement and verify them explicitly or reject unsupported requests.

## **14. Security model**

NATS authentication and authorization shall form the foundation of inter-agent security.

The agent shall support TLS, NATS credentials, NKeys, namespace isolation, and appropriately scoped subject permissions.

Agents shall be authorized to read permitted catalog scopes but only modify records they own.

KV key naming and NATS subject permissions must be designed together to enforce this ownership. Client-supplied node identifiers alone must not establish authorization.

Additional requirements include:

- Local HTTP and DNS listeners bound to loopback by default.
- Authentication for remotely exposed mutation APIs.
- Namespace-based discovery authorization.
- Restricted health check destinations where appropriate.
- Explicit administrative permissions for bucket creation and configuration.
- No sensitive credentials stored in service metadata.

NATS bootstrap addresses must be resolvable independently of the registry.

## **15. Observability**

Expose Prometheus-compatible metrics covering registry operations, local catalog state, synchronization, health monitoring, and protocol adapters.

Example metrics:

```
registry_nodes
registry_services
registry_healthy_services
registry_expired_nodes_total
registry_expired_services_total
registry_nats_connected
registry_catalog_ready
registry_catalog_sync_lag_seconds
registry_registration_duration_seconds
registry_discovery_duration_seconds
registry_health_checks_total
registry_health_check_duration_seconds
registry_dns_requests_total
registry_http_requests_total
registry_adapter_errors_total
```

The CLI should provide operational visibility independent of any compatibility layer.

```
registry nodes list
registry services list
registry services inspect matcher
registry discover matcher --healthy
registry health status
registry status
```

## **16. Characterization and compatibility testing**

Compatibility shall be established through black-box characterization testing.

A pinned version of Consul shall serve as the reference implementation for the initial adapter.

The same scenarios shall execute against both systems.

Test coverage shall include service registration, duplicate registration, deregistration, tag filtering, metadata filtering, node discovery, health transitions, DNS response formats, TTL expiration, maintenance modes, invalid input, and restart behavior.

Non-deterministic response fields such as timestamps and generated identifiers shall be normalized only when they are not part of the compatibility contract.

The core shall also have independent tests that do not depend on Consul behavior, particularly for NATS partitions, watcher recovery, ownership fencing, replayed events, and concurrent registration.

Golden fixtures and supported compatibility behavior shall be checked into the repository.

## **17. Proposed performance objectives**

These are initial engineering targets, not measured results.

Idle agent RSS

# **Under 30 MB**

Excluding large catalogs

Local DNS latency

# **Under 1 ms**

p99, warm catalog

Initial catalog capacity

# **100,000**

Service instances

Initial node capacity

# **1,000**

Connected agents

### **Scaling considerations**

The primary scaling concern is not DNS query throughput, since that is distributed across agents. It is the replication and distribution of registry updates.

**JetStream workload estimator**

Connected agents

## **1,000**

Health-monitored services

## **10,000**

Health refresh interval

5s

10s

30s

KV writes / second

# **1,100**

Full catalog event deliveries / second

# **1,100,000**

Node heartbeats

100/s

Health refreshes

1,000/s

Assumes a 10-second node heartbeat and one health update per monitored service per interval. Excludes state changes, replication traffic, retries, and protocol overhead.

At larger scales, health leases could be aggregated per node, and catalog subscriptions could be scoped by namespace or region.

These optimizations should be driven by benchmarks rather than introduced prematurely.

## **18. Implementation roadmap**

**1**

**Characterization**

Inventory current Consul usage. Establish reference HTTP and DNS tests.

**2**

**Registry core**

Implement the native data model, JetStream KV storage, ownership and membership leases.

**3**

**Discovery and health**

Implement catalog synchronization, local queries and asynchronous health checks.

**4**

**Consul adapters**

Implement the required HTTP and DNS compatibility surface.

**5**

**Reliability and deployment**

Exercise partitions, restarts, load tests, security and production observability.

**6**

**Additional protocols**

Introduce native NATS discovery and additional adapters as demand warrants.

## **19. Explicitly out of scope for MVP**

The initial implementation shall exclude service mesh traffic proxying, automatic mTLS certificate issuance, service intentions, custom consensus protocols, gossip networking, full Consul KV compatibility, distributed locks, full etcd emulation, and a graphical management interface.

These capabilities may be introduced independently if justified by actual infrastructure requirements.

## **20. Architectural decisions requiring validation**

| Decision                  | Proposed default                        |
| ------------------------- | --------------------------------------- |
| Runtime language          | Rust                                    |
| Distributed state backend | NATS JetStream KV                       |
| Replication               | Three replicas                          |
| Plugin mechanism          | Compiled-in adapters                    |
| Discovery consistency     | Eventual                                |
| Membership expiration     | 30 seconds                              |
| Health storage            | Separate KV bucket                      |
| Initial compatibility     | Consul HTTP and DNS                     |
| Network topology          | Outbound agent-to-NATS connections      |
| Stale catalog policy      | Continue serving only unexpired records |

The most important unresolved questions are the exact subset of Consul currently used by your infrastructure, the NATS server and client versions to target, and the expected registry scale. The proposed defaults and performance objectives have not yet been tested.

## **Architectural conclusion**

The primary value of this project is the separation of **distributed infrastructure, registry semantics, and external protocol compatibility**.

NATS handles distributed communication, persistence, and consensus. The Rust core implements service registry semantics. Protocol adapters provide compatibility with existing applications.

Consul compatibility becomes the first application of the registry, rather than the defining characteristic of the entire system. This keeps the project extensible while significantly reducing the amount of distributed systems infrastructure that needs to be developed and maintained.
