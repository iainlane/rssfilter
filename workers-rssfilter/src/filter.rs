use headers::{HeaderMapExt, UserAgent};
use headers_accept::Accept;
use http::header::{HeaderMap, HeaderName, HOST};
use http::HeaderValue;
use std::borrow::Borrow;
use std::collections::HashSet;
use std::str::FromStr;
use std::sync::LazyLock;

/// Strip any headers that start with these prefixes.
static KEY_PREFIXES_TO_STRIP: [&str; 1] = ["x-"];

/// Headers that we always strip from our outgoing requests
static HEADERS_TO_STRIP: LazyLock<HashSet<HeaderName>> = LazyLock::new(|| {
    // We always strip the `Host` header, since it will be set by our server.
    [HOST].into_iter().collect()
});

/// Headers that we always set on outgoing requests.
static HEADERS_TO_SET: LazyLock<HeaderMap> = LazyLock::new(|| {
    let rss_accept = Accept::from_str(
        "application/rss+xml, application/rdf+xml;q=0.8, application/atom+xml;q=0.6, application/xml;q=0.4, text/xml;q=0.4"
    ).expect("Invalid RSS Accept header");

    let user_agent = UserAgent::from_static("rssfilter https://github.com/iainlane/rssfilter/");

    let mut map = HeaderMap::new();
    map.typed_insert(rss_accept);
    map.typed_insert(user_agent);
    map
});

/// Determines if a header should be stripped from the incoming headers, based on our filtering rules.
fn should_strip_header<K>(key: &K) -> bool
where
    K: Borrow<HeaderName>,
{
    let header_name = key.borrow();

    HEADERS_TO_SET.contains_key(header_name)
        || HEADERS_TO_STRIP.contains(header_name)
        || KEY_PREFIXES_TO_STRIP
            .iter()
            .any(|prefix| header_name.as_str().starts_with(prefix))
}

/// Filters out headers that should not be passed to the target URL.
/// Headers come from the user, but since we are proxying the request, there are
/// some headers that we should not pass to the target URL, such as `Host`
/// (because it will be the host of our server), and some that we hardcode
/// to ensure the request is valid, such as `Accept` and `User-Agent`.
pub fn filter_request_headers<I, K, V>(headers: I) -> HeaderMap
where
    I: IntoIterator<Item = (K, V)>,
    K: Borrow<HeaderName>,
    V: Borrow<HeaderValue>,
{
    headers
        .into_iter()
        .filter(|(key, _)| !should_strip_header(key))
        .map(|(key, value)| (key.borrow().clone(), value.borrow().clone()))
        .chain(HEADERS_TO_SET.iter().map(|(k, v)| (k.clone(), v.clone())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::header::{HeaderName, HeaderValue, CONTENT_TYPE};
    use test_case::test_case;

    fn build_test_headers(
        headers: Vec<(HeaderName, &str)>,
    ) -> impl Iterator<Item = (HeaderName, HeaderValue)> + use<'_> {
        headers
            .into_iter()
            .map(|(k, v)| (k, HeaderValue::from_str(v).expect("Invalid header value")))
    }

    fn expected_accept_value() -> (HeaderName, HeaderValue) {
        let accept_header = HEADERS_TO_SET
            .typed_get::<Accept>()
            .expect("Failed to get Accept header");

        (
            HeaderName::from_static("accept"),
            HeaderValue::from_str(&accept_header.to_string()).expect("Invalid Accept header"),
        )
    }

    fn expected_user_agent_value() -> (HeaderName, HeaderValue) {
        let user_agent_header = HEADERS_TO_SET
            .typed_get::<UserAgent>()
            .expect("Failed to get User-Agent header");

        (
            HeaderName::from_static("user-agent"),
            HeaderValue::from_str(&user_agent_header.to_string())
                .expect("Invalid User-Agent header"),
        )
    }

    fn build_expected_headers(base_headers: Vec<(HeaderName, &str)>) -> HeaderMap {
        base_headers
            .into_iter()
            .map(|(k, v)| {
                (
                    k,
                    HeaderValue::from_str(v).expect("Invalid header: `{k}: {v}`"),
                )
            })
            .chain([expected_accept_value(), expected_user_agent_value()])
            .collect()
    }

    #[test_case(vec![(CONTENT_TYPE, "application/json")], vec![(CONTENT_TYPE, "application/json")] ; "no headers to filter")]
    #[test_case(vec![(HOST, "example.com")], vec![] ; "filter out host header")]
    #[test_case(vec![(HOST, "example.com"), (CONTENT_TYPE, "application/json")], vec![(CONTENT_TYPE, "application/json")] ; "filter HOST header, retaining content-type")]
    #[test_case(vec![(HOST, "example.com"), (CONTENT_TYPE, "application/json"), (HeaderName::from_static("x-custom-header"), "value")], vec![(CONTENT_TYPE, "application/json")] ; "filter host and x-custom-header headers, retaining content-type")]
    #[test_case(vec![(HOST, "example.com"), (HeaderName::from_static("x-custom-header"), "value")], vec![] ; "filter host and x-custom-header headers")]
    #[test_case(
      vec![(CONTENT_TYPE, "application/json"), (HeaderName::from_static("accept"), "foo/bar")],
      vec![(CONTENT_TYPE, "application/json")];
      "incoming accept header is overwritten"
    )]
    #[test_case(
      vec![(CONTENT_TYPE, "application/json"), (HeaderName::from_static("user-agent"), "custom-agent")],
      vec![(CONTENT_TYPE, "application/json")];
      "incoming user-agent header is overwritten"
    )]
    #[test_case(
      vec![
        (CONTENT_TYPE, "application/json"),
        (HeaderName::from_static("accept"), "foo/bar"),
        (HeaderName::from_static("user-agent"), "custom-agent")
      ],
      vec![(CONTENT_TYPE, "application/json")];
      "incoming accept and user-agent headers are both overwritten"
    )]
    fn test_filter_request_headers(
        input_headers: Vec<(HeaderName, &str)>,
        expected_base: Vec<(HeaderName, &str)>,
    ) {
        let headers = build_test_headers(input_headers);
        let expected_headers = build_expected_headers(expected_base);
        let filtered_headers = filter_request_headers(headers);

        assert_eq!(filtered_headers, expected_headers);
    }

    #[test]
    fn test_with_header_map() {
        let mut headers = HeaderMap::new();
        headers.insert(HOST, HeaderValue::from_static("example.com"));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            HeaderName::from_static("x-custom-header"),
            HeaderValue::from_static("value"),
        );

        let filtered = filter_request_headers(&headers);

        assert!(!filtered.contains_key(HOST));
        assert!(!filtered.contains_key("x-custom-header"));
        assert_eq!(
            filtered.get(CONTENT_TYPE).expect("Missing CONTENT_TYPE"),
            "application/json"
        );
        assert!(filtered.contains_key("accept"));
        assert!(filtered.contains_key("user-agent"));
    }
}
