//! Service definition files (`--file` and `--config-dir`).
//!
//! A file is JSON (`.json`) or TOML (anything else), and holds one service or several:
//!
//! ```toml
//! [[service]]
//! name = "web"
//! port = 8080
//! tags = ["v2"]
//!
//! [[service.checks]]
//! type = "http"
//! url = "http://127.0.0.1:8080/health"
//! interval = "10s"
//! ```
//!
//! Accepted shapes: a single service object; `{"service": {...}}` or `{"service": [...]}`;
//! `{"services": [...]}`; or a JSON array of services.

use lictor_types::ServiceRegistration;
use serde_json::Value;
use std::path::{Path, PathBuf};

pub fn parse(text: &str, json: bool, origin: &str) -> Result<Vec<ServiceRegistration>, String> {
    let value: Value = if json {
        serde_json::from_str(text).map_err(|e| format!("{origin}: invalid JSON: {e}"))?
    } else {
        let t: toml::Value = toml::from_str(text).map_err(|e| format!("{origin}: invalid TOML: {e}"))?;
        serde_json::to_value(t).map_err(|e| format!("{origin}: {e}"))?
    };
    let items = match value {
        Value::Array(items) => items,
        Value::Object(mut map) => {
            if let Some(v) = map.remove("services").or_else(|| map.remove("service")) {
                if !map.is_empty() {
                    let extra: Vec<&String> = map.keys().collect();
                    return Err(format!("{origin}: unexpected top-level keys {extra:?}"));
                }
                match v {
                    Value::Array(items) => items,
                    other => vec![other],
                }
            } else {
                vec![Value::Object(map)]
            }
        }
        _ => return Err(format!("{origin}: expected a service object or a list of services")),
    };
    items
        .into_iter()
        .enumerate()
        .map(|(n, v)| {
            let reg: ServiceRegistration =
                serde_json::from_value(v).map_err(|e| format!("{origin}: service #{}: {e}", n + 1))?;
            reg.validate().map_err(|e| format!("{origin}: service #{}: {e}", n + 1))?;
            Ok(reg)
        })
        .collect()
}

/// Read a definition file; `-` reads stdin.
pub fn read_file(path: &str) -> Result<Vec<ServiceRegistration>, String> {
    if path == "-" {
        let mut text = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut text).map_err(|e| format!("reading stdin: {e}"))?;
        let json = text.trim_start().starts_with(['{', '[']);
        return parse(&text, json, "stdin");
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("reading {path}: {e}"))?;
    parse(&text, path.ends_with(".json"), path)
}

/// Every `*.toml` and `*.json` file in `dir`, sorted by name.
pub fn read_dir(dir: &Path) -> Result<Vec<(PathBuf, Vec<ServiceRegistration>)>, String> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("reading {}: {e}", dir.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "toml" || x == "json"))
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|p| {
            let regs = read_file(&p.to_string_lossy())?;
            Ok((p, regs))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lictor_types::CheckKind;
    use std::time::Duration;

    #[test]
    fn toml_list_with_checks() {
        let regs = parse(
            r#"
            [[service]]
            name = "web"
            port = 8080
            tags = ["v2"]
            [service.metadata]
            shard = "4"
            [[service.checks]]
            type = "http"
            url = "http://127.0.0.1:8080/health"
            interval = "5s"

            [[service]]
            name = "db"
            port = 5432
            "#,
            false,
            "t.toml",
        )
        .unwrap();
        assert_eq!(regs.len(), 2);
        assert_eq!(regs[0].metadata["shard"], "4");
        assert!(matches!(regs[0].checks[0].kind, CheckKind::Http { .. }));
        assert_eq!(regs[0].checks[0].interval, Some(Duration::from_secs(5)));
    }

    #[test]
    fn json_shapes() {
        assert_eq!(parse(r#"{"name":"a","port":1}"#, true, "x").unwrap().len(), 1);
        assert_eq!(parse(r#"[{"name":"a"},{"name":"b"}]"#, true, "x").unwrap().len(), 2);
        assert_eq!(parse(r#"{"services":[{"name":"a"}]}"#, true, "x").unwrap().len(), 1);
        assert_eq!(parse(r#"{"service":{"name":"a"}}"#, true, "x").unwrap().len(), 1);
        assert_eq!(parse("name = \"a\"\nport = 9\n", false, "x").unwrap()[0].port, 9);
    }

    #[test]
    fn helpful_errors() {
        let e = parse(r#"{"name":"a","prot":1}"#, true, "svc.json").unwrap_err();
        assert!(e.contains("svc.json: service #1") && e.contains("unknown field"), "{e}");
        let e = parse(r#"{"name":"a.b"}"#, true, "svc.json").unwrap_err();
        assert!(e.contains("service name"), "{e}");
        assert!(parse("not = [toml", false, "x").unwrap_err().contains("invalid TOML"));
        assert!(parse("42", true, "x").is_err());
        assert!(parse(r#"{"service":{"name":"a"},"other":1}"#, true, "x").is_err());
    }

    #[test]
    fn reads_directories_in_order() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("b.json"), r#"{"name":"b"}"#).unwrap();
        std::fs::write(dir.path().join("a.toml"), "name = \"a\"").unwrap();
        std::fs::write(dir.path().join("README"), "ignored").unwrap();
        let all = read_dir(dir.path()).unwrap();
        let names: Vec<&str> = all.iter().flat_map(|(_, r)| r.iter().map(|s| s.name.as_str())).collect();
        assert_eq!(names, ["a", "b"]);
    }
}

/// The TOML examples in the documentation must stay valid.
#[cfg(test)]
mod doc_examples {
    fn toml_blocks(doc: &str) -> Vec<String> {
        // Checkouts on Windows may have CRLF line endings.
        let doc = doc.replace("\r\n", "\n");
        doc.split("```toml\n").skip(1).filter_map(|b| b.split("```").next()).map(str::to_string).collect()
    }

    #[test]
    fn documentation_examples_parse() {
        let docs = [
            ("README.md", include_str!("../../../README.md")),
            ("docs/configuration.md", include_str!("../../../docs/configuration.md")),
            ("docs/consul.md", include_str!("../../../docs/consul.md")),
        ];
        let mut checked = 0;
        for (name, text) in docs {
            for block in toml_blocks(text) {
                if !block.contains("name =") {
                    continue; // not a service definition (e.g. Cargo snippets)
                }
                super::parse(&block, false, name).unwrap_or_else(|e| panic!("{name}: {e}\n{block}"));
                checked += 1;
            }
        }
        assert!(checked >= 3, "found {checked} examples");
    }

    /// The services the Docker Compose stack registers (deploy/docker/example-services).
    #[test]
    fn compose_example_services_parse() {
        let files = [
            ("nats.toml", include_str!("../../../deploy/docker/example-services/nats.toml")),
            ("demo.toml", include_str!("../../../deploy/docker/example-services/demo.toml")),
        ];
        for (name, text) in files {
            let regs = super::parse(text, false, name).unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(regs.len(), 1, "{name}");
        }
    }
}
