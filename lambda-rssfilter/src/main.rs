use aws_lambda_events::lambda_function_urls::{
    LambdaFunctionUrlRequest, LambdaFunctionUrlResponse,
};
use http::{header::HeaderMap as HttpHeaderMap, HeaderValue, StatusCode};
use lambda_runtime::{
    service_fn,
    tracing::{self, debug, info, instrument},
    Error as LambdaError, LambdaEvent,
};
use once_cell::sync::Lazy;
use regex::Regex;
use snafu::{prelude::*, OptionExt, ResultExt};
use urlencoding::decode;

use filter_rss_feed::{default_reqwest_client, BoxError, FilterRegexes, RssFilter};

mod filter;
use filter::filter_request_headers;

static OK: Lazy<i64> = Lazy::new(|| i64::from(StatusCode::OK.as_u16()));
static NOT_FOUND: Lazy<i64> = Lazy::new(|| i64::from(StatusCode::NOT_FOUND.as_u16()));
static BAD_GATEWAY: Lazy<i64> = Lazy::new(|| i64::from(StatusCode::BAD_GATEWAY.as_u16()));
static BAD_REQUEST: Lazy<i64> = Lazy::new(|| i64::from(StatusCode::BAD_REQUEST.as_u16()));

#[derive(Debug, Snafu)]
enum RssHandlerError {
    #[snafu(display("the parameter {name} could not be decoded: {}", source))]
    MalformedParameter {
        name: &'static str,
        source: std::string::FromUtf8Error,
    },

    #[snafu(display("the regex for {name} is invalid: {}", source))]
    InvalidRegex {
        name: &'static str,
        source: regex::Error,
    },

    #[snafu(display("A url and at least one of title_filter_regex, guid_filter_regex, or link_filter_regex must be provided"))]
    NoParametersProvided,

    #[snafu(display("At least one of title_filter_regex, guid_filter_regex, or link_filter_regex must be provided"))]
    NoFiltersProvided,

    #[snafu(display("A URL must be provided"))]
    NoUrlProvided,

    #[snafu(display("An error occurred while handling the request: {}", source))]
    RequestError { source: reqwest::Error },

    #[snafu(display("An error occurred while filtering the feed: {}", source))]
    FilterError { source: BoxError },
}

impl RssHandlerError {
    /// Was it our fault or theirs?
    /// ours -> Bad Gateway (502)
    /// theirs -> Bad Request (400)
    fn status_code(&self) -> i64 {
        match self {
            RssHandlerError::RequestError { .. } => *BAD_GATEWAY,
            _ => *BAD_REQUEST,
        }
    }
}

