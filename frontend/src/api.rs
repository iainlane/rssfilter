//! Talking to the worker's `/api/feed` endpoint.

use rssfilter_core::FeedItem;

/// Fetch the feed's items as JSON from the same-origin worker endpoint.
pub async fn fetch_items(feed_url: String) -> Result<Vec<FeedItem>, String> {
    let api = format!("/api/feed?url={}", urlencoding::encode(&feed_url));
    let resp = gloo_net::http::Request::get(&api)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.ok() {
        return Err(format!("server returned HTTP {}", resp.status()));
    }
    let text = resp.text().await.map_err(|e| e.to_string())?;
    serde_json::from_str::<Vec<FeedItem>>(&text).map_err(|e| e.to_string())
}
