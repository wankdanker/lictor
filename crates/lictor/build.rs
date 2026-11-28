//! Embeds the web UI (`ui/build/app`, produced by `npm --prefix ui run build`) into the binary.
//!
//! Without a built UI the binary still builds; the agent then serves a page explaining how to
//! build it. Set `LICTOR_REQUIRE_UI=1` (release builds do) to fail instead, and `LICTOR_UI_DIR`
//! to embed a build from somewhere else.

use std::path::{Path, PathBuf};
use std::{env, fs};

fn main() {
    println!("cargo:rerun-if-env-changed=LICTOR_UI_DIR");
    println!("cargo:rerun-if-env-changed=LICTOR_REQUIRE_UI");
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let dir = match env::var_os("LICTOR_UI_DIR") {
        Some(d) => PathBuf::from(d),
        None => manifest.join("../../ui/build/app"),
    };
    // Watch the parent, which always exists (`ui/build` is committed with a README): cargo reruns
    // a build script on every build while a watched path is missing.
    println!("cargo:rerun-if-changed={}", dir.parent().unwrap_or(&dir).display());

    let mut files = Vec::new();
    if dir.join("200.html").is_file() {
        collect(&dir, &dir, &mut files);
        files.sort();
    } else if env::var_os("LICTOR_REQUIRE_UI").is_some_and(|v| v == "1") {
        panic!(
            "LICTOR_REQUIRE_UI=1 but {} has no built UI; run `npm --prefix ui ci && npm --prefix ui run build`",
            dir.display()
        );
    }

    let mut out = String::from("/// Embedded UI files: path relative to the build directory, and contents.\n");
    out.push_str("pub static ASSETS: &[(&str, &[u8])] = &[\n");
    for (rel, abs) in &files {
        out.push_str(&format!("    ({rel:?}, include_bytes!({:?})),\n", abs.display().to_string()));
    }
    out.push_str("];\n");
    fs::write(PathBuf::from(env::var("OUT_DIR").unwrap()).join("ui_assets.rs"), out).unwrap();
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    for entry in fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(root, &path, out);
        } else if path.is_file() {
            let rel = path.strip_prefix(root).unwrap();
            let rel: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
            out.push((rel.join("/"), path.canonicalize().unwrap()));
        }
    }
}
