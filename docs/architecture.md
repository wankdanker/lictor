# Architecture

Lictor separates three things:

1. **Distributed infrastructure**: persistence, replication, change distribution. Delegated to a
   [`Store`](backends.md), normally NATS JetStream KV.
2. **Registry semantics**: membership, ownership, health, eligibility. Implemented once, in `lictor-core`.
3. **Protocols**: the CLI, the native HTTP API and web UI, and Consul-compatible DNS and HTTP;
   native DNS and NATS request/reply adapters next. Adapters depend only on the `Registry` trait.

## Crates

| Crate | Role |
|---|---|
| `lictor-types` | The data model: stored records, queries, views, validation. No I/O. |
| `lictor-store` | The `Store`/`Bucket` traits, the in-memory store, and the backend conformance suite. |
| `lictor-store-nats` | The NATS JetStream KV store (plus a `testing` helper that runs `nats-server`). |
| `lictor-core` | `Catalog`, `Agent`, `Observer`, health engine, and the `Registry` trait. |
| `lictor-consul` | Consul-compatible DNS and HTTP (catalog, health) adapters, answering from the `Registry`. |
| `lictor` | The binary: CLI (CLYOPS), agent runner, native HTTP API server and client, embedded web UI. |

The web UI (`ui/`, SvelteKit) is a single-page app built to static files and embedded into the
`lictor` binary at compile time (`crates/lictor/build.rs`). It is a client of the native HTTP API
like the CLI, with no access of its own. See [ui.md](ui.md).

## Data model

Three buckets, all with `history = 1`:

| Bucket | Key | TTL | Written |
|---|---|---|---|
| `LICTOR_NODES` | `namespace.region.node` | membership TTL (30s) | every heartbeat (10s) |
| `LICTOR_SERVICES` | `namespace.region.service.node.id` | none | on registration only |
| `LICTOR_HEALTH` | same as the service | health TTL (30s) | on change, and every TTL/3 |

Records are JSON with a `schema_version`. Readers ignore records with a newer version, so
mixed-version clusters degrade safely. Examples are in [spec.md](spec.md#6-jetstream-storage-architecture).

An agent hosting hundreds of services writes one heartbeat per interval. Registrations are not rewritten.

## Generations and fencing

Each agent has a persistent **node id** (kept in `<data-dir>/node-id`) and picks a random
**generation** at every start. Its membership record, registrations and health records carry that generation.

- **Heartbeat** is a compare-and-set on the membership record's revision. If it fails, the agent
  reads the record: if another generation wrote it, a newer agent took over this node id, and
  this one stops with exit status 4 (*fenced*). The systemd unit does not restart on status 4.
- **Startup** overwrites the membership record unconditionally (taking over), then
  **reconciles** registrations left by the previous generation: `adopt` (default; rewrite
  them under the new generation and resume their checks), `drop` (delete them) or `keep`
  (leave them inactive until registered again).
- A registration whose generation differs from its node's current generation is **not discoverable**.

## Eligibility

A service instance is returned by discovery when:

1. its registration exists,
2. its node's membership lease has not expired, and
3. the registration's generation matches the node's current generation.

Its health is the worst state of its checks (`passing < warning < unknown < critical`), read
from a health record that is unexpired and of the same generation. Checks without a current
health record are `unknown`. A service with no checks is `passing` while its node is alive.

Queries filter by health: `any` (default), `passing` (`--healthy`) or `not-critical`.

## The catalog

Every participant (agent or `--direct` observer) keeps an in-memory **catalog** of all three
buckets, fed by a watch on each. Queries read only the catalog: no network, no disk.

The catalog is built to tolerate everything a watch can do:

| Situation | Handling |
|---|---|
| Duplicate or replayed events | Each key's last revision is kept; older or equal revisions are ignored. |
| Buckets updated in any order | No cross-bucket atomicity is assumed; inconsistent records are simply ineligible. |
| Watch lost (NATS down) | The catalog keeps serving. Leases keep expiring locally. The watch restarts with backoff. |
| Deletes missed while disconnected | A re-watch redelivers current state, then *Ready*; anything not redelivered is swept. |
| Expiry without delete events | JetStream does not announce `max_age` expiry, so the catalog enforces deadlines itself. |
| Replayed old heartbeats | Leases are computed from the server timestamp, so an old heartbeat cannot revive a node. |

### Leases

On receipt of a membership or health record, its deadline on the local monotonic clock is:

```
age      = max(0, local_now - server_timestamp - clock_skew)
deadline = monotonic_now + ttl - age
```

Wall-clock jumps after receipt do not matter, and up to `--clock-skew` (2s) of difference
between this host and the NATS servers is tolerated. The TTL used is the bucket's actual TTL,
so every participant agrees on lease lengths whatever its own flags say.

## Writes

Writes go to the store first and return only after it confirms persistence (a JetStream ack).
The agent then applies its own write to its catalog immediately, so `register` followed by
`discover` on the same agent is consistent. Other agents see it when the watch delivers it,
typically within milliseconds.

## Health engine

- TCP checks connect; HTTP checks expect 2xx (429 is `warning`); TTL checks are critical unless
  the application reports in time (`lictor checks pass <id>`, or `PUT /v1/agent/checks/<id>/pass`).
- `failures_before_critical` / `successes_before_passing` debounce flapping.
- Checks run concurrently, at most `--max-concurrent-checks` at a time.
- State changes are published immediately; one publisher task writes them in order, always
  the latest state. Unchanged health is refreshed every health TTL / 3.
- `deregister_critical_after` removes a service whose check stays critical too long.

## Cleanup

Registrations are persistent. If a node disappears for good, its registrations stay in
`LICTOR_SERVICES` (ineligible). Every agent deletes registrations whose node has been gone
longer than `--stale-retention` (72h; `0` disables), using a conditional delete so a node
that comes back and re-registers is never affected.

## Consistency model

Lictor provides:

- eventually consistent catalog reads,
- acknowledged, persistent writes,
- conditional single-key updates,
- idempotent registration and deregistration,
- generation-based ownership fencing,
- conservative handling of incomplete state (inconsistent records are not returned).

It does not provide linearizable discovery, multi-key transactions or exactly-once delivery.

## Failure handling

| Failure | Behavior |
|---|---|
| Service crashes | Its checks turn critical; healthy queries stop returning it. |
| Agent crashes / VM dies | Membership expires after the TTL; its services become ineligible everywhere. |
| Agent stops cleanly | It deletes its membership (`--leave-on-shutdown`), so its services disappear at once. |
| NATS connection lost | Agents keep answering from the catalog; unexpired records stay; writes fail. |
| NATS back | Watches resynchronize; heartbeats resume (re-creating an expired lease). |
| Duplicate node id | The newer agent wins; the older one is fenced and exits with status 4. |
| Agent restarts | New generation; previous registrations are adopted (by default). |
| Node gone for good | Its registrations are deleted after the retention period. |

## Security

- NATS authentication and TLS: credentials files, NKeys, user/password, tokens, CA and client certificates.
- The HTTP API listens on loopback by default. Exposing it elsewhere logs a warning unless
  `--token` is set, in which case every request must carry `Authorization: Bearer <token>`.
- Script checks do not exist (yet), so the agent never runs commands on behalf of API callers.
- Still to do (see the roadmap): NATS subject permission templates that limit each agent to
  writing its own keys, and namespace-scoped read permissions.
