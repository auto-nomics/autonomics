use crate::model::ModelInfo;
use crate::model::ProviderType;
use crate::model::model_info::ModelInfoBuilder;
use crate::provider::ProviderPreset;

// ─── Model IDs ──────────────────────────────────────────────────────────────
// Latest flagship
pub const MODEL_GLM_5_3: &str = "glm-5.3";
pub const MODEL_GLM_5_3_FLASH: &str = "glm-5.3-flash";
pub const MODEL_GLM_5_2: &str = "glm-5.2";
pub const MODEL_GLM_5_1: &str = "glm-5.1";
pub const MODEL_GLM_5: &str = "glm-5";
pub const MODEL_GLM_5_TURBO: &str = "glm-5-turbo";
// Multimodal coding
pub const MODEL_GLM_5V_TURBO: &str = "glm-5v-turbo";
// 4.x flagship series
pub const MODEL_GLM_4_7: &str = "glm-4.7";
pub const MODEL_GLM_4_6: &str = "glm-4.6";
pub const MODEL_GLM_4_5: &str = "glm-4.5";
pub const MODEL_GLM_4_5_AIR: &str = "glm-4.5-air";
// Flash / free
pub const MODEL_GLM_4_7_FLASHX: &str = "glm-4.7-flashx";
pub const MODEL_GLM_4_7_FLASH: &str = "glm-4.7-flash";
pub const MODEL_GLM_4_FLASH: &str = "glm-4-flash";
// Vision series
pub const MODEL_GLM_4_1V_THINKING_FLASH: &str = "glm-4.1v-thinking-flash";
pub const MODEL_GLM_4_6V_FLASH: &str = "glm-4.6v-flash";
pub const MODEL_GLM_4V_FLASH: &str = "glm-4v-flash";

/// Endpoint selector for the Zhipu / BigModel open platform.
///
/// The general `Api` endpoint exposes the full model catalogue. The
/// `TokenPlan` endpoint is dedicated to the [GLM 编码套餐](https://bigmodel.cn)
/// (coding token-plan) and is only valid for coding scenarios.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ZaiEndpoint {
    /// General Open API — `https://open.bigmodel.cn/api/paas/v4`
    Api,
    /// GLM coding token-plan (Anthropic-compatible endpoint).
    #[default]
    TokenPlan,
}

impl ZaiEndpoint {
    /// The SDK appends `/v1/messages` to the base URL, so these must be the
    /// Anthropic-compatible root (not the OpenAI-compatible `/api/paas/v4`).
    pub fn base_url(self) -> &'static str {
        match self {
            ZaiEndpoint::Api => "https://open.bigmodel.cn/api/anthropic",
            ZaiEndpoint::TokenPlan => "https://open.bigmodel.cn/api/anthropic",
        }
    }
}

pub struct ZaiProvider;

impl ProviderPreset for ZaiProvider {
    fn provider_type() -> ProviderType {
        ProviderType::Zai
    }
    fn preset_models() -> Vec<ModelInfo> {
        Self::model_definitions()
    }
    fn default_base_url() -> &'static str {
        ZaiEndpoint::default().base_url()
    }

    fn wire_protocol() -> crate::wire::WireProtocolKind {
        crate::wire::WireProtocolKind::ZaiAnthropic
    }
}

impl ZaiProvider {
    /// Preset model catalogue for the zai provider type — metadata only.
    pub fn preset_models() -> Vec<ModelInfo> {
        <Self as ProviderPreset>::preset_models()
    }

