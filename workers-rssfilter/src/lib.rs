use bytes::Bytes;
use http::{Method, Request, Response, StatusCode};
use http_body_util::Full;
use opentelemetry_http::HeaderExtractor;
use regex::Regex;
use rssfilter_telemetry::TracingError;
use std::{borrow::Cow, time::Duration};
use thiserror::Error;
use tracing::{debug, info, instrument};
use tracing_opentelemetry::OpenTelemetrySpanExt;
use url::{ParseError, Url};
use urlencoding::decode;
use uuid::Uuid;
use web_time::Instant;

use worker::{event, Body, Context, Env};

use filter_rss_feed::{FilterRegexes, RssError, RssFilter};

#[cfg(all(test, target_arch = "wasm32"))]
use filter_rss_feed::fake_http_client::FakeHttpClientBuilder;
use rssfilter_telemetry::WorkerConfig;

mod filter;
use filter::filter_request_headers;

mod http_status;
use http_status::*;

#[derive(Debug, Error)]
pub enum RequestValidationError {
    #[error("Not Found")]
    NotFound,
    #[error("Method Not Allowed")]
    MethodNotAllowed,
}

impl From<RequestValidationError> for Response<Bytes> {
    fn from(err: RequestValidationError) -> Response<Bytes> {
        let (status_code, message) = match err {
            RequestValidationError::NotFound => (*NOT_FOUND, "Not Found"),
            RequestValidationError::MethodNotAllowed => (*METHOD_NOT_ALLOWED, "Method Not Allowed"),
        };

        Response::builder()
            .status(status_code)
            .header("content-type", "text/plain")
            .body(Bytes::from(message))
            .unwrap()
    }
}

#[derive(Debug, Error)]
pub enum ValidationError {
    #[error("the parameter {name} could not be decoded: {source}")]
    MalformedParameter {
        name: &'static str,
        #[source]
        source: std::string::FromUtf8Error,
    },

    #[error("the regex for {name} is invalid: {source}")]
    InvalidRegex {
        name: &'static str,
        #[source]
        source: regex::Error,
    },

    #[error("A url and at least one of title_filter_regex, guid_filter_regex, or link_filter_regex must be provided")]
    NoParametersProvided,

    #[error("At least one of title_filter_regex, guid_filter_regex, or link_filter_regex must be provided")]
    NoFiltersProvided,

    #[error("The provided URL is malformed: {source}")]
    UrlParseError {
        #[source]
        source: ParseError,
    },

    #[error("A URL must be provided")]
    NoUrlProvided,
}

