use std::sync::Arc;

use crate::client::Anthropic;
use crate::http::streaming::{StreamConfig, StreamRequestBuilder};
use crate::streaming::MessageStream;
use crate::types::errors::{AnthropicError, Result};
use crate::types::messages::*;

/// Messages API resource for interacting with Claude
pub struct MessagesResource<'a> {
    client: &'a Anthropic,
}

impl<'a> MessagesResource<'a> {
    /// Create a new Messages resource
    pub fn new(client: &'a Anthropic) -> Self {
        Self { client }
    }

    /// Create a message with Claude
    ///
    /// Send a structured list of input messages with text and/or image content,
    /// and Claude will generate the next message in the conversation.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// use agentik_sdk::{Anthropic, agentik_types::MessageCreateBuilder};
    ///
    /// let client = Anthropic::from_env()?;
    ///
    /// let message = client.messages().create(
    ///     MessageCreateBuilder::new("claude-3-5-sonnet-latest", 1024)
    ///         .user("Hello, Claude!")
    ///         .build()
    /// ).await?;
    ///
    /// println!("Claude responded: {:?}", message.content);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn create(&self, params: MessageCreateParams) -> Result<Message> {
        // Encode via the configured wire protocol. The encoder owns the
        // endpoint path, body bytes, and any protocol-specific headers; the
        // HTTP layer (auth, retry) stays unchanged.
        let wire_req = self.client.wire().encode_request(&params, false)?;
        let url = self.client.http_client().build_url(&wire_req.endpoint_path);

        let mut builder = self
            .client
            .http_client()
            .post(&url)
            .header("Content-Type", "application/json");
        for (name, value) in &wire_req.headers {
            builder = builder.header(*name, *value);
        }
        let request = builder.body(wire_req.body.clone()).build().map_err(|e| {
            AnthropicError::Connection {
                message: e.to_string(),
            }
        })?;

        let response = self.client.http_client().send(request).await?;

        // Extract request ID from headers
        let request_id = self.client.http_client().extract_request_id(&response);

        let status = response.status().as_u16();
        let body = response.text().await.map_err(|e| {
            AnthropicError::from_status(status, format!("failed to read response body: {e}"))
        })?;

