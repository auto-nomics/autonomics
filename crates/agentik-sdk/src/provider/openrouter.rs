use uuid::Uuid;

use crate::http::auth::AuthMethod;
use crate::model::ModelInfo;
use crate::model::ProviderType;
use crate::model::model_info::ModelInfoBuilder;
use crate::provider::ProviderPreset;
use crate::wire::WireProtocolKind;

// ─── Model IDs ──────────────────────────────────────────────────────────────
// Anthropic
pub const MODEL_CLAUDE_FABLE_5_1: &str = "anthropic/claude-fable-5.1";
pub const MODEL_CLAUDE_OPUS_5: &str = "anthropic/claude-opus-5";
pub const MODEL_CLAUDE_SONNET_5: &str = "anthropic/claude-sonnet-5";
pub const MODEL_CLAUDE_HAIKU_4_5: &str = "anthropic/claude-haiku-4.5";
// OpenAI
pub const MODEL_GPT_5_5: &str = "openai/gpt-5.5";
pub const MODEL_GPT_5_1: &str = "openai/gpt-5.1";
// Open-weight / China ecosystem
pub const MODEL_KIMI_K3: &str = "moonshotai/kimi-k3";
pub const MODEL_GLM_5: &str = "z-ai/glm-5";
pub const MODEL_MINIMAX_M3: &str = "minimax/minimax-m3";
pub const MODEL_DEEPSEEK_V4_PRO: &str = "deepseek/deepseek-v4-pro";
pub const MODEL_QWEN3_MAX: &str = "qwen/qwen3-max";

/// OpenRouter OpenAI-compatible API endpoint. The OpenAI chat wire appends
/// `/v1/chat/completions`, yielding `https://openrouter.ai/api/v1/chat/completions`.
pub const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api";

pub struct OpenrouterProvider;

impl ProviderPreset for OpenrouterProvider {
    fn provider_type() -> ProviderType {
        ProviderType::Openrouter
    }
    fn preset_models() -> Vec<ModelInfo> {
        Self::model_definitions()
    }
    fn default_base_url() -> &'static str {
        DEFAULT_BASE_URL
    }
    fn default_auth_method() -> AuthMethod {
        AuthMethod::Bearer
    }
    fn wire_protocol() -> WireProtocolKind {
        WireProtocolKind::OpenaiChat
    }
    fn supports_remote_catalog() -> bool {
        true
    }
}

impl OpenrouterProvider {
    /// Preset model catalogue for the openrouter provider type — metadata only.
    ///
    /// A small cross-vendor selection so the tree view works before the first
    /// remote refresh; the full live catalogue is available via
    /// [`fetch_remote_catalog`](Self::fetch_remote_catalog).
    pub fn preset_models() -> Vec<ModelInfo> {
        <Self as ProviderPreset>::preset_models()
    }

