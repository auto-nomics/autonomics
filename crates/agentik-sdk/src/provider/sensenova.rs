use crate::model::ModelInfo;
use crate::model::ProviderType;
use crate::model::model_info::ModelInfoBuilder;
use crate::provider::ProviderPreset;

pub const MODEL_SENSENOVA_6_7_FLASH_LITE: &str = "sensenova-6.7-flash-lite";
pub const MODEL_DEEPSEEK_V4_FLASH: &str = "deepseek-v4-flash";

pub const DEFAULT_BASE_URL: &str = "https://token.sensenova.cn";

pub struct SensenovaProvider;

impl ProviderPreset for SensenovaProvider {
    fn provider_type() -> ProviderType {
        ProviderType::Sensenova
    }
    fn preset_models() -> Vec<ModelInfo> {
        Self::model_definitions()
    }
    fn default_base_url() -> &'static str {
        DEFAULT_BASE_URL
    }
}

impl SensenovaProvider {
    /// Preset model catalogue for the sensenova provider type — metadata only.
    pub fn preset_models() -> Vec<ModelInfo> {
        <Self as ProviderPreset>::preset_models()
    }

    fn model_definitions() -> Vec<ModelInfo> {
        vec![
            // SenseNova 6.7 Flash-Lite — multimodal, 256K context, 64K output
            ModelInfoBuilder::new(MODEL_SENSENOVA_6_7_FLASH_LITE)
                .context(262_144, 65_536)
                .capabilities(true, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            // DeepSeek V4 Flash — 256K context, 64K output, thinking mode
            ModelInfoBuilder::new(MODEL_DEEPSEEK_V4_FLASH)
                .context(262_144, 65_536)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
        ]
    }
}
