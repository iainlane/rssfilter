use std::borrow::Cow;

use aws_lambda_events::{
    apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse},
    query_map::QueryMap,
};
use http::{header::HeaderMap as HttpHeaderMap, HeaderName, HeaderValue, StatusCode};
use lambda_runtime::{service_fn, Error as LambdaError, LambdaEvent, Runtime};
use once_cell::sync::Lazy;
use opentelemetry_http::HeaderExtractor;
use regex::Regex;
use thiserror::Error;
use tracing::{self, debug, info, instrument, Span};
use tracing_opentelemetry::OpenTelemetrySpanExt;
use urlencoding::decode;

use filter_rss_feed::{default_reqwest_client, FilterRegexes, RssError, RssFilter};

mod filter;
use filter::filter_request_headers;

mod setup_tracing;
use setup_tracing::init_default_subscriber;

mod extension;

static OK: Lazy<i64> = Lazy::new(|| StatusCode::OK.as_u16().into());
static BAD_GATEWAY: Lazy<i64> = Lazy::new(|| StatusCode::BAD_GATEWAY.as_u16().into());
static BAD_REQUEST: Lazy<i64> = Lazy::new(|| StatusCode::BAD_REQUEST.as_u16().into());
static NOT_FOUND: Lazy<i64> = Lazy::new(|| StatusCode::NOT_FOUND.as_u16().into());

#[derive(Debug, Error)]
enum RssHandlerError {
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

    #[error("A URL must be provided")]
    NoUrlProvided,

    #[error("An error occurred while handling the request: {source}")]
    SendRequestError {
        #[source]
        source: RssError,
    },

    #[error("An error occurred while receiving the request: {source}")]
    ReceiveRequestError {
        #[source]
        source: reqwest::Error,
    },

    #[error("An error occurred while filtering the feed: {source}")]
    FilterError {
        #[source]
        source: RssError,
    },

    #[error("The Lambda context does not contain a path")]
    NoPathInContextError,
}

impl RssHandlerError {
    /// Was it our fault or theirs?
    /// ours -> Bad Gateway (502)
    /// theirs -> Bad Request (400)
    fn status_code(&self) -> i64 {
        match self {
            RssHandlerError::SendRequestError { .. } => *BAD_GATEWAY,
            RssHandlerError::ReceiveRequestError { .. } => *BAD_GATEWAY,
            _ => *BAD_REQUEST,
        }
    }
}

async fn handle_root_path(
    reqwest_client: reqwest::Client,
    event: LambdaEvent<ApiGatewayV2httpRequest>,
) -> Result<ApiGatewayV2httpResponse, LambdaError> {
    rss_handler(reqwest_client, event).await.or_else(|err| {
        Ok(ApiGatewayV2httpResponse {
            status_code: err.status_code(),
            headers: HttpHeaderMap::new(),
            multi_value_headers: HttpHeaderMap::new(),
            body: Some(err.to_string().into()),
            is_base64_encoded: false,
            cookies: vec![],
        })
    })
}

fn handle_not_found(path: &str) -> Result<ApiGatewayV2httpResponse, LambdaError> {
    let headers = vec![(
        HeaderName::from_static("cache-control"),
        HeaderValue::from_static("public, max-age=86400"),
    )]
    .into_iter()
    .collect();

    info!(path, status_code = *NOT_FOUND, "Path not found");

    Ok(ApiGatewayV2httpResponse {
        status_code: *NOT_FOUND,
        headers,
        multi_value_headers: HttpHeaderMap::new(),
        body: Some("Not Found".into()),
        is_base64_encoded: false,
        cookies: vec![],
    })
}

