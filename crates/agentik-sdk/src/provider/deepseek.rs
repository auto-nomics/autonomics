use crate::model::ModelInfo;
use crate::model::ProviderType;
use crate::model::model_info::ModelInfoBuilder;
use crate::provider::ProviderPreset;

// ─── Model IDs ──────────────────────────────────────────────────────────────
// Current generation
pub const MODEL_DEEPSEEK_V4_PRO: &str = "deepseek-v4-pro";
pub const MODEL_DEEPSEEK_V4_FLASH: &str = "deepseek-v4-flash";
// Deprecated aliases — still accepted, mapped server-side to v4-flash.
pub const MODEL_DEEPSEEK_CHAT: &str = "deepseek-chat";
pub const MODEL_DEEPSEEK_REASONER: &str = "deepseek-reasoner";

pub const DEFAULT_BASE_URL: &str = "https://api.deepseek.com/anthropic";

pub struct DeepseekProvider;

impl ProviderPreset for DeepseekProvider {
    fn provider_type() -> ProviderType {
        ProviderType::Deepseek
    }
    fn preset_models() -> Vec<ModelInfo> {
        Self::model_definitions()
    }
    fn default_base_url() -> &'static str {
        DEFAULT_BASE_URL
    }
}

impl DeepseekProvider {
    /// Preset model catalogue for the deepseek provider type — metadata only.
    ///
    /// `provider_id` is left nil; the caller binds a model to a provider
    /// instance when persisting it.
    pub fn preset_models() -> Vec<ModelInfo> {
        <Self as ProviderPreset>::preset_models()
    }

    fn model_definitions() -> Vec<ModelInfo> {
        vec![
            // ── Current generation ───────────────────────────────────────
            // V4 Pro — flagship reasoning model.
            // Peak, cache-miss pricing (USD/1M): $1.32 input / $3.96 output.
            ModelInfoBuilder::new(MODEL_DEEPSEEK_V4_PRO)
                .context(1_000_000, 384_000)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(1.32, 3.96)
                .build(),
            // V4 Flash — fast, low-cost, still supports thinking.
            ModelInfoBuilder::new(MODEL_DEEPSEEK_V4_FLASH)
                .context(1_000_000, 32_000)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.1, 0.3)
                .build(),
            // ── Deprecated aliases (kept for backwards compatibility) ─────
            // deepseek-chat — non-thinking mode of v4-flash.
            ModelInfoBuilder::new(MODEL_DEEPSEEK_CHAT)
                .context(64_000, 8_000)
                .capabilities(false, true, true, false)
                .pricing(0.1, 0.3)
                .build(),
            // deepseek-reasoner — thinking mode of v4-flash.
            ModelInfoBuilder::new(MODEL_DEEPSEEK_REASONER)
                .context(64_000, 8_000)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.1, 0.3)
                .build(),
        ]
    }
}
