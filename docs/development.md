# Development

## Requirements

- Rust 1.89 or newer (`rustup update`).
- `nats-server` 2.10+ on your `PATH` for the integration tests (`brew install nats-server`,
  your distribution's package, or `./scripts/install-nats-server.sh v2.15.3 /tmp/nats`).
  Without it, NATS tests print "skipping" and pass.

- For the web UI: Node.js 22.17 or newer (`ui/.nvmrc` names the version CI uses). Rust builds and
  tests work without it; the binary then serves a "UI not built" page at `/ui`.

For a real NATS backend without installing `nats-server`:
`docker compose -f deploy/docker/compose.nats.yaml up -d` (JetStream on `127.0.0.1:4222`, monitoring
on `8222`), then `cargo run -- agent --nats-url nats://127.0.0.1:4222`.
`deploy/docker/compose.yaml` runs NATS plus two agents built from source (UIs on 7171 and 7172).

## Build and run

```sh
cargo build                          # debug build
cargo run -- agent --dev             # an agent with an in-memory store
cargo run -- discover web            # in another terminal
cargo build --release                # target/release/lictor
```

## Tests

```sh
cargo test --workspace --all-features            # everything
LICTOR_REQUIRE_NATS=1 cargo test --workspace --all-features   # fail instead of skipping without nats-server
cargo test -p lictor-core                        # one crate
```

| Where | What |
|---|---|
| `crates/*/src` (unit) | Validation, serialization, catalog eligibility, leases, thresholds, parsing, output. |
| `crates/lictor-store/tests/conformance.rs` | The backend conformance suite against the in-memory store. |
| `crates/lictor-store-nats/tests/conformance.rs` | The same suite against a real `nats-server`. |
| `crates/lictor-core/tests/agent.rs` | Agent behavior on the in-memory store with paused time: leases, fencing, restarts, outages, TTL checks, cleanup. |
| `crates/lictor/tests/nats_registry.rs` | Several agents on a real `nats-server`: expiry, fencing, NATS restarts, watches, concurrency. |
| `crates/lictor/tests/cli_dev.rs` | The real binary end to end against `lictor agent --dev` (runs on every OS). |
| `crates/lictor/tests/cli_nats.rs` | Several `lictor agent` processes on a real `nats-server`, driven by the CLI. |
| `crates/lictor-consul/tests/` | Consul DNS (real UDP/TCP sockets) and HTTP adapters against an agent: names, records, limits, filters, blocking queries. |
| `crates/lictor/tests/consul_compat.rs` | With `LICTOR_DOCKER_TESTS=1`: the same services in Consul 1.22 and Lictor, DNS and HTTP answers compared; a real Prometheus discovering targets through Lictor. |

Tests that need time to pass use `tokio::time::pause` (in-memory store) or short leases (NATS), so the whole suite runs in well under a minute.

## Web UI

The UI lives in `ui/` (SvelteKit, Svelte 5, TypeScript). `npm run build` writes it to `ui/build/app`,
and `crates/lictor/build.rs` embeds whatever is there into the next `cargo build`.

```sh
cd ui
npm ci
npm run dev            # http://localhost:5173/ui, proxying /v1 to an agent on 127.0.0.1:7171
                       # (start one with `cargo run -- agent --dev`; LICTOR_AGENT=http://host:port to change)
npm run check          # svelte-check (types, a11y), warnings are errors
npm run lint           # prettier --check (npm run format fixes)
npm run test:unit      # vitest: unit tests (node) and component tests (headless Chromium)
npm run build          # then `cargo build -p lictor` to embed it
npm run test:e2e       # Playwright against two real `lictor agent --dev` processes (needs the build above)
```

The first component or end-to-end test run needs a browser: `npx playwright install chromium`.

| Where | What |
|---|---|
| `ui/src/lib/*.spec.ts` | API client, NDJSON watch parsing, definition form conversion and validation, formatting. |
| `ui/src/lib/components/*.svelte.spec.ts` | Components in a real browser (the service editor). |
| `ui/tests/*.e2e.ts` | Every management flow against real agents, including one started with `--token`. |
| `crates/lictor/src/ui.rs` | Serving: content types, caching, the app fallback, security headers. |

`LICTOR_REQUIRE_UI=1` makes `cargo build` fail when the UI is not built (release builds set it).
`LICTOR_UI_DIR` embeds a build from another directory.

## Code style

- `cargo fmt --all` (config in `rustfmt.toml`) and `cargo clippy --workspace --all-targets --all-features -- -D warnings`.
- CLI registrations in `crates/lictor/src/cli.rs` stay one per line (`#[rustfmt::skip]`).
- The core must not depend on NATS, HTTP or DNS types; adapters depend on `lictor_core::Registry`.

## CI

`.github/workflows/ci.yml` runs on every push and pull request:

| Job | |
|---|---|
| Format, Clippy, Docs | `cargo fmt --check`, clippy with `-D warnings`, rustdoc with `-D warnings`. |
| Test (Linux) | The whole suite against NATS 2.10, 2.11, 2.12, 2.14 and 2.15. |
| Test (macOS, Windows) | The whole suite against the latest NATS. |
| Minimum Rust version | `cargo check` on the `rust-version` from `Cargo.toml`. |
| Coverage | `cargo llvm-cov`; summary in the job page, `lcov.info` as an artifact. |
| Consul compatibility | `consul_compat.rs` against Consul and Prometheus containers. |
| Web UI | Prettier, svelte-check, unit and component tests, then the end-to-end tests against the embedded build. |
| Package smoke test | Builds the static Linux binary (with the UI), `.deb` and `.rpm`; installs the `.deb`; checks `/ui`. |

## Releasing

1. Bump `version` in the root `Cargo.toml` and run `cargo check` to update `Cargo.lock`.
2. Commit, then tag and push: `git tag v0.2.0 && git push origin v0.2.0`.
3. `.github/workflows/release.yml` builds:

| Target | Artifacts |
|---|---|
| `x86_64-unknown-linux-musl` | `.tar.gz` (static), `.deb`, `.rpm` |
| `aarch64-unknown-linux-musl` | `.tar.gz` (static), `.deb`, `.rpm` |
| `x86_64-apple-darwin`, `aarch64-apple-darwin` | `.tar.gz` |
| `x86_64-pc-windows-msvc` | `.zip` |

The web UI is built once and embedded into every binary (the builds fail without it).
It then publishes a GitHub release with `SHA256SUMS`, and a multi-arch image
`ghcr.io/<owner>/lictor:<version>` (`linux/amd64`, `linux/arm64`).
Run the workflow by hand (Actions, Release, Run workflow) to build everything without publishing.

Build one target locally with `./scripts/package.sh <target> dist` (Linux targets need
`cargo-zigbuild` and `zig`; packages need `cargo-deb` and `cargo-generate-rpm`).

## Known issues

- **Windows: absolute paths for `--config` and `--config-dir`.** CLYOPS 0.13.1 treats only paths
  starting with `/` as absolute, so `C:\...` is joined onto the current directory. Use a path
  relative to the current directory on Windows until CLYOPS is fixed. The three tests that cover
  these options are ignored on Windows (`cfg_attr(windows, ignore)` in `crates/lictor/tests/cli_dev.rs`).

## Dependencies

- [CLYOPS](https://clyops.io) is used from git, pinned to a tag in the root `Cargo.toml`.
  Switch to the crates.io release once it is published.
- TLS uses rustls with the `ring` provider everywhere (no OpenSSL, no C toolchain needed to cross-compile).
