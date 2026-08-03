use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Metadata describing a single model entry in the pool.
///
/// `ModelInfo` holds only model capabilities — it does **not** carry connection
/// config. Connection details (`base_url`, `api_key`, `auth_method`) live on the
/// referenced [`crate::model::ProviderConfig`], looked up at pool-build time via
/// `provider_id`.
#[derive(Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub model_name: String,
    /// Reference to the provider instance that supplies this model's connection.
    pub provider_id: Uuid,
    pub context_length: u64,
    pub max_output_tokens: u64,
    pub vision_ability: bool,
    pub supports_function_calling: bool,
    pub supports_streaming: bool,
    pub supports_thinking: bool,
    /// Whether extended thinking / reasoning is **actively enabled** for
    /// requests using this model. Distinct from `supports_thinking` (a
    /// capability declaration): a model may support thinking but have it
    /// turned off by default. When both `supports_thinking` and
    /// `thinking_enabled` are true, the request layer injects a
    /// [`ThinkingConfig`](agentik_types::ThinkingConfig) into the outgoing
    /// request.
    #[serde(default)]
    pub thinking_enabled: bool,
    /// Optional token budget for the thinking phase. When `None`, the
    /// request layer derives a sensible default from `max_output_tokens`
    /// (half of `max_output_tokens`, clamped to ≥1024).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_budget: Option<u32>,
    pub input_token_price: f64,
    pub output_token_price: f64,
}

impl Default for ModelInfo {
    fn default() -> Self {
        Self {
            model_name: String::new(),
            provider_id: Uuid::nil(),
            context_length: 0,
            max_output_tokens: 0,
            vision_ability: false,
            supports_function_calling: false,
            supports_streaming: false,
            supports_thinking: false,
            thinking_enabled: false,
            thinking_budget: None,
            input_token_price: 0.0,
            output_token_price: 0.0,
        }
    }
}

impl std::fmt::Debug for ModelInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelInfo")
            .field("model_name", &self.model_name)
            .field("provider_id", &self.provider_id)
            .field("context_length", &self.context_length)
            .field("max_output_tokens", &self.max_output_tokens)
            .field("vision_ability", &self.vision_ability)
            .field("supports_function_calling", &self.supports_function_calling)
            .field("supports_streaming", &self.supports_streaming)
            .field("supports_thinking", &self.supports_thinking)
            .field("thinking_enabled", &self.thinking_enabled)
            .field("thinking_budget", &self.thinking_budget)
            .field("input_token_price", &self.input_token_price)
            .field("output_token_price", &self.output_token_price)
            .finish()
    }
}

impl std::fmt::Display for ModelInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.model_name)
    }
}

impl PartialEq for ModelInfo {
    fn eq(&self, other: &Self) -> bool {
        self.model_name == other.model_name
            && self.provider_id == other.provider_id
            && self.context_length == other.context_length
            && self.max_output_tokens == other.max_output_tokens
            && self.vision_ability == other.vision_ability
            && self.supports_function_calling == other.supports_function_calling
            && self.supports_streaming == other.supports_streaming
            && self.supports_thinking == other.supports_thinking
            && self.thinking_enabled == other.thinking_enabled
            && self.thinking_budget == other.thinking_budget
            && self.input_token_price == other.input_token_price
            && self.output_token_price == other.output_token_price
    }
}

// ── Builder ──────────────────────────────────────────────────────────────────

/// Fluent builder for [`ModelInfo`].
///
/// Starts with sensible preset defaults (`provider_id = nil`, all capabilities
/// `false`, prices `0.0`). Use the grouped setters (`.context()`,
/// `.capabilities()`, `.pricing()`) to reduce boilerplate in provider
/// definitions, or set individual fields as needed.
///
/// # Example
/// ```
/// use agentik_sdk::model::model_info::{ModelInfo, ModelInfoBuilder};
///
/// let info = ModelInfoBuilder::new("my-model")
///     .context(128_000, 32_000)
///     .capabilities(false, true, true, true)
///     .pricing(0.5, 2.0)
///     .build();
/// ```
pub struct ModelInfoBuilder {
    model_name: String,
    provider_id: Uuid,
    context_length: u64,
    max_output_tokens: u64,
    vision_ability: bool,
    supports_function_calling: bool,
    supports_streaming: bool,
    supports_thinking: bool,
    thinking_enabled: bool,
    thinking_budget: Option<u32>,
    input_token_price: f64,
    output_token_price: f64,
}

impl ModelInfoBuilder {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            model_name: name.into(),
            provider_id: Uuid::nil(),
            context_length: 0,
            max_output_tokens: 0,
            vision_ability: false,
            supports_function_calling: false,
            supports_streaming: false,
            supports_thinking: false,
            thinking_enabled: false,
            thinking_budget: None,
            input_token_price: 0.0,
            output_token_price: 0.0,
        }
    }

    pub fn context(mut self, length: u64, max_output: u64) -> Self {
        self.context_length = length;
        self.max_output_tokens = max_output;
        self
    }

    pub fn capabilities(
        mut self,
        vision: bool,
        function_calling: bool,
        streaming: bool,
        thinking: bool,
    ) -> Self {
        self.vision_ability = vision;
        self.supports_function_calling = function_calling;
        self.supports_streaming = streaming;
        self.supports_thinking = thinking;
        self
    }

    /// Enable extended thinking for this model preset, optionally with a
    /// custom token budget. When omitted, the request layer derives a default
    /// from `max_output_tokens`.
    pub fn thinking_enabled(mut self, budget: Option<u32>) -> Self {
        self.thinking_enabled = true;
        self.thinking_budget = budget;
        self
    }

    pub fn pricing(mut self, input: f64, output: f64) -> Self {
        self.input_token_price = input;
        self.output_token_price = output;
        self
    }

    pub fn provider_id(mut self, id: Uuid) -> Self {
        self.provider_id = id;
        self
    }

    pub fn build(self) -> ModelInfo {
        ModelInfo {
            model_name: self.model_name,
            provider_id: self.provider_id,
            context_length: self.context_length,
            max_output_tokens: self.max_output_tokens,
            vision_ability: self.vision_ability,
            supports_function_calling: self.supports_function_calling,
            supports_streaming: self.supports_streaming,
            supports_thinking: self.supports_thinking,
            thinking_enabled: self.thinking_enabled,
            thinking_budget: self.thinking_budget,
            input_token_price: self.input_token_price,
            output_token_price: self.output_token_price,
        }
    }
}
