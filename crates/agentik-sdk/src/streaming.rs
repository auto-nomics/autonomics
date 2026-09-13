//! Streaming support for real-time message generation.
//!
//! This module provides the `MessageStream` struct which handles Server-Sent Events (SSE)
//! from the Anthropic API, accumulates messages from incremental updates, and provides
//! an event-driven API for processing streaming responses.

pub mod events;

use futures::Stream;
use pin_project::pin_project;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use tokio::sync::{broadcast, oneshot};
use tokio_stream::wrappers::BroadcastStream;

use crate::types::{
    AnthropicError, ContentBlock, ContentBlockDelta, Message, MessageStreamEvent, Result,
};

use self::events::{EventHandler, EventType};

/// A streaming response from the Anthropic API.
///
/// `MessageStream` provides an event-driven interface for processing streaming responses
/// from Claude. It accumulates message content from incremental updates and provides
/// both callback-based and async iteration APIs.
///
/// # Examples
///
/// ## Callback-based processing:
/// ```ignore
/// # use agentik_sdk::{Anthropic, MessageCreateBuilder};
/// # async fn example() -> agentik_sdk::Result<()> {
/// let client = Anthropic::new("your-api-key")?;
/// let stream = client.messages().create_stream(
///     MessageCreateBuilder::new("claude-3-5-sonnet-latest", 1024)
///         .user("Write a story about AI")
///         .stream(true)
///         .build()
/// ).await?;
///
/// let final_message = stream
///     .on_text(|delta, _snapshot| {
///         print!("{}", delta);
///     })
///     .on_error(|error| {
///         eprintln!("Stream error: {}", error);
///     })
///     .final_message().await?;
/// # Ok(())
/// # }
/// ```
///
/// ## Async iteration:
/// ```ignore
/// # use agentik_sdk::{Anthropic, MessageCreateBuilder, MessageStreamEvent};
/// # use futures::StreamExt;
/// # async fn example() -> agentik_sdk::Result<()> {
/// let client = Anthropic::new("your-api-key")?;
/// let mut stream = client.messages().create_stream(
///     MessageCreateBuilder::new("claude-3-5-sonnet-latest", 1024)
///         .user("Tell me a joke")
///         .stream(true)
///         .build()
/// ).await?;
///
/// while let Some(event) = stream.next().await {
///     match event? {
///         MessageStreamEvent::ContentBlockDelta { delta, .. } => {
///             // Process incremental content
///         }
///         MessageStreamEvent::MessageStop => break,
///         _ => {}
///     }
/// }
/// # Ok(())
/// # }
/// ```
#[pin_project]
pub struct MessageStream {
    /// Current accumulated message snapshot (single writer: bg task; multiple readers)
    current_message: Arc<RwLock<Option<Message>>>,

    /// Event handlers for different event types
    event_handlers: Arc<Mutex<HashMap<EventType, Vec<EventHandler>>>>,

    // NOTE: The broadcast sender is intentionally NOT stored here.
    // The background task holds the sole sender clone. When it finishes,
    // the clone is dropped, the broadcast channel closes, and
    // `BroadcastStream::poll_next` reliably returns `Ready(None)` — which
    // is the primary termination signal for async-iteration consumers.
    //
    // Previously, keeping a sender alive in this struct prevented the
    // channel from ever closing. Combined with a race in the watch-channel
    // fallback (the `changed()` future is created-and-dropped per
    // `poll_next`, unregistering the waker before the notification arrives),
    // this caused consumers to hang permanently after the background task
    // finished — especially with non-standard providers (e.g. Zhipu GLM)
    // that send `MessageStart` + `MessageStop` with no content blocks in a
    // single burst.
    /// Stream for events from the underlying HTTP stream
    #[pin]
    event_stream: BroadcastStream<MessageStreamEvent>,

    /// Channel for signaling when the stream ends
    completion_sender: Option<oneshot::Sender<Result<Message>>>,
    completion_receiver: oneshot::Receiver<Result<Message>>,

    /// Whether the stream has ended (one-way flag: false → true)
    ended: Arc<AtomicBool>,

    /// Watch channel for stream-end signalling.  The background task
    /// `store(true)` on the sender when it finishes.  Unlike `Notify`,
    /// `watch::Receiver::changed()` is *sticky*: if the value changed
    /// since the last observation the future resolves immediately, so a
    /// late consumer that missed the change still wakes up.
    ended_watch: tokio::sync::watch::Receiver<bool>,

    /// Whether an error occurred (one-way flag: false → true)
    errored: Arc<AtomicBool>,

    /// Whether the stream was aborted by the user (one-way flag: false → true)
    aborted: Arc<AtomicBool>,

    /// Response metadata
    request_id: Option<String>,

    /// Handle to the background stream-processing task.
    /// Shared via Arc so abort() can cancel it from &self.
    _background_task: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

impl MessageStream {
    /// Create a MessageStream from a predefined list of events and a final message.
    ///
    /// Intended for unit tests. A background task drains the event list into the
    /// broadcast channel (after routing it through the same `accumulate_event`
    /// and `dispatch_event` helpers the production path uses), then delivers
    /// the accumulated final message through the oneshot completion channel.
    #[cfg(any(test, feature = "test-util"))]
    pub fn from_events(events: Vec<MessageStreamEvent>, final_message: Message) -> Self {
        let (event_sender, event_receiver) = broadcast::channel(events.len().max(1));
        let (completion_sender, completion_receiver) = oneshot::channel();

        let current_message = Arc::new(RwLock::new(None));
        let event_handlers = Arc::new(Mutex::new(HashMap::new()));
        let ended = Arc::new(AtomicBool::new(false));
        let (ended_watch_tx, ended_watch_rx) = tokio::sync::watch::channel(false);
        let errored = Arc::new(AtomicBool::new(false));
        let aborted = Arc::new(AtomicBool::new(false));

        let cm = current_message.clone();
        let handlers = event_handlers.clone();
        let end = ended.clone();
        // The background task owns the sole sender. When it finishes, the
        // sender is dropped and the broadcast channel closes.
        let tx = event_sender;

        tokio::spawn(async move {
            // `running_final` mirrors `final_message` through the
            // accumulator so the value sent on the oneshot reflects any
            // MessageDelta / ContentBlockStop updates from the event list.
            let mut running_final = Some(final_message);
            for event in &events {
                MessageStream::accumulate_event(event, &cm, &mut running_final);
                MessageStream::dispatch_event(event, &handlers, &cm);
                let _ = tx.send(event.clone());
            }
            // Drop the sender before signalling completion so that
            // BroadcastStream consumers see Ready(None) immediately.
            drop(tx);
            let _ = completion_sender.send(running_final.ok_or_else(|| {
                AnthropicError::StreamError("Stream ended without message".to_string())
            }));
            end.store(true, Ordering::Release);
            let _ = ended_watch_tx.send(true);
        });

        Self {
            current_message,
            event_handlers,
            event_stream: BroadcastStream::new(event_receiver),
            completion_sender: None,
            completion_receiver,
            ended,
            ended_watch: ended_watch_rx,
            errored,
            aborted,
            request_id: None,
            _background_task: Arc::new(Mutex::new(None)),
        }
    }

