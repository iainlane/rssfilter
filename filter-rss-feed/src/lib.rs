mod header_cf_cache_status;
mod http_client;

/// Mock HTTP client for testing RSS filtering without external dependencies.
///
/// This module provides a fake HTTP client implementation that returns
/// pre-configured responses, allowing tests to run reliably without
/// depending on external services or network conditions.
#[cfg(any(test, feature = "testing"))]
pub mod fake_http_client;

use bytes::Bytes;
use headers::{ContentLength, ContentType, HeaderMapExt};
use http::{HeaderMap, Method, Request as HttpRequest, Response as HttpResponse};
use regex::Regex;
use rss::{Channel, Item};
use std::error::Error as StdError;
use thiserror::Error;
use tracing::{debug, info, instrument};

use http_client::{HttpClient, HttpClientError};

pub type BoxError = Box<dyn StdError + Send + Sync>;

/// The maximum size of the RSS feed we'll accept, to prevent excessive memory usage.
static MAX_RSS_SIZE: u64 = 10 * 1024 * 1024; // 10MB limit

#[derive(Error, Debug)]
pub enum RssError {
    #[error("HTTP request error: {0}")]
    Http(#[from] http::Error),

    #[error("HTTP client error: {0}")]
    HttpClient(#[from] HttpClientError),

    #[error("RSS feed is too large (max {max_size} bytes)")]
    FeedTooLarge { max_size: u64 },

    #[error("Invalid content type: {content_type}. Expected XML or RSS content")]
    InvalidContentType { content_type: String },

    #[error("RSS parsing error: {0}")]
    RSSParse(#[from] rss::Error),

    #[error("I/O error: {0}")]
    IO(#[from] std::io::Error),

    #[error("UTF-8 error: {0}")]
    UTF8(#[from] std::string::FromUtf8Error),
}
/// Validate response size to prevent memory issues
fn validate_response_size(resp: &HttpResponse<Bytes>) -> Result<(), RssError> {
    if resp
        .headers()
        .typed_get::<ContentLength>()
        .is_some_and(|len| len.0 > MAX_RSS_SIZE)
    {
        return Err(RssError::FeedTooLarge {
            max_size: MAX_RSS_SIZE,
        });
    }

    Ok(())
}

fn validate_content_type(resp: &HttpResponse<Bytes>) -> Result<(), RssError> {
    const RSS_MIME_TYPES: &[&str] = &[
        "application/rss+xml",
        "application/atom+xml",
        "text/xml",
        "application/xml",
    ];

    let content_type = resp
        .headers()
        .typed_get::<ContentType>()
        .map(|ct| ct.to_string())
        .unwrap_or_else(|| "<none>".to_owned());

    RSS_MIME_TYPES
        .iter()
        .any(|&expected| content_type == expected)
        .then_some(())
        .ok_or(RssError::InvalidContentType { content_type })
}

#[derive(Debug)]
pub struct FilterRegexes<'a> {
    pub title_regexes: &'a [Regex],
    pub guid_regexes: &'a [Regex],
    pub link_regexes: &'a [Regex],
}

pub struct RssFilter<'a> {
    filter_regexes: &'a FilterRegexes<'a>,
    http_client: Box<dyn HttpClient>,
}

impl<'a> RssFilter<'a> {
    pub fn new(filter_regexes: &'a FilterRegexes<'a>) -> Result<Self, RssError> {
        let http_client = crate::http_client::create_http_client()?;
        Ok(Self::new_with_http_client(filter_regexes, http_client))
    }

    /// Create an RSS filter with a custom HTTP client.
    ///
    /// This constructor allows dependency injection of the HTTP client,
    /// enabling the use of mock clients in tests whilst using real
    /// HTTP clients in production.
    pub fn new_with_http_client(
        filter_regexes: &'a FilterRegexes<'a>,
        http_client: Box<dyn HttpClient>,
    ) -> Self {
        Self {
            filter_regexes,
            http_client,
        }
    }
    #[instrument(skip(self))]
    pub async fn fetch(
        &self,
        url: &str,
        headers: HeaderMap,
    ) -> Result<HttpResponse<Bytes>, RssError> {
        debug!("Requesting URL: {}", url);

        let mut request_builder = HttpRequest::builder().method(Method::GET).uri(url);

        request_builder = headers
            .iter()
            .fold(request_builder, |builder, (key, value)| {
                builder.header(key.as_str(), value)
            });

        let request = request_builder.body(Bytes::new()).map_err(|e| {
            RssError::HttpClient(HttpClientError::Request(format!(
                "Failed to build request: {e}"
            )))
        })?;

        let response = self.http_client.send(request).await?;

        validate_response_size(&response)?;

        Ok(response)
    }
    #[instrument(skip(self))]
    fn filter_out(&self, regexes: &[Regex], value: Option<&str>) -> bool {
        value.is_some_and(|v| regexes.iter().any(|r| r.is_match(v)))
    }

    #[instrument(skip(self, channel))]
    fn filter(&self, mut channel: Channel) -> Result<Bytes, RssError> {
        info!("Filtering items from RSS feed");

        let n_items_at_start = channel.items.len();

        type ItemGetter = fn(&Item) -> Option<&str>;

        let filter_regexes: &[(&[Regex], ItemGetter)] = &[
            (self.filter_regexes.title_regexes, |item: &Item| {
                item.title()
            }),
            (self.filter_regexes.guid_regexes, |item: &Item| {
                item.guid().map(|guid| guid.value())
            }),
            (self.filter_regexes.link_regexes, |item: &Item| item.link()),
        ];

        channel.items.retain(|item| {
            !filter_regexes.iter().any(|(regexes, getter)| {
                let filter = self.filter_out(regexes, getter(item));

                if filter {
                    debug!(item = item.link(), "Filtering out item");
                }

                filter
            })
        });

        let n_items_at_end = channel.items.len();
        let n_items_filtered = n_items_at_start - n_items_at_end;

        let channel_url = channel.link();

        if n_items_filtered > 0 {
            info!(
                channel_url,
                n_items_at_start, n_items_at_end, n_items_filtered, "Filtered items from RSS feed"
            );
        } else {
            info!(channel_url, "No items filtered from RSS feed");
        }

        let mut buf = Vec::new();
        channel.pretty_write_to(&mut buf, b' ', 2)?;

        Ok(Bytes::from(buf))
    }

    #[instrument(skip(self, response), fields(status = %response.status()))]
    pub async fn filter_response(&self, response: HttpResponse<Bytes>) -> Result<Bytes, RssError> {
        debug!("Received response");
        let content = response.into_body();
        let channel = Channel::read_from(&content[..])?;

        self.filter(channel)
    }

    pub async fn try_filter_response(
        &self,
        response: HttpResponse<Bytes>,
    ) -> Result<HttpResponse<Bytes>, RssError> {
        if !response.status().is_success() {
            return Ok(response);
        }

        validate_content_type(&response)?;

        let status_code = response.status();
        debug!(status = status_code.as_str(), "Received response",);

        let response_builder = HttpResponse::builder().status(status_code.as_u16());
        let response_builder = response
            .headers()
            .clone()
            .iter()
            .fold(response_builder, |builder, (key, value)| {
                builder.header(key.as_str(), value)
            });

        let filtered_body = self.filter_response(response).await?;
        let resp_out = response_builder.body(filtered_body)?;

        Ok(resp_out)
    }

    pub async fn fetch_and_filter_with_headers(
        &self,
        url: &str,
        headers: HeaderMap,
    ) -> Result<HttpResponse<Bytes>, RssError> {
        let response = self.fetch(url, headers).await?;

        self.try_filter_response(response).await
    }

    pub async fn fetch_and_filter(&self, url: &str) -> Result<HttpResponse<Bytes>, RssError> {
        self.fetch_and_filter_with_headers(url, HeaderMap::new())
            .await
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::{env, io::Cursor, sync::LazyLock};

    use super::*;

    use ctor::ctor;
    use headers::Mime;
    use http::StatusCode;
    use test_case::test_case;

    use rssfilter_telemetry::{init_default_subscriber, WorkerConfig};
    use test_utils::feed::serve_test_rss_feed;

    #[ctor]
    fn init_tracing() {
        env::set_var("RUST_LOG", "debug");
        init_default_subscriber(WorkerConfig::default());
    }

    static INTERNAL_SERVER_ERROR: LazyLock<usize> =
        LazyLock::new(|| StatusCode::INTERNAL_SERVER_ERROR.as_u16() as usize);

    #[allow(clippy::needless_lifetimes)]
    async fn filter<'a>(
        filter: &RssFilter<'a>,
        url: &str,
        expected: Vec<Option<&str>>,
    ) -> Result<(), BoxError> {
        let unfiltered_feed = filter
            .fetch_and_filter_with_headers(url, HeaderMap::new())
            .await?
            .into_body();

        let cursor = Cursor::new(unfiltered_feed);

        let channel = Channel::read_from(cursor)?;
        let titles = channel
            .items()
            .iter()
            .map(|i| i.title())
            .collect::<Vec<_>>();

        assert_eq!(titles, expected);

        Ok(())
    }

    #[test_case(&FilterRegexes {
        title_regexes: &[Regex::new("^Test Item 1$").unwrap()],
        guid_regexes: &[],
        link_regexes: &[],
    }, vec![Some("Test Item 2")] ; "title filter only")]
    #[test_case(&FilterRegexes {
        title_regexes: &[Regex::new("^Test Item 1$").unwrap(), Regex::new("^Test Item 2$").unwrap()],
        guid_regexes: &[],
        link_regexes: &[],
    }, vec![] ; "title filter only, both items match")]
    #[test_case(&FilterRegexes {
        title_regexes: &[],
        guid_regexes: &[Regex::new("1").unwrap()],
        link_regexes: &[],
    }, vec![Some("Test Item 2")] ; "guid filter only")]
    #[test_case(&FilterRegexes {
        title_regexes: &[],
        guid_regexes: &[],
        link_regexes: &[Regex::new("test2").unwrap()],
    }, vec![Some("Test Item 1")] ; "link filter only")]
    #[test_case(&FilterRegexes {
        title_regexes: &[],
        guid_regexes: &[],
        link_regexes: &[],
    }, vec![Some("Test Item 1"), Some("Test Item 2")] ; "no filters")]
    #[tokio::test]
    #[allow(clippy::needless_lifetimes)]
    async fn test_fetch_and_filter<'a>(
        filter_regexes: &FilterRegexes<'a>,
        expected: Vec<Option<&str>>,
    ) -> Result<(), BoxError> {
        let server = serve_test_rss_feed(&["1", "2"]).await?;
        let url = server.url();

        let rss_filter = RssFilter::new(filter_regexes)?;
        filter(&rss_filter, &url, expected).await?;

        Ok(())
    }

    #[tokio::test]
    async fn test_server_error() -> Result<(), BoxError> {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/")
            .with_status(*INTERNAL_SERVER_ERROR)
            .create_async()
            .await;

        let url = server.url();

        let filter_regexes = FilterRegexes {
            title_regexes: &[],
            guid_regexes: &[],
            link_regexes: &[],
        };

        let filter = RssFilter::new(&filter_regexes)?;
        let result = filter
            .fetch_and_filter_with_headers(&url, HeaderMap::new())
            .await
            .expect("Expected fetch to succeed");

        assert_eq!(result.status(), StatusCode::INTERNAL_SERVER_ERROR);

        Ok(())
    }

    #[tokio::test]
    async fn test_content_type_validation_success() {
        // Test RSS content type
        let mut response_builder = HttpResponse::builder();
        let headers = response_builder
            .headers_mut()
            .expect("Failed to get headers");
        headers.typed_insert(ContentType::from(
            "application/rss+xml".parse::<Mime>().unwrap(),
        ));

        let response = response_builder
            .body(Bytes::new())
            .expect("Failed to build response");

        let result = validate_content_type(&response);
        assert!(result.is_ok());

        // Test XML content type
        let mut response_builder = HttpResponse::builder();
        let headers = response_builder
            .headers_mut()
            .expect("Failed to get headers");
        headers.typed_insert(ContentType::from(
            "application/xml".parse::<Mime>().unwrap(),
        ));

        let response = response_builder
            .body(Bytes::new())
            .expect("Failed to build response");

        let result = validate_content_type(&response);
        assert!(result.is_ok());

        // Test Atom content type
        let mut response_builder = HttpResponse::builder();
        let headers = response_builder
            .headers_mut()
            .expect("Failed to get headers");
        headers.typed_insert(ContentType::from(
            "application/atom+xml".parse::<Mime>().unwrap(),
        ));

        let response = response_builder
            .body(Bytes::new())
            .expect("Failed to build response");

        let result = validate_content_type(&response);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_feed_size_validation() {
        // Create a response with large content length
        let mut response_builder = HttpResponse::builder();
        let headers = response_builder
            .headers_mut()
            .expect("Failed to get headers");
        headers.typed_insert(ContentLength(MAX_RSS_SIZE * 2));

        let response = response_builder
            .body(Bytes::new())
            .expect("Failed to build response");

        let result = validate_response_size(&response);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), RssError::FeedTooLarge { .. }));
    }

    #[tokio::test]
    async fn test_feed_size_validation_success() {
        let mut response_builder = HttpResponse::builder();
        let headers = response_builder
            .headers_mut()
            .expect("Failed to get headers");
        headers.typed_insert(ContentLength(1000));

        let response = response_builder
            .body(Bytes::new())
            .expect("Failed to build response");

        let result = validate_response_size(&response);
        assert!(result.is_ok());
    }
}
