use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use futures::SinkExt;
use futures::StreamExt;
use futures::channel::mpsc;
use serde::Deserialize;
use tokio::sync::Mutex;
use tokio::time;

use chromiumoxide::cdp::browser_protocol::network::{
    EnableParams, EventLoadingFinished, EventResponseReceived, GetResponseBodyParams,
};
use chromiumoxide::error::CdpError;
use chromiumoxide::page::Page;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("enable_network: {0}")]
    EnableNetwork(CdpError),
    #[error("get_response_body: {0}")]
    GetResponseBody(CdpError),
    #[error("base64_decode: {0}")]
    Base64Decode(base64::DecodeError),
}

#[derive(Clone, Debug, Default)]
pub struct EventStreamConfig {
    pub url_substring_filter: Option<String>,
    pub content_type_substring_filter: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Event {
    pub url: String,
    #[serde(rename = "contentType", default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub status: Option<u16>,
    pub body: String,
}

// Internal structure to track pending responses
#[derive(Clone, Debug)]
struct PendingResponse {
    url: String,
    content_type: Option<String>,
    status: Option<u16>,
}

// Helper function to check if an event should be captured
fn should_capture(config: &EventStreamConfig, url: &str, content_type: Option<&str>) -> bool {
    let url_ok = config
        .url_substring_filter
        .as_ref()
        .map(|filter| url.contains(filter))
        .unwrap_or(true);

    let ct_ok = config
        .content_type_substring_filter
        .as_ref()
        .map(|filter| content_type.map(|ct| ct.contains(filter)).unwrap_or(false))
        .unwrap_or(true);

    url_ok && ct_ok
}

/// Start a background task that captures network events via CDP and streams them over a mpsc channel.
/// Returns the receiver; the task ends when the `Page` errors or the sender is dropped.
pub async fn start_event_stream(
    page: Page,
    config: EventStreamConfig,
) -> Result<mpsc::UnboundedReceiver<Event>, Error> {
    // Enable network tracking via CDP
    page.execute(EnableParams::default())
        .await
        .map_err(Error::EnableNetwork)?;

    let (mut tx, rx) = mpsc::unbounded();

    // Shared state to track pending responses (request_id -> metadata)
    // Use RequestId directly by cloning it
    let pending_responses: Arc<Mutex<HashMap<String, PendingResponse>>> =
        Arc::new(Mutex::new(HashMap::new()));

    let pending_clone = pending_responses.clone();

    // Spawn task to handle response received events
    let page_response = page.clone();
    tokio::spawn(async move {
        let mut events = match page_response
            .event_listener::<EventResponseReceived>()
            .await
        {
            Ok(e) => e,
            Err(_) => return, // page error
        };

        while let Some(event) = events.next().await {
            // Extract response metadata
            let url = event.response.url.clone();
            let status = Some(event.response.status as u16);
            let headers = &event.response.headers;

            // Extract content-type from headers
            let headers_value = headers.inner();
            let content_type = headers_value
                .get("content-type")
                .or_else(|| headers_value.get("Content-Type"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            // Check if we should capture this response
            if should_capture(&config, &url, content_type.as_deref()) {
                let pending = PendingResponse {
                    url,
                    content_type,
                    status,
                };

                // Store pending response by request_id (use inner() to get String)
                pending_clone
                    .lock()
                    .await
                    .insert(event.request_id.inner().clone(), pending);
            }
        }
    });

    // Spawn task to handle loading finished events and fetch bodies
    tokio::spawn(async move {
        let mut events = match page.event_listener::<EventLoadingFinished>().await {
            Ok(e) => e,
            Err(_) => return, // page error
        };

        while let Some(event) = events.next().await {
            let request_id_str = event.request_id.inner().clone();

            // Get pending response metadata
            let pending = pending_responses.lock().await.remove(&request_id_str);
            let pending = match pending {
                Some(p) => p,
                None => continue, // Not a response we're tracking
            };

            // Fetch the response body
            let body_result = page
                .execute(GetResponseBodyParams {
                    request_id: event.request_id.clone(),
                })
                .await;

            let body = match body_result {
                Ok(result) => {
                    // CDP returns body as base64 if binary, or plain text
                    if result.base64_encoded {
                        // Decode base64
                        match base64::engine::general_purpose::STANDARD.decode(&result.body) {
                            Ok(bytes) => String::from_utf8_lossy(&bytes).to_string(),
                            Err(e) => {
                                eprintln!("Failed to decode base64 body: {}", e);
                                continue;
                            }
                        }
                    } else {
                        result.body.clone()
                    }
                }
                Err(_) => {
                    // Failed to get body, skip this event
                    continue;
                }
            };

            // Create and send event
            let event = Event {
                url: pending.url,
                content_type: pending.content_type,
                status: pending.status,
                body,
            };

            if tx.send(event).await.is_err() {
                return; // receiver dropped
            }
        }
    });

    Ok(rx)
}

pub enum EventResult {
    Timeout,
    StreamClosed,
    Ok(Event),
}

/// Wait for the next event from the receiver with a timeout.
/// Returns `Ok(Some(event))` if an event is received, `Ok(None)` if the stream is closed,
/// or `Err(())` if the timeout expires before an event is received.
pub async fn wait_for_event_with_timeout(
    rx: &mut mpsc::UnboundedReceiver<Event>,
    timeout: Duration,
) -> EventResult {
    match time::timeout(timeout, rx.next()).await {
        Ok(Some(event)) => EventResult::Ok(event),
        Ok(None) => EventResult::StreamClosed,
        Err(_) => EventResult::Timeout,
    }
}
