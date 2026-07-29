use crate::model::ModelInfo;
use crate::model::ProviderType;
use crate::model::model_info::ModelInfoBuilder;
use crate::provider::ProviderPreset;

// ── Model IDs ──────────────────────────────────────────────────────────────

// Latest flagship
pub const MODEL_MINIMAX_M3: &str = "MiniMax-M3";
// Current flagship
pub const MODEL_MINIMAX_M2_7: &str = "MiniMax-M2.7";
pub const MODEL_MINIMAX_M2_7_HIGHSPEED: &str = "MiniMax-M2.7-highspeed";
// Performance series
pub const MODEL_MINIMAX_M2_5: &str = "MiniMax-M2.5";
pub const MODEL_MINIMAX_M2_5_HIGHSPEED: &str = "MiniMax-M2.5-highspeed";
// Coding series
pub const MODEL_MINIMAX_M2_1: &str = "MiniMax-M2.1";
pub const MODEL_MINIMAX_M2_1_HIGHSPEED: &str = "MiniMax-M2.1-highspeed";
// Budget agent series
pub const MODEL_MINIMAX_M2: &str = "MiniMax-M2";

/// MiniMax Anthropic-compatible API endpoint.
pub const DEFAULT_BASE_URL: &str = "https://api.minimaxi.com/anthropic";

pub struct MinimaxProvider;

impl ProviderPreset for MinimaxProvider {
    fn provider_type() -> ProviderType {
        ProviderType::Minimax
    }
    fn preset_models() -> Vec<ModelInfo> {
        Self::model_definitions()
    }
    fn default_base_url() -> &'static str {
        DEFAULT_BASE_URL
    }
}

impl MinimaxProvider {
    /// Preset model catalogue for the minimax provider type — metadata only.
    pub fn preset_models() -> Vec<ModelInfo> {
        <Self as ProviderPreset>::preset_models()
    }

    fn model_definitions() -> Vec<ModelInfo> {
        vec![
            // ── Latest flagship: M3 — multimodal (image+video), 1M context ─
            // Reasoning, tools, structured output. 512K max output.
            ModelInfoBuilder::new(MODEL_MINIMAX_M3)
                .context(1_000_000, 524_288)
                .capabilities(true, true, true, true)
                .pricing(0.30, 1.20)
                .build(),
            // ── Flagship: M2.7 — multimodal, 1M context ──────────────────
            ModelInfoBuilder::new(MODEL_MINIMAX_M2_7)
                .context(1_000_000, 131_072)
                .capabilities(true, true, true, true)
                .pricing(1.0, 4.0)
                .build(),
            // Highspeed variant: same multimodal capabilities, faster output.
            ModelInfoBuilder::new(MODEL_MINIMAX_M2_7_HIGHSPEED)
                .context(1_000_000, 131_072)
                .capabilities(true, true, true, true)
                .pricing(1.0, 8.0)
                .build(),
            // ── Performance: M2.5 — SOTA coding & agentic, 205K context ───
            // Standard variant: ~60 TPS output.
            ModelInfoBuilder::new(MODEL_MINIMAX_M2_5)
                .context(204_800, 131_072)
                .capabilities(false, true, true, true)
                .pricing(0.30, 1.20)
                .build(),
            // Highspeed variant: same quality, ~100 TPS output, higher
            // output price reflects the faster inference tier.
            ModelInfoBuilder::new(MODEL_MINIMAX_M2_5_HIGHSPEED)
                .context(204_800, 131_072)
                .capabilities(false, true, true, true)
                .pricing(0.30, 2.40)
                .build(),
            // ── Coding: M2.1 — strong multilingual programming ───────────
            ModelInfoBuilder::new(MODEL_MINIMAX_M2_1)
                .context(204_800, 131_072)
                .capabilities(false, true, true, true)
                .pricing(0.30, 1.20)
                .build(),
            ModelInfoBuilder::new(MODEL_MINIMAX_M2_1_HIGHSPEED)
                .context(204_800, 131_072)
                .capabilities(false, true, true, true)
                .pricing(0.30, 2.40)
                .build(),
            // ── Budget: M2 — efficient coding & agent workflows ──────────
            ModelInfoBuilder::new(MODEL_MINIMAX_M2)
                .context(204_800, 131_072)
                .capabilities(false, true, true, false)
                .pricing(0.10, 0.40)
                .build(),
        ]
    }
}
