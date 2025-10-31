use std::time::Duration;

use futures::channel::mpsc;
use futures::SinkExt;
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
pub struct JsonStreamConfig {
    pub poll_interval_ms: u64,
    pub url_substring_filter: Option<String>,
    pub content_type_substring_filter: Option<String>,
}

impl Default for JsonStreamConfig {
    fn default() -> Self {
        Self {
            poll_interval_ms: 300,
            url_substring_filter: None,
            content_type_substring_filter: Some("application/json".to_string()),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct JsonEvent {
    pub url: String,
    #[serde(rename = "contentType", default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub status: Option<u16>,
    pub body: String,
}

/// Install JS hooks to capture JSON responses from fetch/XHR into a window buffer.
pub async fn install_json_hooks(page: &Page, config: &JsonStreamConfig) -> Result<(), Error> {
    let url_filter_js = serde_json::to_string(&config.url_substring_filter).unwrap_or("null".into());
    let ct_filter_js = serde_json::to_string(&config.content_type_substring_filter).unwrap_or("null".into());

    let js = format!(
        r#"(function(cfg){{
  try {{
    window.__json_stream = window.__json_stream || [];
    const urlFilter = cfg.urlFilter; // string or null
    const ctFilter = cfg.ctFilter;   // string or null

    function shouldCapture(url, ct) {{
      const okUrl = !urlFilter || (url && url.indexOf(urlFilter) !== -1);
      const okCt = !ctFilter || (ct && ct.indexOf(ctFilter) !== -1);
      const looksJson = ct && ct.indexOf('application/json') !== -1;
      return okUrl && (okCt || looksJson);
    }}

    // fetch hook
    if (!window.__json_fetch_hooked) {{
      window.__json_fetch_hooked = true;
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
                window.__json_stream.push({{ url: url, body: txt, contentType: ct, status: res.status }});
              }} catch(e) {{}}
            }});
          }}
        }} catch(e) {{}}
        return res;
      }};
    }}

    // XHR hook
    if (!window.__json_xhr_hooked) {{
      window.__json_xhr_hooked = true;
      const origOpen = XMLHttpRequest.prototype.open;
      const origSend = XMLHttpRequest.prototype.send;
      XMLHttpRequest.prototype.open = function(method, url) {{
        try {{ this.__json_url = url; }} catch(e) {{}}
        return origOpen.apply(this, arguments);
      }};
      XMLHttpRequest.prototype.send = function(body) {{
        this.addEventListener('load', function() {{
          try {{
            const ct = (this.getResponseHeader && this.getResponseHeader('content-type')) || '';
            const url = this.responseURL || this.__json_url || '';
            if (shouldCapture(url, ct)) {{
              window.__json_stream.push({{ url: url, body: this.responseText || '', contentType: ct, status: this.status }});
            }}
          }} catch(e) {{}}
        }});
        return origSend.apply(this, arguments);
      }};
    }}
  }} catch(e) {{}}
}})({{ urlFilter: {}, ctFilter: {} }});"#,
        url_filter_js,
        ct_filter_js,
    );

    page.evaluate_expression(js)
        .await
        .map_err(Error::InjectJs)?;
    Ok(())
}

/// Drain and parse all captured JSON events from the page buffer.
pub async fn drain_json_events(page: &Page) -> Result<Vec<JsonEvent>, Error> {
    let js = "(() => { try { if (!window.__json_stream) return '[]'; const a = window.__json_stream.splice(0); return JSON.stringify(a); } catch(e) { return '[]'; } })()";
    let mut s: String = page
        .evaluate_expression(js)
        .await
        .map_err(Error::DrainJs)?
        .into_value()
        .unwrap_or_default();
    if s.is_empty() { s = "[]".to_string(); }
    let events: Vec<JsonEvent> = serde_json::from_str(&s).map_err(Error::ParseJson)?;
    Ok(events)
}

/// Start a background task that polls for captured JSON events and streams them over a mpsc channel.
/// Returns the receiver; the task ends when the `Page` errors or the sender is dropped.
pub async fn start_json_stream(
    page: Page,
    config: JsonStreamConfig,
) -> Result<mpsc::UnboundedReceiver<JsonEvent>, Error> {
    install_json_hooks(&page, &config).await?;

    let (mut tx, rx) = mpsc::unbounded();
    let interval = config.poll_interval_ms;

    tokio::spawn(async move {
        loop {
            match drain_json_events(&page).await {
                Ok(events) => {
                    for ev in events {
                        if tx.send(ev).await.is_err() {
                            return; // receiver dropped
                        }
                    }
                },
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