    fn model_definitions() -> Vec<ModelInfo> {
        vec![
            // ── Anthropic ──────────────────────────────────────────────
            // Fable 5.1 — current frontier leader. Vision + thinking.
            ModelInfoBuilder::new(MODEL_CLAUDE_FABLE_5_1)
                .context(1_048_576, 128_000)
                .capabilities(true, true, true, true)
                .thinking_enabled(None)
                .pricing(10.0, 50.0)
                .build(),
            // Opus 5 — coding & hard agent tasks.
            ModelInfoBuilder::new(MODEL_CLAUDE_OPUS_5)
                .context(1_048_576, 128_000)
                .capabilities(true, true, true, true)
                .thinking_enabled(None)
                .pricing(5.0, 25.0)
                .build(),
            // Sonnet 5 — balanced workhorse.
            ModelInfoBuilder::new(MODEL_CLAUDE_SONNET_5)
                .context(1_048_576, 128_000)
                .capabilities(true, true, true, true)
                .thinking_enabled(None)
                .pricing(2.0, 10.0)
                .build(),
            // Haiku 4.5 — budget tier with free variants available.
            ModelInfoBuilder::new(MODEL_CLAUDE_HAIKU_4_5)
                .context(200_000, 64_000)
                .capabilities(true, true, true, true)
                .thinking_enabled(None)
                .pricing(1.0, 5.0)
                .build(),
            // ── OpenAI ─────────────────────────────────────────────────
            // GPT-5.5 — flagship, 1.05M context. Vision + reasoning.
            ModelInfoBuilder::new(MODEL_GPT_5_5)
                .context(1_050_000, 128_000)
                .capabilities(true, true, true, true)
                .thinking_enabled(None)
                .pricing(5.0, 30.0)
                .build(),
            // GPT-5.1 — cost-efficient reasoning.
            ModelInfoBuilder::new(MODEL_GPT_5_1)
                .context(400_000, 128_000)
                .capabilities(true, true, true, true)
                .thinking_enabled(None)
                .pricing(1.25, 10.0)
                .build(),
            // ── Open-weight / China ecosystem ──────────────────────────
            // Kimi K3 — 1M context, vision + video input, 943K max output.
            ModelInfoBuilder::new(MODEL_KIMI_K3)
                .context(1_048_576, 943_718)
                .capabilities(true, true, true, true)
                .thinking_enabled(None)
                .pricing(3.0, 15.0)
                .build(),
            // GLM-5 — text-only, agentic + coding. Cheapest thinking tier.
            ModelInfoBuilder::new(MODEL_GLM_5)
                .context(204_800, 128_000)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.60, 1.92)
                .build(),
            // MiniMax M3 — 1M context, vision + video input.
            ModelInfoBuilder::new(MODEL_MINIMAX_M3)
                .context(1_048_576, 512_000)
                .capabilities(true, true, true, true)
                .thinking_enabled(None)
                .pricing(0.30, 1.20)
                .build(),
            // DeepSeek V4 Pro — flagship reasoning, text-only.
            ModelInfoBuilder::new(MODEL_DEEPSEEK_V4_PRO)
                .context(1_048_576, 384_000)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(1.04, 2.08)
                .build(),
            // Qwen3 Max — text-only, hybrid thinking.
            ModelInfoBuilder::new(MODEL_QWEN3_MAX)
                .context(262_144, 65_536)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.78, 3.90)
                .build(),
        ]
    }
}

// ─── Remote catalogue ───────────────────────────────────────────────────────

/// Subset of OpenRouter's `GET /v1/models` response schema. Field names match
/// the live API; everything optional so entries missing a field degrade
/// gracefully instead of failing the whole fetch.
#[derive(serde::Deserialize)]
struct RemoteModelsResponse {
    data: Vec<RemoteModel>,
}

#[derive(serde::Deserialize)]
struct RemoteModel {
    id: String,
    #[serde(default)]
    context_length: u64,
    #[serde(default)]
    top_provider: Option<RemoteTopProvider>,
    #[serde(default)]
    pricing: Option<RemotePricing>,
    #[serde(default)]
    architecture: Option<RemoteArchitecture>,
    #[serde(default)]
    supported_parameters: Vec<String>,
}

#[derive(serde::Deserialize)]
struct RemoteTopProvider {
    #[serde(default)]
    max_completion_tokens: Option<u64>,
}

/// Pricing fields arrive as decimal strings ("0.000002") — USD per token.
#[derive(serde::Deserialize)]
struct RemotePricing {
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    completion: Option<String>,
}

#[derive(serde::Deserialize)]
struct RemoteArchitecture {
    #[serde(default)]
    input_modalities: Vec<String>,
}

/// Convert a per-token USD string to a per-million-token price.
fn per_million(v: &Option<String>) -> f64 {
    v.as_deref()
        .and_then(|s| s.parse::<f64>().ok())
        .map(|p| p * 1_000_000.0)
        .unwrap_or(0.0)
}