#[derive(Debug, Error)]
pub enum ProcessingError {
    #[error("RSS processing failed: {0}")]
    Rss(#[from] RssError),

    #[error("HTTP request building failed: {source}")]
    RequestBuild {
        #[source]
        source: http::Error,
    },
}

#[derive(Debug, Error)]
pub enum RssHandlerError {
    #[error("Request validation failed: {0}")]
    Validation(#[from] ValidationError),

    #[error("RSS processing failed: {0}")]
    Processing(#[from] ProcessingError),

    #[error("Worker error: {0}")]
    Worker(#[from] worker::Error),

    #[error("Tracing error: {0}")]
    Tracing(#[from] TracingError),
}

// Manual conversions for cases where we can't use #[from]
impl From<ParseError> for ValidationError {
    fn from(value: ParseError) -> Self {
        ValidationError::UrlParseError { source: value }
    }
}

impl From<http::Error> for ProcessingError {
    fn from(value: http::Error) -> Self {
        ProcessingError::RequestBuild { source: value }
    }
}

impl From<RssError> for RssHandlerError {
    fn from(value: RssError) -> Self {
        RssHandlerError::Processing(ProcessingError::Rss(value))
    }
}

impl From<&RssHandlerError> for Response<Bytes> {
    fn from(err: &RssHandlerError) -> Response<Bytes> {
        let message: Bytes = err.to_string().into();

        let status_code = match err {
            RssHandlerError::Processing(processing_err) => match processing_err {
                ProcessingError::RequestBuild { .. } => *BAD_GATEWAY,
                ProcessingError::Rss(rss_err) => match rss_err {
                    RssError::Http { .. } => *BAD_GATEWAY,
                    RssError::FeedTooLarge { .. } => *PAYLOAD_TOO_LARGE,
                    RssError::HttpClient { .. } => *BAD_GATEWAY,
                    RssError::InvalidContentType { .. } => *UNSUPPORTED_MEDIA_TYPE,
                    RssError::IO { .. } => *INTERNAL_SERVER_ERROR,
                    RssError::RSSParse { .. } => *BAD_REQUEST,
                    RssError::UTF8 { .. } => *INTERNAL_SERVER_ERROR,
                },
            },
            RssHandlerError::Tracing { .. } => *INTERNAL_SERVER_ERROR,
            RssHandlerError::Validation { .. } => *BAD_REQUEST,
            RssHandlerError::Worker { .. } => *INTERNAL_SERVER_ERROR,
        };

        Response::builder()
            .status(status_code)
            .header("Content-Type", "text/plain")
            .body(message)
            .unwrap()
    }
}

impl From<RssHandlerError> for Response<Bytes> {
    fn from(err: RssHandlerError) -> Response<Bytes> {
        (&err).into()
    }
}

struct RegexParams {
    title_regexes: Vec<Regex>,
    guid_regexes: Vec<Regex>,
    link_regexes: Vec<Regex>,
}

impl std::fmt::Debug for RegexParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let regexes_to_str = |regexes: &Vec<Regex>| {
            regexes
                .iter()
                .map(|r| r.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };

        write!(
            f,
            "title: [{}], guid: [{}], link: [{}]",
            regexes_to_str(&self.title_regexes),
            regexes_to_str(&self.guid_regexes),
            regexes_to_str(&self.link_regexes)
        )
    }
}

#[derive(Debug)]
pub struct Params<'a> {
    regex_params: RegexParams,
    url: Cow<'a, str>,
}

impl<'a> From<&'a RegexParams> for FilterRegexes<'a> {
    fn from(params: &'a RegexParams) -> Self {
        FilterRegexes {
            title_regexes: &params.title_regexes,
            guid_regexes: &params.guid_regexes,
            link_regexes: &params.link_regexes,
        }
    }
}

/// Validate request method and path
fn validate_request<T>(req: &Request<T>) -> Result<(), RequestValidationError> {
    let path = req.uri().path();

    if path != "/" {
        return Err(RequestValidationError::NotFound);
    }

    let method = req.method();

    if method != Method::GET {
        return Err(RequestValidationError::MethodNotAllowed);
    }

    Ok(())
}

/// Validate content type to ensure we're processing RSS/XML
/// Log request metrics for observability
fn log_request_metrics(url: &str, status: StatusCode, duration_ms: Duration) {
    info!(
        url = url,
        status = status.to_string(),
        duration_ms = duration_ms.as_millis(),
        "Request completed"
    );
}

#[instrument]
fn decode_and_compile_regex(url: &Url, key: &'static str) -> Result<Vec<Regex>, ValidationError> {
    url.query_pairs()
        .filter(|(k, _)| k == key)
        .map(|(_, value)| {
            let value_string = value.to_string();
            let decoded =
                decode(&value_string).map_err(|err| ValidationError::MalformedParameter {
                    name: key,
                    source: err,
                })?;
            Regex::new(&decoded).map_err(|err| ValidationError::InvalidRegex {
                name: key,
                source: err,
            })
        })
        .collect()
}

#[instrument]
fn validate_parameters(url: &Url) -> Result<Params, ValidationError> {
    let title_regexes = decode_and_compile_regex(url, "title_filter_regex")?;
    let guid_regexes = decode_and_compile_regex(url, "guid_filter_regex")?;
    let link_regexes = decode_and_compile_regex(url, "link_filter_regex")?;
    let feed_url = url
        .query_pairs()
        .find_map(|(k, v)| (k == "url").then_some(v));

    let any_filters_provided = [&title_regexes, &guid_regexes, &link_regexes]
        .iter()
        .any(|regexes| !regexes.is_empty());
    let url_provided = feed_url.is_some();

    match (any_filters_provided, url_provided) {
        (false, false) => return Err(ValidationError::NoParametersProvided),
        (false, true) => return Err(ValidationError::NoFiltersProvided),
        (true, false) => return Err(ValidationError::NoUrlProvided),
        _ => {}
    }

    Ok(Params {
        regex_params: RegexParams {
            title_regexes,
            guid_regexes,
            link_regexes,
        },
        url: feed_url.unwrap(),
    })
}

/// Handles the incoming request for the RSS filter. The query string parameters
/// are used to filter the RSS feed. Each item in the RSS feed is checked against
/// the provided regexes. If any one of the regex matches, the item is filtered
/// out.
///
/// The following query string parameters are supported:
/// - `title_filter_regex`: A regex to filter the title of the item.
/// - `guid_filter_regex`: A regex to filter the guid of the item.
/// - `link_filter_regex`: A regex to filter the link of the item.
///
/// At least one of `title_filter_regex`, `guid_filter_regex`, or
/// `link_filter_regex` must be provided. Each can be given multiple times.
///
/// The `url` query string parameter is required and is the URL of the RSS feed.
///
/// The response will be the filtered RSS feed.
///
/// # Example
/// Given the following RSS feed:
/// ```xml
/// <rss version="2.0">
///   <channel>
///     <title>Example Feed</title>
///     <link>http://example.com/</link>
///     <description>Example feed</description>
///     <item>
///       <title>Item 1</title>
///       <link>http://example.com/item1</link>
///       <guid>1</guid>
///     </item>
///     <item>
///       <title>Item 2</title>
///       <link>http://example.com/item2</link>
///       <guid>2</guid>
///     </item>
///   </channel>
/// </rss>
/// ```
///
/// and the following query string parameters:
/// - `title_filter_regex=Item 1`
/// - `url=http://example.com/rss`
///
/// The response will be:
/// ```xml
/// <rss version="2.0">
///   <channel>
///     <title>Example Feed</title>
///     <link>http://example.com/</link>
///     <description>Example feed</description>
///     <item>
///       <title>Item 2</title>
///       <link>http://example.com/item2</link>
///       <guid>2</guid>
///     </item>
///   </channel>
/// </rss>
/// ```
///
/// The `Item 1` item was filtered out because it matched the `title_filter_regex`.
#[instrument(skip(req), fields(request_id))]
async fn rss_handler(req: Request<Body>) -> Result<Response<Bytes>, RssHandlerError> {
    let start_time = Instant::now();

    let uri = req.uri();
    let url = uri.to_string().parse().map_err(ValidationError::from)?;
    let params = validate_parameters(&url)?;
    let feed_url = &params.url;

    let filter_regexes: FilterRegexes = (&params.regex_params).into();

    debug!(
        regexes = ?&params.regex_params,
        url = feed_url.as_ref(),
        "Filtering RSS feed"
    );

    let rss_filter = RssFilter::new(&filter_regexes)?;

    let headers = req.headers();

    let resp = rss_filter
        .fetch_and_filter_with_headers(feed_url, filter_request_headers(headers))
        .await?;

    let duration = start_time.elapsed();
    log_request_metrics(feed_url, resp.status(), duration);

    Ok(resp)
}

/// Performs one-time initialisation of OpenTelemetry tracing subscriber. This sets up a global, so
/// it can't be called multiple times.
fn initialise_otel_with_config(config: &WorkerConfig) -> &'static Result<(), RssHandlerError> {
    use std::sync::OnceLock;