    /// Create a new MessageStream from an HttpStreamClient.
    ///
    /// This connects a real HTTP stream to the MessageStream, providing
    /// proper streaming functionality for real-time response processing.
    ///
    /// This entry point is retry-free: if the underlying connection
    /// stalls, the stream terminates with an error. Use
    /// [`from_http_stream_with_retry`] (or
    /// [`crate::resources::MessagesResource::create_stream_with_config`])
    /// to get automatic reconnect on pre-`MessageStart` stalls.
    pub fn from_http_stream(http_stream: crate::http::streaming::HttpStreamClient) -> Result<Self> {
        // Disable retries at the SDK layer by handing in a reconnect
        // closure that always fails with a clear error. The single
        // attempt that just produced `http_stream` is then consumed by
        // the background task as usual.
        let never_reconnect = || async {
            Err(crate::types::AnthropicError::StreamError(
                "retry disabled: stream was not configured with from_http_stream_with_retry"
                    .to_string(),
            ))
        };
        let config = http_stream.config().clone();
        Self::from_http_stream_with_retry(http_stream, never_reconnect, config)
    }

    /// Create a MessageStream that transparently reconnects on stalls
    /// **before** any `MessageStart` event has been observed.
    ///
    /// Semantics:
    /// - Each chunk is gated by `config.event_timeout` seconds. If no
    ///   event arrives in that window, the current HttpStreamClient is
    ///   dropped (canceling its underlying reqwest connection) and
    ///   `reconnect` is invoked to open a fresh one. The retry counter
    ///   is bounded by `config.max_retries`.
    /// - Once a `MessageStart` event has been emitted, the stream is
    ///   considered "in flight" and any subsequent timeout / network
    ///   error terminates the stream with
    ///   [`crate::types::AnthropicError::StreamError`] (the partial
    ///   message accumulated so far is still returned via
    ///   `final_message()`).
    /// - If `reconnect` itself returns an error, the retry counter is
    ///   still consumed and the stream terminates with that error
    ///   once `max_retries` is exhausted.
    /// - Setting `config.retry_on_error = false` short-circuits retries
    ///   entirely: a stall immediately terminates the stream.
    pub fn from_http_stream_with_retry<F, Fut>(
        mut http_stream: crate::http::streaming::HttpStreamClient,
        reconnect: F,
        config: crate::http::streaming::StreamConfig,
    ) -> Result<Self>
    where
        F: Fn() -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<crate::http::streaming::HttpStreamClient>>
            + Send
            + 'static,
    {
        let (event_sender, event_receiver) = broadcast::channel(config.buffer_size);
        let (completion_sender, completion_receiver) = oneshot::channel();

        let current_message = Arc::new(RwLock::new(None));
        let event_handlers = Arc::new(Mutex::new(HashMap::new()));
        let ended = Arc::new(AtomicBool::new(false));
        let (ended_watch_tx, ended_watch_rx) = tokio::sync::watch::channel(false);
        let errored = Arc::new(AtomicBool::new(false));
        let aborted = Arc::new(AtomicBool::new(false));
        let request_id = http_stream.request_id().map(|s| s.to_string());

        // Idle timeout (per chunk). Default 300s (5 minutes), matching StreamConfig.
        let event_timeout_secs = config.event_timeout.unwrap_or(300);
        let max_retries = config.max_retries.unwrap_or(3);
        let retry_on_error = config.retry_on_error;

        // Clones handed off to the background task.
        let current_message_bg = current_message.clone();
        let event_handlers_bg = event_handlers.clone();
        let ended_bg = ended.clone();
        let errored_bg = errored.clone();
        let aborted_bg = aborted.clone();
        let event_sender_bg = event_sender;

        let bg_handle = Arc::new(Mutex::new(None));
        let bg_handle_clone = bg_handle.clone();

        let background_handle = tokio::spawn(async move {
            use futures::StreamExt;

            let mut final_message: Option<crate::types::Message> = None;
            let mut completion_sender = Some(completion_sender);
            let timeout_duration = std::time::Duration::from_secs(event_timeout_secs);
            let mut saw_message_start = false;
            let mut retries_used: u32 = 0;

            'outer: loop {
                if aborted_bg.load(Ordering::Acquire) {
                    break;
                }

                loop {
                    let next_result =
                        tokio::time::timeout(timeout_duration, http_stream.next()).await;

                    match next_result {
                        Err(_elapsed) => {
                            tracing::warn!(
                                timeout_secs = event_timeout_secs,
                                saw_message_start,
                                "stream idle timeout: no event received"
                            );
                            if !saw_message_start && retry_on_error && retries_used < max_retries {
                                retries_used += 1;
                                tracing::info!(
                                    retry = retries_used,
                                    max_retries,
                                    "reconnecting stream after idle timeout (no MessageStart yet)"
                                );
                                match reconnect().await {
                                    Ok(new_stream) => {
                                        // Drop the old HttpStreamClient — this cancels
                                        // the underlying reqwest Response because the
                                        // bytes_stream() future is no longer polled.
                                        http_stream = new_stream;
                                        continue 'outer;
                                    }
                                    Err(e) => {
                                        tracing::error!(error = %e, "reconnect attempt failed");
                                        Self::fire_error(&event_handlers_bg, &errored_bg, &e);
                                        if let Some(s) = completion_sender.take() {
                                            let _ = s.send(Err(e));
                                        }
                                        break 'outer;
                                    }
                                }
                            }
                            let err = if saw_message_start {
                                crate::types::AnthropicError::StreamError(format!(
                                    "stream stalled after MessageStart (no event for {event_timeout_secs}s)"
                                ))
                            } else {
                                crate::types::AnthropicError::Timeout
                            };
                            Self::fire_error(&event_handlers_bg, &errored_bg, &err);
                            if let Some(s) = completion_sender.take() {
                                let _ = s.send(Err(err));
                            }
                            break 'outer;
                        }
                        Ok(Some(Ok(event))) => {
                            if matches!(
                                event,
                                crate::types::MessageStreamEvent::MessageStart { .. }
                            ) {
                                saw_message_start = true;
                                tracing::info!("background task: saw MessageStart");
                            }

                            // 1. Accumulate into the running snapshot and the
                            //    final-message candidate we hold for oneshot.
                            Self::accumulate_event(&event, &current_message_bg, &mut final_message);

                            // 2. Fire registered callbacks.
                            Self::dispatch_event(&event, &event_handlers_bg, &current_message_bg);

                            // 3. Broadcast to async-iteration consumers.
                            if let Err(e) = event_sender_bg.send(event.clone()) {
                                tracing::debug!("broadcast send failed (receiver lagged): {e}");
                            }

                            // 4. Terminal event?
                            if matches!(event, crate::types::MessageStreamEvent::MessageStop) {
                                tracing::info!(
                                    has_final_message = final_message.is_some(),
                                    "background task: received MessageStop, sending completion"
                                );
                                ended_bg.store(true, Ordering::Release);
                                let _ = ended_watch_tx.send(true);
                                let result = final_message.clone().ok_or_else(|| {
                                    crate::types::AnthropicError::StreamError(
                                        "Stream ended without message".to_string(),
                                    )
                                });
                                if let Some(s) = completion_sender.take() {
                                    let _ = s.send(result);
                                }
                                break 'outer;
                            }
                        }
                        Ok(Some(Err(e))) => {
                            let is_timeout = matches!(e, crate::types::AnthropicError::Timeout);
                            let stallable = matches!(
                                e,
                                crate::types::AnthropicError::NetworkError(_)
                                    | crate::types::AnthropicError::Connection { .. }
                                    | crate::types::AnthropicError::StreamError(_)
                            ) || is_timeout;

                            if !saw_message_start
                                && retry_on_error
                                && stallable
                                && retries_used < max_retries
                            {
                                retries_used += 1;
                                tracing::warn!(
                                    error = %e,
                                    retry = retries_used,
                                    max_retries,
                                    "reconnecting stream after error (no MessageStart yet)"
                                );
                                match reconnect().await {
                                    Ok(new_stream) => {
                                        http_stream = new_stream;
                                        continue 'outer;
                                    }
                                    Err(reconnect_err) => {
                                        tracing::error!(
                                            error = %reconnect_err,
                                            "reconnect attempt failed"
                                        );
                                        Self::fire_error(
                                            &event_handlers_bg,
                                            &errored_bg,
                                            &reconnect_err,
                                        );
                                        if let Some(s) = completion_sender.take() {
                                            let _ = s.send(Err(reconnect_err));
                                        }
                                        break 'outer;
                                    }
                                }
                            }
                            Self::fire_error(&event_handlers_bg, &errored_bg, &e);
                            if let Some(s) = completion_sender.take() {
                                let _ = s.send(Err(e));
                            }
                            break 'outer;
                        }
                        Ok(None) => {
                            tracing::info!(
                                saw_message_start,
                                has_final_message = final_message.is_some(),
                                "background task: HTTP stream ended (None) — no more SSE events"
                            );
                            ended_bg.store(true, Ordering::Release);
                            let _ = ended_watch_tx.send(true);
                            break 'outer;
                        }
                    }
                }
            }

            // Drop the broadcast sender so the channel closes immediately.
            // This guarantees that any consumer parked on
            // `BroadcastStream::poll_next` receives `Ready(None)` without
            // having to wait for the drain loop or the watch-channel
            // fallback. The drain loop below only touches the HTTP body
            // (`http_stream`), not the broadcast channel.
            drop(event_sender_bg);

            // Gracefully drain remaining HTTP body bytes to prevent
            // sending RST_STREAM(CANCEL) to the server.  The background
            // task has already accumulated the full message; we just need
            // to consume any remaining data from the HTTP response body
            // so that hyper/h2 doesn't cancel the stream on drop.
            tracing::info!("background task: entering drain loop");
            drain_http_body(http_stream).await;
            // Fallback: deliver whatever was accumulated even if the stream
            // ended without a MessageStop (e.g. non-conforming server), so
            // that final_message() does not hang forever.
            if let Some(sender) = completion_sender.take() {
                let result = if let Some(msg) = final_message {
                    tracing::info!(
                        msg_id = %msg.id,
                        "background task: fallback — sending accumulated message via completion channel"
                    );
                    Ok(msg)
                } else {
                    tracing::warn!("background task: fallback — stream ended without any message");
                    Err(crate::types::AnthropicError::StreamError(
                        "Stream ended without message".to_string(),
                    ))
                };
                let _ = sender.send(result);
            } else {
                tracing::info!(
                    "background task: completion sender already consumed (MessageStop path)"
                );
            }
            tracing::info!("background task: exiting — setting ended=true and notifying");
            ended_bg.store(true, Ordering::Release);
            let _ = ended_watch_tx.send(true);
        });
        *bg_handle_clone.lock().unwrap() = Some(background_handle);

