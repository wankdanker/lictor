//! Text and JSON rendering for CLI results.
//!
//! JSON output is the exact API types, pretty-printed, so scripts can rely on it.
//! Text output is aligned tables for humans.

use lictor_types::{AgentStatus, CheckView, NodeInfo, ServiceInstance, ServiceSummary, format_duration};
use serde::Serialize;
use std::fmt::Write as _;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Text,
    Json,
}

impl Format {
    pub fn parse(s: &str) -> Format {
        if s == "json" { Format::Json } else { Format::Text }
    }
}

/// Pretty JSON with a trailing newline.
pub fn json<T: Serialize + ?Sized>(v: &T) -> String {
    let mut s = serde_json::to_string_pretty(v).expect("serializable");
    s.push('\n');
    s
}

/// Render rows as a left-aligned table with a header. Empty tables render as `empty`.
pub fn table(header: &[&str], rows: &[Vec<String>], empty: &str) -> String {
    if rows.is_empty() {
        return format!("{empty}\n");
    }
    let mut widths: Vec<usize> = header.iter().map(|h| h.len()).collect();
    for r in rows {
        for (i, c) in r.iter().enumerate() {
            widths[i] = widths[i].max(c.chars().count());
        }
    }
    let mut out = String::new();
    let line = |cells: Vec<&str>, out: &mut String| {
        let last = cells.len() - 1;
        for (i, c) in cells.iter().enumerate() {
            if i == last {
                out.push_str(c);
            } else {
                let _ = write!(out, "{c:<w$}  ", w = widths[i]);
            }
        }
        while out.ends_with(' ') {
            out.pop();
        }
        out.push('\n');
    };
    line(header.to_vec(), &mut out);
    for r in rows {
        line(r.iter().map(String::as_str).collect(), &mut out);
    }
    out
}

fn join(v: &[String]) -> String {
    if v.is_empty() { "-".into() } else { v.join(",") }
}

fn meta(m: &std::collections::BTreeMap<String, String>) -> String {
    if m.is_empty() { "-".into() } else { m.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(",") }
}

fn first_line(s: &str, max: usize) -> String {
    let line = s.lines().next().unwrap_or("");
    if line.chars().count() > max {
        format!("{}...", line.chars().take(max).collect::<String>())
    } else {
        line.to_string()
    }
}

pub fn instances(list: &[ServiceInstance]) -> String {
    let rows: Vec<Vec<String>> = list
        .iter()
        .map(|i| {
            vec![
                i.service_name.clone(),
                i.service_id.clone(),
                i.node_id.clone(),
                format!("{}:{}", i.address, i.port),
                i.status.to_string(),
                join(&i.tags),
                format!("{}/{}", i.namespace, i.region),
            ]
        })
        .collect();
    table(&["SERVICE", "ID", "NODE", "ADDRESS", "STATUS", "TAGS", "SCOPE"], &rows, "No instances found.")
}

pub fn summaries(list: &[ServiceSummary]) -> String {
    let rows: Vec<Vec<String>> = list
        .iter()
        .map(|s| {
            vec![
                s.name.clone(),
                s.instances.to_string(),
                s.passing.to_string(),
                s.warning.to_string(),
                s.critical.to_string(),
                s.unknown.to_string(),
                join(&s.tags),
            ]
        })
        .collect();
    table(&["NAME", "INSTANCES", "PASSING", "WARNING", "CRITICAL", "UNKNOWN", "TAGS"], &rows, "No services registered.")
}

pub fn nodes(list: &[NodeInfo]) -> String {
    let rows: Vec<Vec<String>> = list
        .iter()
        .map(|n| {
            vec![
                n.node_id.clone(),
                n.hostname.clone(),
                join(&n.addresses),
                n.namespace.clone(),
                n.region.clone(),
                n.services.to_string(),
                format_duration(Duration::from_secs(n.lease_remaining_ms / 1000)),
                n.agent_version.clone(),
            ]
        })
        .collect();
    table(
        &["NODE", "HOSTNAME", "ADDRESSES", "NAMESPACE", "REGION", "SERVICES", "LEASE", "VERSION"],
        &rows,
        "No live nodes.",
    )
}

