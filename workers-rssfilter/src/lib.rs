use bytes::Bytes;
use http::{Method, Request, Response, StatusCode};
use http_body_util::{BodyExt, Full};
use opentelemetry_http::HeaderExtractor;
use regex::Regex;
use rssfilter_telemetry::TracingError;
use std::borrow::Cow;
use std::time::Duration;
use thiserror::Error;
use tracing::{debug, error, info, instrument};
use tracing_opentelemetry::OpenTelemetrySpanExt;
use url::{ParseError, Url};
use urlencoding::decode;
use uuid::Uuid;
use web_time::Instant;

use worker::{Body, Context, Env, HttpResponse, event};

use filter_rss_feed::{FilterRegexes, RssError, RssFilter};
use rssfilter_core::{
    GUID_FILTER_REGEX_PARAM, LINK_FILTER_REGEX_PARAM, TITLE_FILTER_REGEX_PARAM, URL_PARAM,
};

use rssfilter_telemetry::WorkerConfig;

mod filter;
use filter::filter_request_headers;

mod http_status;
use http_status::*;

/// `Cache-Control` applied to `/api/feed`, letting browsers (and intermediate
/// caches) reuse the items JSON for this long.
const API_FEED_CACHE_CONTROL: &str = "public, max-age=300";

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

    #[error(
        "A url and at least one of title_filter_regex, guid_filter_regex, or link_filter_regex must be provided"
    )]
    NoParametersProvided,

    #[error(
        "At least one of title_filter_regex, guid_filter_regex, or link_filter_regex must be provided"
    )]
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

#[derive(Debug)]
pub struct Params<'a> {
    filter_regexes: FilterRegexes,
    url: Cow<'a, str>,
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
fn validate_parameters(url: &Url) -> Result<Params<'_>, ValidationError> {
    let title_regexes = decode_and_compile_regex(url, TITLE_FILTER_REGEX_PARAM)?;
    let guid_regexes = decode_and_compile_regex(url, GUID_FILTER_REGEX_PARAM)?;
    let link_regexes = decode_and_compile_regex(url, LINK_FILTER_REGEX_PARAM)?;
    let feed_url = url
        .query_pairs()
        .find_map(|(k, v)| (k == URL_PARAM).then_some(v));

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
        filter_regexes: FilterRegexes {
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
    let filter_regexes = &params.filter_regexes;

    debug!(
        regexes = ?filter_regexes,
        url = feed_url.as_ref(),
        "Filtering RSS feed"
    );

    let rss_filter = RssFilter::new(filter_regexes)?;

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
    if let Err(err) = span.set_parent(parent_ctx) {
        // TODO: move to our `From` once
        // https://github.com/tokio-rs/tracing-opentelemetry/issues/236 is solved.
        return Response::builder()
            .status(*INTERNAL_SERVER_ERROR)
            .body(err.to_string().into())
            .unwrap();
    };

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

/// Returns the `url` query parameter, or a `NoUrlProvided` error.
fn feed_url_param(uri: &http::Uri) -> Result<String, ValidationError> {
    uri.query()
        .and_then(|q| {
            url::form_urlencoded::parse(q.as_bytes())
                .find_map(|(k, v)| (k == URL_PARAM).then(|| v.into_owned()))
        })
        .ok_or(ValidationError::NoUrlProvided)
}

/// `GET /api/feed?url=…` — fetch the feed and return its items as JSON, so the
/// browser preview can apply filters locally. Unfiltered: the preview decides
/// what to hide.
async fn api_feed(req: Request<Body>) -> Result<Response<Bytes>, RssHandlerError> {
    let feed_url = feed_url_param(req.uri())?;

    let no_filters = FilterRegexes::default();
    let rss_filter = RssFilter::new(&no_filters)?;
    let response = rss_filter
        .fetch(&feed_url, filter_request_headers(req.headers()))
        .await?;

    if !response.status().is_success() {
        // Surface the upstream status, matching how the filter endpoint passes
        // upstream failures through.
        return Ok(Response::builder()
            .status(response.status())
            .header("content-type", "text/plain")
            .body(Bytes::from_static(b"upstream feed returned an error"))
            .unwrap());
    }

    // Validates the content-type (415 for non-RSS) before parsing, matching the
    // filter endpoint so the preview agrees with what the worker serves.
    let items = filter_rss_feed::parse_feed_items(&response)?;
    let body = match serde_json::to_vec(&items) {
        Ok(body) => body,
        Err(err) => {
            error!(?err, "failed to serialise feed items");
            return Ok(Response::builder()
                .status(*INTERNAL_SERVER_ERROR)
                .header("content-type", "text/plain")
                .body(Bytes::from_static(b"failed to serialise feed items"))
                .unwrap());
        }
    };

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json; charset=utf-8")
        .header("cache-control", API_FEED_CACHE_CONTROL)
        .body(Bytes::from(body))
        .unwrap())
}

