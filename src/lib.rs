use std::time::Duration;

use futures::SinkExt;
use futures::channel::mpsc;
use serde::Deserialize;

use chromiumoxide::error::CdpError;
use chromiumoxide::page::Page;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("inject_js: {0}")]
    InjectJs(CdpError),
    #[error("drain_js: {0}")]
    DrainJs(CdpError),
    #[error("parse_json: {0}")]
    ParseJson(serde_json::Error),
}

#[derive(Clone, Debug)]
pub struct EventStreamConfig {
    pub poll_interval_ms: u64,
    pub url_substring_filter: Option<String>,
    pub content_type_substring_filter: Option<String>,
}

impl Default for EventStreamConfig {
    fn default() -> Self {
        Self {
            poll_interval_ms: 300,
            url_substring_filter: None,
            content_type_substring_filter: None,
        }
    }
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

/// Install JS hooks to capture responses (any content-type) from fetch/XHR into a window buffer.
async fn install_event_hooks(page: &Page, config: &EventStreamConfig) -> Result<(), Error> {
    let url_filter_js =
        serde_json::to_string(&config.url_substring_filter).unwrap_or("null".into());
    let ct_filter_js =
        serde_json::to_string(&config.content_type_substring_filter).unwrap_or("null".into());

    let js = format!(
        r#"(function(cfg){{
  try {{
    window.__event_stream = window.__event_stream || [];
    const urlFilter = cfg.urlFilter; // string or null
    const ctFilter = cfg.ctFilter;   // string or null

    function shouldCapture(url, ct) {{
      const okUrl = !urlFilter || (url && url.indexOf(urlFilter) !== -1);
      const okCt = !ctFilter || (ct && ct.indexOf(ctFilter) !== -1);
      return okUrl && okCt;
    }}

    // fetch hook
    if (!window.__event_fetch_hooked) {{
      window.__event_fetch_hooked = true;
      const origFetch = window.fetch;
      window.fetch = async function(input, init) {{
        const res = await origFetch.apply(this, arguments);
        try {{
          const ct = (res.headers && res.headers.get && res.headers.get('content-type')) || '';
          const url = res.url || (typeof input === 'string' ? input : (input && input.url) || '');
          if (shouldCapture(url, ct)) {{
            const clone = res.clone();
            clone.text().then(function(txt) {{
              try {{
                window.__event_stream.push({{ url: url, body: txt, contentType: ct, status: res.status }});
              }} catch(e) {{}}
            }});
          }}
        }} catch(e) {{}}
        return res;
      }};
    }}

    // XHR hook
    if (!window.__event_xhr_hooked) {{
      window.__event_xhr_hooked = true;
      const origOpen = XMLHttpRequest.prototype.open;
      const origSend = XMLHttpRequest.prototype.send;
      XMLHttpRequest.prototype.open = function(method, url) {{
        try {{ this.__event_url = url; }} catch(e) {{}}
        return origOpen.apply(this, arguments);
      }};
      XMLHttpRequest.prototype.send = function(body) {{
        this.addEventListener('load', function() {{
          try {{
            const ct = (this.getResponseHeader && this.getResponseHeader('content-type')) || '';
            const url = this.responseURL || this.__event_url || '';
            if (shouldCapture(url, ct)) {{
              window.__event_stream.push({{ url: url, body: this.responseText || '', contentType: ct, status: this.status }});
            }}
          }} catch(e) {{}}
        }});
        return origSend.apply(this, arguments);
      }};
    }}
  }} catch(e) {{}}
}})({{ urlFilter: {}, ctFilter: {} }});"#,
        url_filter_js, ct_filter_js,
    );

    page.evaluate_expression(js)
        .await
        .map_err(Error::InjectJs)?;
    Ok(())
}

/// Drain and parse all captured raw events from the page buffer.
async fn drain_events(page: &Page) -> Result<Vec<Event>, Error> {
    let js = "(() => { try { if (!window.__event_stream) return '[]'; const a = window.__event_stream.splice(0); return JSON.stringify(a); } catch(e) { return '[]'; } })()";
    let mut s: String = page
        .evaluate_expression(js)
        .await
        .map_err(Error::DrainJs)?
        .into_value()
        .unwrap_or_default();
    if s.is_empty() {
        s = "[]".to_string();
    }
    let events: Vec<Event> = serde_json::from_str(&s).map_err(Error::ParseJson)?;
    Ok(events)
}

/// Start a background task that polls for captured events and streams them over a mpsc channel.
/// Returns the receiver; the task ends when the `Page` errors or the sender is dropped.
pub async fn start_event_stream(
    page: Page,
    config: EventStreamConfig,
) -> Result<mpsc::UnboundedReceiver<Event>, Error> {
    install_event_hooks(&page, &config).await?;

    let (mut tx, rx) = mpsc::unbounded();
    let interval = config.poll_interval_ms;

    tokio::spawn(async move {
        loop {
            match drain_events(&page).await {
                Ok(events) => {
                    for ev in events {
                        if tx.send(ev).await.is_err() {
                            return; // receiver dropped
                        }
                    }
                }
                Err(_e) => {
                    // page likely went away; stop
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(interval)).await;
        }
    });

    Ok(rx)
}