/// Map decoded catalogue entries to metadata-only [`ModelInfo`]s.
/// Batch (`:batch`) routing variants are filtered out; `:free` variants are
/// kept (they are genuinely distinct — zero cost).
fn map_remote(data: Vec<RemoteModel>) -> Vec<ModelInfo> {
    data.into_iter()
        .filter(|m| !m.id.ends_with(":batch"))
        .map(|m| {
            let RemoteModel {
                id,
                context_length,
                top_provider,
                pricing,
                architecture,
                supported_parameters,
            } = m;
            let vision = architecture
                .as_ref()
                .map(|a| a.input_modalities.iter().any(|x| x == "image"))
                .unwrap_or(false);
            let max_output = top_provider
                .and_then(|t| t.max_completion_tokens)
                .filter(|t| *t > 0)
                .unwrap_or(context_length);
            let (input_price, output_price) = pricing
                .map(|p| (per_million(&p.prompt), per_million(&p.completion)))
                .unwrap_or((0.0, 0.0));
            ModelInfoBuilder::new(id)
                .context(context_length, max_output)
                .capabilities(
                    vision,
                    supported_parameters.iter().any(|p| p == "tools"),
                    true,
                    supported_parameters.iter().any(|p| p == "reasoning"),
                )
                .pricing(input_price, output_price)
                .build()
        })
        .collect()
}

impl OpenrouterProvider {
    /// Fetch the live model catalogue from OpenRouter's public endpoint
    /// (`GET {base_url}/v1/models` — no auth required).
    ///
    /// Returns metadata-only [`ModelInfo`] entries with live pricing;
    /// `provider_id` is left nil for the caller to bind when persisting.
    ///
    /// # Errors
    ///
    /// Returns a human-readable message on connection failure, non-2xx
    /// status, or a decode error — suitable for direct display.
    pub async fn fetch_remote_catalog(
        base_url: &str,
    ) -> std::result::Result<Vec<ModelInfo>, String> {
        let url = format!("{}/v1/models", base_url.trim_end_matches('/'));
        let response = reqwest::Client::new()
            .get(&url)
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| format!("catalogue request failed: {e}"))?;
        if !response.status().is_success() {
            return Err(format!("catalogue endpoint returned {}", response.status()));
        }
        let catalogue: RemoteModelsResponse = response
            .json()
            .await
            .map_err(|e| format!("catalogue decode failed: {e}"))?;
        Ok(map_remote(catalogue.data))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_catalogue_has_cross_vendor_entries() {
        let models = OpenrouterProvider::preset_models();
        assert!(models.len() >= 10);
        assert!(
            models
                .iter()
                .any(|m| m.model_name == "anthropic/claude-sonnet-5")
        );
        assert!(models.iter().any(|m| m.model_name == "z-ai/glm-5"));
        // Every preset entry keeps a nil provider_id for the caller to bind.
        assert!(models.iter().all(|m| m.provider_id == Uuid::nil()));
    }

    #[test]
    fn remote_catalogue_json_maps_to_model_info() {
        let json = r#"{"data":[{
            "id": "vendor/model-x",
            "context_length": 262144,
            "top_provider": {"max_completion_tokens": 65536},
            "pricing": {"prompt": "0.00000078", "completion": "0.0000039"},
            "architecture": {"input_modalities": ["text", "image"]},
            "supported_parameters": ["tools", "reasoning"]
        }, {
            "id": "vendor/model-x:batch",
            "context_length": 262144,
            "pricing": {}
        }, {
            "id": "vendor/model-y",
            "name": "missing everything else"
        }]}"#;
        let cat: RemoteModelsResponse = serde_json::from_str(json).unwrap();
        let models = map_remote(cat.data);

        assert_eq!(models.len(), 2, ":batch variant filtered");

        let x = &models[0];
        assert_eq!(x.model_name, "vendor/model-x");
        assert_eq!(x.context_length, 262_144);
        assert_eq!(x.max_output_tokens, 65_536);
        assert!(x.vision_ability);
        assert!(x.supports_function_calling);
        assert!(x.supports_thinking);
        assert!((x.input_token_price - 0.78).abs() < 1e-9);
        assert!((x.output_token_price - 3.90).abs() < 1e-9);

        // Degenerate entry: absent optionals fall back to defaults.
        let y = &models[1];
        assert!(!y.vision_ability);
        assert_eq!(y.max_output_tokens, 0);
        assert_eq!(y.input_token_price, 0.0);
    }
}