/// Serve a static asset (the SPA) for the given request via the `ASSETS`
/// binding, collected into the `http`-typed response the handler returns.
async fn serve_asset(req: Request<Body>, env: &Env) -> worker::Result<Response<Full<Bytes>>> {
    let response: HttpResponse = env.assets("ASSETS")?.fetch_request(req).await?;
    let (parts, body) = response.into_parts();
    let bytes = body.collect().await?.to_bytes();
    Ok(Response::from_parts(parts, Full::new(bytes)))
}

/// Main entry point for the RSS filter worker.
///
/// Routing:
/// - `GET /` (no query) → the single-page app, served from static assets.
/// - `GET /?url=…&*_filter_regex=…` → the filtered RSS feed (feed-reader URL).
/// - `GET /api/feed?url=…` → the feed's items as JSON for the live preview.
/// - anything else → 404 / 405.
///
/// `/api/feed`'s upstream fetch is handled by the worker `fetch` integration,
/// which Cloudflare subrequest-caches according to the upstream's headers; our
/// response also carries `Cache-Control` for the browser. The filter endpoint
/// works the same way, so the two endpoints share their caching behaviour.
///
/// Status codes for the filter/API paths: 400 invalid params, 404 wrong path,
/// 405 wrong method, 413 feed too large, 415 bad content type, 422 processing
/// error, 502 upstream fetch error.
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

    let path = req.uri().path().to_owned();

    if path == "/api/feed" {
        if req.method() != Method::GET {
            return Ok(Response::from(RequestValidationError::MethodNotAllowed).map(Full::new));
        }

        console_error_panic_hook::set_once();
        if let Err(err) = initialise_otel_with_config(&config) {
            return Ok(Response::<Bytes>::from(err).map(Full::new));
        }

        let response = api_feed(req).await.unwrap_or_else(|err| {
            info!(err = %err, "Error serving /api/feed");
            (&err).into()
        });
        return Ok(response.map(Full::new));
    }

    // The SPA: a bare `GET /` is served from static assets.
    if req.method() == Method::GET
        && path == "/"
        && req.uri().query().filter(|q| !q.is_empty()).is_none()
    {
        return serve_asset(req, &env).await;
    }

    // `/?url=…` is the filter endpoint; other paths/methods get 404/405.
    Ok(real_main(req, config).await.map(Full::new))
}

// Integration tests that require mockito (non-WASM only)
#[cfg(all(test, not(target_arch = "wasm32")))]
mod integration_tests {
    use super::*;

    use filter_rss_feed::{FilterRegexes, RssFilter};
    use matches::assert_matches;
    use std::sync::LazyLock;
    use test_utils::feed::serve_test_rss_feed;
    use test_utils::test_request_builder;

    static TEMPORARY_REDIRECT: LazyLock<u16> =
        LazyLock::new(|| StatusCode::TEMPORARY_REDIRECT.as_u16());

    fn contains_string(data: &Bytes, needle: &str) -> bool {
        data.as_ref()
            .windows(needle.len())
            .any(|window| window == needle.as_bytes())
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
        assert_eq!(params.filter_regexes.title_regexes.len(), 1);
    }

    #[tokio::test]
    async fn test_parameter_validation_multiple_regexes() {
        let url = "https://test.example.com/?url=http://example.com/rss&title_filter_regex=test1&title_filter_regex=test2&guid_filter_regex=guid".parse().unwrap();
        let result = validate_parameters(&url);
        assert!(result.is_ok());
        let params = result.unwrap();
        assert_eq!(params.filter_regexes.title_regexes.len(), 2);
        assert_eq!(params.filter_regexes.guid_regexes.len(), 1);
        assert_eq!(params.filter_regexes.link_regexes.len(), 0);
    }