/// Handles the incoming request. Only the root path `/` is supported. Other
/// paths will return a 404.
#[instrument(
    skip(event, reqwest_client),
    fields(
        context_xray_trace_id = event.context.xray_trace_id.as_deref().unwrap_or("unknown"),
        faas.trigger = "http",
        header_xray_trace_id = event.payload.headers.get("x-amzn-trace-id").map(|v| v.to_str().unwrap_or("unknown")),
        ip = event.payload.request_context.http.source_ip.as_deref().unwrap_or("unknown"),
        path = event.payload.request_context.http.path.as_deref().unwrap_or("unknown"),
        request_id = event.context.request_id,
        user_agent = event.payload.request_context.http.user_agent.as_deref().unwrap_or("unknown"),
    )
)]
async fn handler(
    reqwest_client: reqwest::Client,
    mut event: LambdaEvent<ApiGatewayV2httpRequest>,
) -> Result<ApiGatewayV2httpResponse, LambdaError> {
    // Overwrite the `x-amzn-trace-id` header with the incoming trace context's
    // trace ID. This seems to be different for us, perhaps because we're an
    // `ApiGatewayV2httpRequest`?
    if let Some(trace_id) = event.context.xray_trace_id.as_deref() {
        event
            .payload
            .headers
            .insert("x-amzn-trace-id", HeaderValue::from_str(trace_id)?);
        debug!(
            trace_id = ?event.payload.headers.get("x-amzn-trace-id").unwrap(),
            "Set x-amzn-trace-id header"
        )
    }

    let parent_ctx = opentelemetry::global::get_text_map_propagator(|propagator| {
        propagator.extract(&HeaderExtractor(&event.payload.headers))
    });
    Span::current().set_parent(parent_ctx.clone());

    let path = event
        .payload
        .request_context
        .http
        .path
        .as_ref()
        .ok_or(RssHandlerError::NoPathInContextError)?;

    match path.as_str() {
        "/" => handle_root_path(reqwest_client, event).await,
        _ => handle_not_found(path),
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

#[instrument]
fn decode_and_compile_regex(
    query_string_parameters: &QueryMap,
    key: &'static str,
) -> Result<Vec<Regex>, RssHandlerError> {
    query_string_parameters
        .all(key)
        .unwrap_or_default()
        .into_iter()
        .map(|value| {
            let decoded = decode(value).map_err(|err| RssHandlerError::MalformedParameter {
                name: key,
                source: err,
            })?;
            Regex::new(&decoded).map_err(|err| RssHandlerError::InvalidRegex {
                name: key,
                source: err,
            })
        })
        .collect()
}

#[instrument]
fn validate_parameters(query_string_parameters: &QueryMap) -> Result<Params, RssHandlerError> {
    let title_regexes = decode_and_compile_regex(query_string_parameters, "title_filter_regex")?;
    let guid_regexes = decode_and_compile_regex(query_string_parameters, "guid_filter_regex")?;
    let link_regexes = decode_and_compile_regex(query_string_parameters, "link_filter_regex")?;
    let url = query_string_parameters
        .first("url")
        .map(decode)
        .transpose()
        .map_err(|err| RssHandlerError::MalformedParameter {
            name: "url",
            source: err,
        })?;

    let any_filters_provided =
        !(title_regexes.is_empty() && guid_regexes.is_empty() && link_regexes.is_empty());
    let url_provided = url.is_some();

    if !any_filters_provided {
        if !url_provided {
            return Err(RssHandlerError::NoParametersProvided);
        }
        return Err(RssHandlerError::NoFiltersProvided);
    }

    let url = url.ok_or(RssHandlerError::NoUrlProvided)?;

    Ok(Params {
        regex_params: RegexParams {
            title_regexes,
            guid_regexes,
            link_regexes,
        },
        url,
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
#[instrument(skip(reqwest_client, event))]
async fn rss_handler(
    reqwest_client: reqwest::Client,
    event: LambdaEvent<ApiGatewayV2httpRequest>,
) -> Result<ApiGatewayV2httpResponse, RssHandlerError> {
    let params = validate_parameters(&event.payload.query_string_parameters)?;
    let url = &params.url;

    let filter_regexes: FilterRegexes = (&params.regex_params).into();

    info!(
        regexes = ?&params.regex_params,
        url = url.as_ref(),
        "Filtering RSS feed"
    );

    let rss_filter = RssFilter::new_with_client(&filter_regexes, reqwest_client);

    let resp = rss_filter
        .fetch(url, filter_request_headers(event.payload.headers))
        .await
        .map_err(|err| RssHandlerError::SendRequestError { source: err })?;

    let status_code = resp.status().as_u16().into();
    let headers = resp.headers().clone();

    if status_code == *OK {
        let body = rss_filter
            .filter_response(resp)
            .await
            .map_err(|err| RssHandlerError::FilterError { source: err })?;

        return Ok(ApiGatewayV2httpResponse {
            status_code,
            headers,
            multi_value_headers: HttpHeaderMap::new(),
            body: Some(body.into()),
            is_base64_encoded: false,
            cookies: vec![],
        });
    }

    Ok(ApiGatewayV2httpResponse {
        status_code,
        headers,
        multi_value_headers: HttpHeaderMap::new(),
        body: resp
            .bytes()
            .await
            .map_err(|err| RssHandlerError::ReceiveRequestError { source: err })
            .map(|b| b.to_vec().into())
            .ok(),
        is_base64_encoded: true,
        cookies: vec![],
    })
}

#[tokio::main]
async fn main() -> Result<(), LambdaError> {
    debug!("Starting RSS filter application");

    let tracer_provider = init_default_subscriber()?;

    let (lambda_extension, flush_extension) =
        extension::FlushExtension::new_extension(tracer_provider).await?;

    let client = &default_reqwest_client()?;

    let runtime = Runtime::new(service_fn(
        |event: LambdaEvent<ApiGatewayV2httpRequest>| async {
            let flush_extension = flush_extension.clone();

            let res: Result<ApiGatewayV2httpResponse, LambdaError> =
                handler(client.clone(), event).await;

            if res.is_ok() {
                flush_extension.notify_request_done()?;
            }

            res
        },
    ));

    tokio::try_join!(runtime.run(), lambda_extension.run())?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{collections::HashMap, str};

    use filter_rss_feed::BoxError;
    use http::HeaderName;

    use test_utils::serve_test_rss_feed;

    struct LambdaEventBuilder {
        event: LambdaEvent<ApiGatewayV2httpRequest>,
    }

    static TEMPORARY_REDIRECT: Lazy<i64> =
        Lazy::new(|| StatusCode::TEMPORARY_REDIRECT.as_u16().into());

    impl Default for LambdaEventBuilder {
        fn default() -> Self {
            static LAMBDA_EVENT: Lazy<LambdaEvent<ApiGatewayV2httpRequest>> =
                Lazy::new(|| LambdaEvent {
                    payload: ApiGatewayV2httpRequest::default(),
                    context: lambda_runtime::Context::default(),
                });

            LambdaEventBuilder {
                event: LAMBDA_EVENT.clone(),
            }
        }
    }

    impl LambdaEventBuilder {
        fn new() -> Self {
            LambdaEventBuilder::default()
        }

        fn with_query_string_parameters(mut self, params: Vec<(&str, &str)>) -> Self {
            let mut query_string_parameters = HashMap::new();

            params.into_iter().for_each(|(k, v)| {
                query_string_parameters
                    .entry(k.to_owned())
                    .or_insert_with(Vec::new)
                    .push(v.to_owned());
            });

            self.event.payload.query_string_parameters = query_string_parameters.into();

            self
        }

        fn with_headers(mut self, headers: Vec<(HeaderName, HeaderValue)>) -> Self {
            self.event.payload.headers = headers.into_iter().collect();

            self
        }

        fn with_path(mut self, path: &str) -> Self {
            self.event.payload.request_context.http.path = Some(path.to_string());

            self
        }

        fn build(self) -> LambdaEvent<ApiGatewayV2httpRequest> {
            self.event
        }
    }

    #[tokio::test]
    async fn test_no_query_params() -> Result<(), BoxError> {
        let client = default_reqwest_client()?;

        let res = rss_handler(client, LambdaEventBuilder::new().with_path("/").build()).await;

        assert!(res.is_err());

        let res_err = res.unwrap_err();
        assert!(matches!(res_err, RssHandlerError::NoParametersProvided));

        Ok(())
    }

    #[tokio::test]
    async fn test_no_url_param() -> Result<(), BoxError> {
        let client = default_reqwest_client()?;

        let res = rss_handler(
            client,
            LambdaEventBuilder::new()
                .with_path("/")
                .with_query_string_parameters(vec![("title_filter_regex", ".*")])
                .build(),
        )
        .await;

        assert!(res.is_err());

        let res_err = res.unwrap_err();
        assert!(matches!(res_err, RssHandlerError::NoUrlProvided));

        Ok(())
    }

    #[tokio::test]
    async fn test_no_filters() -> Result<(), BoxError> {
        let client = default_reqwest_client()?;

        let res = rss_handler(
            client,
            LambdaEventBuilder::new()
                .with_path("/")
                .with_query_string_parameters(vec![("url", "http://example.com/rss")])
                .build(),
        )
        .await;

        assert!(res.is_err());

        let res_err = res.unwrap_err();
        assert!(matches!(res_err, RssHandlerError::NoFiltersProvided));

        Ok(())
    }

    #[tokio::test]
    async fn test_error_status_mapping_bad_request() -> Result<(), BoxError> {
        let client = default_reqwest_client()?;

        let res = handler(client, LambdaEventBuilder::new().with_path("/").build()).await;

        assert!(res.is_ok());

        let resp = res.unwrap();
        assert_eq!(resp.status_code, *BAD_REQUEST);

        Ok(())
    }

    #[tokio::test]
    async fn test_error_status_mapping_bad_gateway() -> Result<(), BoxError> {
        let client = default_reqwest_client()?;

        let res = handler(
            client,
            LambdaEventBuilder::new()
                .with_path("/")
                .with_query_string_parameters(vec![
                    // This port is invalid, so we should get a connection error
                    ("url", "http://localhost:99999"),
                    ("title_filter_regex", ".*"),
                ])
                .build(),
        )
        .await;

        assert!(res.is_ok());

        let resp = res.unwrap();
        assert_eq!(resp.status_code, *BAD_GATEWAY);

        Ok(())
    }

    #[tokio::test]
    async fn test_filter_title() -> Result<(), BoxError> {
        let client = default_reqwest_client()?;

        let server = serve_test_rss_feed(&["1", "2"]).await?;
        let url = server.url();

        let res = handler(
            client,
            LambdaEventBuilder::new()
                .with_path("/")
                .with_query_string_parameters(vec![
                    ("title_filter_regex", "Test Item 1"),
                    ("url", &url),
                ])
                .build(),
        )
        .await?;

        assert_eq!(res.status_code, *OK);

        let b = res.body.unwrap();
        let body = str::from_utf8(&b).unwrap();

        assert!(body.contains("Item 2"));
        assert!(!body.contains("Item 1"));

        Ok(())
    }

    #[tokio::test]
    async fn test_filter_guid() -> Result<(), BoxError> {
        let client = default_reqwest_client()?;

        let server = serve_test_rss_feed(&["1", "2"]).await?;
        let url = server.url();

        let res = handler(
            client,
            LambdaEventBuilder::new()
                .with_path("/")
                .with_query_string_parameters(vec![("guid_filter_regex", "1"), ("url", &url)])
                .build(),
        )
        .await?;

        assert_eq!(res.status_code, *OK);

        let b = res.body.unwrap();
        let body = str::from_utf8(&b).unwrap();

        assert!(body.contains("Item 2"));
        assert!(!body.contains("Item 1"));

        Ok(())
    }

    #[tokio::test]
    async fn test_filter_link() -> Result<(), BoxError> {
        let client = default_reqwest_client()?;

        let server = serve_test_rss_feed(&["1", "2"]).await?;
        let url = server.url();

        let res = handler(
            client,
            LambdaEventBuilder::new()
                .with_path("/")
                .with_query_string_parameters(vec![("link_filter_regex", "test2"), ("url", &url)])
                .build(),
        )
        .await?;

        assert_eq!(res.status_code, *OK);

        let b = res.body.unwrap();
        let body = str::from_utf8(&b).unwrap();

        assert!(body.contains("Item 1"));
        assert!(!body.contains("Item 2"));

        Ok(())
    }

    #[tokio::test]
    async fn test_filter_link_multiple() -> Result<(), BoxError> {
        let client = default_reqwest_client()?;

        let server = serve_test_rss_feed(&["1", "2", "3"]).await?;
        let url = server.url();

        let res = handler(
            client,
            LambdaEventBuilder::new()
                .with_path("/")
                .with_query_string_parameters(vec![
                    ("link_filter_regex", "test1"),
                    ("link_filter_regex", "test2"),
                    ("url", &url),
                ])
                .build(),
        )
        .await?;

        assert_eq!(res.status_code, *OK);

        let b = res.body.unwrap();
        let body = str::from_utf8(&b).unwrap();

        assert!(!body.contains("Item 1"));
        assert!(!body.contains("Item 2"));
        assert!(body.contains("Item 3"));

        Ok(())
    }

    #[tokio::test]
    async fn test_404() -> Result<(), BoxError> {
        let client = default_reqwest_client()?;

        let res = handler(
            client,
            LambdaEventBuilder::new().with_path("/favicon.ico").build(),
        )
        .await?;

        assert_eq!(res.status_code, *NOT_FOUND);

        Ok(())
    }

    #[tokio::test]
    async fn test_header_passthrough() -> Result<(), BoxError> {
        let client = default_reqwest_client()?;

        let mut server = serve_test_rss_feed(&["1", "2"]).await?;
        server.reset();
        server
            .mock("GET", "/")
            .with_status((*TEMPORARY_REDIRECT).try_into().expect("status code"))
            .match_header("my-test-header", "value")
            .create_async()
            .await;

        let url = server.url();

        let res = handler(
            client,
            LambdaEventBuilder::new()
                .with_path("/")
                .with_query_string_parameters(vec![("title_filter_regex", "Item 1"), ("url", &url)])
                .with_headers(vec![(
                    HeaderName::from_static("my-test-header"),
                    HeaderValue::from_static("value"),
                )])
                .build(),
        )
        .await?;

        assert_eq!(res.status_code, *TEMPORARY_REDIRECT);

        Ok(())
    }
}
