# chromiumoxide_event_stream
Capture network responses from Chromium pages using chromiumoxide, with simple JS hooks for fetch/XHR. Stream events back to Rust with optional URL and content-type filtering. Works with any text-based content.

## Features
- Capture fetch/XHR responses into a page-side buffer
- Poll and stream events to Rust using `tokio` and `futures::channel::mpsc`
- Filter by URL substring and/or content-type substring
- API for Generic text events (`start_event_stream`, `Event`)

## Quick start (generic events)
```rust
use chromiumoxide::Browser; // set up your browser/Page as usual
use chromiumoxide_event_stream::{
    EventStreamConfig,
    start_event_stream,
};

# async fn demo(page: chromiumoxide::page::Page) -> anyhow::Result<()> {
let rx = start_event_stream(
    page,
    EventStreamConfig {
        poll_interval_ms: 300,
        url_substring_filter: Some("/Search".to_string()),                    // e.g. Some("/api/")
        content_type_substring_filter: Some("application/json".to_string()),  // e.g. Some("text/html")
    },
).await?;

tokio::spawn(async move {
    let mut rx = rx;
    while let Some(ev) = rx.next().await {
        println!("{} {} -> {} bytes", ev.status.unwrap_or(0), ev.url, ev.body.len());
    }
});
# Ok(()) }
```

## What event types can I capture?
Anything text-based returned by fetch/XHR:
- HTML: `text/html`
- JSON: `application/json`
- XML: `application/xml`, `text/xml`
- Plain text: `text/plain` (including logs, NDJSON if sent as text)
- CSV/TSV: `text/csv`, `text/tab-separated-values`
- JavaScript/CSS: `application/javascript`, `text/javascript`, `text/css`
- SVG: `image/svg+xml`
- Form responses: `application/x-www-form-urlencoded`, `multipart/form-data` (text parts only)

#### Warning:
Binary payloads (e.g., `image/png`, `application/pdf`, `application/octet-stream`) will be coerced via `response.text()` and get mangled. If you need binary, consider extending the hooks to `arrayBuffer()` + base64.

## API overview
- `install_event_hooks(&Page, &EventStreamConfig)` – install generic hooks
- `drain_events(&Page) -> Vec<Event>` – drain all buffered events
- `start_event_stream(Page, EventStreamConfig) -> mpsc::UnboundedReceiver<Event>` – spawn poller and get a receiver

## Filters
- `url_substring_filter`: only capture events whose URL contains this substring.
- `content_type_substring_filter`: only capture events whose content-type contains this substring.

Examples:
- Only HTML: `content_type_substring_filter = Some("text/html".into())`
- Only CSV: `content_type_substring_filter = Some("text/csv".into())`
- All text: leave both filters as `None` and filter client-side.

This project was bootstrapped with the assistance of Cursor AI.