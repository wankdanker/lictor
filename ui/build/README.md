`npm run build` writes the UI to `app/` here, and `crates/lictor/build.rs` embeds it into the
`lictor` binary. This directory is committed so that build script has something to watch; `app/`
is not.