        self.client
            .wire()
            .decode_response(status, &body, request_id)
    }

    /// Create a streaming message with Claude
    ///
    /// Send a message request and receive a real-time stream of the response.
    /// This allows you to process Claude's response as it's being generated.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use agentik_sdk::{Anthropic, MessageCreateBuilder};
    /// use futures::StreamExt;
    ///
    /// let client = Anthropic::from_env()?;
    ///
    /// let stream = client.messages().create_stream(
    ///     MessageCreateBuilder::new("claude-3-5-sonnet-latest", 1024)
    ///         .user("Write a story about AI")
    ///         .stream(true)
    ///         .build()
    /// ).await?;
    ///
    /// // Option 1: Use callbacks
    /// let final_message = stream
    ///     .on_text(|delta, _| print!("{}", delta))
    ///     .on_error(|error| eprintln!("Error: {}", error))
    ///     .final_message().await?;
    ///
    /// // Option 2: Manual iteration
    /// while let Some(event) = stream.next().await {
    ///     // Process each event as needed
    /// }
    /// ```
    pub async fn create_stream(&self, params: MessageCreateParams) -> Result<MessageStream> {
        self.create_stream_with_config(params, StreamConfig::default())
            .await
    }

    /// Create a streaming message with explicit `StreamConfig`.
    ///
    /// The supplied config controls per-chunk idle timeout (`event_timeout`),
    /// automatic reconnect-and-retry on pre-`MessageStart` stalls
    /// (`retry_on_error` / `max_retries`), and the broadcast buffer size.
    pub async fn create_stream_with_config(
        &self,
        mut params: MessageCreateParams,
        config: StreamConfig,
    ) -> Result<MessageStream> {
        // Ensure streaming is enabled
        params.stream = Some(true);

        // Encode once via the wire so we know the endpoint path, body, and
        // protocol-specific headers. The reconnect closure re-encodes on each
        // retry (cheap relative to the network round-trip) so the wire stays
        // the single source of truth for the on-the-wire shape.
        let wire_req = self.client.wire().encode_request(&params, true)?;
        // `post_stream` expects the path without a leading slash.
        let endpoint = wire_req.endpoint_path.trim_start_matches('/').to_string();

        // Snapshot everything we need to re-issue the request later from
        // inside the background task.
        let http_client = self.client.http_client().client().clone();
        let base_url = self.client.config().base_url.clone();
        let api_key = self.client.config().api_key.clone();
        let auth_header = format!("Bearer {}", api_key);
        let wire_headers = wire_req.headers.clone();
        let body = wire_req.body.clone();
        let config = Arc::new(config);

        // Build the initial streaming request with proper authentication.
        // Wire-supplied headers (e.g. `anthropic-version`) augment the auth
        // headers managed by the stream builder.
        let mut stream_builder =
            StreamRequestBuilder::new(http_client.clone(), base_url.clone())
                .header("Authorization", &auth_header)
                .header("Content-Type", "application/json")
                .wire(self.client.wire().clone())
                .config((*config).clone());
        for (name, value) in &wire_headers {
            stream_builder = stream_builder.header(name, value);
        }

        // The wire produced a raw body; pass it through as a JSON value so
        // `post_stream` doesn't re-serialise a different shape.
        let raw_body: serde_json::Value = serde_json::from_slice(&body).map_err(|e| {
            AnthropicError::Connection {
                message: format!("wire body was not valid JSON: {e}"),
            }
        })?;

        // Make the streaming request to get the real HTTP stream
        let http_stream = stream_builder.post_stream(&endpoint, &raw_body).await?;

        // The reconnect closure re-issues the same request, returning a
        // fresh HttpStreamClient. The background task invokes it whenever
        // the current connection stalls *before* any `MessageStart` event
        // has been observed.
        let reconnect = {
            let http_client = http_client.clone();
            let base_url = base_url.clone();
            let auth_header = auth_header.clone();
            let wire_headers = wire_headers.clone();
            let wire = self.client.wire().clone();
            let endpoint = endpoint.clone();
            let body = body.clone();
            let config = config.clone();
            move || {
                let http_client = http_client.clone();
                let base_url = base_url.clone();
                let auth_header = auth_header.clone();
                let wire_headers = wire_headers.clone();
                let wire = wire.clone();
                let endpoint = endpoint.clone();
                let body = body.clone();
                let config = config.clone();
                async move {
                    let mut builder =
                        StreamRequestBuilder::new(http_client, base_url)
                            .header("Authorization", &auth_header)
                            .header("Content-Type", "application/json")
                            .wire(wire)
                            .config((*config).clone());
                    for (name, value) in &wire_headers {
                        builder = builder.header(name, value);
                    }
                    let raw: serde_json::Value = serde_json::from_slice(&body).map_err(|e| {
                        AnthropicError::Connection {
                            message: format!("wire body was not valid JSON: {e}"),
                        }
                    })?;
                    builder.post_stream(&endpoint, &raw).await
                }
            }
        };

        // Create MessageStream that processes the real HTTP stream events
        // and reconnects transparently on pre-start stalls.
        let message_stream =
            MessageStream::from_http_stream_with_retry(http_stream, reconnect, (*config).clone())?;

        Ok(message_stream)
    }

    /// Create a streaming message using the builder pattern
    ///
    /// This is a convenience method that provides an ergonomic API for creating streaming messages.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use agentik_sdk::Anthropic;
    ///
    /// let client = Anthropic::from_env()?;
    ///
    /// let final_message = client.messages()
    ///     .create_with_builder("claude-3-5-sonnet-latest", 1024)
    ///     .user("Write a poem about the ocean")
    ///     .system("You are a creative poet.")
    ///     .temperature(0.8)
    ///     .stream()
    ///     .await?
    ///     .on_text(|delta, _| print!("{}", delta))
    ///     .final_message()
    ///     .await?;
    /// ```
    pub async fn stream(&self, params: MessageCreateParams) -> Result<MessageStream> {
        self.create_stream(params).await
    }

    /// Create a message using the builder pattern
    ///
    /// This is a convenience method that provides an ergonomic API for creating messages.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// use agentik_sdk::Anthropic;
    ///
    /// let client = Anthropic::from_env()?;
    ///
    /// let message = client.messages()
    ///     .create_with_builder("claude-3-5-sonnet-latest", 1024)
    ///     .user("What is the capital of France?")
    ///     .system("You are a helpful geography assistant.")
    ///     .temperature(0.3)
    ///     .send()
    ///     .await?;
    ///
    /// println!("Response: {:?}", message.content);
    /// # Ok(())
    /// # }
    /// ```
    pub fn create_with_builder(
        &'a self,
        model: impl Into<String>,
        max_tokens: u32,
    ) -> MessageCreateBuilderWithClient<'a> {
        MessageCreateBuilderWithClient {
            resource: self,
            builder: MessageCreateBuilder::new(model, max_tokens),
        }
    }
}

