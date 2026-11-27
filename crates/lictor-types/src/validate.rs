//! Input validation shared by every entry point (CLI, HTTP, config files).
//!
//! Identifiers become segments of store keys (`namespace.region.service.node.id`),
//! so they may not contain dots, wildcards or whitespace.

use std::collections::BTreeMap;

/// Longest identifier accepted.
pub const MAX_IDENT_LEN: usize = 128;
/// Longest tag accepted.
pub const MAX_TAG_LEN: usize = 255;
/// Longest metadata value accepted.
pub const MAX_META_VALUE_LEN: usize = 512;
/// Most metadata pairs on one record.
pub const MAX_META_PAIRS: usize = 64;

/// Check that `value` is a valid identifier: 1 to 128 ASCII letters, digits, `_` or `-`,
/// not starting with `-`. `what` names the field in the error message.
pub fn ident(what: &str, value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("{what} must not be empty"));
    }
    if value.len() > MAX_IDENT_LEN {
        return Err(format!("{what} '{value}' is longer than {MAX_IDENT_LEN} characters"));
    }
    if value.starts_with('-') {
        return Err(format!("{what} '{value}' must not start with '-'"));
    }
    if let Some(c) = value.chars().find(|c| !(c.is_ascii_alphanumeric() || *c == '_' || *c == '-')) {
        return Err(format!("{what} '{value}' contains '{c}'; use letters, digits, '_' or '-'"));
    }
    Ok(())
}

/// Check one tag: non-empty, at most 255 characters, no whitespace or control characters.
pub fn tag(value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err("tags must not be empty".into());
    }
    if value.len() > MAX_TAG_LEN {
        return Err(format!("tag '{value}' is longer than {MAX_TAG_LEN} characters"));
    }
    if value.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(format!("tag '{value}' must not contain whitespace"));
    }
    Ok(())
}

/// Check a metadata map: keys are identifiers, values are at most 512 characters.
pub fn metadata(map: &BTreeMap<String, String>) -> Result<(), String> {
    if map.len() > MAX_META_PAIRS {
        return Err(format!("at most {MAX_META_PAIRS} metadata pairs are allowed"));
    }
    for (k, v) in map {
        ident("metadata key", k)?;
        if v.len() > MAX_META_VALUE_LEN {
            return Err(format!("metadata value for '{k}' is longer than {MAX_META_VALUE_LEN} characters"));
        }
    }
    Ok(())
}

/// Parse `key=value` (or `key:value`) pairs into a metadata map.
pub fn parse_pairs<'a, I: IntoIterator<Item = &'a str>>(pairs: I) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    for pair in pairs {
        let (k, v) = pair
            .split_once('=')
            .or_else(|| pair.split_once(':'))
            .ok_or_else(|| format!("expected key=value, got '{pair}'"))?;
        out.insert(k.trim().to_string(), v.trim().to_string());
    }
    metadata(&out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idents() {
        for ok in ["matcher", "node-1", "a_b", "A9", "x"] {
            ident("name", ok).unwrap();
        }
        for bad in ["", "-x", "a.b", "a b", "a*", "a>", "ünï", &"x".repeat(129)] {
            assert!(ident("name", bad).is_err(), "{bad:?} should be rejected");
        }
        assert_eq!(
            ident("service name", "a.b").unwrap_err(),
            "service name 'a.b' contains '.'; use letters, digits, '_' or '-'"
        );
    }

    #[test]
    fn tags() {
        tag("v2").unwrap();
        tag("env=prod").unwrap();
        assert!(tag("").is_err());
        assert!(tag("has space").is_err());
        assert!(tag(&"t".repeat(256)).is_err());
    }

    #[test]
    fn pairs() {
        let m = parse_pairs(["shard=4", "version:2.1.0", " zone = a "]).unwrap();
        assert_eq!(m["shard"], "4");
        assert_eq!(m["version"], "2.1.0");
        assert_eq!(m["zone"], "a");
        assert!(parse_pairs(["novalue"]).is_err());
        assert!(parse_pairs(["bad.key=1"]).is_err());
        let long = format!("k={}", "v".repeat(513));
        assert!(parse_pairs([long.as_str()]).is_err());
    }
}
