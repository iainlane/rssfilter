use bytes::Bytes;
use http::{Request, Response};
use http_body_util::Full;
use rssfilter_telemetry::WorkerConfig;
use worker::{Body, Context, Env, event};

use crate::real_main;

/// Main entry point for the RSS filter worker.
///
/// Accepts GET requests to "/" with query parameters:
/// - `url`: The RSS feed URL to filter (required)
/// - `title_filter_regex`: Regex to filter items by title (at least one filter required)
/// - `guid_filter_regex`: Regex to filter items by GUID (at least one filter required)
/// - `link_filter_regex`: Regex to filter items by link (at least one filter required)
///
/// Returns:
/// - 200: Filtered RSS feed
/// - 400: Invalid parameters or malformed request
/// - 404: Wrong path (not "/")
/// - 405: Wrong HTTP method (not GET)
/// - 413: RSS feed too large
/// - 415: Invalid content type (not RSS/XML)
/// - 422: Error processing the RSS feed
/// - 502: Error fetching the upstream RSS feed
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
    Ok(real_main(req, config).await.map(Full::new))
}
