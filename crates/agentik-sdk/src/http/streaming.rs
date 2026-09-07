//! HTTP streaming client for Server-Sent Events (SSE) processing.
//!
//! This module handles the HTTP layer for streaming responses from the Anthropic API,
//! parsing SSE events and converting them into MessageStreamEvent objects.

use futures::{Stream, StreamExt, TryStreamExt};
use pin_project::pin_project;
use reqwest::Response;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use crate::types::{AnthropicError, MessageStreamEvent, Result};
use crate::wire::{AnthropicWire, StreamState, WireProtocol};

/// Configuration for SSE streaming requests.
#[derive(Debug, Clone)]
pub struct StreamConfig {
    /// Buffer size for the event channel
    pub buffer_size: usize,
    /// Timeout for individual events (in seconds)
    pub event_timeout: Option<u64>,
    /// Whether to retry on connection errors
    pub retry_on_error: bool,
    /// Maximum retry attempts
    pub max_retries: Option<u32>,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            buffer_size: 1000,
            event_timeout: Some(300),
            retry_on_error: true,
            max_retries: Some(3),
        }
    }
}

/// HTTP streaming client for processing Server-Sent Events.
///
/// This client handles the low-level HTTP streaming and SSE parsing,
/// converting raw SSE data into structured MessageStreamEvent objects.
#[pin_project]
pub struct HttpStreamClient {
    /// The underlying SSE event stream
    #[pin]
    event_stream: Pin<Box<dyn Stream<Item = Result<MessageStreamEvent>> + Send>>,

    /// Configuration for the stream
    config: StreamConfig,

    /// Whether the stream has ended
    ended: bool,

    /// Request ID from response headers
    request_id: Option<String>,
}

impl HttpStreamClient {
    /// Create a new HTTP stream client from a response.
    ///
    /// This method takes a reqwest Response (which should be from a streaming endpoint)
    /// and converts it into a stream of MessageStreamEvent objects.
    pub async fn from_response(
        response: Response,
        config: StreamConfig,
        wire: Arc<dyn WireProtocol>,
    ) -> Result<Self> {
        let request_id = response
            .headers()
            .get("request-id")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        // Convert the HTTP response into an SSE stream
        let event_stream = Self::create_event_stream(response, wire).await?;

        Ok(Self {
            event_stream: Box::pin(event_stream),
            config,
            ended: false,
            request_id,
        })
    }

    /// Create a stream of MessageStreamEvent from an HTTP response.
    ///
    /// The raw SSE events are delegated to the wire protocol's
    /// [`WireProtocol::adapt_sse_event`] adapter, which translates them into
    /// the canonical Anthropic-shaped event stream.
    async fn create_event_stream(
        response: Response,
        wire: Arc<dyn WireProtocol>,
    ) -> Result<impl Stream<Item = Result<MessageStreamEvent>>> {
        // Check that we got a successful response
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            // Include the URL and status code in the error message so the user
            // can diagnose routing/config issues even when the response body is
            // empty (e.g. bare 404 from a gateway).
            let message = if text.trim().is_empty() {
                format!("[HTTP {}] (empty response body)", status.as_u16())
            } else {
                text
            };
            return Err(AnthropicError::from_status(status.as_u16(), message));
        }

        // Convert the response into a byte stream
        let byte_stream = response.bytes_stream();

        // Use eventsource-stream to parse SSE events
        use eventsource_stream::Eventsource;

        // Wrap each byte chunk's error in a `Debug`-friendly log. reqwest
        // wraps every body error as `Kind::Decode` with the message
        // "error decoding response body", which is useless on its own —
        // the real cause is in the source chain (h2 stream error,
        // connection reset, transfer-encoding failure, etc.). Logging
        // the whole chain here turns the cryptic upstream message into
        // something actionable.
        let byte_stream = byte_stream.inspect_err(|e| {
            use std::error::Error as _;
            use std::fmt::Write as _;
            let mut chain = String::new();
            let _ = write!(&mut chain, "{e}");
            let mut src = e.source();
            let mut depth = 0;
            while let Some(s) = src {
                let _ = write!(&mut chain, " :: caused_by[{depth}]: {s}");
                src = s.source();
                depth += 1;
                if depth > 8 {
                    break;
                }
            }
            tracing::warn!(
                is_decode = e.is_decode(),
                is_connect = e.is_connect(),
                is_timeout = e.is_timeout(),
                is_body = e.is_body(),
                error = %chain,
                "create_event_stream: raw byte_stream error before SSE parser wraps it"
            );
        });