/// Handles the incoming request. Only the root path `/` is supported. Other
/// paths will return a 404.
#[instrument(skip(event, reqwest_client), fields(req_id = %event.context.request_id, ip = %event.payload.request_context.http.source_ip.as_deref().unwrap_or("unknown")))]
async fn handler(
    reqwest_client: reqwest::Client,
    event: LambdaEvent<LambdaFunctionUrlRequest>,
) -> Result<LambdaFunctionUrlResponse, LambdaError> {
    let path = event
        .payload
        .request_context
        .http
        .path
        .as_ref()
        .ok_or(LambdaError::from("request_context.http.path is required"))?;

    info!(path = %path, "Handling request");

    match path.as_str() {
        "/" => match rss_handler(reqwest_client, event).await {
            Ok(response) => Ok(response),
            Err(e) => Ok(LambdaFunctionUrlResponse {
                status_code: e.status_code(),
                headers: HttpHeaderMap::new(),
                body: Some(e.to_string()),
                is_base64_encoded: false,
                cookies: vec![],
            }),
        },
        _ => {
            let mut headers = HttpHeaderMap::new();
            headers.insert(
                "cache-control",
                HeaderValue::from_static("public, max-age=86400"),
            );

            info!(path = %path, status_code = *NOT_FOUND, "Path not found");

            Ok(LambdaFunctionUrlResponse {
                status_code: *NOT_FOUND,
                headers,
                body: Some("Not Found".to_string()),
                is_base64_encoded: false,
                cookies: vec![],
            })
        }
    }
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
/// `link_filter_regex` must be provided.
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
#[instrument (skip(reqwest_client, event), fields(req_id = %event.context.request_id, ip = %event.payload.request_context.http.source_ip.as_deref().unwrap_or("unknown")))]
async fn rss_handler(
    reqwest_client: reqwest::Client,
    event: LambdaEvent<LambdaFunctionUrlRequest>,
) -> Result<LambdaFunctionUrlResponse, RssHandlerError> {
    let params = event.payload.query_string_parameters;

    let decode_param = |key: &'static str| {
        params
            .get(key)
            .map(|value| decode(value).context(MalformedParameterSnafu { name: key }))
            .transpose()
    };

    let title_regex = decode_param("title_filter_regex")?
        .map(|r| Regex::new(&r))
        .transpose()
        .context(InvalidRegexSnafu {
            name: "title_filter_regex",
        })?;
    let guid_regex = decode_param("guid_filter_regex")?
        .map(|r| Regex::new(&r))
        .transpose()
        .context(InvalidRegexSnafu {
            name: "guid_filter_regex",
        })?;
    let link_regex = decode_param("link_filter_regex")?
        .map(|r| Regex::new(&r))
        .transpose()
        .context(InvalidRegexSnafu {
            name: "link_filter_regex",
        })?;

    let url = decode_param("url")?;

    let any_filters_provided =
        title_regex.is_some() || guid_regex.is_some() || link_regex.is_some();
    let url_provided = url.is_some();

    ensure!(
        any_filters_provided || url_provided,
        NoParametersProvidedSnafu
    );

    ensure!(any_filters_provided, NoFiltersProvidedSnafu);

    let url = url.context(NoUrlProvidedSnafu)?;

    info!(
        title_regex = ?title_regex,
        guid_regex = ?guid_regex,
        link_regex = ?link_regex,
        url = ?url,
        "Filtering RSS feed");

    let rss_filter = RssFilter::new_with_client(
        FilterRegexes {
            title_regex,
            guid_regex,
            link_regex,
        },
        reqwest_client,
    );

    let resp = rss_filter
        .fetch(&url, filter_request_headers(event.payload.headers))
        .await
        .context(RequestSnafu)?;

    let status_code = resp.status().as_u16().into();
    let headers = resp.headers().clone();

    if status_code == *OK {
        let body = rss_filter
            .filter_response(resp)
            .await
            .context(FilterSnafu)?;

        return Ok(LambdaFunctionUrlResponse {
            status_code,
            headers,
            body: Some(body),
            is_base64_encoded: false,
            cookies: vec![],
        });
    }

    Ok(LambdaFunctionUrlResponse {
        status_code,
        headers,
        body: resp.text().await.ok(),
        is_base64_encoded: false,
        cookies: vec![],
    })
}

#[tokio::main]
async fn main() -> Result<(), LambdaError> {
    tracing::init_default_subscriber();

    debug!("Starting RSS filter application");

    let client = &default_reqwest_client()?;

    lambda_runtime::run(service_fn(move |event| async move {
        handler(client.clone(), event).await.map_err(Box::new)
    }))
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    use aws_lambda_events::lambda_function_urls::{
        LambdaFunctionUrlRequestContext, LambdaFunctionUrlRequestContextHttpDescription,
    };
    use filter_rss_feed::BoxError;
    use http::HeaderName;
    use matches::matches;
    use once_cell::sync::Lazy;
    use test_utils::serve_test_rss_feed;

    struct LambdaEventBuilder {
        event: LambdaEvent<LambdaFunctionUrlRequest>,
    }

    impl Default for LambdaEventBuilder {
        fn default() -> Self {
            static LAMBDA_EVENT: Lazy<LambdaEvent<LambdaFunctionUrlRequest>> =
                Lazy::new(|| LambdaEvent {
                    payload: LambdaFunctionUrlRequest {
                        request_context: LambdaFunctionUrlRequestContext {
                            http: LambdaFunctionUrlRequestContextHttpDescription {
                                method: None,
                                path: None,
                                protocol: None,
                                source_ip: None,
                                user_agent: None,
                            },
                            account_id: None,
                            request_id: None,
                            authorizer: None,
                            apiid: None,
                            domain_name: None,
                            domain_prefix: None,
                            time: None,
                            time_epoch: 0,
                        },
                        query_string_parameters: Default::default(),
                        headers: Default::default(),
                        body: None,
                        is_base64_encoded: false,
                        cookies: None,
                        version: Some("2.0".to_string()),
                        raw_path: Some("/".to_string()),
                        raw_query_string: Some("".to_string()),
                    },
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
            self.event.payload.query_string_parameters = params
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();

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

        fn build(self) -> LambdaEvent<LambdaFunctionUrlRequest> {
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

        let body = res.body.unwrap();

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

        let body = res.body.unwrap();

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

        let body = res.body.unwrap();

        assert!(body.contains("Item 1"));
        assert!(!body.contains("Item 2"));

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
            .with_status(307)
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

        assert_eq!(res.status_code, 307);

        Ok(())
    }
}