        Ok(Self {
            current_message,
            event_handlers,
            event_stream: BroadcastStream::new(event_receiver),
            completion_sender: None, // Already consumed by the task
            completion_receiver,
            ended,
            ended_watch: ended_watch_rx,
            errored,
            aborted,
            request_id,
            _background_task: bg_handle,
        })
    }

    /// Register a callback for text delta events.
    ///
    /// The callback receives two parameters:
    /// - `delta`: The new text being appended
    /// - `snapshot`: The current accumulated text
    ///
    /// # Examples
    /// ```rust,no_run
    /// # use agentik_sdk::MessageStream;
    /// # async fn example(stream: MessageStream) {
    /// stream.on_text(|delta, snapshot| {
    ///     print!("{}", delta);
    ///     println!("Total so far: {}", snapshot);
    /// });
    /// # }
    /// ```
    pub fn on_text<F>(self, callback: F) -> Self
    where
        F: Fn(&str, &str) + Send + Sync + 'static,
    {
        self.on(
            EventType::Text,
            EventHandler::Text(std::sync::Arc::new(Box::new(callback))),
        )
    }

    /// Register a callback for stream events.
    ///
    /// This provides access to all raw stream events and the current message snapshot.
    ///
    /// # Examples
    /// ```rust,no_run
    /// # use agentik_sdk::{MessageStream, MessageStreamEvent, Message};
    /// # async fn example(stream: MessageStream) {
    /// stream.on_stream_event(|event, snapshot| {
    ///     match event {
    ///         MessageStreamEvent::ContentBlockStart { .. } => {
    ///             println!("New content block started");
    ///         }
    ///         _ => {}
    ///     }
    /// });
    /// # }
    /// ```
    pub fn on_stream_event<F>(self, callback: F) -> Self
    where
        F: Fn(&MessageStreamEvent, &Message) + Send + Sync + 'static,
    {
        self.on(
            EventType::StreamEvent,
            EventHandler::StreamEvent(std::sync::Arc::new(Box::new(callback))),
        )
    }

    /// Register a callback for when a complete message is received.
    ///
    /// # Examples
    /// ```rust,no_run
    /// # use agentik_sdk::{MessageStream, Message};
    /// # async fn example(stream: MessageStream) {
    /// stream.on_message(|message| {
    ///     println!("Received message: {:?}", message);
    /// });
    /// # }
    /// ```
    pub fn on_message<F>(self, callback: F) -> Self
    where
        F: Fn(&Message) + Send + Sync + 'static,
    {
        self.on(
            EventType::Message,
            EventHandler::Message(std::sync::Arc::new(Box::new(callback))),
        )
    }

    /// Register a callback for when the final message is complete.
    ///
    /// # Examples
    /// ```rust,no_run
    /// # use agentik_sdk::{MessageStream, Message};
    /// # async fn example(stream: MessageStream) {
    /// stream.on_final_message(|message| {
    ///     println!("Final message: {:?}", message);
    /// });
    /// # }
    /// ```
    pub fn on_final_message<F>(self, callback: F) -> Self
    where
        F: Fn(&Message) + Send + Sync + 'static,
    {
        self.on(
            EventType::FinalMessage,
            EventHandler::FinalMessage(std::sync::Arc::new(Box::new(callback))),
        )
    }

    /// Register a callback for errors.
    ///
    /// # Examples
    /// ```rust,no_run
    /// # use agentik_sdk::{MessageStream, AnthropicError};
    /// # async fn example(stream: MessageStream) {
    /// stream.on_error(|error| {
    ///     eprintln!("Stream error: {}", error);
    /// });
    /// # }
    /// ```
    pub fn on_error<F>(self, callback: F) -> Self
    where
        F: Fn(&AnthropicError) + Send + Sync + 'static,
    {
        self.on(
            EventType::Error,
            EventHandler::Error(std::sync::Arc::new(Box::new(callback))),
        )
    }

    /// Register a callback for when the stream ends.
    ///
    /// # Examples
    /// ```rust,no_run
    /// # use agentik_sdk::MessageStream;
    /// # async fn example(stream: MessageStream) {
    /// stream.on_end(|| {
    ///     println!("Stream ended");
    /// });
    /// # }
    /// ```
    pub fn on_end<F>(self, callback: F) -> Self
    where
        F: Fn() + Send + Sync + 'static,
    {
        self.on(
            EventType::End,
            EventHandler::End(std::sync::Arc::new(Box::new(callback))),
        )
    }