pub fn checks(list: &[CheckView]) -> String {
    let rows: Vec<Vec<String>> = list
        .iter()
        .map(|c| {
            vec![c.id.clone(), c.service_id.clone(), c.kind.clone(), c.status.to_string(), first_line(&c.output, 60)]
        })
        .collect();
    table(&["CHECK", "SERVICE", "TYPE", "STATUS", "OUTPUT"], &rows, "No checks registered.")
}

/// Detailed, multi-line view of instances (inspect / health status NAME).
pub fn instance_details(list: &[ServiceInstance], empty: &str) -> String {
    if list.is_empty() {
        return format!("{empty}\n");
    }
    let mut out = String::new();
    for (n, i) in list.iter().enumerate() {
        if n > 0 {
            out.push('\n');
        }
        let _ = writeln!(out, "{} ({}) on {} [{}]", i.service_name, i.service_id, i.node_id, i.status);
        let _ = writeln!(out, "  Address:   {}:{} ({})", i.address, i.port, i.protocol);
        let _ = writeln!(out, "  Scope:     {}/{}", i.namespace, i.region);
        let _ = writeln!(out, "  Tags:      {}", join(&i.tags));
        let _ = writeln!(out, "  Metadata:  {}", meta(&i.metadata));
        if !i.node_hostname.is_empty() {
            let _ = writeln!(out, "  Node:      {} ({})", i.node_hostname, join(&i.node_addresses));
        }
        let _ = writeln!(out, "  Revision:  {}  Generation: {}", i.revision, i.generation);
        if i.checks.is_empty() {
            let _ = writeln!(out, "  Checks:    none");
        } else {
            let _ = writeln!(out, "  Checks:");
            for c in &i.checks {
                let _ = writeln!(
                    out,
                    "    {:<9} {} ({}) {}",
                    c.status.to_string(),
                    c.id,
                    c.kind,
                    first_line(&c.output, 70)
                );
            }
        }
    }
    out
}

pub fn status(s: &AgentStatus, addr: Option<&str>) -> String {
    let mut out = String::new();
    let yes = |b: bool| if b { "yes" } else { "no" };
    match &s.node {
        Some(n) => {
            let _ = writeln!(out, "Node:              {} ({})", n.node_id, n.hostname);
            let _ = writeln!(out, "Scope:             {}/{}", n.namespace, n.region);
            let _ = writeln!(out, "Generation:        {}", n.generation);
            let _ = writeln!(out, "Addresses:         {}", join(&n.addresses));
            let _ = writeln!(
                out,
                "Membership lease:  {} remaining",
                format_duration(Duration::from_secs(n.lease_remaining_ms / 1000))
            );
        }
        None => {
            let _ = writeln!(out, "Node:              (none: direct read-only access)");
        }
    }
    if let Some(a) = addr {
        let _ = writeln!(out, "Agent API:         {a}");
    }
    let _ = writeln!(out, "Version:           {}", s.version);
    let _ = writeln!(out, "Backend:           {}", s.backend);
    let _ = writeln!(out, "Connected:         {}", yes(s.connected));
    let _ = writeln!(out, "Catalog ready:     {}", yes(s.catalog_ready));
    let _ = writeln!(
        out,
        "Catalog:           {} nodes, {} registrations (index {})",
        s.catalog_nodes, s.catalog_services, s.catalog_index
    );
    if s.node.is_some() {
        let _ = writeln!(out, "Local:             {} services, {} checks", s.local_services, s.local_checks);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_align() {
        let t = table(&["A", "LONG"], &[vec!["xxx".into(), "1".into()], vec!["y".into(), "22".into()]], "none");
        assert_eq!(t, "A    LONG\nxxx  1\ny    22\n");
        assert_eq!(table(&["A"], &[], "Nothing."), "Nothing.\n");
    }

    #[test]
    fn helpers() {
        assert_eq!(join(&[]), "-");
        assert_eq!(first_line("abc\ndef", 10), "abc");
        assert_eq!(first_line("abcdef", 3), "abc...");
        assert_eq!(Format::parse("json"), Format::Json);
        assert_eq!(Format::parse("text"), Format::Text);
    }
}
