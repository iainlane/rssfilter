//! The rssfilter landing page: a Leptos client-side app that loads a feed via
//! the worker's `/api/feed` endpoint and previews, live, which items the given
//! filters would remove. Filtering reuses `rssfilter-core` so the preview
//! matches what the worker serves exactly.

use leptos::prelude::*;
use leptos_use::signal_debounced;
use regex::Regex;
use rssfilter_core::{FeedItem, FilterRegexes};

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(App);
}

/// Fetch the feed's items as JSON from the same-origin worker endpoint.
async fn fetch_items(feed_url: String) -> Result<Vec<FeedItem>, String> {
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

/// Compile a single optional regex from a text input (empty input = no filter).
fn compile(input: &str) -> Result<Vec<Regex>, regex::Error> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        Ok(Vec::new())
    } else {
        Ok(vec![Regex::new(trimmed)?])
    }
}

/// Owned compiled filters, so the last valid set can live in a signal.
#[derive(Clone, Default)]
struct CompiledFilters {
    title: Vec<Regex>,
    guid: Vec<Regex>,
    link: Vec<Regex>,
}

impl CompiledFilters {
    fn as_regexes(&self) -> FilterRegexes<'_> {
        FilterRegexes {
            title_regexes: &self.title,
            guid_regexes: &self.guid,
            link_regexes: &self.link,
        }
    }
}

fn compile_all(title: &str, guid: &str, link: &str) -> Result<CompiledFilters, String> {
    let one = |s: &str| compile(s).map_err(|e| e.to_string());
    Ok(CompiledFilters {
        title: one(title)?,
        guid: one(guid)?,
        link: one(link)?,
    })
}

/// Whether a feed-provided URL is safe to render as a clickable link. Feeds are
/// untrusted, so reject anything that isn't plain http(s) (e.g. `javascript:`).
fn is_safe_link(href: &str) -> bool {
    let lower = href.trim().to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// The feed-reader URL the current inputs describe (empty filters omitted).
fn build_feed_url(feed: &str, title: &str, guid: &str, link: &str) -> String {
    let mut params = vec![format!("url={}", urlencoding::encode(feed))];
    for (key, value) in [
        ("title_filter_regex", title),
        ("guid_filter_regex", guid),
        ("link_filter_regex", link),
    ] {
        let value = value.trim();
        if !value.is_empty() {
            params.push(format!("{key}={}", urlencoding::encode(value)));
        }
    }
    format!("/?{}", params.join("&"))
}

#[component]
fn FilterInput(id: &'static str, label: &'static str, value: RwSignal<String>) -> impl IntoView {
    view! {
        <label for=id>{label}</label>
        <input
            id=id
            type="text"
            prop:value=move || value.get()
            on:input=move |ev| value.set(event_target_value(&ev))
        />
    }
}

#[component]
fn App() -> impl IntoView {
    let feed = RwSignal::new(String::new());
    let title = RwSignal::new(String::new());
    let guid = RwSignal::new(String::new());
    let link = RwSignal::new(String::new());

    let load = Action::new_local(|url: &String| {
        let url = url.clone();
        async move { fetch_items(url).await }
    });

    // Debounce the filter inputs so the preview only recomputes once typing
    // settles. (The displayed feed URL below stays live, off the raw signals.)
    let title_d: Signal<String> = signal_debounced(title, 250.0);
    let guid_d: Signal<String> = signal_debounced(guid, 250.0);
    let link_d: Signal<String> = signal_debounced(link, 250.0);

    // Hold the last *valid* compiled filters, so an in-progress invalid regex
    // surfaces an error without throwing away the current preview.
    let filters = RwSignal::new(CompiledFilters::default());
    let regex_error = RwSignal::new(None::<String>);
    Effect::new(
        move |_| match compile_all(&title_d.get(), &guid_d.get(), &link_d.get()) {
            Ok(compiled) => {
                filters.set(compiled);
                regex_error.set(None);
            }
            Err(e) => regex_error.set(Some(e)),
        },
    );

    let preview = move || {
        if load.pending().get() {
            return view! { <p>"Loading\u{2026}"</p> }.into_any();
        }
        let items = match load.value().get() {
            None => return ().into_any(),
            Some(Err(e)) => {
                return view! { <p class="error">{format!("Failed to load feed: {e}")}</p> }
                    .into_any();
            }
            Some(Ok(items)) => items,
        };

        let total = items.len();
        let items_for_count = items.clone();
        let summary = move || {
            let kept = filters.with(|f| {
                let regexes = f.as_regexes();
                items_for_count
                    .iter()
                    .filter(|item| !item.filtered_out(&regexes))
                    .count()
            });
            format!("Keeping {kept} of {total} items")
        };

        // Rows are built once; only each row's `removed` class is reactive, so
        // changing a filter toggles strikethroughs rather than rebuilding the list.
        let rows = items
            .into_iter()
            .map(|item| {
                let label = item.title.clone().unwrap_or_else(|| "(untitled)".into());
                let href = item.link.clone();
                view! {
                    <li class:removed=move || {
                        filters.with(|f| item.filtered_out(&f.as_regexes()))
                    }>
                        <span class="item-title">{label}</span>
                        {href.map(|href| {
                            if is_safe_link(&href) {
                                let text = href.clone();
                                view! {
                                    <a class="item-link" href=href target="_blank" rel="noopener">
                                        {text}
                                    </a>
                                }
                                .into_any()
                            } else {
                                view! { <span class="item-link">{href}</span> }.into_any()
                            }
                        })}
                    </li>
                }
            })
            .collect_view();

        view! {
            <p class="summary">{summary}</p>
            <ul class="items">{rows}</ul>
        }
        .into_any()
    };

    view! {
        <h1>"rssfilter"</h1>
        <p class="tagline">
            "Filter an RSS feed by title, link, or GUID. Load a feed, type filters, and see "
            "which items would be removed \u{2014} then copy the feed URL into your reader."
        </p>

        <form on:submit=move |ev| {
            ev.prevent_default();
            load.dispatch(feed.get());
        }>
            <label for="url">"Feed URL"</label>
            <input
                id="url"
                type="text"
                required=true
                prop:value=move || feed.get()
                on:input=move |ev| feed.set(event_target_value(&ev))
            />
            <button type="submit">"Load feed"</button>
        </form>

        <FilterInput id="title" label="Title filter regex" value=title />
        <FilterInput id="guid" label="GUID filter regex" value=guid />
        <FilterInput id="link" label="Link filter regex" value=link />

        {move || {
            regex_error.get().map(|e| view! { <p class="error">{format!("Invalid regex: {e}")}</p> })
        }}

        <p class="feed-url">
            "Feed URL: "
            <code>
                {move || build_feed_url(&feed.get(), &title.get(), &guid.get(), &link.get())}
            </code>
        </p>

        {preview}
    }
}
