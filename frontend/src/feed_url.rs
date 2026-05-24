//! Building the absolute feed-reader URL the user can copy into their reader,
//! plus a small helper that decides whether a feed-provided URL is safe to
//! render as a clickable link.

use std::sync::OnceLock;

use rssfilter_core::{
    GUID_FILTER_REGEX_PARAM, LINK_FILTER_REGEX_PARAM, TITLE_FILTER_REGEX_PARAM, URL_PARAM,
};

/// Whether a feed-provided URL is safe to render as a clickable link. Only
/// `http(s)` URLs become anchors; anything else (e.g. `javascript:`) is shown
/// as text.
pub fn is_safe_link(href: &str) -> bool {
    let lower = href.trim().to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// The current page's origin (e.g. `https://rssfilter.orangesquash.org.uk`),
/// cached after the first call.
fn current_origin() -> &'static str {
    static ORIGIN: OnceLock<String> = OnceLock::new();
    ORIGIN.get_or_init(|| {
        web_sys::window()
            .and_then(|w| w.location().origin().ok())
            .unwrap_or_default()
    })
}

/// The fully-qualified feed-reader URL the current inputs describe. Empty
/// filter inputs are omitted.
pub fn build_feed_url(feed: &str, title: &str, guid: &str, link: &str) -> String {
    let mut params = vec![format!("{URL_PARAM}={}", urlencoding::encode(feed))];
    for (key, value) in [
        (TITLE_FILTER_REGEX_PARAM, title),
        (GUID_FILTER_REGEX_PARAM, guid),
        (LINK_FILTER_REGEX_PARAM, link),
    ] {
        let value = value.trim();
        if !value.is_empty() {
            params.push(format!("{key}={}", urlencoding::encode(value)));
        }
    }
    format!("{}/?{}", current_origin(), params.join("&"))
}
