# Lictor web UI

The management UI every `lictor agent` serves at `/ui`. It's a SvelteKit single-page app, built to
static files and embedded into the `lictor` binary at compile time.

- What it does and how access works: [docs/ui.md](../docs/ui.md)
- Developing, testing and building it: [docs/development.md](../docs/development.md#web-ui)

```sh
npm ci
npm run dev        # with an agent on 127.0.0.1:7171 (`cargo run -- agent --dev`)
```
