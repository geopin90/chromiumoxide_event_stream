# chromiumoxide_event_stream
Capture network responses from Chromium pages using chromiumoxide CDP (Chrome DevTools Protocol). Stream events back to Rust with optional URL and content-type filtering. Works with any text-based content.

## Features
- **Event-driven**: Uses Chrome DevTools Protocol (CDP) network events for reliable, real-time monitoring
- **Comprehensive coverage**: Captures all network requests, not just fetch/XHR
- **Streaming API**: Returns `mpsc::UnboundedReceiver<Event>` for async processing
- **Flexible filtering**: Filter by URL substring and/or content-type substring
- **Automatic base64 decoding**: Handles binary responses transparently

## Why Use This Library?

While you *could* use chromiumoxide's CDP APIs directly, this library saves you from writing ~150+ lines of complex coordination code:

### What This Library Provides

**One function call**:

```rust
let mut rx = start_event_stream(
    page,
    EventStreamConfig {
        url_substring_filter: Some("/api/".to_string()),
        content_type_substring_filter: Some("application/json".to_string()),
    },
).await?;

while let Some(event) = rx.next().await {
    // Use the event - URL, headers, status, body all ready
    println!("{} -> {} bytes", ev.url, ev.body.len());
}
```

**Result**: ~150 lines of complex async code → **3 lines of simple API usage**

### When to Use chromiumoxide Directly

Use chromiumoxide directly if you need:
- Fine-grained control over individual CDP commands
- Custom event coordination logic
- Access to raw CDP event structures
- Very specific performance optimizations

### For the common case of "capture network responses with filtering", this library is the right choice.

## What event types can I capture?

The library captures all network responses via CDP, including:
- **HTML**: `text/html`
- **JSON**: `application/json`
- **XML**: `application/xml`, `text/xml`
- **Plain text**: `text/plain` (including logs, NDJSON if sent as text)
- **CSV/TSV**: `text/csv`, `text/tab-separated-values`
- **JavaScript/CSS**: `application/javascript`, `text/javascript`, `text/css`
- **SVG**: `image/svg+xml` (XML text format)
- **Form responses**: `application/x-www-form-urlencoded`, `multipart/form-data` (text parts)
- **Binary Responses**: Binary payloads (e.g., `image/png`, `application/pdf`, `application/octet-stream`) are automatically base64-decoded when possible. The body is converted to a UTF-8 string using lossy conversion (`String::from_utf8_lossy`), so binary data may not be fully recoverable. For true binary handling, consider using chromiumoxide's CDP APIs directly.

---

This project was bootstrapped with the assistance of Cursor AI.