use http::header::{HeaderName, HOST};
use http::{HeaderMap, HeaderValue};
use once_cell::sync::Lazy;
use std::collections::HashSet;

const RSS_ACCEPT: &str = "application/rss+xml, application/rdf+xml;q=0.8, application/atom+xml;q=0.6, application/xml;q=0.4, text/xml;q=0.4";

/// Filters out headers that should not be passed to the target URL.
/// Headers come from the user, but since we are proxying the request, there are
/// some headers that we should not pass to the target URL, such as `Host`
/// (because it will be the host of the Lambda).
pub fn filter_request_headers(mut headers: HeaderMap) -> HeaderMap {
    const KEY_PREFIXES_TO_STRIP: [&str; 1] = ["x-"];
    static HEADERS_TO_STRIP: Lazy<HashSet<HeaderName>> = Lazy::new(|| [HOST].into_iter().collect());
    static HEADERS_TO_SET: [(&str, HeaderValue); 1] =
        [("Accept", HeaderValue::from_static(RSS_ACCEPT))];

    let headers_to_remove = headers
        .keys()
        .filter(|k| {
            KEY_PREFIXES_TO_STRIP
                .iter()
                .any(|p| k.as_str().starts_with(p))
                || HEADERS_TO_STRIP.contains(*k)
        })
        .cloned()
        .collect::<Vec<HeaderName>>();

    headers_to_remove.iter().for_each(|h| {
        headers.remove(h);
    });

    HEADERS_TO_SET.iter().for_each(|(k, v)| {
        headers.insert(*k, v.clone());
    });

    headers
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::header::{HeaderName, CONTENT_TYPE};
    use test_case::test_case;

    #[test_case(vec![(CONTENT_TYPE, "application/json")], vec![(CONTENT_TYPE, "application/json")] ; "no headers to filter")]
    #[test_case(vec![(HOST, "example.com")], vec![] ; "filter out host header")]
    #[test_case(vec![(HOST, "example.com"), (CONTENT_TYPE, "application/json")], vec![(CONTENT_TYPE, "application/json")] ; "filter HOST header, retaining content-type")]
    #[test_case(vec![(HOST, "example.com"), (CONTENT_TYPE, "application/json"), (HeaderName::from_static("x-custom-header"), "value")], vec![(CONTENT_TYPE, "application/json")] ; "filter host and x-custom-header headers, retaining content-type")]
    #[test_case(vec![(HOST, "example.com"), (HeaderName::from_static("x-custom-header"), "value")], vec![] ; "filter host and x-custom-header headers")]
    fn test_filter_request_headers(
        headers: Vec<(HeaderName, &str)>,
        expected: Vec<(HeaderName, &str)>,
    ) {
        let headers = headers
            .into_iter()
            .map(|(k, v)| (k, HeaderValue::from_str(v).unwrap()))
            .collect();

        let expected_headers = expected
            .into_iter()
            // This header is always set
            .chain(vec![(HeaderName::from_static("accept"), RSS_ACCEPT)])
            .map(|(k, v)| (k, HeaderValue::from_str(v).unwrap()))
            .collect();

        let filtered_headers = filter_request_headers(headers);

        assert_eq!(filtered_headers, expected_headers);
    }
}
