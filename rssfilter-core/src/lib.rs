//! Filtering primitives shared between the Cloudflare Worker (`filter-rss-feed`)
//! and the browser preview (`frontend`).
//!
//! Keeping the per-item match decision here — depending only on `regex` — means
//! the live preview in the browser and the feed the worker actually serves use
//! identical regular-expression semantics.

use regex::Regex;
use serde::{Deserialize, Serialize};

/// A single RSS item, reduced to the fields we filter on. This is the JSON
/// contract between the worker's `/api/feed` endpoint (serialises) and the
/// browser preview (deserialises).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedItem {
    pub title: Option<String>,
    pub link: Option<String>,
    pub guid: Option<String>,
}

impl FeedItem {
    /// Whether this item is removed by the given filters.
    pub fn filtered_out(&self, filters: &FilterRegexes) -> bool {
        filters.item_filtered_out(
            self.title.as_deref(),
            self.guid.as_deref(),
            self.link.as_deref(),
        )
    }
}

/// The compiled regexes to match each RSS item field against.
#[derive(Debug)]
pub struct FilterRegexes<'a> {
    pub title_regexes: &'a [Regex],
    pub guid_regexes: &'a [Regex],
    pub link_regexes: &'a [Regex],
}

impl FilterRegexes<'_> {
    /// Whether an item with these fields should be removed from the feed, i.e.
    /// any configured regex matches the corresponding field.
    pub fn item_filtered_out(
        &self,
        title: Option<&str>,
        guid: Option<&str>,
        link: Option<&str>,
    ) -> bool {
        any_match(self.title_regexes, title)
            || any_match(self.guid_regexes, guid)
            || any_match(self.link_regexes, link)
    }
}

fn any_match(regexes: &[Regex], value: Option<&str>) -> bool {
    value.is_some_and(|v| regexes.iter().any(|r| r.is_match(v)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn re(p: &str) -> Regex {
        Regex::new(p).unwrap()
    }

    #[test]
    fn matches_on_any_field() {
        let title = [re("^Ad: ")];
        let guid = [re("sponsored")];
        let link = [re("/ads/")];
        let f = FilterRegexes {
            title_regexes: &title,
            guid_regexes: &guid,
            link_regexes: &link,
        };

        assert!(f.item_filtered_out(Some("Ad: buy now"), None, None));
        assert!(f.item_filtered_out(None, Some("post-sponsored-1"), None));
        assert!(f.item_filtered_out(None, None, Some("http://x/ads/1")));
        assert!(!f.item_filtered_out(Some("Real post"), Some("123"), Some("http://x/1")));
    }

    #[test]
    fn missing_field_never_matches() {
        let title = [re(".*")];
        let f = FilterRegexes {
            title_regexes: &title,
            guid_regexes: &[],
            link_regexes: &[],
        };
        // A `.*` title regex matches present titles but a missing title must not.
        assert!(f.item_filtered_out(Some(""), None, None));
        assert!(!f.item_filtered_out(None, None, None));
    }

    #[test]
    fn no_regexes_keeps_everything() {
        let f = FilterRegexes {
            title_regexes: &[],
            guid_regexes: &[],
            link_regexes: &[],
        };
        assert!(!f.item_filtered_out(Some("anything"), Some("g"), Some("l")));
    }
}