    /// Generic method to register event handlers.
    fn on(self, event_type: EventType, handler: EventHandler) -> Self {
        {
            let mut handlers = self.event_handlers.lock().unwrap();
            handlers.entry(event_type).or_default().push(handler);
        }
        self
    }

    /// Wait for the stream to complete and return the final message.
    ///
    /// This method will block until the stream ends and return the accumulated message.
    ///
    /// # Examples
    /// ```rust,no_run
    /// # use agentik_sdk::MessageStream;
    /// # async fn example(stream: MessageStream) -> agentik_sdk::Result<()> {
    /// let final_message = stream.final_message().await?;
    /// println!("Claude said: {:?}", final_message.content);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn final_message(self) -> Result<Message> {
        let Self {
            completion_receiver,
            ..
        } = self;
        completion_receiver
            .await
            .map_err(|_| AnthropicError::StreamError("Stream ended unexpectedly".to_string()))?
    }

    /// Wait for the stream to complete without returning the message.
    ///
    /// This is useful when you're processing events with callbacks and just need
    /// to wait for completion.
    ///
    /// # Examples
    /// ```rust,no_run
    /// # use agentik_sdk::MessageStream;
    /// # async fn example(stream: MessageStream) -> agentik_sdk::Result<()> {
    /// stream.on_text(|delta, _| print!("{}", delta))
    ///     .done().await?;
    /// println!("\nStream completed!");
    /// # Ok(())
    /// # }
    /// ```
    pub async fn done(self) -> Result<()> {
        let Self {
            completion_receiver,
            ..
        } = self;
        completion_receiver
            .await
            .map_err(|_| AnthropicError::StreamError("Stream ended unexpectedly".to_string()))?
            .map(|_| ())
    }

    /// Get the current accumulated message snapshot.
    ///
    /// Returns `None` if the stream hasn't started or no message has been received yet.
    pub fn current_message(&self) -> Option<Message> {
        self.current_message.read().unwrap().clone()
    }

    /// Check if the stream has ended.
    pub fn ended(&self) -> bool {
        self.ended.load(Ordering::Acquire)
    }

    /// Check if an error occurred.
    pub fn errored(&self) -> bool {
        self.errored.load(Ordering::Acquire)
    }

    /// Check if the stream was aborted.
    pub fn aborted(&self) -> bool {
        self.aborted.load(Ordering::Acquire)
    }

    /// Get the request ID.
    pub fn request_id(&self) -> Option<&str> {
        self.request_id.as_deref()
    }

    /// Abort the stream.
    ///
    /// Marks the stream as aborted and cancels the background task.
    pub fn abort(&self) {
        self.aborted.store(true, Ordering::Release);
        if let Some(handle) = self._background_task.lock().unwrap().take() {
            handle.abort();
        }
    }

    /// Accumulate one event into the live snapshot and the final-message
    /// candidate. Called by the background task; takes shared locks so it
    /// can run without `&self`.
    fn accumulate_event(
        event: &MessageStreamEvent,
        current: &RwLock<Option<Message>>,
        final_message: &mut Option<Message>,
    ) {
        // `MessageStart` is a full replace; everything else is incremental.
        if let MessageStreamEvent::MessageStart { message } = event {
            *current.write().unwrap() = Some(message.clone());
            *final_message = Some(message.clone());
            return;
        }

        fn apply_to(msg: &mut Message, event: &MessageStreamEvent) {
            match event {
                MessageStreamEvent::MessageStart { .. } => unreachable!(),
                MessageStreamEvent::ContentBlockStart {
                    content_block,
                    index,
                } => {
                    while msg.content.len() <= *index {
                        msg.content.push(ContentBlock::Text {
                            text: String::new(),
                        });
                    }
                    msg.content[*index] = content_block.clone();
                }
                MessageStreamEvent::ContentBlockDelta { delta, index } => {
                    if let Some(block) = msg.content.get_mut(*index) {
                        MessageStream::apply_delta(block, delta);
                    }
                }
                MessageStreamEvent::ContentBlockStop { index } => {
                    // Finalize a streaming tool_use block: convert the
                    // accumulated partial-JSON string into a real Value
                    // so downstream consumers see structured input.
                    if let Some(ContentBlock::ToolUse { input, .. }) = msg.content.get_mut(*index)
                        && let serde_json::Value::String(accumulated) = input
                    {
                        // If the accumulated JSON fails to parse (truncated
                        // stream, malformed output), fall back to an empty
                        // object rather than a string — a string input is not
                        // a valid dictionary and causes API errors when the
                        // message is replayed to any provider.
                        *input = serde_json::from_str(accumulated)
                            .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new()));
                    }
                }
                MessageStreamEvent::MessageDelta { delta, usage } => {
                    if let Some(sr) = &delta.stop_reason {
                        msg.stop_reason = Some(sr.clone());
                    }
                    if let Some(ss) = &delta.stop_sequence {
                        msg.stop_sequence = Some(ss.clone());
                    }
                    let u = msg.usage.get_or_insert_with(crate::types::Usage::default);
                    u.output_tokens = usage.output_tokens;
                    if let Some(it) = usage.input_tokens {
                        u.input_tokens = it;
                    }
                    if let Some(cc) = usage.cache_creation_input_tokens {
                        u.cache_creation_input_tokens = Some(cc);
                    }
                    if let Some(cr) = usage.cache_read_input_tokens {
                        u.cache_read_input_tokens = Some(cr);
                    }
                }
                MessageStreamEvent::MessageStop => {
                    // 安全网：适配器漏发 ContentBlockStop 时，终结前把
                    // 仍是字符串缓冲的工具参数解析为对象（与
                    // ContentBlockStop 的终结逻辑一致——字符串 input 不
                    // 是合法字典，工具执行与消息回放都会失败）。
                    for block in msg.content.iter_mut() {
                        if let ContentBlock::ToolUse { input, .. } = block
                            && let serde_json::Value::String(accumulated) = input
                        {
                            *input = serde_json::from_str(accumulated).unwrap_or_else(|_| {
                                serde_json::Value::Object(serde_json::Map::new())
                            });
                        }
                    }
                }
            }
        }

        if let Some(msg) = current.write().unwrap().as_mut() {
            apply_to(msg, event);
        }
        if let Some(msg) = final_message.as_mut() {
            apply_to(msg, event);
        }
    }

    /// Apply a content block delta to update the content.
    ///
    /// For `ToolUse` blocks, partial JSON is appended to a `Value::String`
    /// placeholder. The accumulated string is parsed into a `Value` on
    /// `ContentBlockStop` (see [`accumulate_event`]).
    fn apply_delta(content_block: &mut ContentBlock, delta: &ContentBlockDelta) {
        match (content_block, delta) {
            (ContentBlock::Text { text }, ContentBlockDelta::TextDelta { text: dt }) => {
                text.push_str(dt);
            }
            (
                ContentBlock::Thinking { thinking, .. },
                ContentBlockDelta::ThinkingDelta { thinking: dt },
            ) => {
                thinking.push_str(dt);
            }
            (
                ContentBlock::Thinking { signature, .. },
                ContentBlockDelta::SignatureDelta { signature: ds },
            ) => {
                signature.push_str(ds);
            }
            (
                ContentBlock::ToolUse { input, .. },
                ContentBlockDelta::InputJsonDelta { partial_json },
            ) => match input {
                // Already accumulating as a string — just append.
                serde_json::Value::String(buf) => buf.push_str(partial_json),
                // First delta: the initial value from ContentBlockStart is
                // `{}` (empty object) or similar — replace with a string
                // buffer so subsequent deltas can append.
                other => *other = serde_json::Value::String(partial_json.to_string()),
            },
            _ => {}
        }
    }

    /// Fire registered event-handler callbacks for one event.
    ///
    /// Takes the shared handler-table and current-message locks so it can
    /// run from the background task without `&self`.
    ///
    /// Clones the handler list and message snapshot before invoking any
    /// callback, so the locks are released before user code runs. This
    /// prevents lock poisoning if a callback panics.
    fn dispatch_event(
        event: &MessageStreamEvent,
        handlers: &Mutex<HashMap<EventType, Vec<EventHandler>>>,
        current: &RwLock<Option<Message>>,
    ) {
        // Clone handler lists and message snapshot while holding the locks,
        // then release both before invoking any callbacks.
        let (
            stream_handlers,
            text_handlers,
            msg_handlers,
            end_handlers,
            final_handlers,
            current_snapshot,
        ) = {
            let handlers_guard = handlers.lock().unwrap();
            let current_snapshot = current.read().unwrap().clone();

            let stream_handlers: Vec<EventHandler> = handlers_guard
                .get(&EventType::StreamEvent)
                .cloned()
                .unwrap_or_default();
            let text_handlers: Vec<EventHandler> = handlers_guard
                .get(&EventType::Text)
                .cloned()
                .unwrap_or_default();
            let msg_handlers: Vec<EventHandler> = handlers_guard
                .get(&EventType::Message)
                .cloned()
                .unwrap_or_default();
            let end_handlers: Vec<EventHandler> = handlers_guard
                .get(&EventType::End)
                .cloned()
                .unwrap_or_default();
            let final_handlers: Vec<EventHandler> = handlers_guard
                .get(&EventType::FinalMessage)
                .cloned()
                .unwrap_or_default();

            // Both locks released here.
            (
                stream_handlers,
                text_handlers,
                msg_handlers,
                end_handlers,
                final_handlers,
                current_snapshot,
            )
        };

        // Every event fans out to raw stream-event subscribers.
        for handler in &stream_handlers {
            if let EventHandler::StreamEvent(cb) = handler
                && let Some(msg) = &current_snapshot
            {
                cb(event, msg);
            }
        }

        match event {
            MessageStreamEvent::ContentBlockDelta {
                delta: ContentBlockDelta::TextDelta { text },
                ..
            } => {
                let snapshot = current_snapshot
                    .as_ref()
                    .map(Self::accumulated_text)
                    .unwrap_or_default();
                for handler in &text_handlers {
                    if let EventHandler::Text(cb) = handler {
                        cb(text, &snapshot);
                    }
                }
            }
            MessageStreamEvent::MessageStart { message } => {
                for handler in &msg_handlers {
                    if let EventHandler::Message(cb) = handler {
                        cb(message);
                    }
                }
            }
            MessageStreamEvent::MessageStop => {
                for handler in &end_handlers {
                    if let EventHandler::End(cb) = handler {
                        cb();
                    }
                }
                if let Some(msg) = &current_snapshot {
                    for handler in &final_handlers {
                        if let EventHandler::FinalMessage(cb) = handler {
                            cb(msg);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// Mark the stream as errored and fire registered error handlers.
    fn fire_error(
        handlers: &Mutex<HashMap<EventType, Vec<EventHandler>>>,
        errored: &AtomicBool,
        error: &AnthropicError,
    ) {
        errored.store(true, Ordering::Release);
        // Clone error handlers before invoking to avoid holding the lock
        // during callbacks (prevents poisoning on panic).
        let error_handlers: Vec<EventHandler> = handlers
            .lock()
            .unwrap()
            .get(&EventType::Error)
            .cloned()
            .unwrap_or_default();
        for handler in error_handlers {
            if let EventHandler::Error(cb) = handler {
                cb(error);
            }
        }
    }

    /// Concatenate text from every `Text` content block, in order.
    fn accumulated_text(message: &Message) -> String {
        message
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }
}

/// Drain the remaining HTTP body without allowing a non-conforming stream
/// to keep the background task alive indefinitely.
///
/// The timeout wraps the entire drain future. Recreating a timer inside the
/// poll loop would restart the deadline every time a malformed stream became
/// ready, defeating the timeout.
async fn drain_http_body<S>(mut http_stream: S)
where
    S: futures::Stream<Item = Result<MessageStreamEvent>> + Unpin,
{
    use futures::StreamExt;

    let drain = async {
        let mut saw_error = false;
        while let Some(result) = http_stream.next().await {
            if result.is_err() {
                saw_error = true;
                tracing::warn!("background task: HTTP body drain ended after stream error");
                break;
            }
        }
        if !saw_error {
            tracing::info!("background task: drain loop — stream returned None, fully consumed");
        }
    };

    if tokio::time::timeout(std::time::Duration::from_secs(5), drain)
        .await
        .is_err()
    {
        tracing::warn!("HTTP body drain timed out after 5s");
    }
}

impl Stream for MessageStream {
    type Item = Result<MessageStreamEvent>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        use futures::Stream as FuturesStream;

        let mut this = self.project();

        // Loop so that lagged events can be skipped without
        // returning a fatal error to the consumer.
        loop {
            // Re-check on every iteration so that a notify that
            // arrives between a Lagged skip and the next poll is not
            // missed.
            let stream_ended = this.ended.load(Ordering::Acquire);

            match FuturesStream::poll_next(this.event_stream.as_mut(), cx) {
                std::task::Poll::Ready(Some(Ok(event))) => {
                    // Got a real event -- always return it regardless
                    // of the ended flag (buffered events must be drained).
                    return std::task::Poll::Ready(Some(Ok(event)));
                }
                std::task::Poll::Ready(Some(Err(_err))) => {
                    // Broadcast channel lagged -- an event was dropped but
                    // the stream is still alive.  Skip it and continue
                    // polling for the next event.
                    tracing::debug!("broadcast receiver lagged, skipping event");
                    continue;
                }
                std::task::Poll::Ready(None) => {
                    // Broadcast channel closed (all senders dropped).
                    tracing::info!(
                        "MessageStream::poll_next: broadcast channel closed → returning None"
                    );
                    return std::task::Poll::Ready(None);
                }
                std::task::Poll::Pending => {
                    if stream_ended {
                        tracing::info!(
                            "MessageStream::poll_next: ended flag set, broadcast Pending → returning None"
                        );
                        return std::task::Poll::Ready(None);
                    }
                    // Stream not ended yet.  The broadcast receiver's
                    // waker is already registered (inside BroadcastStream
                    // poll_next), but the sender is kept alive by this
                    // struct so it will never emit Ready(None).  We poll
                    // the watch receiver — unlike Notify, `changed()` is
                    // *sticky*: if the sender wrote `true` between our
                    // last observation and this poll it resolves immediately,
                    // closing the race where the consumer parks just before
                    // the background task finishes.
                    let mut changed = std::pin::pin!(this.ended_watch.changed());
                    match changed.as_mut().poll(cx) {
                        std::task::Poll::Ready(Ok(())) => {
                            // Watch value changed — background task done.
                            // Re-check the ended AtomicBool (set by the same
                            // background task that wrote to the watch sender).
                            if this.ended.load(Ordering::Acquire) {
                                return std::task::Poll::Ready(None);
                            }
                            // Spurious — stay pending.
                            return std::task::Poll::Pending;
                        }
                        std::task::Poll::Ready(Err(_)) => {
                            // Watch sender dropped — treat as ended.
                            return std::task::Poll::Ready(None);
                        }
                        std::task::Poll::Pending => {
                            return std::task::Poll::Pending;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod drain_tests {
    use super::*;
    use futures::Stream;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    struct InfiniteErrorStream {
        polls: usize,
    }

    impl Stream for InfiniteErrorStream {
        type Item = Result<MessageStreamEvent>;

        fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            self.polls += 1;
            Poll::Ready(Some(Err(AnthropicError::StreamError(
                "persistent stream error".to_string(),
            ))))
        }
    }

    #[tokio::test]
    async fn drain_http_body_stops_after_first_error() {
        let mut stream = InfiniteErrorStream { polls: 0 };

        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            drain_http_body(&mut stream),
        )
        .await
        .expect("drain must stop after the first stream error");

        assert_eq!(stream.polls, 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_handlers() {
        use std::collections::HashMap;
        use std::sync::{Arc, Mutex};

        // Test creating a dummy response for testing
        let text_called = Arc::new(Mutex::new(false));
        let text_called_clone = text_called.clone();

        let _handler =
            EventHandler::Text(std::sync::Arc::new(Box::new(move |_delta, _snapshot| {
                *text_called_clone.lock().unwrap() = true;
            })));

        // Test event type equality
        assert_eq!(EventType::Text, EventType::Text);
        assert_ne!(EventType::Text, EventType::Error);

        // Test using event types as hash keys
        let mut map: HashMap<EventType, String> = HashMap::new();
        map.insert(EventType::Text, "text_handler".to_string());
        assert_eq!(map.get(&EventType::Text), Some(&"text_handler".to_string()));
    }

    /// Spin up a single-shot HTTP/1.1 server that responds with the
    /// provided raw bytes and then closes the connection. Used to
    /// exercise the SSE stream end-to-end without hitting the network.
    async fn single_shot_server(
        reply: &'static [u8],
        delay_before_reply: std::time::Duration,
    ) -> (String, tokio::task::JoinHandle<()>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                // Read the request (we don't actually care about its content)
                let mut buf = vec![0u8; 4096];
                let _ = sock.read(&mut buf).await;
                if !delay_before_reply.is_zero() {
                    tokio::time::sleep(delay_before_reply).await;
                }
                // Wrap the raw SSE body in a minimal valid HTTP/1.1
                // response so reqwest/hyper can parse it. We avoid
                // declaring Content-Length or Transfer-Encoding; a
                // connection close marks the end of the body, which is
                // the path we want to exercise (eventsource-stream
                // converts the close into a clean `None`).
                let response = "HTTP/1.1 200 OK\r\n\
                     content-type: text/event-stream\r\n\
                     connection: close\r\n\
                     \r\n"
                    .to_string();
                let _ = sock.write_all(response.as_bytes()).await;
                let _ = sock.write_all(reply).await;
                let _ = sock.shutdown().await;
            }
        });
        (format!("http://{addr}"), handle)
    }

    /// Server variant: accepts the connection, sends valid HTTP
    /// response headers, then deliberately stalls forever (does not
    /// write a body, does not close). Used to drive the client's
    /// per-chunk idle timeout.
    async fn single_shot_stall_server() -> (String, tokio::task::JoinHandle<()>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = vec![0u8; 4096];
                let _ = sock.read(&mut buf).await;
                let headers = "HTTP/1.1 200 OK\r\n\
                     content-type: text/event-stream\r\n\
                     connection: close\r\n\
                     \r\n"
                    .to_string();
                let _ = sock.write_all(headers.as_bytes()).await;
                // Sleep "forever" from the test's perspective — the
                // test's tokio clock is paused, so this would normally
                // block forever. We instead just hold the socket open.
                // The test will exit and the handle is dropped, which
                // closes the socket.
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
                }
            }
        });
        (format!("http://{addr}"), handle)
    }

    /// Valid SSE body mirroring a complete (but minimal) Anthropic
    /// streaming response.
    const VALID_SSE: &[u8] = b"\
event: message_start\r\n\
data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_x\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"claude\",\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":1,\"output_tokens\":0,\"cache_creation_input_tokens\":null,\"cache_read_input_tokens\":null,\"server_tool_use\":null,\"service_tier\":null}}}\r\n\
\r\n\
event: content_block_start\r\n\
data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\r\n\
\r\n\
event: content_block_delta\r\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}\r\n\
\r\n\
event: content_block_stop\r\n\
data: {\"type\":\"content_block_stop\",\"index\":0}\r\n\
\r\n\
event: message_delta\r\n\
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":2,\"input_tokens\":1,\"cache_creation_input_tokens\":null,\"cache_read_input_tokens\":null}}\r\n\
\r\n\
event: message_stop\r\n\
data: {\"type\":\"message_stop\"}\r\n\
\r\n";

    #[tokio::test(start_paused = true)]
    async fn from_http_stream_with_retry_idle_timeout_then_reconnect_succeeds() {
        // First server accepts the connection, sends the HTTP response
        // headers, then never writes any body and never closes — the
        // client's per-chunk idle timeout (1s) should fire and trigger
        // a reconnect. The second server returns a valid SSE body.
        let (url1, _h1) = single_shot_stall_server().await;
        let (url2, h2) = single_shot_server(VALID_SSE, std::time::Duration::ZERO).await;
        let _keep_h2 = h2; // ensure it lives long enough

        let client = reqwest::Client::new();
        let first_response = client
            .get(&url1)
            .send()
            .await
            .expect("first response should be reachable");

        let config = crate::http::streaming::StreamConfig {
            buffer_size: 64,
            event_timeout: Some(1),
            retry_on_error: true,
            max_retries: Some(2),
        };

        // Build a real HttpStreamClient from the first (stalled) response.
        let http_stream = crate::http::streaming::HttpStreamClient::from_response(
            first_response,
            config.clone(),
            Arc::new(crate::wire::AnthropicWire),
        )
        .await
        .expect("from_response should succeed for 200");

        // Reconnect closure: re-issue the same GET against a *different*
        // URL (the second server). In real code it would re-issue the
        // original POST; for this test we just want to prove the
        // background task will call it and resume on success.
        let url2 = url2.clone();
        let reconnect_config = config.clone();
        let reconnect = move || {
            let url2 = url2.clone();
            let client = client.clone();
            let config = reconnect_config.clone();
            async move {
                let resp = client.get(&url2).send().await.map_err(|e| {
                    crate::types::AnthropicError::Connection {
                        message: e.to_string(),
                    }
                })?;
                crate::http::streaming::HttpStreamClient::from_response(
                    resp,
                    config,
                    Arc::new(crate::wire::AnthropicWire),
                )
                .await
            }
        };

        let stream = MessageStream::from_http_stream_with_retry(http_stream, reconnect, config)
            .expect("from_http_stream_with_retry should construct ok");

        // The test runs under a paused tokio clock. We need to advance
        // it past the 1s idle timeout, then past the second request's
        // round-trip. Spawn a helper that periodically advances the
        // clock so the background task can make progress.
        tokio::spawn(async {
            for _ in 0..50 {
                tokio::time::advance(std::time::Duration::from_millis(200)).await;
            }
        });

        // Bound the wall-clock duration so a regression that drops the
        // timeout still surfaces as a test failure (the paused clock
        // makes this fast).
        let final_msg =
            tokio::time::timeout(std::time::Duration::from_secs(30), stream.final_message())
                .await
                .expect("final_message should resolve in bounded time")
                .expect("final_message should be Ok after a successful reconnect");

        assert_eq!(final_msg.id, "msg_x");
    }

    /// Server variant: accepts the connection, sends valid HTTP
    /// headers, writes a *prefix* body, then stalls forever. Used to
    /// simulate a server that begins the response but freezes
    /// mid-stream.
    async fn single_shot_partial_then_stall_server(
        prefix: &'static [u8],
    ) -> (String, tokio::task::JoinHandle<()>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = vec![0u8; 4096];
                let _ = sock.read(&mut buf).await;
                let headers = "HTTP/1.1 200 OK\r\n\
                     content-type: text/event-stream\r\n\
                     connection: close\r\n\
                     \r\n"
                    .to_string();
                let _ = sock.write_all(headers.as_bytes()).await;
                let _ = sock.write_all(prefix).await;
                // Hold the socket open without closing; the test's
                // tokio clock is paused, so this loop is essentially
                // "wait for the test to drop me".
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
                }
            }
        });
        (format!("http://{addr}"), handle)
    }

    #[tokio::test(start_paused = true)]
    async fn from_http_stream_with_retry_post_message_start_does_not_retry() {
        // Server writes MessageStart (and a content_block_start) and
        // then stalls. The retry policy must NOT reconnect here —
        // once we've started receiving the assistant message, partial
        // progress is irrecoverable. The stream should surface the
        // idle-timeout as a StreamError.
        let prefix = b"\
event: message_start\r\n\
data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_p\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"claude\",\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":1,\"output_tokens\":0,\"cache_creation_input_tokens\":null,\"cache_read_input_tokens\":null,\"server_tool_use\":null,\"service_tier\":null}}}\r\n\
\r\n\
event: content_block_start\r\n\
data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\r\n\
\r\n";

        let (url, _h) = single_shot_partial_then_stall_server(prefix).await;

        // Count how many times reconnect is invoked — should be zero.
        let reconnect_calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let rc = reconnect_calls.clone();
        let reconnect = move || {
            rc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            async {
                Err(crate::types::AnthropicError::StreamError(
                    "should not be called after MessageStart".to_string(),
                ))
            }
        };

        let client = reqwest::Client::new();
        let response = client.get(&url).send().await.expect("response");

        let config = crate::http::streaming::StreamConfig {
            buffer_size: 64,
            event_timeout: Some(1),
            retry_on_error: true,
            max_retries: Some(3),
        };

        let http_stream = crate::http::streaming::HttpStreamClient::from_response(
            response,
            config.clone(),
            Arc::new(crate::wire::AnthropicWire),
        )
        .await
        .unwrap();

        let stream = MessageStream::from_http_stream_with_retry(http_stream, reconnect, config)
            .expect("construct");

        // Spawn a clock-advance helper so the paused timeout can fire.
        tokio::spawn(async {
            for _ in 0..30 {
                tokio::time::advance(std::time::Duration::from_millis(200)).await;
            }
        });

        // Allow the idle timeout to elapse.
        let result =
            tokio::time::timeout(std::time::Duration::from_secs(30), stream.final_message())
                .await
                .expect("final_message resolves in bounded time");

        // After MessageStart, the stall must surface as an error rather
        // than trigger a silent retry.
        assert!(
            result.is_err(),
            "expected error after post-start stall, got {:?}",
            result.as_ref().map(|m| m.id.clone())
        );
        assert_eq!(
            reconnect_calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "reconnect must not fire after MessageStart"
        );
    }

    // ---- focused unit tests for the dispatch and accumulate paths ----

    /// Feed a small event list through `from_events` and verify every
    /// registered callback fires with the right payload.
    #[tokio::test]
    async fn callbacks_fire_for_dispatched_events() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let text_deltas = Arc::new(Mutex::new(String::new()));
        let text_snapshots = Arc::new(Mutex::new(Vec::<String>::new()));
        let stream_events = Arc::new(AtomicUsize::new(0));
        let end_called = Arc::new(AtomicUsize::new(0));
        let final_message_id = Arc::new(Mutex::new(None::<String>));

        let mut stream = MessageStream::from_events(
            vec![
                MessageStreamEvent::MessageStart {
                    message: sample_message("msg_1", 0, vec![]),
                },
                MessageStreamEvent::ContentBlockStart {
                    index: 0,
                    content_block: ContentBlock::Text {
                        text: String::new(),
                    },
                },
                MessageStreamEvent::ContentBlockDelta {
                    index: 0,
                    delta: ContentBlockDelta::TextDelta { text: "hi".into() },
                },
                MessageStreamEvent::ContentBlockDelta {
                    index: 0,
                    delta: ContentBlockDelta::TextDelta {
                        text: " there".into(),
                    },
                },
                MessageStreamEvent::ContentBlockStop { index: 0 },
                MessageStreamEvent::MessageDelta {
                    delta: crate::types::MessageDelta {
                        stop_reason: Some(crate::types::StopReason::EndTurn),
                        stop_sequence: None,
                    },
                    usage: crate::types::MessageDeltaUsage {
                        output_tokens: 2,
                        input_tokens: Some(1),
                        cache_creation_input_tokens: None,
                        cache_read_input_tokens: None,
                        server_tool_use: None,
                    },
                },
                MessageStreamEvent::MessageStop,
            ],
            sample_message(
                "msg_1",
                2,
                vec![ContentBlock::Text {
                    text: "hi there".into(),
                }],
            ),
        );

        let td = text_deltas.clone();
        let ts = text_snapshots.clone();
        let se = stream_events.clone();
        let ec = end_called.clone();
        let fm = final_message_id.clone();
        stream = stream
            .on_text(move |delta, snapshot| {
                td.lock().unwrap().push_str(delta);
                ts.lock().unwrap().push(snapshot.to_string());
            })
            .on_stream_event(move |_event, _msg| {
                se.fetch_add(1, Ordering::SeqCst);
            })
            .on_end(move || {
                ec.fetch_add(1, Ordering::SeqCst);
            })
            .on_final_message(move |msg| {
                *fm.lock().unwrap() = Some(msg.id.clone());
            });

        // Hand the stream off to the background drain.
        let final_msg = stream.final_message().await.unwrap();
        assert_eq!(final_msg.id, "msg_1");
        assert_eq!(
            final_msg.stop_reason,
            Some(crate::types::StopReason::EndTurn)
        );

        // on_text: deltas accumulated, snapshot grew monotonically.
        assert_eq!(text_deltas.lock().unwrap().as_str(), "hi there");
        assert_eq!(
            text_snapshots.lock().unwrap().as_slice(),
            &["hi".to_string(), "hi there".to_string()],
        );

        // on_stream_event: fires for every event (MessageStart, ContentBlockStart,
        // 2× ContentBlockDelta, ContentBlockStop, MessageDelta, MessageStop) = 7
        assert_eq!(stream_events.load(Ordering::SeqCst), 7);

        // on_end + on_final_message both fire on MessageStop.
        assert_eq!(end_called.load(Ordering::SeqCst), 1);
        assert_eq!(final_message_id.lock().unwrap().as_deref(), Some("msg_1"));
    }

    /// `on_error` should fire from the background task when the HTTP
    /// stream errors out (this also exercises the new `fire_error` path
    /// on the error branch).
    #[tokio::test]
    async fn on_error_fires_when_stream_errors() {
        use std::sync::atomic::{AtomicBool, Ordering};

        // A stream whose only event is an error.
        let stream = MessageStream::from_events(
            vec![],
            // We never get a final message; the background task exits
            // before it ever sees a MessageStop, exercising the fallback
            // "stream ended without message" path.
            sample_message("msg_never", 0, vec![]),
        );

        let called = Arc::new(AtomicBool::new(false));
        let called_c = called.clone();
        let stream = stream.on_error(move |_e| {
            called_c.store(true, Ordering::SeqCst);
        });

        // The from_events test path doesn't actually surface an error,
        // so this test only verifies the *registration* path compiles
        // and on_error() returns a usable stream. Real error dispatch
        // is exercised in the from_http_stream tests via stall servers.
        let _ = stream;
    }

    /// A tool_use block must accumulate the partial JSON stream as a
    /// `Value::String` while the block is open, and parse it on
    /// `ContentBlockStop` so the final message holds a structured Value.
    #[tokio::test]
    async fn tool_use_input_json_parses_on_block_stop() {
        let final_message = sample_message(
            "msg_tool",
            0,
            vec![ContentBlock::ToolUse {
                id: "tool_1".into(),
                name: "get_weather".into(),
                input: serde_json::json!({"city": "Beijing", "unit": "c"}),
            }],
        );

        let stream = MessageStream::from_events(
            vec![
                MessageStreamEvent::MessageStart {
                    message: sample_message("msg_tool", 0, vec![]),
                },
                MessageStreamEvent::ContentBlockStart {
                    index: 0,
                    content_block: ContentBlock::ToolUse {
                        id: "tool_1".into(),
                        name: "get_weather".into(),
                        input: serde_json::Value::String(String::new()),
                    },
                },
                MessageStreamEvent::ContentBlockDelta {
                    index: 0,
                    delta: ContentBlockDelta::InputJsonDelta {
                        partial_json: r#"{"city":"#.into(),
                    },
                },
                MessageStreamEvent::ContentBlockDelta {
                    index: 0,
                    delta: ContentBlockDelta::InputJsonDelta {
                        partial_json: r#""Beijing","unit":"c"}"#.into(),
                    },
                },
                MessageStreamEvent::ContentBlockStop { index: 0 },
                MessageStreamEvent::MessageStop,
            ],
            final_message,
        );

        let final_msg = stream.final_message().await.unwrap();
        let block = &final_msg.content[0];
        match block {
            ContentBlock::ToolUse { input, .. } => {
                let obj = input.as_object().expect("input should parse to an object");
                assert_eq!(obj.get("city").and_then(|v| v.as_str()), Some("Beijing"));
                assert_eq!(obj.get("unit").and_then(|v| v.as_str()), Some("c"));
            }
            other => panic!("expected ToolUse block, got {:?}", other),
        }
    }

    /// 安全网回归：适配器漏发 ContentBlockStop 时（Responses wire 曾如此），
    /// `MessageStop` 也必须把字符串缓冲的工具参数解析为对象——否则工具
    /// 执行器收到 `invalid type: string` 的 input。
    #[tokio::test]
    async fn tool_use_input_json_parses_on_message_stop_safety_net() {
        let final_message = sample_message(
            "msg_tool_nostop",
            0,
            vec![ContentBlock::ToolUse {
                id: "tool_1".into(),
                name: "list_agents".into(),
                input: serde_json::json!({}),
            }],
        );

        // 事件流里没有 ContentBlockStop——直接从 InputJsonDelta 跳到
        // MessageStop。
        let stream = MessageStream::from_events(
            vec![
                MessageStreamEvent::MessageStart {
                    message: sample_message("msg_tool_nostop", 0, vec![]),
                },
                MessageStreamEvent::ContentBlockStart {
                    index: 0,
                    content_block: ContentBlock::ToolUse {
                        id: "tool_1".into(),
                        name: "list_agents".into(),
                        input: serde_json::Value::String(String::new()),
                    },
                },
                MessageStreamEvent::ContentBlockDelta {
                    index: 0,
                    delta: ContentBlockDelta::InputJsonDelta {
                        partial_json: "{}".into(),
                    },
                },
                MessageStreamEvent::MessageStop,
            ],
            final_message,
        );

        let final_msg = stream.final_message().await.unwrap();
        match &final_msg.content[0] {
            ContentBlock::ToolUse { input, .. } => {
                assert!(
                    input.is_object(),
                    "input must be parsed to an object, got {input}"
                );
            }
            other => panic!("expected ToolUse block, got {:?}", other),
        }
    }

    fn sample_message(id: &str, output_tokens: u64, content: Vec<ContentBlock>) -> Message {
        use crate::types::Role;
        Message {
            id: id.to_string(),
            type_: "message".to_string(),
            role: Role::Assistant,
            content,
            model: Some("claude-test".to_string()),
            stop_reason: None,
            stop_sequence: None,
            usage: Some(crate::types::Usage {
                input_tokens: 1,
                output_tokens,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
                server_tool_use: None,
                service_tier: None,
            }),
            request_id: None,
        }
    }

    /// Verify that poll_next drains all buffered events before returning None.
    /// This is a regression test for a bug where the `_ => Ready(None)` arm
    /// in poll_next's `ended` branch converted `Pending` into premature None,
    /// losing events that were buffered in the broadcast channel.
    #[tokio::test]
    async fn poll_next_drains_buffered_events_before_returning_none() {
        let mut stream = MessageStream::from_events(
            vec![
                MessageStreamEvent::MessageStart {
                    message: sample_message("msg_drain", 0, vec![]),
                },
                MessageStreamEvent::ContentBlockStart {
                    index: 0,
                    content_block: ContentBlock::Text {
                        text: String::new(),
                    },
                },
                MessageStreamEvent::ContentBlockDelta {
                    index: 0,
                    delta: ContentBlockDelta::TextDelta { text: "a".into() },
                },
                MessageStreamEvent::ContentBlockDelta {
                    index: 0,
                    delta: ContentBlockDelta::TextDelta { text: "b".into() },
                },
                MessageStreamEvent::MessageStop,
            ],
            sample_message(
                "msg_drain",
                2,
                vec![ContentBlock::Text { text: "ab".into() }],
            ),
        );

        use futures::StreamExt;
        let mut count = 0u32;
        while let Some(result) = stream.next().await {
            assert!(result.is_ok(), "event should be Ok, got: {:?}", result);
            count += 1;
        }
        assert_eq!(count, 5, "all events should be drained before stream ends");
    }

    /// Regression test for the Zhipu GLM hang: when a provider sends only
    /// `MessageStart` + `MessageStop` in a single burst (no content blocks),
    /// the consumer must drain both events and then receive `None` from
    /// `poll_next` — not hang forever waiting for a watch-channel wakeup
    /// that was unregistered when the `changed()` future was dropped.
    #[tokio::test]
    async fn poll_next_exits_on_empty_burst_response() {
        let mut stream = MessageStream::from_events(
            vec![
                MessageStreamEvent::MessageStart {
                    message: sample_message("msg_empty", 0, vec![]),
                },
                MessageStreamEvent::MessageStop,
            ],
            sample_message("msg_empty", 0, vec![]),
        );

        use futures::StreamExt;
        let mut count = 0u32;
        // If the bug is present, this loop will hang forever.
        let result = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while let Some(event) = stream.next().await {
                assert!(event.is_ok(), "event should be Ok");
                count += 1;
            }
        })
        .await;

        assert!(
            result.is_ok(),
            "stream consumption timed out — consumer hung"
        );
        assert_eq!(count, 2, "should receive MessageStart and MessageStop");
    }
}
