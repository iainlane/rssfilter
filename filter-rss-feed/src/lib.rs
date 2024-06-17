use regex::Regex;
use reqwest::{header::HeaderMap, Error as ReqwestError, Response};
use rss::Channel;
use std::error::Error as StdError;
use tracing::{debug, info, instrument};

pub type BoxError = Box<dyn StdError + Send + Sync>;

#[derive(Default, PartialEq)]
pub struct RssError {
    message: String,
}

impl RssError {
    pub fn new(error: ReqwestError) -> Self {
        let message = if error.is_status() {
            format!(
                "HTTP error fetching {:?}: {}",
                error.url().map_or("unknown", |u| u.as_str()),
                error.status().unwrap()
            )
        } else {
            error.to_string()
        };

        RssError { message }
    }
}

impl std::fmt::Display for RssError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::fmt::Debug for RssError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}

impl std::error::Error for RssError {}

impl From<ReqwestError> for RssError {
    fn from(error: ReqwestError) -> Self {
        RssError::new(error)
    }
}

#[derive(Debug)]
pub struct FilterRegexes {
    pub title_regex: Option<Regex>,
    pub guid_regex: Option<Regex>,
    pub link_regex: Option<Regex>,
}

#[derive(Debug)]
pub struct RssFilter {
    filter_regexes: FilterRegexes,

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

impl RssFilter {
    pub fn new(filter_regexes: FilterRegexes) -> Result<Self, ReqwestError> {
        let reqwest_client = default_reqwest_client()?;

        Ok(Self::new_with_client(filter_regexes, reqwest_client))
    }

    pub fn new_with_client(filter_regexes: FilterRegexes, reqwest_client: reqwest::Client) -> Self {
        RssFilter {
            filter_regexes,
            reqwest_client,
        }
    }

    #[instrument(skip(self))]
    pub async fn fetch(&self, url: &str, headers: HeaderMap) -> Result<Response, ReqwestError> {
        info!("Requesting URL");
        self.reqwest_client.get(url).headers(headers).send().await
    }

    #[instrument(skip(self, response), fields(status = %response.status()))]
    pub async fn filter_response(&self, response: Response) -> Result<String, BoxError> {
        info!("Received response");
        let content = response.bytes().await?;
        let channel = Channel::read_from(&content[..])?;

        Ok(self.filter(channel))
    }

    pub async fn fetch_and_filter(&self, url: &str) -> Result<String, BoxError> {
        self.fetch(url, HeaderMap::new())
            .await?
            .error_for_status()
            .map_err(RssError::from)
            .map(|resp| self.filter_response(resp))?
            .await
    }

    #[instrument(skip(self))]
    fn filter_out(&self, regex: &Option<Regex>, value: Option<&str>) -> bool {
        if let Some(regex) = regex {
            if let Some(val) = value {
                if regex.is_match(val) {
                    debug!("Filtering out item");
                    return true;
                }
            }
        }

        false
    }

    #[instrument(skip(self, channel))]
    fn filter(&self, mut channel: Channel) -> String {
        info!("Filtering items from RSS feed");

        let [title_regex, guid_regex, link_regex] = [
            &self.filter_regexes.title_regex,
            &self.filter_regexes.guid_regex,
            &self.filter_regexes.link_regex,
        ];
        let items: Vec<_> = channel
            .items()
            .iter()
            .filter(|item| {
                ![
                    (title_regex, item.title()),
                    (guid_regex, item.guid().map(|guid| guid.value())),
                    (link_regex, item.link()),
                ]
                .iter()
                .any(|(regex, value)| self.filter_out(regex, *value))
            })
            // TODO: can I avoid this clone?
            .cloned()
            .collect();

        channel.set_items(items);

        // `Channel.toString()` doesn't use pretty printing
        let buf = channel
            .pretty_write_to(Vec::new(), b' ', 2)
            .unwrap_or_default();
        String::from_utf8(buf).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use test_case::test_case;

    use test_utils::serve_test_rss_feed;

    async fn filter(
        fr: FilterRegexes,
        url: &str,
        expected: Vec<Option<&str>>,
    ) -> Result<(), BoxError> {
        let unfiltered_feed = RssFilter::new(fr)?.fetch_and_filter(url).await?;

        let channel = Channel::read_from(unfiltered_feed.as_bytes())?;
        let titles = channel
            .items()
            .iter()
            .map(|i| i.title())
            .collect::<Vec<_>>();

        assert!(titles == expected);

        Ok(())
    }

    // Copy all the test cases from `filters` below
    #[test_case(FilterRegexes {
        title_regex: Some(Regex::new("^Test Item 1$").unwrap()),
        guid_regex: None,
        link_regex: None,
    }, vec![Some("Test Item 2")] ; "title filter only")]
    #[test_case(FilterRegexes {
        title_regex: None,
        guid_regex: Some(Regex::new("1").unwrap()),
        link_regex: None,
    }, vec![Some("Test Item 2")] ; "guid filter only")]
    #[test_case(FilterRegexes {
        title_regex: None,
        guid_regex: None,
        link_regex: Some(Regex::new("test2").unwrap()),
    }, vec![Some("Test Item 1")] ; "link filter only")]
    #[test_case(FilterRegexes {
        title_regex: None,
        guid_regex: None,
        link_regex: None,
    }, vec![Some("Test Item 1"), Some("Test Item 2")] ; "no filters")]
    #[tokio::test]
    async fn test_fetch_and_filter(
        filter_regexes: FilterRegexes,
        expected: Vec<Option<&str>>,
    ) -> Result<(), BoxError> {
        let server = serve_test_rss_feed(&["1", "2"]).await?;
        let url = server.url();

        filter(filter_regexes, &url, expected).await?;

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

        let result = RssFilter::new(FilterRegexes {
            title_regex: None,
            guid_regex: None,
            link_regex: None,
        })?
        .fetch_and_filter(&url)
        .await;

        assert!(result.is_err());

        assert!(result.unwrap_err().is::<RssError>());

        Ok(())
    }
}