        // Per-stream adapter state (used by OpenAI wires to track tool-call
        // accumulation; unused by AnthropicWire since Anthropic SSE is
        // self-describing).
        let mut state = StreamState::default();

        let sse_stream = byte_stream
            .eventsource()
            .flat_map(move |result| {
                // The eventsource-stream crate wraps every body error as
                // `Transport error: ...` — losing the original reqwest
                // error type and source chain. Unwrap it here so we can
                // log the actual cause (connection reset, h2 protocol
                // error, transfer-encoding failure, etc.).
                if let Err(e) = &result {
                    use std::fmt::Write as _;
                    let mut chain = String::new();
                    let _ = write!(&mut chain, "{e}");
                    let mut src = std::error::Error::source(&e);
                    let mut depth = 0;
                    while let Some(s) = src {
                        let _ = write!(&mut chain, " :: caused by[{depth}]: {s}");
                        src = s.source();
                        depth += 1;
                        if depth > 8 { break; }
                    }
                    tracing::warn!(error = %chain, "create_event_stream: eventsource-stream error (with cause chain)");
                }
                let events: Vec<Result<MessageStreamEvent>> = match result {
                    Ok(event) => {
                        // Delegate parsing to the wire protocol adapter.
                        // `Ok(None)` means "skip this event"; `Ok(Some(_))`
                        // yields a canonical MessageStreamEvent.
                        match wire.adapt_sse_event(event.event.as_str(), &event.data, &mut state) {
                            Ok(main) => {
                                // Adapter-queued events go first (e.g. the
                                // usage-carrying MessageDelta that must
                                // precede a terminal MessageStop), then the
                                // adapter's own event for this SSE event.
                                let mut out: Vec<Result<MessageStreamEvent>> =
                                    state.pending_events.drain(..).map(Ok).collect();
                                if let Some(ev) = main {
                                    out.push(Ok(ev));
                                }
                                out
                            }
                            Err(e) => vec![Err(e)],
                        }
                    }
                    Err(e) => vec![Err(AnthropicError::StreamError(
                        format!("SSE stream error: {}", e)
                    ))],
                };
                futures::stream::iter(events)
            });

        Ok(sse_stream)
    }

    /// Get the request ID from the response headers.
    pub fn request_id(&self) -> Option<&str> {
        self.request_id.as_deref()
    }

    /// Get the stream configuration.
    pub fn config(&self) -> &StreamConfig {
        &self.config
    }

    /// Check if the stream has ended.
    pub fn ended(&self) -> bool {
        self.ended
    }
}

impl Stream for HttpStreamClient {
    type Item = Result<MessageStreamEvent>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();

        if *this.ended {
            return Poll::Ready(None);
        }