    use rssfilter_telemetry::init_default_subscriber;

    static INIT_SUBSCRIBER: OnceLock<Result<(), RssHandlerError>> = OnceLock::new();

    let initialisation_result = INIT_SUBSCRIBER.get_or_init(|| {
        let _tracer_provider = init_default_subscriber(config.clone())?;

        debug!("Initialised tracing subscriber with worker environment variables");

        Ok(())
    });

    initialisation_result
}

pub async fn real_main(req: Request<Body>, config: WorkerConfig) -> Response<Bytes> {
    console_error_panic_hook::set_once();

    // Check the stored result and return early if it failed
    if let Err(err) = initialise_otel_with_config(&config) {
        return err.into();
    };
    use rssfilter_telemetry::extract_context_from_headers;

    let request_id = Uuid::new_v4().to_string();

    let parent_ctx = extract_context_from_headers(HeaderExtractor(req.headers()));

    // Add request ID to tracing span
    let span = tracing::info_span!("request", request_id = %request_id);
    span.set_parent(parent_ctx);
    let _enter = span.enter();

    // Validate request early
    if let Err(validation_error) = validate_request(&req) {
        return validation_error.into();
    }

    rss_handler(req).await.unwrap_or_else(|err| {
        info!(
          err = %err,
          "Error processing request",
        );

        err.into()
    })
}

/// Main entry point for the RSS filter worker.
///
/// Accepts GET requests to "/" with query parameters:
/// - `url`: The RSS feed URL to filter (required)
/// - `title_filter_regex`: Regex to filter items by title (at least one filter required)
/// - `guid_filter_regex`: Regex to filter items by GUID (at least one filter required)
/// - `link_filter_regex`: Regex to filter items by link (at least one filter required)
///
/// Returns:
/// - 200: Filtered RSS feed
/// - 400: Invalid parameters or malformed request
/// - 404: Wrong path (not "/")
/// - 405: Wrong HTTP method (not GET)
/// - 413: RSS feed too large
/// - 415: Invalid content type (not RSS/XML)
/// - 422: Error processing the RSS feed
/// - 502: Error fetching the upstream RSS feed
#[event(fetch)]
async fn main(
    req: Request<Body>,
    env: Env,
    _ctx: Context,
) -> worker::Result<Response<Full<Bytes>>> {
    let config = WorkerConfig {
        log_format: env.var("LOG_FORMAT").ok().map(|s| s.to_string()),
        rust_log: env.var("RUST_LOG").ok().map(|s| s.to_string()),
    };
    Ok(real_main(req, config).await.map(Full::new))
}

// Integration tests that require mockito (non-WASM only)
#[cfg(all(test, not(target_arch = "wasm32")))]
mod integration_tests {
    use super::*;

    use ctor::ctor;
    use filter_rss_feed::{FilterRegexes, RssFilter};
    use matches::assert_matches;
    use std::sync::LazyLock;
    use test_utils::{feed::serve_test_rss_feed, test_request_builder};