    #[tokio::test]
    async fn test_rss_filtering_basic() {
        let server = serve_test_rss_feed(&["1", "2"]).await.unwrap();
        let url = server.url();

        let title_regex = Regex::new("Test Item 1").unwrap();
        let filter_regexes = FilterRegexes {
            title_regexes: vec![title_regex],
            guid_regexes: vec![],
            link_regexes: vec![],
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
            title_regexes: vec![],
            guid_regexes: vec![guid_regex],
            link_regexes: vec![],
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
            title_regexes: vec![],
            guid_regexes: vec![],
            link_regexes: vec![link_regex],
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
            title_regexes: vec![title_regex],
            guid_regexes: vec![],
            link_regexes: vec![],
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
        assert_eq!(params.filter_regexes.title_regexes[0].as_str(), "Test Item");
    }

    #[tokio::test]
    async fn test_empty_regex_matches() {
        let server = serve_test_rss_feed(&["1", "2"]).await.unwrap();
        let url = server.url();

        // Test regex that matches everything
        let title_regex = Regex::new(".*").unwrap();
        let filter_regexes = FilterRegexes {
            title_regexes: vec![title_regex],
            guid_regexes: vec![],
            link_regexes: vec![],
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
            title_regexes: vec![title_regex],
            guid_regexes: vec![],
            link_regexes: vec![],
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
            title_regexes: vec![title_regex],
            guid_regexes: vec![guid_regex],
            link_regexes: vec![],
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
            title_regexes: vec![],
            guid_regexes: vec![],
            link_regexes: vec![link_regex1, link_regex2],
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

    fn api_feed_request(feed_url: &str) -> Request<Body> {
        Request::builder()
            .method(Method::GET)
            .uri(format!(
                "https://test.example.com/api/feed?url={}",
                urlencoding::encode(feed_url)
            ))
            .body(Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn test_api_feed_returns_items_json() {
        let server = serve_test_rss_feed(&["1", "2"]).await.unwrap();

        let response = api_feed(api_feed_request(&server.url()))
            .await
            .expect("api_feed should succeed");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get("content-type").unwrap(),
            "application/json; charset=utf-8"
        );
        assert!(response.headers().get("cache-control").is_some());

        let items: Vec<filter_rss_feed::FeedItem> =
            serde_json::from_slice(&response.into_body()).expect("body is a FeedItem array");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].title.as_deref(), Some("Test Item 1"));
        assert_eq!(items[1].guid.as_deref(), Some("2"));
    }

    #[tokio::test]
    async fn test_api_feed_preserves_upstream_status() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/")
            .with_status(503)
            .create_async()
            .await;

        let response = api_feed(api_feed_request(&server.url()))
            .await
            .expect("api_feed returns an Ok error-response");

        assert_eq!(response.status().as_u16(), 503);
    }

    #[tokio::test]
    async fn test_api_feed_rejects_non_rss_content_type() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/")
            .with_status(200)
            .with_header("content-type", "text/html")
            .with_body("<html></html>")
            .create_async()
            .await;

        let err = api_feed(api_feed_request(&server.url()))
            .await
            .expect_err("non-RSS content type should error");
        let response: Response<Bytes> = (&err).into();
        assert_eq!(response.status().as_u16(), *UNSUPPORTED_MEDIA_TYPE);
    }

    #[tokio::test]
    async fn test_api_feed_missing_url_param() {
        let req = Request::builder()
            .method(Method::GET)
            .uri("https://test.example.com/api/feed")
            .body(Body::empty())
            .unwrap();

        let err = api_feed(req).await.expect_err("missing url should error");
        let response: Response<Bytes> = (&err).into();
        assert_eq!(response.status().as_u16(), *BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_api_feed_malformed_feed() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/")
            .with_status(200)
            .with_header("content-type", "application/rss+xml")
            .with_body("<not-rss/>")
            .create_async()
            .await;

        let err = api_feed(api_feed_request(&server.url()))
            .await
            .expect_err("malformed feed should error");
        let response: Response<Bytes> = (&err).into();
        assert_eq!(response.status().as_u16(), *BAD_REQUEST);
    }
}

#[cfg(all(test, target_arch = "wasm32"))]
mod wasm_tests {
    use super::*;

    use filter_rss_feed::fake_http_client::FakeHttpClientBuilder;
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
            title_regexes: vec![title_regex1, title_regex2],
            guid_regexes: vec![guid_regex],
            link_regexes: vec![],
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
            title_regexes: vec![title_regex],
            guid_regexes: vec![guid_regex],
            link_regexes: vec![],
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
        // The SPA is served by the assets binding in `main`; `real_main` only
        // handles the filter endpoint, so a bare `/` here is missing params.
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
    async fn test_bare_root_is_filter_endpoint() {
        // The SPA is served from static assets in `main`; reaching `real_main`
        // with a bare `/` means no filter params, so it's a 400 (not the SPA).
        let req = Request::builder()
            .method(Method::GET)
            .uri("https://test.example.com/")
            .body(Body::empty())
            .unwrap();

        let response = real_main(req, WorkerConfig::default()).await;
        assert_eq!(response.status().as_u16(), *BAD_REQUEST);
    }

    #[test]
    fn test_feed_url_param() {
        let uri: http::Uri = "https://x/api/feed?url=https%3A%2F%2Fexample.com%2Ffeed.xml"
            .parse()
            .unwrap();
        assert_eq!(
            feed_url_param(&uri).unwrap(),
            "https://example.com/feed.xml"
        );

        let missing: http::Uri = "https://x/api/feed".parse().unwrap();
        assert!(matches!(
            feed_url_param(&missing),
            Err(ValidationError::NoUrlProvided)
        ));
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
