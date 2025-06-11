use headers::{Header, HeaderName, HeaderValue};
use std::fmt;
use std::str::FromStr;

/// Represents the possible values for the `cf-cache-status` header
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CfCacheStatus {
    /// Cache hit - served from Cloudflare cache
    Hit,
    /// Cache miss - not in cache, fetched from origin
    Miss,
    /// Dynamic content that bypassed cache
    Dynamic,
    /// Expired content served while revalidating
    Expired,
    /// Content was revalidated and served from cache
    Revalidated,
    /// Content was updated in cache
    Updating,
    /// Cache was bypassed due to configuration
    Bypass,
    /// Unknown or custom status
    Other(String),
}

impl fmt::Display for CfCacheStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CfCacheStatus::Hit => write!(f, "HIT"),
            CfCacheStatus::Miss => write!(f, "MISS"),
            CfCacheStatus::Dynamic => write!(f, "DYNAMIC"),
            CfCacheStatus::Expired => write!(f, "EXPIRED"),
            CfCacheStatus::Revalidated => write!(f, "REVALIDATED"),
            CfCacheStatus::Updating => write!(f, "UPDATING"),
            CfCacheStatus::Bypass => write!(f, "BYPASS"),
            CfCacheStatus::Other(s) => write!(f, "{}", s),
        }
    }
}

/// Parses the `cf-cache-status` header value from a string.
impl FromStr for CfCacheStatus {
    type Err = headers::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_uppercase().as_str() {
            "HIT" => Ok(CfCacheStatus::Hit),
            "MISS" => Ok(CfCacheStatus::Miss),
            "DYNAMIC" => Ok(CfCacheStatus::Dynamic),
            "EXPIRED" => Ok(CfCacheStatus::Expired),
            "REVALIDATED" => Ok(CfCacheStatus::Revalidated),
            "UPDATING" => Ok(CfCacheStatus::Updating),
            "BYPASS" => Ok(CfCacheStatus::Bypass),
            _ => Ok(CfCacheStatus::Other(s.to_string())),
        }
    }
}