    static TEMPORARY_REDIRECT: LazyLock<u16> =
        LazyLock::new(|| StatusCode::TEMPORARY_REDIRECT.as_u16());

    fn contains_string(data: &Bytes, needle: &str) -> bool {
        data.as_ref()
            .windows(needle.len())
            .any(|window| window == needle.as_bytes())
    }

    #[ctor]
    fn init_tracing() {
        initialise_otel_with_config(&WorkerConfig::default());
    }

    #[tokio::test]
    async fn test_parameter_validation_no_params() {
        let url = "https://test.example.com/".parse().unwrap();
        let result = validate_parameters(&url);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ValidationError::NoParametersProvided
        ));
    }

    #[tokio::test]
    async fn test_parameter_validation_no_url() {
        let url = "https://test.example.com/?title_filter_regex=test"
            .parse()
            .unwrap();
        let result = validate_parameters(&url);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ValidationError::NoUrlProvided
        ));
    }

    #[tokio::test]
    async fn test_parameter_validation_no_filters() {
        let url = "https://test.example.com/?url=http://example.com/rss"
            .parse()
            .unwrap();
        let result = validate_parameters(&url);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ValidationError::NoFiltersProvided
        ));
    }

    #[tokio::test]
    async fn test_parameter_validation_invalid_regex() {
        let url =
            "https://test.example.com/?url=http://example.com/rss&title_filter_regex=[invalid"
                .parse()
                .unwrap();
        let result = validate_parameters(&url);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ValidationError::InvalidRegex { .. }
        ));
    }

    #[tokio::test]
    async fn test_parameter_validation_success() {
        let url = "https://test.example.com/?url=http://example.com/rss&title_filter_regex=test"
            .parse()
            .unwrap();
        let result = validate_parameters(&url);
        assert!(result.is_ok());
        let params = result.unwrap();
        assert_eq!(params.url, "http://example.com/rss");
        assert_eq!(params.regex_params.title_regexes.len(), 1);
    }

    #[tokio::test]
    async fn test_parameter_validation_multiple_regexes() {
        let url = "https://test.example.com/?url=http://example.com/rss&title_filter_regex=test1&title_filter_regex=test2&guid_filter_regex=guid".parse().unwrap();
        let result = validate_parameters(&url);
        assert!(result.is_ok());
        let params = result.unwrap();
        assert_eq!(params.regex_params.title_regexes.len(), 2);
        assert_eq!(params.regex_params.guid_regexes.len(), 1);
        assert_eq!(params.regex_params.link_regexes.len(), 0);
    }

    #[tokio::test]
    async fn test_rss_filtering_basic() {
        let server = serve_test_rss_feed(&["1", "2"]).await.unwrap();
        let url = server.url();

        let title_regex = Regex::new("Test Item 1").unwrap();
        let filter_regexes = FilterRegexes {
            title_regexes: &[title_regex],
            guid_regexes: &[],
            link_regexes: &[],
        };

        let rss_filter = RssFilter::new(&filter_regexes).expect("Failed to create RSS filter");
        let response = rss_filter.fetch(&url, Default::default()).await.unwrap();
        let body = rss_filter.filter_response(response).await.unwrap();

        // Should filter out item 1, keep item 2
        assert!(!contains_string(&body, "Item 1"));
        assert!(contains_string(&body, "Item 2"));
    }

    #[tokio::test]
    async fn test_rss_filtering_guid() {
        let server = serve_test_rss_feed(&["1", "2", "3"]).await.unwrap();
        let url = server.url();

        let guid_regex = Regex::new("^2$").unwrap();
        let filter_regexes = FilterRegexes {
            title_regexes: &[],
            guid_regexes: &[guid_regex],
            link_regexes: &[],
        };

        let rss_filter = RssFilter::new(&filter_regexes).expect("Failed to create RSS filter");
        let response = rss_filter.fetch(&url, Default::default()).await.unwrap();
        let body = rss_filter.filter_response(response).await.unwrap();

        // Should filter out item 2, keep items 1 and 3
        assert!(contains_string(&body, "Item 1"));
        assert!(!contains_string(&body, "Item 2"));
        assert!(contains_string(&body, "Item 3"));
    }

    #[tokio::test]
    async fn test_rss_filtering_link() {
        let server = serve_test_rss_feed(&["1", "2"]).await.unwrap();
        let url = server.url();

        let link_regex = Regex::new("test1").unwrap();
        let filter_regexes = FilterRegexes {
            title_regexes: &[],
            guid_regexes: &[],
            link_regexes: &[link_regex],
        };

        let rss_filter = RssFilter::new(&filter_regexes).expect("Failed to create RSS filter");
        let response = rss_filter.fetch(&url, Default::default()).await.unwrap();
        let body = rss_filter.filter_response(response).await.unwrap();

        // Should filter out item 1 (link contains "test1"), keep item 2
        assert!(!contains_string(&body, "Item 1"));
        assert!(contains_string(&body, "Item 2"));
    }

    #[tokio::test]
    async fn test_http_error_handling() {
        // Test with a URL that will return an error
        let title_regex = Regex::new("test").unwrap();
        let filter_regexes = FilterRegexes {
            title_regexes: &[title_regex],
            guid_regexes: &[],
            link_regexes: &[],
        };

        let rss_filter = RssFilter::new(&filter_regexes).expect("Failed to create RSS filter");
        let result = rss_filter
            .fetch("http://localhost:99999/nonexistent", Default::default())
            .await;

        // Should get a network error
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_url_encoding_in_parameters() {
        // Test URL with encoded parameters
        let url =
            "https://test.example.com/?url=http%3A//example.com/rss&title_filter_regex=Test%20Item"
                .parse()
                .unwrap();
        let result = validate_parameters(&url);
        assert!(result.is_ok());
        let params = result.unwrap();
        assert_eq!(params.url, "http://example.com/rss");
        assert_eq!(params.regex_params.title_regexes[0].as_str(), "Test Item");
    }

    #[tokio::test]
    async fn test_empty_regex_matches() {
        let server = serve_test_rss_feed(&["1", "2"]).await.unwrap();
        let url = server.url();

        // Test regex that matches everything
        let title_regex = Regex::new(".*").unwrap();
        let filter_regexes = FilterRegexes {
            title_regexes: &[title_regex],
            guid_regexes: &[],
            link_regexes: &[],
        };

        let rss_filter = RssFilter::new(&filter_regexes).expect("Failed to create RSS filter");
        let response = rss_filter.fetch(&url, Default::default()).await.unwrap();
        let body = rss_filter.filter_response(response).await.unwrap();

        // Should filter out all items since regex matches everything
        assert!(!contains_string(&body, "Item 1"));
        assert!(!contains_string(&body, "Item 2"));
    }

    #[tokio::test]
    async fn test_regex_no_matches() {
        let server = serve_test_rss_feed(&["1", "2"]).await.unwrap();
        let url = server.url();

        let title_regex = Regex::new("^nonexistent$").unwrap();
        let filter_regexes = FilterRegexes {
            title_regexes: &[title_regex],
            guid_regexes: &[],
            link_regexes: &[],
        };

        let rss_filter = RssFilter::new(&filter_regexes).expect("Failed to create RSS filter");
        let response = rss_filter.fetch(&url, Default::default()).await.unwrap();
        let body = rss_filter.filter_response(response).await.unwrap();

        // Should keep all items since regex matches nothing
        assert!(contains_string(&body, "Item 1"));
        assert!(contains_string(&body, "Item 2"));
    }

    #[tokio::test]
    async fn test_mixed_filter_types() {
        let server = serve_test_rss_feed(&["1", "2", "3"]).await.unwrap();
        let url = server.url();

        // Mix of title and guid filters
        let title_regex = Regex::new("Test Item 1").unwrap();
        let guid_regex = Regex::new("3").unwrap();
        let filter_regexes = FilterRegexes {
            title_regexes: &[title_regex],
            guid_regexes: &[guid_regex],
            link_regexes: &[],
        };

        let rss_filter = RssFilter::new(&filter_regexes).expect("Failed to create RSS filter");
        let response = rss_filter.fetch(&url, Default::default()).await.unwrap();
        let body = rss_filter.filter_response(response).await.unwrap();

        // Should filter out items 1 and 3, keep item 2
        assert!(!contains_string(&body, "Item 1"));
        assert!(contains_string(&body, "Item 2"));
        assert!(!contains_string(&body, "Item 3"));
    }

    #[tokio::test]
    async fn test_filter_link_multiple() {
        let server = serve_test_rss_feed(&["1", "2", "3"]).await.unwrap();
        let url = server.url();

        let link_regex1 = Regex::new("test1").unwrap();
        let link_regex2 = Regex::new("test2").unwrap();
        let filter_regexes = FilterRegexes {
            title_regexes: &[],
            guid_regexes: &[],
            link_regexes: &[link_regex1, link_regex2],
        };

        let rss_filter = RssFilter::new(&filter_regexes).expect("Failed to create RSS filter");
        let response = rss_filter.fetch(&url, Default::default()).await.unwrap();
        let body = rss_filter.filter_response(response).await.unwrap();

        let body_str = std::str::from_utf8(&body).unwrap();

        assert!(!body_str.contains("Item 1"));
        assert!(!body_str.contains("Item 2"));
        assert!(body_str.contains("Item 3"));
    }

    #[tokio::test]
    async fn test_404() {
        use http::{Method, Request};
        use worker::Body;

        let req = Request::builder()
            .method(Method::GET)
            .uri("https://test.example.com/favicon.ico")
            .body(Body::empty())
            .unwrap();

        let error = validate_request(&req).expect_err("Expected request validation to fail");
        assert_matches!(error, RequestValidationError::NotFound);
    }

    #[tokio::test]
    async fn test_header_passthrough() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/")
            .with_status(*TEMPORARY_REDIRECT as usize)
            .with_header("my-test-header", "value")
            .create_async()
            .await;

        let url = server.url();

        let mut headers = http::HeaderMap::new();
        headers.insert("my-test-header", "value".parse().unwrap());

        let request = test_request_builder::RequestBuilder::new()
            .with_method(Method::GET)
            .with_feed_url(&url)
            .with_title_filter_regex(".*")
            .build()
            .expect("Failed to build request");

        let response = real_main(request, WorkerConfig::default()).await;

        assert_eq!(response.status().as_u16(), *TEMPORARY_REDIRECT);
        let headers = response.headers();
        assert_eq!(headers.get("my-test-header").unwrap(), "value",);
    }
}

