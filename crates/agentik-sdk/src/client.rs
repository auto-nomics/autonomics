use std::sync::Arc;

use crate::config::ClientConfig;
use crate::http::HttpClient;
use crate::resources::{BatchesResource, FilesResource, MessagesResource, ModelsResource};
use crate::types::errors::Result;
use crate::wire::{AnthropicWire, WireProtocol};

/// Main Anthropic API client.
///
/// Despite the historical name, this client is **wire-protocol-agnostic**: it
/// holds an [`Arc<dyn WireProtocol>`] and routes every request through it. The
/// default wire is [`AnthropicWire`] (Anthropic Messages API); construct with
/// [`Self::with_wire`] to swap in another protocol impl (OpenAI Chat /
/// Responses adapters will ship in subsequent milestones).
pub struct Anthropic {
    config: ClientConfig,
    http_client: HttpClient,
    wire: Arc<dyn WireProtocol>,
}

impl Anthropic {
    /// Create a new Anthropic client with the provided API key.
    ///
    /// Uses the default [`AnthropicWire`] protocol.
    pub fn new(api_key: impl Into<String>, base_url: impl Into<String>) -> Result<Self> {
        Self::with_wire(api_key, base_url, Arc::new(AnthropicWire))
    }

    /// Create a new Anthropic client with custom configuration.
    ///
    /// The wire protocol defaults to [`AnthropicWire`]; use
    /// [`Self::with_config_and_wire`] to supply a different impl.
    pub fn with_config(config: ClientConfig) -> Result<Self> {
        Self::with_config_and_wire(config, Arc::new(AnthropicWire))
    }

    /// Create a client with an explicit API key, base URL, and wire protocol.
    ///
    /// Use this when routing through a non-default protocol (e.g. an OpenAI
    /// adapter once those ship).
    pub fn with_wire(
        api_key: impl Into<String>,
        base_url: impl Into<String>,
        wire: Arc<dyn WireProtocol>,
    ) -> Result<Self> {
        let config = ClientConfig::new(api_key, base_url);
        Self::with_config_and_wire(config, wire)
    }

    /// Create a client with an explicit config and wire protocol.
    pub fn with_config_and_wire(
        config: ClientConfig,
        wire: Arc<dyn WireProtocol>,
    ) -> Result<Self> {
        let http_client = HttpClient::new(config.clone())?;
        Ok(Self {
            config,
            http_client,
            wire,
        })
    }

    /// Get the current configuration
    pub fn config(&self) -> &ClientConfig {
        &self.config
    }

    /// Get a reference to the HTTP client for internal use
    pub(crate) fn http_client(&self) -> &HttpClient {
        &self.http_client
    }

    /// Get the wire protocol in use.
    pub fn wire(&self) -> &Arc<dyn WireProtocol> {
        &self.wire
    }

    /// Test the connection by making a simple API call
    pub async fn test_connection(&self) -> Result<()> {
        // This will be implemented once we have the messages API
        // For now, we'll just validate the configuration
        self.config.validate()?;
        tracing::info!("Anthropic client initialized successfully");
        Ok(())
    }

    /// Access to the Messages API
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
    pub fn messages(&self) -> MessagesResource<'_> {
        MessagesResource::new(self)
    }

    /// Access to the Message Batches API (Beta)
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// use agentik_sdk::{Anthropic, agentik_types::{BatchRequest, BatchCreateParams}};
    ///
    /// let client = Anthropic::from_env()?;
    ///
    /// let requests = vec![
    ///     BatchRequest::builder("req1", "claude-3-5-sonnet-latest", 1024)
    ///         .user("Hello, world!")
    ///         .build(),
    /// ];
    ///
    /// let batch = client.batches().create(
    ///     BatchCreateParams::new(requests)
    /// ).await?;
    ///
    /// println!("Created batch: {}", batch.id);
    /// # Ok(())
    /// # }
    /// ```
    pub fn batches(&self) -> BatchesResource {
        BatchesResource::new(std::sync::Arc::new(self.http_client.clone()))
    }

    /// Access to the Files API (Beta)
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// use agentik_sdk::{Anthropic, FileUploadParams, FilePurpose};
    ///
    /// let client = Anthropic::from_env()?;
    ///
    /// let upload_params = FileUploadParams::new(
    ///     std::fs::read("document.pdf")?,
    ///     "document.pdf",
    ///     "application/pdf",
    ///     FilePurpose::Document,
    /// );
    ///
    /// let file_object = client.files().upload(upload_params).await?;
    /// println!("Uploaded file: {}", file_object.id);
    /// # Ok(())
    /// # }
    /// ```
    pub fn files(&self) -> FilesResource {
        FilesResource::new(std::sync::Arc::new(self.http_client.clone()))
    }

    /// Access to the Models API
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// use agentik_sdk::{Anthropic, ModelListParams};
    ///
    /// let client = Anthropic::from_env()?;
    ///
    /// // List all models
    /// let models = client.models().list(None).await?;
    /// println!("Found {} models", models.data.len());
    ///
    /// // Get specific model
    /// let model = client.models().get("claude-3-5-sonnet-latest").await?;
    /// println!("Model: {} ({})", model.display_name, model.id);
    /// # Ok(())
    /// # }
    /// ```
    pub fn models(&self) -> ModelsResource<'_> {
        ModelsResource::new(self)
    }
}

impl std::fmt::Debug for Anthropic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Anthropic")
            .field("base_url", &self.config.base_url)
            .field("timeout", &self.config.timeout)
            .field("max_retries", &self.config.max_retries)
            .field("log_level", &self.config.log_level)
            .finish_non_exhaustive()
    }
}