    fn model_definitions() -> Vec<ModelInfo> {
        vec![
            // ── Latest flagship: GLM-5.3 — 1M context, agent engineering ─
            // Thinking is always enabled; `low`, `high`, and `max` are the
            // valid reasoning-effort levels.
            ModelInfoBuilder::new(MODEL_GLM_5_3)
                .context(1_000_000, 131_072)
                .capabilities(false, true, true, true)
                .thinking_required()
                .pricing(1.40, 4.40)
                .build(),
            // ── Native multimodal coding: GLM-5.3-Flash ────────────────
            ModelInfoBuilder::new(MODEL_GLM_5_3_FLASH)
                .context(1_000_000, 131_072)
                .capabilities(true, true, true, true)
                .thinking_required()
                .pricing(0.14, 0.44)
                .build(),
            // ── Latest flagship: GLM-5.2 — 1M context, long-horizon tasks ─
            // Stable 1M token context, 128K max output.
            // Pricing: $1.40 input / $4.40 output (Z.AI official).
            ModelInfoBuilder::new(MODEL_GLM_5_2)
                .context(1_000_000, 131_072)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(1.40, 4.40)
                .build(),
            // ── Previous flagship: GLM-5.1 — 200K context ────────────────
            // Same pricing as GLM-5.2; 128K max output.
            ModelInfoBuilder::new(MODEL_GLM_5_1)
                .context(200_000, 131_072)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(1.40, 4.40)
                .build(),
            // ── Multimodal coding: GLM-5V-Turbo ──────────────────────────
            // Vision-capable coding model (image input + code output).
            ModelInfoBuilder::new(MODEL_GLM_5V_TURBO)
                .context(200_000, 32_000)
                .capabilities(true, true, true, true)
                .thinking_enabled(None)
                .pricing(1.40, 4.40)
                .build(),
            // ── GLM-5 base — Agentic Engineering foundation ──────────────
            ModelInfoBuilder::new(MODEL_GLM_5)
                .context(200_000, 32_000)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(1.40, 4.40)
                .build(),
            // ── GLM-5-Turbo — fast variant, no thinking mode ─────────────
            ModelInfoBuilder::new(MODEL_GLM_5_TURBO)
                .context(200_000, 32_000)
                .capabilities(false, true, true, false)
                .pricing(0.70, 2.20)
                .build(),
            // ── 4.x flagship series (200K context) ───────────────────────
            // GLM-4.7 — enhanced coding & multi-step reasoning, 128K output
            ModelInfoBuilder::new(MODEL_GLM_4_7)
                .context(200_000, 131_072)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.57, 2.27)
                .build(),
            // GLM-4.6 — advanced coding & complex reasoning, 128K output
            ModelInfoBuilder::new(MODEL_GLM_4_6)
                .context(200_000, 131_072)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.50, 2.00)
                .build(),
            // GLM-4.5 — 355B MoE foundational model, 96K max output
            ModelInfoBuilder::new(MODEL_GLM_4_5)
                .context(128_000, 96_000)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.50, 2.00)
                .build(),
            // ── Air / mid-tier ───────────────────────────────────────────
            ModelInfoBuilder::new(MODEL_GLM_4_5_AIR)
                .context(128_000, 16_000)
                .capabilities(false, true, true, false)
                .pricing(0.15, 0.60)
                .build(),
            // ── Flash / free tier ────────────────────────────────────────
            // GLM-4.7-FlashX — free, 200K context, 128K output
            ModelInfoBuilder::new(MODEL_GLM_4_7_FLASHX)
                .context(200_000, 131_072)
                .capabilities(false, true, true, false)
                .pricing(0.0, 0.0)
                .build(),
            // GLM-4.7-Flash — free, lighter variant
            ModelInfoBuilder::new(MODEL_GLM_4_7_FLASH)
                .context(128_000, 16_000)
                .capabilities(false, true, true, false)
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_GLM_4_FLASH)
                .context(128_000, 16_000)
                .capabilities(false, true, true, false)
                .pricing(0.10, 0.10)
                .build(),
            // ── Vision series ────────────────────────────────────────────
            ModelInfoBuilder::new(MODEL_GLM_4_1V_THINKING_FLASH)
                .context(64_000, 8_000)
                .capabilities(true, true, true, true)
                .thinking_enabled(None)
                .pricing(0.50, 0.50)
                .build(),
            ModelInfoBuilder::new(MODEL_GLM_4_6V_FLASH)
                .context(64_000, 8_000)
                .capabilities(true, true, true, false)
                .pricing(0.50, 0.50)
                .build(),
            ModelInfoBuilder::new(MODEL_GLM_4V_FLASH)
                .context(64_000, 8_000)
                .capabilities(true, true, true, false)
                .pricing(0.10, 0.10)
                .build(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glm_5_3_models_require_thinking() {
        for model in ZaiProvider::preset_models() {
            if model.model_name != MODEL_GLM_5_3 && model.model_name != MODEL_GLM_5_3_FLASH {
                continue;
            }
            assert!(model.supports_thinking);
            assert!(model.thinking_enabled);
            assert!(model.thinking_required);
            assert_eq!(model.context_length, 1_000_000);
            assert_eq!(model.max_output_tokens, 131_072);
            assert_eq!(
                model.vision_ability,
                model.model_name == MODEL_GLM_5_3_FLASH
            );
        }
    }
}
