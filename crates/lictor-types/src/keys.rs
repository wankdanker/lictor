//! Store key layout.
//!
//! | Record  | Key                                           |
//! |---------|-----------------------------------------------|
//! | Node    | `{namespace}.{region}.{node}`                 |
//! | Service | `{namespace}.{region}.{service}.{node}.{id}`  |
//! | Health  | same as the service key                       |
//!
//! Segments are validated identifiers (see [`crate::validate::ident`]), so they never contain dots.

/// Key of a node membership record.
pub fn node(namespace: &str, region: &str, node: &str) -> String {
    format!("{namespace}.{region}.{node}")
}

/// Key of a service registration (and of its health record).
pub fn service(namespace: &str, region: &str, service: &str, node: &str, id: &str) -> String {
    format!("{namespace}.{region}.{service}.{node}.{id}")
}

/// The parts of a node key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeKey<'a> {
    pub namespace: &'a str,
    pub region: &'a str,
    pub node: &'a str,
}

/// The parts of a service key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceKey<'a> {
    pub namespace: &'a str,
    pub region: &'a str,
    pub service: &'a str,
    pub node: &'a str,
    pub id: &'a str,
}

/// Split a node key; `None` if it does not have three segments.
pub fn parse_node(key: &str) -> Option<NodeKey<'_>> {
    let mut it = key.split('.');
    let k = NodeKey { namespace: it.next()?, region: it.next()?, node: it.next()? };
    it.next().is_none().then_some(k)
}

/// Split a service key; `None` if it does not have five segments.
pub fn parse_service(key: &str) -> Option<ServiceKey<'_>> {
    let mut it = key.split('.');
    let k =
        ServiceKey { namespace: it.next()?, region: it.next()?, service: it.next()?, node: it.next()?, id: it.next()? };
    it.next().is_none().then_some(k)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let k = node("production", "us-east", "node123");
        assert_eq!(k, "production.us-east.node123");
        assert_eq!(parse_node(&k).unwrap(), NodeKey { namespace: "production", region: "us-east", node: "node123" });
        let k = service("production", "us-east", "matcher", "node123", "instance01");
        assert_eq!(k, "production.us-east.matcher.node123.instance01");
        let p = parse_service(&k).unwrap();
        assert_eq!((p.service, p.node, p.id), ("matcher", "node123", "instance01"));
    }

    #[test]
    fn rejects_wrong_shapes() {
        assert!(parse_node("a.b").is_none());
        assert!(parse_node("a.b.c.d").is_none());
        assert!(parse_service("a.b.c.d").is_none());
        assert!(parse_service("a.b.c.d.e.f").is_none());
    }
}
