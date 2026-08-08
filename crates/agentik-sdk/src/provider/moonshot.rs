use crate::http::auth::AuthMethod;
use crate::model::ModelInfo;
use crate::model::ProviderType;
use crate::model::model_info::ModelInfoBuilder;
use crate::provider::ProviderPreset;

// ─── Model IDs ──────────────────────────────────────────────────────────────
// Flagship
pub const MODEL_KIMI_K3: &str = "kimi-k3";
// Coding
pub const MODEL_KIMI_K2_7_CODE: &str = "kimi-k2.7-code";
pub const MODEL_KIMI_K2_7_CODE_HIGHSPEED: &str = "kimi-k2.7-code-highspeed";
// Multimodal
pub const MODEL_KIMI_K2_6: &str = "kimi-k2.6";

/// Moonshot / Kimi Anthropic-compatible API endpoint.
pub const DEFAULT_BASE_URL: &str = "https://api.moonshot.cn/anthropic";

pub struct MoonshotProvider;

impl ProviderPreset for MoonshotProvider {
    fn provider_type() -> ProviderType {
        ProviderType::Moonshot
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
}

impl MoonshotProvider {
    /// Preset model catalogue for the moonshot provider type — metadata only.
    ///
    /// `provider_id` is left nil; the caller binds a model to a provider
    /// instance when persisting it.
    pub fn preset_models() -> Vec<ModelInfo> {
        <Self as ProviderPreset>::preset_models()
    }

    fn model_definitions() -> Vec<ModelInfo> {
        vec![
            // ── Flagship: Kimi K3 — 1M context, multimodal ──────────────
            // 2.8T params; native vision understanding, deep reasoning,
            // software engineering & knowledge work. Supports thinking mode.
            // Pricing (CNY/1M, cache-miss): ¥20 in / ¥100 out ≈ $2.78 / $13.89.
            ModelInfoBuilder::new(MODEL_KIMI_K3)
                .context(1_048_576, 65_536)
                .capabilities(true, true, true, true)
                .thinking_enabled(None)
                .pricing(2.78, 13.89)
                .build(),
            // ── Coding: Kimi K2.7-Code — 256K context ────────────────────
            // Coding-specialized; reliable instruction following in long
            // contexts. ~180 TPS output (up to 260 TPS for short contexts).
            // Pricing (CNY/1M, cache-miss): ¥6.50 in / ¥27 out ≈ $0.90 / $3.75.
            ModelInfoBuilder::new(MODEL_KIMI_K2_7_CODE)
                .context(262_144, 16_384)
                .capabilities(false, true, true, false)
                .pricing(0.90, 3.75)
                .build(),
            // Highspeed variant — same model, faster output tier.
            ModelInfoBuilder::new(MODEL_KIMI_K2_7_CODE_HIGHSPEED)
                .context(262_144, 16_384)
                .capabilities(false, true, true, false)
                .pricing(0.90, 3.75)
                .build(),
            // ── Multimodal: Kimi K2.6 — 256K context ─────────────────────
            // Vision + text input; supports thinking & non-thinking modes,
            // conversation and agent tasks.
            // Pricing (CNY/1M, cache-miss): ¥6.50 in / ¥27 out ≈ $0.90 / $3.75.
            ModelInfoBuilder::new(MODEL_KIMI_K2_6)
                .context(262_144, 16_384)
                .capabilities(true, true, true, true)
                .thinking_enabled(None)
                .pricing(0.90, 3.75)
                .build(),
        ]
    }
}
