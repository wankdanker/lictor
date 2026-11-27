//! Host facts: host name, data directory, node id, advertise address.

use std::net::{IpAddr, UdpSocket};
use std::path::{Path, PathBuf};

pub fn hostname() -> String {
    gethostname::gethostname().to_string_lossy().trim().to_string()
}

/// `/var/lib/lictor` when running as root on Unix, otherwise a per-user directory.
pub fn default_data_dir() -> PathBuf {
    #[cfg(unix)]
    {
        if std::env::var("USER").is_ok_and(|u| u == "root") || std::env::var_os("HOME").is_none() {
            return PathBuf::from("/var/lib/lictor");
        }
    }
    if cfg!(windows)
        && let Some(local) = std::env::var_os("LOCALAPPDATA")
    {
        return PathBuf::from(local).join("lictor");
    }
    if cfg!(target_os = "macos")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join("Library/Application Support/lictor");
    }
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(xdg).join("lictor");
    }
    match std::env::var_os("HOME") {
        Some(home) => PathBuf::from(home).join(".local/share/lictor"),
        None => PathBuf::from("lictor-data"),
    }
}

/// Lowercase, keep identifier characters, collapse the rest to `-`.
pub fn sanitize(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() || c == '_' {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').to_string();
    out.chars().take(100).collect()
}

/// Read the node id from `dir/node-id`, creating it (`<hostname>-<random>`) on first use.
pub fn node_id(dir: &Path) -> Result<String, String> {
    let file = dir.join("node-id");
    if let Ok(text) = std::fs::read_to_string(&file) {
        let id = text.trim().to_string();
        lictor_types::validate::ident("node id", &id).map_err(|e| format!("{}: {e}", file.display()))?;
        return Ok(id);
    }
    let base = sanitize(&hostname());
    let base = if base.is_empty() { "node".into() } else { base };
    let id = format!("{base}-{:04x}", rand::random::<u16>());
    std::fs::create_dir_all(dir).map_err(|e| format!("creating data dir {}: {e}", dir.display()))?;
    std::fs::write(&file, format!("{id}\n")).map_err(|e| format!("writing {}: {e}", file.display()))?;
    tracing::info!(node_id = %id, file = %file.display(), "generated a new node id");
    Ok(id)
}

/// The address of the interface used for outbound traffic (no packet is sent).
pub fn primary_address() -> String {
    let probe = |bind: &str, target: &str| -> Option<IpAddr> {
        let s = UdpSocket::bind(bind).ok()?;
        s.connect(target).ok()?;
        let ip = s.local_addr().ok()?.ip();
        (!ip.is_unspecified()).then_some(ip)
    };
    probe("0.0.0.0:0", "192.0.2.1:9")
        .or_else(|| probe("[::]:0", "[2001:db8::1]:9"))
        .map(|ip| ip.to_string())
        .unwrap_or_else(|| "127.0.0.1".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_host_names() {
        assert_eq!(sanitize("Web-01.example.com"), "web-01-example-com");
        assert_eq!(sanitize("..x..y.."), "x-y");
        assert_eq!(sanitize("🙂"), "");
    }

    #[test]
    fn node_id_is_created_once() {
        let dir = tempfile::tempdir().unwrap();
        let a = node_id(dir.path()).unwrap();
        let b = node_id(dir.path()).unwrap();
        assert_eq!(a, b);
        lictor_types::validate::ident("node id", &a).unwrap();
        std::fs::write(dir.path().join("node-id"), "bad id\n").unwrap();
        assert!(node_id(dir.path()).is_err());
    }

    #[test]
    fn finds_an_address() {
        assert!(primary_address().parse::<IpAddr>().is_ok());
    }
}
