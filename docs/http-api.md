# Native HTTP API

Every agent serves a small JSON API, by default on `127.0.0.1:7171`. The CLI and the
[web UI](ui.md) use it; so can your applications. (The Consul-compatible API is a separate adapter, on the [roadmap](roadmap.md).)

With `--token`, every request needs `Authorization: Bearer <token>` (or `X-Lictor-Token: <token>`).

## Errors

Non-2xx responses carry:

```json
{ "error": { "code": "not_found", "message": "service 'web' not found" } }
```

| Code | HTTP | Meaning |
|---|---|---|
| `invalid` | 400 | Bad input, including a body that is not JSON or has an unknown field. |
| `unauthorized` | 401 | Missing or wrong token. |
| `not_found` | 404 | No such service, check or endpoint. |
| `conflict` | 409 | A conflicting write won. |
| `unsupported` | 501 | Not available here. |
| `unavailable` | 503 | The store (NATS) cannot be reached; reads still work. |
| `fenced`, `not_ready` | 503 | The agent was taken over, or has not loaded the catalog. |

## Catalog (any node)

### `GET /v1/discover/{name}`

Live instances of a service. Query parameters, all optional and combinable:

| Parameter | Example | Meaning |
|---|---|---|
| `health` | `passing`, `not-critical`, `any` | Health filter (`?passing` alone also works). |
| `tag` | `tag=v2&tag=primary` | Required tags. |
| `meta` | `meta=version:2.1.0` | Required metadata (`key:value` or `key=value`). |
| `node` | `node=host-a` | Only on this node. |
| `namespace`, `region` | `region=us-east` | Scope. |

```json
[
  {
    "namespace": "default",
    "region": "dc1",
    "service_id": "web",
    "service_name": "web",
    "address": "10.0.1.5",
    "port": 8080,
    "protocol": "tcp",
    "tags": ["v2"],
    "metadata": { "shard": "4" },
    "status": "passing",
    "checks": [
      { "id": "service:web", "service_id": "web", "name": "Service 'web' check", "type": "ttl", "status": "passing", "output": "ok" }
    ],
    "node_id": "host-a",
    "node_hostname": "host-a.example",
    "node_addresses": ["10.0.1.5"],
    "node_metadata": { "arch": "x86_64", "os": "linux" },
    "generation": "6d3254fcb410c472",
    "revision": 41
  }
]
```

Instances are sorted by namespace, region, node and service id.

### `GET /v1/watch/{name}`

Same parameters as discover. Streams `application/x-ndjson`: one line with the current result,
then one line per change, until the client disconnects.

```json
{"index":57,"instances":[ ... ]}
```

### `GET /v1/services?namespace=&region=`

```json
[{ "name": "web", "instances": 2, "passing": 1, "warning": 0, "critical": 1, "unknown": 0, "tags": ["v1", "v2"] }]
```

### `GET /v1/nodes?namespace=&region=`

```json
[{ "namespace": "default", "region": "dc1", "node_id": "host-a", "generation": "6d32...", "hostname": "host-a",
   "addresses": ["10.0.1.5"], "metadata": {}, "agent_version": "0.1.0", "services": 3, "lease_remaining_ms": 24890 }]
```

### `GET /v1/status`

```json
{ "version": "0.1.0", "backend": "nats", "connected": true, "catalog_ready": true,
  "node": { ...node... }, "local_services": 3, "local_checks": 2,
  "catalog_nodes": 12, "catalog_services": 40, "catalog_index": 1234 }
```

## This agent's services

### `POST /v1/agent/services`

Register (or replace) a service. The body is a [service definition](configuration.md#service-definitions) in JSON.
Returns `201`:

```json
{ "service_id": "web", "service_name": "web", "key": "default.dc1.web.host-a.web",
  "revision": 41, "generation": "6d32...", "checks": ["service:web"] }
```

```sh
curl -X POST localhost:7171/v1/agent/services -d '{"name":"web","port":8080,"checks":[{"type":"ttl","ttl":"30s"}]}'
```

### `GET /v1/agent/services`, `GET /v1/agent/services/{id}`

This agent's services (whether or not currently discoverable), in the instance format above.

### `GET /v1/agent/services/{id}/definition`

The stored definition of one of this agent's services, with every default filled in (id, address,
protocol, check ids and names). It is the format `POST /v1/agent/services` accepts, so it can be
edited and registered again; `lictor services export ID` prints it.

```json
{ "id": "web", "name": "web", "address": "10.0.1.5", "port": 8080, "protocol": "tcp", "tags": ["v2"],
  "checks": [{ "id": "service:web", "name": "Service 'web' check", "type": "ttl", "ttl": "30s",
               "initial_status": "critical" }] }
```

### `DELETE /v1/agent/services/{id}`

Deregister. `204` on success.

## This agent's checks

### `GET /v1/agent/checks`

```json
[{ "id": "service:web", "service_id": "web", "name": "...", "type": "ttl", "status": "passing", "output": "ok" }]
```

### `POST /v1/agent/checks`

Add a check to a local service. Body: a check definition plus `service_id`:

```json
{ "service_id": "web", "type": "http", "url": "http://127.0.0.1:8080/health", "interval": "5s" }
```

Returns `201` `{"check_id": "service:web"}`.

### `DELETE /v1/agent/checks/{id}`

Remove a check. `204`.

### `PUT /v1/agent/checks/{id}/pass`, `/warn`, `/fail`

Report a TTL check. Optional body `{"output": "text to record"}`. `204`.

```sh
curl -X PUT localhost:7171/v1/agent/checks/service:web/pass
```