#[cfg(all(test, target_arch = "wasm32"))]
mod wasm_tests {
    use super::*;

    use http::{Method, Request};
    use matches::assert_matches;
    use test_utils::test_request_builder::RequestBuilder;
    use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};
    use worker::Body;

    wasm_bindgen_test_configure!(run_in_node_experimental);

    #[wasm_bindgen_test]
    async fn test_no_query_params() {
        let req = RequestBuilder::new().build().unwrap();

        let res = rss_handler(req).await;

        assert!(res.is_err());
        let res_err = res.unwrap_err();
        assert_matches!(
            res_err,
            RssHandlerError::Validation(ValidationError::NoParametersProvided)
        );
    }

    #[wasm_bindgen_test]
    async fn test_no_url_param() {
        let req = RequestBuilder::new()
            .with_title_filter_regex(".*")
            .build()
            .unwrap();

        let res = rss_handler(req).await;

        assert!(res.is_err());
        let res_err = res.unwrap_err();
        assert_matches!(
            res_err,
            RssHandlerError::Validation(ValidationError::NoUrlProvided)
        );
    }

    #[wasm_bindgen_test]
    async fn test_no_filters() {
        let req = RequestBuilder::new()
            .with_feed_url("http://example.com/rss")
            .build()
            .unwrap();

        let res = rss_handler(req).await;

        assert!(res.is_err());
        let res_err = res.unwrap_err();
        assert_matches!(
            res_err,
            RssHandlerError::Validation(ValidationError::NoFiltersProvided)
        );
    }

    #[wasm_bindgen_test]
    async fn test_invalid_regex() {
        let req = RequestBuilder::new()
            .with_feed_url("http://example.com/rss")
            .with_title_filter_regex("[invalid regex") // Invalid regex
            .build()
            .unwrap();

        let res = rss_handler(req).await;
        assert!(res.is_err());

        let rss_error = res.unwrap_err();
        assert_matches!(
            rss_error,
            RssHandlerError::Validation(ValidationError::InvalidRegex { .. })
        );

        let response: Response<Bytes> = rss_error.into();
        assert_eq!(response.status().as_u16(), *BAD_REQUEST);
    }

    #[wasm_bindgen_test]
    async fn test_malformed_url_encoding() {
        // Create a request with invalid UTF-8 in URL encoding
        let url_str = "https://test.example.com/?url=http://example.com&title_filter_regex=%FF%FE";
        let req = Request::builder()
            .method(Method::GET)
            .uri(url_str)
            .body(Body::empty())
            .unwrap();

        let res = rss_handler(req).await;
        assert!(res.is_err());

        let rss_error = res.unwrap_err();
        // The error might be different depending on how the URL is parsed and what response we get
        // It could be MalformedParameter, InvalidRegex, InvalidContentType, or a network error
        let is_expected_error = matches!(
            rss_error,
            RssHandlerError::Validation(ValidationError::MalformedParameter { .. })
                | RssHandlerError::Validation(ValidationError::InvalidRegex { .. })
                | RssHandlerError::Processing(ProcessingError::Rss(RssError::HttpClient { .. }))
                | RssHandlerError::Processing(ProcessingError::Rss(
                    RssError::InvalidContentType { .. }
                ))
        );
        assert!(
            is_expected_error,
            "Expected a parameter-related or content error, got: {rss_error:?}"
        );

        let response: Response<Bytes> = rss_error.into();
        // The status code could be 400, 415, or 502 depending on the specific error
        assert!(
            response.status().as_u16() == *BAD_REQUEST
                || response.status().as_u16() == *BAD_GATEWAY
                || response.status().as_u16() == *UNSUPPORTED_MEDIA_TYPE
        );
    }

    #[wasm_bindgen_test]
    async fn test_multiple_regex_parameters() {
        let fake_client = FakeHttpClientBuilder::default()
            .with_json_response("https://example.com/json", r#"{"key": "value"}"#)
            .build()
            .expect("Failed to build fake client");

        let title_regex1 = Regex::new(".*1.*").expect("Invalid regex");
        let title_regex2 = Regex::new(".*2.*").expect("Invalid regex");
        let guid_regex = Regex::new("test").expect("Invalid regex");

        let filter_regexes = FilterRegexes {
            title_regexes: &[title_regex1, title_regex2],
            guid_regexes: &[guid_regex],
            link_regexes: &[],
        };

        let rss_filter = RssFilter::new_with_http_client(&filter_regexes, Box::new(fake_client));
        let result = rss_filter
            .fetch_and_filter("https://example.com/json")
            .await;

        assert!(result.is_err());
        let error = result.unwrap_err();

        assert_matches!(error, RssError::InvalidContentType { .. });
    }

    #[wasm_bindgen_test]
    async fn test_parse_error() {
        let fake_client = FakeHttpClientBuilder::default()
            .with_xml_response(
                "https://example.com/xml",
                "<root><item>not rss</item></root>",
            )
            .build()
            .expect("Failed to build fake client");

        let title_regex = Regex::new(".*1.*").expect("Invalid regex");
        let guid_regex = Regex::new("test").expect("Invalid regex");

        let filter_regexes = FilterRegexes {
            title_regexes: &[title_regex],
            guid_regexes: &[guid_regex],
            link_regexes: &[],
        };

        let rss_filter = RssFilter::new_with_http_client(&filter_regexes, Box::new(fake_client));
        let result = rss_filter.fetch_and_filter("https://example.com/xml").await;

        assert!(result.is_err());
        let error = result.unwrap_err();

        assert_matches!(error, RssError::RSSParse { .. });
    }

    #[wasm_bindgen_test]
    async fn test_error_status_mapping_bad_request() {
        let req = RequestBuilder::new().build().unwrap();

        let res = rss_handler(req).await;
        assert!(res.is_err());

        let rss_error = res.unwrap_err();
        let response: Response<Bytes> = rss_error.into();

        assert_eq!(response.status().as_u16(), *BAD_REQUEST);
    }

    #[wasm_bindgen_test]
    async fn test_error_status_mapping_bad_gateway() {
        let req = RequestBuilder::new()
            // 99999 is not a valid port
            .with_feed_url("http://localhost:99999")
            .with_title_filter_regex(".*")
            .build()
            .unwrap();

        let res = rss_handler(req).await;
        assert!(res.is_err());

        let rss_error = res.unwrap_err();
        let response: Response<Bytes> = rss_error.into();

        assert_eq!(response.status().as_u16(), *BAD_GATEWAY);
    }

    #[wasm_bindgen_test]
    async fn test_main_wrong_path() {
        let req = RequestBuilder::new().with_path("/wrong").build().unwrap();

        let result = real_main(req, WorkerConfig::default()).await;
        assert_eq!(result.status().as_u16(), *NOT_FOUND);
    }

    #[wasm_bindgen_test]
    async fn test_main_wrong_method() {
        let req = RequestBuilder::new()
            .with_method(Method::POST)
            .build()
            .unwrap();

        let result = real_main(req, WorkerConfig::default()).await;
        assert_eq!(result.status().as_u16(), *METHOD_NOT_ALLOWED);
    }

    #[wasm_bindgen_test]
    async fn test_main_no_params() {
        let req = RequestBuilder::new().build().unwrap();

        let result = real_main(req, WorkerConfig::default()).await;
        assert_eq!(result.status().as_u16(), *BAD_REQUEST);
    }

    #[wasm_bindgen_test]
    async fn test_validate_request_function() {
        let valid_req = RequestBuilder::new().build().unwrap();
        assert!(validate_request(&valid_req).is_ok());

        let wrong_path = RequestBuilder::new().with_path("/wrong").build().unwrap();
        let err = validate_request(&wrong_path).unwrap_err();
        assert_matches!(err, RequestValidationError::NotFound);

        let wrong_method = RequestBuilder::new()
            .with_method(Method::POST)
            .build()
            .unwrap();

        let err = validate_request(&wrong_method).expect_err("Expected method validation to fail");
        assert_matches!(err, RequestValidationError::MethodNotAllowed);
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod request_validation_integration_tests {
    use super::*;
    use headers::{ContentType, HeaderMapExt};
    use http::{Method, Request};
    use test_case::test_case;
    use worker::Body;

    #[tokio::test]
    async fn test_validate_request_not_found_integration() {
        let req = Request::builder()
            .method(Method::GET)
            .uri("https://test.example.com/nonexistent")
            .body(Body::empty())
            .unwrap();

        let response = real_main(req, WorkerConfig::default()).await;
        assert_eq!(response.status().as_u16(), *NOT_FOUND);

        let body = response.into_body();
        let body_str = std::str::from_utf8(&body).unwrap();
        assert_eq!(body_str, "Not Found");
    }

    #[test_case(Method::POST; "post method")]
    #[test_case(Method::PUT; "put method")]
    #[test_case(Method::DELETE; "delete method")]
    #[test_case(Method::PATCH; "patch method")]
    #[test_case(Method::HEAD; "head method")]
    #[test_case(Method::OPTIONS; "options method")]
    #[tokio::test]
    async fn test_validate_request_method_not_allowed(method: Method) {
        let req = Request::builder()
            .method(method)
            .uri("https://test.example.com/")
            .body(Body::empty())
            .unwrap();

        let response = real_main(req, WorkerConfig::default()).await;
        assert_eq!(response.status().as_u16(), *METHOD_NOT_ALLOWED);
    }

    #[test_case("/favicon.ico"; "favicon")]
    #[test_case("/robots.txt"; "robots")]
    #[test_case("/api/v1/something"; "api endpoint")]
    #[test_case("/health"; "health check")]
    #[test_case("/status"; "status check")]
    #[test_case("/.well-known/something"; "well known")]
    #[tokio::test]
    async fn test_validate_request_various_wrong_paths(path: &str) {
        let req = Request::builder()
            .method(Method::GET)
            .uri(format!("https://test.example.com{path}"))
            .body(Body::empty())
            .unwrap();

        let response = real_main(req, WorkerConfig::default()).await;
        assert_eq!(
            response.status().as_u16(),
            *NOT_FOUND,
            "Expected 404 for path: {path}"
        );
    }

    #[tokio::test]
    async fn test_validate_request_content_type_header() {
        // Test that content-type is set correctly for validation errors
        let req = Request::builder()
            .method(Method::POST)
            .uri("https://test.example.com/")
            .body(Body::empty())
            .unwrap();

        let response = real_main(req, WorkerConfig::default()).await;
        assert_eq!(response.status().as_u16(), *METHOD_NOT_ALLOWED);

        let content_type = response
            .headers()
            .typed_get::<headers::ContentType>()
            .expect("Content-Type header should be present");
        assert_eq!(content_type, ContentType::text());
    }

    #[tokio::test]
    async fn test_validate_request_successful_validation() {
        // Test that a valid request passes validation and reaches parameter validation
        let req = Request::builder()
            .method(Method::GET)
            .uri("https://test.example.com/")
            .body(Body::empty())
            .unwrap();

        let response = real_main(req, WorkerConfig::default()).await;
        // Should get 400 for missing parameters, not 404/405 for validation
        assert_eq!(response.status().as_u16(), *BAD_REQUEST);
    }
}

// Only test Response conversion on WASM where worker types are available
#[cfg(all(test, target_arch = "wasm32"))]
mod error_conversion_tests {
    use super::*;

    #[test]
    fn test_error_conversion() {
        let error = RssHandlerError::Validation(ValidationError::NoParametersProvided);
        let response: Response<Bytes> = error.into();
        assert_eq!(response.status().as_u16(), *BAD_REQUEST);

        let error = RssHandlerError::Processing(ProcessingError::Rss(RssError::FeedTooLarge {
            max_size: 1024 * 1024, // 1MB example
        }));
        let response: Response<Bytes> = error.into();
        assert_eq!(response.status().as_u16(), *PAYLOAD_TOO_LARGE);

        let error =
            RssHandlerError::Processing(ProcessingError::Rss(RssError::InvalidContentType {
                content_type: "text/html".to_string(),
            }));
        let response: Response<Bytes> = error.into();
        assert_eq!(response.status().as_u16(), *UNSUPPORTED_MEDIA_TYPE);
    }

    #[test]
    fn test_request_validation_error_conversion() {
        let error = RequestValidationError::NotFound;
        let response: Response<Bytes> = error.into();
        assert_eq!(response.status().as_u16(), *NOT_FOUND);

        let body = response.into_body();
        let body_str = std::str::from_utf8(&body).unwrap();
        assert_eq!(body_str, "Not Found");

        let error = RequestValidationError::MethodNotAllowed;
        let response: Response<Bytes> = error.into();
        assert_eq!(response.status().as_u16(), *METHOD_NOT_ALLOWED);

        let body = response.into_body();
        let body_str = std::str::from_utf8(&body).unwrap();
        assert_eq!(body_str, "Method Not Allowed");
    }

    #[test]
    fn test_worker_config_default() {
        let config = WorkerConfig::default();
        assert_eq!(config.log_format, None);
        assert_eq!(config.rust_log, None);
    }

    #[test]
    fn test_worker_config_debug_clone() {
        let config = WorkerConfig {
            log_format: Some("json".to_string()),
            rust_log: Some("debug".to_string()),
        };

        let cloned = config.clone();
        assert_eq!(config.log_format, cloned.log_format);
        assert_eq!(config.rust_log, cloned.rust_log);

        // Verify Debug trait works
        let debug_str = format!("{config:?}");
        assert!(debug_str.contains("json"));
        assert!(debug_str.contains("debug"));
    }
}