        match this.event_stream.poll_next(cx) {
            Poll::Ready(Some(Ok(event))) => {
                let is_stop = matches!(event, MessageStreamEvent::MessageStop);
                tracing::trace!(
                    event = ?event,
                    is_stop,
                    "HttpStreamClient: received SSE event"
                );
                if is_stop {
                    *this.ended = true;
                    tracing::info!("HttpStreamClient: stream ended (MessageStop)");
                }
                Poll::Ready(Some(Ok(event)))
            }
            Poll::Ready(Some(Err(e))) => {
                *this.ended = true;
                tracing::warn!(error = %e, "HttpStreamClient: SSE error");
                Poll::Ready(Some(Err(e)))
            }
            Poll::Ready(None) => {
                tracing::info!(
                    "HttpStreamClient: underlying SSE stream returned None (connection closed)"
                );
                *this.ended = true;
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Builder for creating HTTP streaming requests.
#[derive(Clone)]
pub struct StreamRequestBuilder {
    /// HTTP client for making requests
    client: reqwest::Client,
    /// Base URL for the API
    base_url: String,
    /// Request headers
    headers: reqwest::header::HeaderMap,
    /// Stream configuration
    config: StreamConfig,
    /// Wire protocol used to adapt SSE events. Defaults to
    /// [`AnthropicWire`] for backward compatibility.
    wire: Arc<dyn WireProtocol>,
    /// When true, replace the client's per-request `timeout` with
    /// `Duration::MAX` for this stream. Streaming bodies are open-ended
    /// (the server can trickle events indefinitely), so the global
    /// request timeout — which reqwest computes from connection start
    /// to the last body byte — eventually fires and aborts perfectly
    /// healthy streams. Per-chunk idle timeouts in [`StreamConfig`]
    /// still cover real stalls.
    disable_request_timeout: bool,
}

impl std::fmt::Debug for StreamRequestBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamRequestBuilder")
            .field("base_url", &self.base_url)
            .field("config", &self.config)
            .field("wire_id", &self.wire.id())
            .field("disable_request_timeout", &self.disable_request_timeout)
            .finish()
    }
}

impl StreamRequestBuilder {
    /// Create a new stream request builder.
    ///
    /// Defaults to [`AnthropicWire`] for SSE event adaptation; supply a
    /// different protocol via [`.wire(...)`](Self::wire) for OpenAI
    /// endpoints.
    pub fn new(client: reqwest::Client, base_url: String) -> Self {
        Self {
            client,
            base_url,
            headers: reqwest::header::HeaderMap::new(),
            config: StreamConfig::default(),
            wire: Arc::new(AnthropicWire),
            // Streams are open-ended by design; rely on per-chunk idle
            // timeouts (`StreamConfig::event_timeout`) instead of the
            // client's overall request timeout.
            disable_request_timeout: true,
        }
    }

    /// Set the wire protocol used to adapt SSE events from the stream.
    ///
    /// Must match the protocol the endpoint speaks; otherwise the adapter
    /// will fail to parse the event stream.
    #[must_use]
    pub fn wire(mut self, wire: Arc<dyn WireProtocol>) -> Self {
        self.wire = wire;
        self
    }

    /// Override the default behaviour of disabling the per-request
    /// timeout. Defaults to disabled for safety; tests or callers that
    /// explicitly want a request-level deadline can re-enable it.
    pub fn disable_request_timeout(mut self, disable: bool) -> Self {
        self.disable_request_timeout = disable;
        self
    }

    /// Add a header to the request.
    pub fn header(mut self, key: &str, value: &str) -> Self {
        if let (Ok(key), Ok(value)) = (
            reqwest::header::HeaderName::from_bytes(key.as_bytes()),
            reqwest::header::HeaderValue::from_str(value),
        ) {
            self.headers.insert(key, value);
        }
        self
    }

    /// Set the stream configuration.
    pub fn config(mut self, config: StreamConfig) -> Self {
        self.config = config;
        self
    }

