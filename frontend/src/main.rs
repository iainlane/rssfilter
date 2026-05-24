//! The rssfilter landing page: a Leptos client-side app that loads a feed via
//! the worker's `/api/feed` endpoint and previews, live, which items the given
//! filters would remove. Filtering reuses `rssfilter-core` so the preview
//! matches what the worker serves exactly.

mod api;
mod feed_url;
mod filters;

use leptos::prelude::*;
use leptos_use::signal_debounced;
use rssfilter_core::{FeedItem, FilterRegexes};

use crate::api::fetch_items;
use crate::feed_url::{build_feed_url, is_safe_link};
use crate::filters::compile_all;

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(App);
}

#[component]
fn FilterInput(id: &'static str, label: &'static str, value: RwSignal<String>) -> impl IntoView {
    view! {
        <label for=id>{label}</label>
        <input id=id type="text" bind:value=value />
    }
}

/// The loaded feed with a live indication of which items the current filters
/// remove. The removed-set is computed once per filter change and shared by the
/// summary and the rows; each row's `removed` class is the only reactive bit,
/// so changing a filter just toggles the strikethroughs.
#[component]
fn Preview(items: Vec<FeedItem>, filters: RwSignal<FilterRegexes>) -> impl IntoView {
    // One copy of the item list, shared by the removed-set memo and the rows.
    let items = StoredValue::new(items);
    let total = items.with_value(Vec::len);

    let removed = Memo::new(move |_| {
        filters.with(|f| {
            items.with_value(|items| {
                items
                    .iter()
                    .map(|item| item.filtered_out(f))
                    .collect::<Vec<bool>>()
            })
        })
    });
    let kept = move || removed.with(|flags| flags.iter().filter(|&&removed| !removed).count());

    let rows = (0..total)
        .map(|idx| {
            let label = items
                .with_value(|items| items[idx].title.clone())
                .unwrap_or_else(|| "(untitled)".into());
            let href = items.with_value(|items| items[idx].link.clone());
            view! {
                <li class:removed=move || removed.with(|flags| flags[idx])>
                    <span class="item-title">{label}</span>
                    {href.map(|href| {
                        if is_safe_link(&href) {
                            let text = href.clone();
                            view! {
                                <a
                                    class="item-link"
                                    href=href
                                    target="_blank"
                                    rel="noopener noreferrer"
                                >
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
        <p class="summary">{move || format!("Keeping {} of {} items", kept(), total)}</p>
        <ul class="items">{rows}</ul>
    }
}

#[component]
fn App() -> impl IntoView {
    let feed = RwSignal::new(String::new());
    let title = RwSignal::new(String::new());
    let guid = RwSignal::new(String::new());
    let link = RwSignal::new(String::new());

    // `gloo-net`'s future is `!Send` (it uses JS futures), so the local action
    // variant is the right fit for a CSR-only app.
    let load = Action::new_local(|url: &String| {
        let url = url.clone();
        async move { fetch_items(url).await }
    });

    // Debounce the filter inputs so the preview recomputes once typing settles.
    // (The displayed feed URL below updates live, off the raw signals.)
    let title_d: Signal<String> = signal_debounced(title, 250.0);
    let guid_d: Signal<String> = signal_debounced(guid, 250.0);
    let link_d: Signal<String> = signal_debounced(link, 250.0);

    // Hold the last *valid* compiled filters; an in-progress invalid regex
    // surfaces the error and the current preview stays as-is.
    let filters = RwSignal::new(FilterRegexes::default());
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
        match load.value().get() {
            None => ().into_any(),
            Some(Err(e)) => {
                view! { <p class="error">{format!("Failed to load feed: {e}")}</p> }.into_any()
            }
            Some(Ok(items)) => view! { <Preview items=items filters=filters /> }.into_any(),
        }
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
            <input id="url" type="text" required=true bind:value=feed />
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