/// A message builder with a client reference for sending requests
pub struct MessageCreateBuilderWithClient<'a> {
    resource: &'a MessagesResource<'a>,
    builder: MessageCreateBuilder,
}

impl<'a> MessageCreateBuilderWithClient<'a> {
    /// Add a message to the conversation
    pub fn message(mut self, role: Role, content: impl Into<MessageContent>) -> Self {
        self.builder = self.builder.message(role, content);
        self
    }

    /// Add a user message
    pub fn user(mut self, content: impl Into<MessageContent>) -> Self {
        self.builder = self.builder.user(content);
        self
    }

    /// Add an assistant message
    pub fn assistant(mut self, content: impl Into<MessageContent>) -> Self {
        self.builder = self.builder.assistant(content);
        self
    }

    /// Set the system prompt
    pub fn system(mut self, system: impl Into<String>) -> Self {
        self.builder = self.builder.system(system);
        self
    }

    /// Set the temperature
    pub fn temperature(mut self, temperature: f32) -> Self {
        self.builder = self.builder.temperature(temperature);
        self
    }

    /// Set top_p
    pub fn top_p(mut self, top_p: f32) -> Self {
        self.builder = self.builder.top_p(top_p);
        self
    }

    /// Set top_k
    pub fn top_k(mut self, top_k: u32) -> Self {
        self.builder = self.builder.top_k(top_k);
        self
    }

    /// Set custom stop sequences
    pub fn stop_sequences(mut self, stop_sequences: Vec<String>) -> Self {
        self.builder = self.builder.stop_sequences(stop_sequences);
        self
    }

    /// Enable streaming
    pub fn stream(mut self, stream: bool) -> Self {
        self.builder = self.builder.stream(stream);
        self
    }

    /// Send the message request
    pub async fn send(self) -> Result<Message> {
        self.resource.create(self.builder.build()).await
    }

    /// Send the message request as a stream
    ///
    /// This enables streaming mode and returns a MessageStream for real-time processing.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let stream = client.messages()
    ///     .create_with_builder("claude-3-5-sonnet-latest", 1024)
    ///     .user("Tell me a story")
    ///     .stream_send()
    ///     .await?;
    ///
    /// let final_message = stream
    ///     .on_text(|delta, _| print!("{}", delta))
    ///     .final_message()
    ///     .await?;
    /// ```
    pub async fn stream_send(self) -> Result<MessageStream> {
        let params = self.builder.stream(true).build();
        self.resource.create_stream(params).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::messages::{ContentBlockParam, MessageContent};

    #[test]
    fn test_message_create_params_serialization() {
        let params = MessageCreateBuilder::new("claude-3-5-sonnet-latest", 1024)
            .user("Hello, world!")
            .system("You are helpful")
            .temperature(0.7)
            .build();

        let json = serde_json::to_value(&params).unwrap();

        assert_eq!(json["model"], "claude-3-5-sonnet-latest");
        assert_eq!(json["max_tokens"], 1024);
        assert_eq!(json["messages"].as_array().unwrap().len(), 1);
        assert_eq!(json["system"], "You are helpful");

        // Handle floating point precision by checking if the value is close to 0.7
        let temperature = json["temperature"].as_f64().unwrap();
        assert!(
            (temperature - 0.7).abs() < 0.001,
            "Temperature should be close to 0.7, got {}",
            temperature
        );
    }

    #[test]
    fn test_complex_message_content() {
        let content = MessageContent::Blocks(vec![
            ContentBlockParam::text("Here's an image:"),
            ContentBlockParam::image_base64("image/jpeg", "base64data"),
        ]);

        let params = MessageCreateBuilder::new("claude-3-5-sonnet-latest", 1024)
            .user(content)
            .build();

        let json = serde_json::to_value(&params).unwrap();
        let message_content = &json["messages"][0]["content"];

        assert!(message_content.is_array());
        assert_eq!(message_content.as_array().unwrap().len(), 2);
        assert_eq!(message_content[0]["type"], "text");
        assert_eq!(message_content[1]["type"], "image");
    }

    #[test]
    fn test_multi_message_conversation() {
        let params = MessageCreateBuilder::new("claude-3-5-sonnet-latest", 1024)
            .user("Hello!")
            .assistant("Hi there! How can I help you?")
            .user("What's the weather like?")
            .build();

        assert_eq!(params.messages.len(), 3);
        assert_eq!(params.messages[0].role, Role::User);
        assert_eq!(params.messages[1].role, Role::Assistant);
        assert_eq!(params.messages[2].role, Role::User);
    }
}
