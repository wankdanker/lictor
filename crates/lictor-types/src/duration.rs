use std::time::Duration;

/// Parse a human-readable duration such as `10s`, `500ms`, `1m30s` or `2h`.
///
/// A bare integer is read as seconds, so `30` equals `30s`.
pub fn parse_duration(text: &str) -> Result<Duration, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("duration is empty".into());
    }
    if let Ok(secs) = text.parse::<u64>() {
        return Ok(Duration::from_secs(secs));
    }
    humantime::parse_duration(text).map_err(|e| format!("invalid duration '{text}': {e}"))
}

/// Format a duration the way [`parse_duration`] reads it, e.g. `10s` or `1m 30s`.
pub fn format_duration(d: Duration) -> String {
    if d.is_zero() {
        return "0s".into();
    }
    humantime::format_duration(d).to_string().replace(' ', "")
}

pub(crate) mod serde_impl {
    use super::{format_duration, parse_duration};
    use serde::{Deserialize, Deserializer, Serializer, de};
    use std::time::Duration;

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Repr {
        Text(String),
        Secs(u64),
    }

    fn from_repr<E: de::Error>(r: Repr) -> Result<Duration, E> {
        match r {
            Repr::Text(t) => parse_duration(&t).map_err(E::custom),
            Repr::Secs(s) => Ok(Duration::from_secs(s)),
        }
    }

    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format_duration(*d))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        from_repr(Repr::deserialize(d)?)
    }

    pub mod option {
        use super::*;

        pub fn serialize<S: Serializer>(d: &Option<Duration>, s: S) -> Result<S::Ok, S::Error> {
            match d {
                Some(d) => super::serialize(d, s),
                None => s.serialize_none(),
            }
        }

        pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Duration>, D::Error> {
            match Option::<Repr>::deserialize(d)? {
                Some(r) => from_repr(r).map(Some),
                None => Ok(None),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_forms() {
        assert_eq!(parse_duration("10s").unwrap(), Duration::from_secs(10));
        assert_eq!(parse_duration("500ms").unwrap(), Duration::from_millis(500));
        assert_eq!(parse_duration("1m30s").unwrap(), Duration::from_secs(90));
        assert_eq!(parse_duration("2h").unwrap(), Duration::from_secs(7200));
        assert_eq!(parse_duration("30").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_duration(" 5s ").unwrap(), Duration::from_secs(5));
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_duration("").is_err());
        assert!(parse_duration("ten seconds").is_err());
        assert!(parse_duration("-5s").is_err());
    }

    #[test]
    fn formats_round_trip() {
        for text in ["10s", "500ms", "1m30s", "2h", "0s"] {
            let d = parse_duration(text).unwrap();
            assert_eq!(parse_duration(&format_duration(d)).unwrap(), d, "{text}");
        }
        assert_eq!(format_duration(Duration::from_secs(90)), "1m30s");
    }

    #[test]
    fn serde_accepts_strings_and_numbers() {
        #[derive(serde::Deserialize, serde::Serialize)]
        struct T {
            #[serde(with = "serde_impl")]
            d: Duration,
            #[serde(with = "serde_impl::option", default)]
            o: Option<Duration>,
        }
        let t: T = serde_json::from_str(r#"{"d":"3s","o":15}"#).unwrap();
        assert_eq!(t.d, Duration::from_secs(3));
        assert_eq!(t.o, Some(Duration::from_secs(15)));
        let t: T = serde_json::from_str(r#"{"d":2}"#).unwrap();
        assert_eq!(t.o, None);
        assert_eq!(serde_json::to_string(&t).unwrap(), r#"{"d":"2s","o":null}"#);
        assert!(serde_json::from_str::<T>(r#"{"d":"soon"}"#).is_err());
    }
}
