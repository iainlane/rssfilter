use std::{fmt, str::FromStr};

use headers::{Header, HeaderName, HeaderValue};

use crate::header_cf_cache_status::CfCacheStatus;

/// Typed header for `x-rssfilter-cache-status`, wrapping `CfCacheStatus`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RssFilterCacheStatus(pub CfCacheStatus);

impl fmt::Display for RssFilterCacheStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for RssFilterCacheStatus {
    type Err = headers::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(CfCacheStatus::from_str(s)?))
    }
}

impl Header for RssFilterCacheStatus {
    fn name() -> &'static HeaderName {
        static NAME: HeaderName = HeaderName::from_static("x-rssfilter-cache-status");
        &NAME
    }

    fn decode<'i, I>(values: &mut I) -> Result<Self, headers::Error>
    where
        I: Iterator<Item = &'i HeaderValue>,
    {
        let value = values.next().ok_or_else(headers::Error::invalid)?;
        let s = value.to_str().map_err(|_| headers::Error::invalid())?;
        RssFilterCacheStatus::from_str(s)
    }

    fn encode<E>(&self, values: &mut E)
    where
        E: Extend<HeaderValue>,
    {
        self.0.encode(values);
    }
}