/// Provides typesafe access to the `cf-cache-status` header via the `headers` crate.
impl Header for CfCacheStatus {
    fn name() -> &'static HeaderName {
        static CF_CACHE_STATUS: HeaderName = HeaderName::from_static("cf-cache-status");
        &CF_CACHE_STATUS
    }

    fn decode<'i, I>(values: &mut I) -> Result<Self, headers::Error>
    where
        I: Iterator<Item = &'i HeaderValue>,
    {
        let value = values.next().ok_or_else(headers::Error::invalid)?;

        let s = value.to_str().map_err(|_| headers::Error::invalid())?;

        CfCacheStatus::from_str(s)
    }

    fn encode<E>(&self, values: &mut E)
    where
        E: Extend<HeaderValue>,
    {
        let value = match self {
            CfCacheStatus::Hit => HeaderValue::from_static("HIT"),
            CfCacheStatus::Miss => HeaderValue::from_static("MISS"),
            CfCacheStatus::Dynamic => HeaderValue::from_static("DYNAMIC"),
            CfCacheStatus::Expired => HeaderValue::from_static("EXPIRED"),
            CfCacheStatus::Revalidated => HeaderValue::from_static("REVALIDATED"),
            CfCacheStatus::Updating => HeaderValue::from_static("UPDATING"),
            CfCacheStatus::Bypass => HeaderValue::from_static("BYPASS"),
            CfCacheStatus::Other(s) => HeaderValue::try_from(s.as_str())
                .unwrap_or_else(|_| HeaderValue::from_static("INVALID")),
        };

        values.extend(std::iter::once(value));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use headers::{Header, HeaderMapExt};
    use http::{HeaderMap, HeaderValue};
    use test_case::test_case;

    #[test_case("HIT", CfCacheStatus::Hit; "hit uppercase")]
    #[test_case("hit", CfCacheStatus::Hit; "hit lowercase")]
    #[test_case("Hit", CfCacheStatus::Hit; "hit mixed case")]
    #[test_case("MISS", CfCacheStatus::Miss; "miss uppercase")]
    #[test_case("miss", CfCacheStatus::Miss; "miss lowercase")]
    #[test_case("DYNAMIC", CfCacheStatus::Dynamic; "dynamic uppercase")]
    #[test_case("dynamic", CfCacheStatus::Dynamic; "dynamic lowercase")]
    #[test_case("EXPIRED", CfCacheStatus::Expired; "expired uppercase")]
    #[test_case("REVALIDATED", CfCacheStatus::Revalidated; "revalidated uppercase")]
    #[test_case("UPDATING", CfCacheStatus::Updating; "updating uppercase")]
    #[test_case("BYPASS", CfCacheStatus::Bypass; "bypass uppercase")]
    #[test_case("CUSTOM", CfCacheStatus::Other("CUSTOM".to_string()); "custom status")]
    #[test_case("unknown-status", CfCacheStatus::Other("unknown-status".to_string()); "unknown status with hyphen")]
    fn test_decode_success(input: &str, expected: CfCacheStatus) {
        let header_value = HeaderValue::from_str(input).unwrap();
        let mut values = std::iter::once(&header_value);

        let result = CfCacheStatus::decode(&mut values).unwrap();
        assert_eq!(result, expected);
    }

    #[test]
    fn test_decode_empty_iterator() {
        let mut values = std::iter::empty();
        let result = CfCacheStatus::decode(&mut values);
        assert!(result.is_err());
    }

    #[test]
    fn test_decode_invalid_utf8() {
        // Create a HeaderValue with invalid UTF-8
        let header_value = HeaderValue::from_bytes(&[0xFF, 0xFE]).unwrap();
        let mut values = std::iter::once(&header_value);

        let result = CfCacheStatus::decode(&mut values);
        assert!(result.is_err());
    }

    #[test_case(CfCacheStatus::Hit, "HIT"; "hit encodes to uppercase")]
    #[test_case(CfCacheStatus::Miss, "MISS"; "miss encodes to uppercase")]
    #[test_case(CfCacheStatus::Dynamic, "DYNAMIC"; "dynamic encodes to uppercase")]
    #[test_case(CfCacheStatus::Expired, "EXPIRED"; "expired encodes to uppercase")]
    #[test_case(CfCacheStatus::Revalidated, "REVALIDATED"; "revalidated encodes to uppercase")]
    #[test_case(CfCacheStatus::Updating, "UPDATING"; "updating encodes to uppercase")]
    #[test_case(CfCacheStatus::Bypass, "BYPASS"; "bypass encodes to uppercase")]
    #[test_case(CfCacheStatus::Other("custom".to_string()), "custom"; "other preserves original case")]
    fn test_encode(status: CfCacheStatus, expected: &str) {
        let mut values = Vec::new();
        status.encode(&mut values);

        assert_eq!(values.len(), 1);
        assert_eq!(values[0].to_str().unwrap(), expected);
    }

    #[test]
    fn test_encode_other_with_invalid_characters() {
        let status = CfCacheStatus::Other("invalid\x00char".to_string());
        let mut values = Vec::new();
        status.encode(&mut values);

        assert_eq!(values.len(), 1);
        // Should fallback to "INVALID" for unparseable strings
        assert_eq!(values[0].to_str().unwrap(), "INVALID");
    }

    #[test_case(CfCacheStatus::Hit, "HIT"; "hit displays as uppercase")]
    #[test_case(CfCacheStatus::Miss, "MISS"; "miss displays as uppercase")]
    #[test_case(CfCacheStatus::Dynamic, "DYNAMIC"; "dynamic displays as uppercase")]
    #[test_case(CfCacheStatus::Expired, "EXPIRED"; "expired displays as uppercase")]
    #[test_case(CfCacheStatus::Revalidated, "REVALIDATED"; "revalidated displays as uppercase")]
    #[test_case(CfCacheStatus::Updating, "UPDATING"; "updating displays as uppercase")]
    #[test_case(CfCacheStatus::Bypass, "BYPASS"; "bypass displays as uppercase")]
    #[test_case(CfCacheStatus::Other("custom".to_string()), "custom"; "other displays original")]
    fn test_display(status: CfCacheStatus, expected: &str) {
        assert_eq!(format!("{}", status), expected);
    }

    #[test]
    fn test_header_name() {
        let name = CfCacheStatus::name();
        assert_eq!(name.as_str(), "cf-cache-status");
    }

    #[test]
    fn test_roundtrip_encode_decode() {
        let original_statuses = vec![
            CfCacheStatus::Hit,
            CfCacheStatus::Miss,
            CfCacheStatus::Dynamic,
            CfCacheStatus::Expired,
            CfCacheStatus::Revalidated,
            CfCacheStatus::Updating,
            CfCacheStatus::Bypass,
            CfCacheStatus::Other("custom-status".to_string()),
        ];

        for original in original_statuses {
            let mut values = Vec::new();
            original.encode(&mut values);

            let mut iter = values.iter();
            let decoded = CfCacheStatus::decode(&mut iter).unwrap();

            assert_eq!(original, decoded);
        }
    }

    #[test]
    fn test_header_map_integration() {
        let mut headers = HeaderMap::new();

        headers.typed_insert(CfCacheStatus::Hit);

        let retrieved = headers.typed_get::<CfCacheStatus>().unwrap();
        assert_eq!(retrieved, CfCacheStatus::Hit);

        // Test manual header access
        let header_value = headers.get("cf-cache-status").unwrap();
        assert_eq!(header_value.to_str().unwrap(), "HIT");
    }

    #[test]
    fn test_header_map_replace() {
        let mut headers = HeaderMap::new();

        headers.typed_insert(CfCacheStatus::Miss);
        headers.typed_insert(CfCacheStatus::Hit);

        let retrieved = headers.typed_get::<CfCacheStatus>().unwrap();
        assert_eq!(retrieved, CfCacheStatus::Hit);
    }

    #[test]
    fn test_multiple_header_values_takes_first() {
        let mut headers = HeaderMap::new();

        headers.insert("cf-cache-status", HeaderValue::from_static("HIT"));
        headers.append("cf-cache-status", HeaderValue::from_static("MISS"));

        // decode should take the first value
        let retrieved = headers.typed_get::<CfCacheStatus>().unwrap();
        assert_eq!(retrieved, CfCacheStatus::Hit);
    }
}
