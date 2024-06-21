use regex::Regex;
use reqwest::{header::HeaderMap, Error as ReqwestError, Response};
use rss::{Channel, Item};
use std::error::Error as StdError;
use thiserror::Error;
use tracing::{debug, info, instrument};

pub type BoxError = Box<dyn StdError + Send + Sync>;

#[derive(Error, Debug)]
pub enum RssError {
    #[error("HTTP error fetching {url}: {status}")]
    HttpError {
        url: String,
        status: reqwest::StatusCode,
    },

    #[error("Network error: {0}")]
    NetworkError(#[from] ReqwestError),

    #[error("RSS parsing error: {0}")]
    RssParseError(#[from] rss::Error),

    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("UTF-8 error: {0}")]
    Utf8Error(#[from] std::string::FromUtf8Error),
}

#[derive(Debug)]
pub struct FilterRegexes<'a> {
    pub title_regexes: &'a [Regex],
    pub guid_regexes: &'a [Regex],
    pub link_regexes: &'a [Regex],
}

#[derive(Debug)]
pub struct RssFilter<'a> {
    filter_regexes: &'a FilterRegexes<'a>,
    reqwest_client: reqwest::Client,
}

pub fn default_reqwest_client() -> Result<reqwest::Client, ReqwestError> {
    reqwest::ClientBuilder::new()
        .user_agent("filter-rss-feed https://github.com/iainlane/filter-rss-feed")
        .gzip(true)
        .brotli(true)
        .zstd(true)
        .deflate(true)
        .build()
}

impl<'a> RssFilter<'a> {
    pub fn new(filter_regexes: &'a FilterRegexes<'a>) -> Result<Self, ReqwestError> {
        let reqwest_client = default_reqwest_client()?;

        Ok(Self::new_with_client(filter_regexes, reqwest_client))
    }

    pub fn new_with_client(
        filter_regexes: &'a FilterRegexes<'a>,
        reqwest_client: reqwest::Client,
    ) -> Self {
        RssFilter {
            filter_regexes,
            reqwest_client,
        }
    }

    #[instrument(skip(self))]
    pub async fn fetch(&self, url: &str, headers: HeaderMap) -> Result<Response, RssError> {
        info!("Requesting URL");
        self.reqwest_client
            .get(url)
            .headers(headers)
            .send()
            .await
            .map_err(RssError::from)
    }

    #[instrument(skip(self))]
    fn filter_out(&self, regexes: &[Regex], value: Option<&str>) -> bool {
        value.map_or(false, |v| regexes.iter().any(|r| r.is_match(v)))
    }

    #[instrument(skip(self, channel))]
    fn filter(&self, mut channel: Channel) -> Result<String, RssError> {
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

        channel
            .pretty_write_to(Vec::new(), b' ', 2)
            .map(String::from_utf8)?
            .map_err(RssError::from)
    }

    #[instrument(skip(self, response), fields(status = %response.status()))]
    pub async fn filter_response(&self, response: Response) -> Result<String, RssError> {
        info!("Received response");
        let content = response.bytes().await?;
        let channel = Channel::read_from(&content[..])?;

        self.filter(channel)
    }

    pub async fn fetch_and_filter(&self, url: &str) -> Result<String, RssError> {
        let response = self.fetch(url, HeaderMap::new()).await?;

        if !response.status().is_success() {
            return Err(RssError::HttpError {
                url: url.to_string(),
                status: response.status(),
            });
        }

        self.filter_response(response).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use test_case::test_case;

    use test_utils::serve_test_rss_feed;

    async fn filter<'a>(
        filter: &RssFilter<'a>,
        url: &str,
        expected: Vec<Option<&str>>,
    ) -> Result<(), BoxError> {
        let unfiltered_feed = filter.fetch_and_filter(url).await?;

        let channel = Channel::read_from(unfiltered_feed.as_bytes())?;
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
            .with_status(500)
            .create_async()
            .await;

        let url = server.url();

        let filter_regexes = FilterRegexes {
            title_regexes: &[],
            guid_regexes: &[],
            link_regexes: &[],
        };

        let filter = RssFilter::new(&filter_regexes)?;
        let result = filter.fetch_and_filter(&url).await;

        assert!(result.is_err());

        match result.unwrap_err() {
            RssError::HttpError { .. } => {}
            _ => panic!("Expected HttpError"),
        }

        Ok(())
    }
}
