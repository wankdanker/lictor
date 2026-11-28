# CLI reference

`lictor` is one binary: `lictor agent` runs the agent, every other command is a client.

Clients talk to the local agent's HTTP API (`--http-addr`, default `127.0.0.1:7171`). Read-only
commands can instead read the registry straight from NATS with `--direct`; that needs no agent,
only NATS access.

Option parsing comes from [CLYOPS](https://clyops.io), so every program behaves the same way:

- `--opt value`, `--opt=value`, `-o value`, `-ovalue`, `--no-flag`, `--` ends options.
- Global options (`-o`, `--http-addr`, `--nats-*`, ...) work before or after the command words.
- Each option can also come from an environment variable or the `--config` file (see [configuration.md](configuration.md)).
- `--help` on any command, `--help-json-schema` for a machine-readable description,
  `--completion bash|zsh|fish` for shell completion.

```sh
# Shell completion
eval "$(lictor --completion bash)"         # ~/.bashrc
eval "$(lictor --completion zsh)"          # ~/.zshrc, after compinit
lictor --completion fish > ~/.config/fish/completions/lictor.fish
```

## Output

Every command prints a table (`-o text`, default) or JSON (`-o json`). JSON output is the
exact API types documented in [http-api.md](http-api.md); it is stable for scripts.
`lictor watch -o json` prints one JSON object per line (NDJSON).

Errors go to stderr: `Error: <message>`, or with `-o json`, `{"error":{"code":"...","message":"..."}}`.

## Exit status

| Status | Meaning |
|---|---|
| 0 | Success (including a discovery with no results) |
| 1 | Invalid input or another error |
| 2 | Not found (service, check, or no live instances for `inspect` / `health status NAME`) |
| 3 | Unavailable: cannot reach the agent or NATS, or the catalog is not ready |
| 4 | Fenced: another agent took over this node id (`lictor agent` only) |

## Commands

### `lictor agent`

Run the agent. See [configuration.md](configuration.md) for every option. Common ones:

```sh
lictor agent --dev                                   # in-memory, single node, for trying things out
lictor agent --nats-url nats://n1:4222,nats://n2:4222 --nats-creds /etc/lictor/nats.creds
lictor agent --namespace prod --region us-east --advertise 10.0.1.5 --config-dir /etc/lictor/conf.d
```

On `SIGTERM`/`Ctrl-C` the agent removes its membership (unless `--leave-on-shutdown=false`) and exits 0.

The agent also serves the [web UI](ui.md) at `/ui` on its HTTP address (`--ui=false` turns it off),
and [Consul-compatible](consul.md) DNS on `127.0.0.1:8600` and HTTP on `127.0.0.1:8500`
(`--dns-addr`, `--consul-http-addr`; `disabled` turns either off).

### `lictor status`

The agent's identity, connection and catalog state. With `--direct`, the catalog's state as seen from NATS.

### `lictor nodes list [--namespace NS] [--region R]`

Live nodes, with their service counts and remaining lease.

### `lictor services ...`

| Command | Does |
|---|---|
| `services list [--namespace] [--region]` | One line per service name: instance and health counts, tags. |
| `services inspect NAME [filters]` | Every live instance, with address, metadata and checks. |
| `services local` | Services registered on this agent (live or not). |
| `services register ...` | Register a service on this agent (see below). |
| `services export ID` | Print the stored definition of one of this agent's services, as JSON for `register --file`. |
| `services deregister ID` | Remove one of this agent's services. |

`services register` takes either a file or flags:

```sh
lictor services register --file web.toml           # JSON or TOML; '-' reads stdin
lictor services register --name web --port 8080 \
    --id web-1 --address 10.0.1.5 --tag v2 --tag primary --meta version=2.1.0 \
    --check-http http://127.0.0.1:8080/health --check-interval 5s --check-timeout 2s
```

| Flag | Meaning |
|---|---|
| `--name`, `--id` | Service name; id unique on this node (default: the name). |
| `--address`, `--port`, `--protocol` | Defaults: node's advertise address, `0`, `tcp`. |
| `-t/--tag`, `-m/--meta key=value` | Repeatable. |
| `--check-tcp host:port` / `--check-http URL` / `--check-ttl 30s` | Add a check (combinable). |
| `--check-interval`, `--check-timeout`, `--check-method`, `--check-tls-skip-verify` | TCP/HTTP settings. |
| `--check-failures N`, `--check-successes N` | Debounce thresholds (default 1). |
| `--check-initial-status` | `critical` (default), `warning`, `passing` or `unknown`. |
| `--check-deregister-after 10m` | Deregister the service after being critical this long. |

Registering again with the same id replaces the registration (checks with unchanged definitions keep their state).

### `lictor discover NAME [filters]`

Live instances of a service. Filters (also on `inspect` and `watch`):

| Filter | Meaning |
|---|---|
| `--healthy` | Only passing instances (same as `--health passing`). |
| `--health any\|passing\|not-critical` | Health filter (default `any`). |
| `-t/--tag TAG` | Must have the tag (repeatable; all must match). |
| `-m/--meta key=value` | Must have the metadata (repeatable). |
| `--node NODE` | Only on this node. |
| `-n/--namespace`, `-r/--region` | Only in this scope (default: everywhere). |

### `lictor watch NAME [filters]`

Prints the current instances, then the full list again every time it changes, until interrupted.

### `lictor health status [NAME]`

Without a name, health counts per service. With a name, each instance and its checks.

### `lictor checks ...`

| Command | Does |
|---|---|
| `checks list` | Checks of this agent's services, with status and last output. |
| `checks register --service ID (--tcp A \| --http URL \| --ttl D) [--id] [--name] [...]` | Add a check to a local service. Same tuning flags as `services register`, without the `check-` prefix. |
| `checks deregister ID` | Remove a check. |
| `checks pass ID [--note TEXT]` | Report a TTL check passing. |
| `checks warn ID [--note TEXT]` | Report it warning. |
| `checks fail ID [--note TEXT]` | Report it critical. |

Check ids default to `service:<service-id>` (`service:<service-id>:<n>` when a service has several).

A TTL heartbeat from a shell script:

```sh
while sleep 10; do
  if curl -fs http://127.0.0.1:8080/ready >/dev/null; then lictor checks pass service:web; else lictor checks fail service:web; fi
done
```

### `lictor version`

The version (also `lictor --version`).