    /// Make a streaming POST request.
    pub async fn post_stream<T: serde::Serialize>(
        self,
        endpoint: &str,
        body: &T,
    ) -> Result<HttpStreamClient> {
        let url = format!(
            "{}/{}",
            self.base_url.trim_end_matches('/'),
            endpoint.trim_start_matches('/')
        );

        let mut headers = self.headers;
        headers.insert(
            reqwest::header::ACCEPT,
            reqwest::header::HeaderValue::from_static("text/event-stream"),
        );
        headers.insert(
            reqwest::header::CACHE_CONTROL,
            reqwest::header::HeaderValue::from_static("no-cache"),
        );
        // SSE bodies must arrive as raw UTF-8 so eventsource-stream can
        // split on \n\n delimiters. We consume `Response::bytes_stream()`
        // directly (no automatic decompression), so we must ask the server
        // to skip gzip/brotli — otherwise reqwest hands us compressed
        // bytes and the parser fails with "error decoding response body".
        headers.insert(
            reqwest::header::ACCEPT_ENCODING,
            reqwest::header::HeaderValue::from_static("identity"),
        );

        tracing::info!(
            url = %url,
            accept = "text/event-stream",
            accept_encoding = "identity",
            disable_request_timeout = self.disable_request_timeout,
            "post_stream: issuing HTTP request"
        );

        // SSE bodies are open-ended: events may trickle in over many
        // minutes. reqwest's overall request timeout (default 600s in
        // this client) eventually aborts perfectly healthy streams and
        // surfaces the failure as "error decoding response body ::
        // operation timed out". Disable the per-request timeout for
        // streams; [`StreamConfig::event_timeout`] still guards against
        // per-chunk stalls.
        let mut request = self.client.post(&url).headers(headers).json(body);
        if self.disable_request_timeout {
            request = request.timeout(std::time::Duration::from_secs(u64::MAX));
        }

        let response = request
            .send()
            .await
            .map_err(|e| AnthropicError::Connection {
                message: e.to_string(),
            })?;

        // Inspect the actual response headers BEFORE handing to the SSE
        // parser. If the server ignored our Accept-Encoding: identity and
        // still returned gzip/brotli, we'll see Content-Encoding here and
        // can fail fast with an actionable error instead of a cryptic
        // "error decoding response body" deep in the parser.
        let content_encoding = response
            .headers()
            .get("content-encoding")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let transfer_encoding = response
            .headers()
            .get("transfer-encoding")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let status = response.status();
        tracing::info!(
            status = %status,
            content_type = content_type.as_deref().unwrap_or("?"),
            content_encoding = content_encoding.as_deref().unwrap_or("(none)"),
            transfer_encoding = transfer_encoding.as_deref().unwrap_or("(none)"),
            "post_stream: got response headers"
        );
        if let Some(enc) = &content_encoding
            && enc != "identity"
            && !enc.is_empty()
        {
            tracing::error!(
                content_encoding = %enc,
                "post_stream: server returned a compressed body despite Accept-Encoding: identity. \
                 bytes_stream() does not auto-decompress, so SSE parsing will fail. \
                 The upstream proxy/gateway is overriding Accept-Encoding."
            );
        }

        HttpStreamClient::from_response(response, self.config, self.wire.clone()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stream_config_default() {
        let config = StreamConfig::default();
        assert_eq!(config.buffer_size, 1000);
        assert_eq!(config.event_timeout, Some(300));
        assert!(config.retry_on_error);
        assert_eq!(config.max_retries, Some(3));
    }

    #[test]
    fn test_stream_request_builder() {
        let client = reqwest::Client::new();
        let builder = StreamRequestBuilder::new(client, "https://api.anthropic.com".to_string())
            .header("Authorization", "Bearer test-key")
            .config(StreamConfig {
                buffer_size: 500,
                ..Default::default()
            });

        assert_eq!(builder.base_url, "https://api.anthropic.com");
        assert_eq!(builder.config.buffer_size, 500);
        assert!(builder.headers.contains_key("authorization"));
    }

    #[tokio::test]
    async fn test_sse_event_parsing() {
        // Test that we can parse a sample SSE event
        let event_data = r#"{"type":"message_start","message":{"id":"msg_123","type":"message","role":"assistant","content":[],"model":"claude-3-5-sonnet-latest","stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":10,"output_tokens":0,"cache_creation_input_tokens":null,"cache_read_input_tokens":null,"server_tool_use":null,"service_tier":null}}}"#;

        let parsed: std::result::Result<MessageStreamEvent, _> = serde_json::from_str(event_data);
        assert!(parsed.is_ok());

        if let Ok(MessageStreamEvent::MessageStart { message }) = parsed {
            assert_eq!(message.id, "msg_123");
            assert_eq!(message.usage.unwrap().input_tokens, 10);
        } else {
            panic!("Expected MessageStart event");
        }
    }
}
