use crate::model::ModelInfo;
use crate::model::ProviderType;
use crate::model::model_info::ModelInfoBuilder;
use crate::provider::ProviderPreset;

pub const MODEL_MIMO_V2_5_PRO: &str = "mimo-v2.5-pro";
pub const MODEL_MIMO_V2_PRO: &str = "mimo-v2-pro";
pub const MODEL_MIMO_V2_5: &str = "mimo-v2.5";
pub const MODEL_MIMO_V2_OMNI: &str = "mimo-v2-omni";
pub const MODEL_MIMO_V2_FLASH: &str = "mimo-v2-flash";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenPlanRegion {
    China,
    Eur,
    Sgp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MimoEndpoint {
    Api,
    TokenPlan(TokenPlanRegion),
}

impl MimoEndpoint {
    pub fn base_url(self) -> &'static str {
        match self {
            MimoEndpoint::Api => "https://api.xiaomimimo.com/anthropic",
            MimoEndpoint::TokenPlan(TokenPlanRegion::China) => {
                "https://token-plan-cn.xiaomimimo.com/anthropic"
            }
            MimoEndpoint::TokenPlan(TokenPlanRegion::Eur) => {
                "https://token-plan-eur.xiaomimimo.com/anthropic"
            }
            MimoEndpoint::TokenPlan(TokenPlanRegion::Sgp) => {
                "https://token-plan-sgp.xiaomimimo.com/anthropic"
            }
        }
    }
}

impl Default for MimoEndpoint {
    fn default() -> Self {
        MimoEndpoint::TokenPlan(TokenPlanRegion::China)
    }
}

pub struct MimoProvider;

impl ProviderPreset for MimoProvider {
    fn provider_type() -> ProviderType {
        ProviderType::Mimo
    }
    fn preset_models() -> Vec<ModelInfo> {
        Self::model_definitions()
    }
    fn default_base_url() -> &'static str {
        // Default to the China token-plan endpoint.
        MimoEndpoint::default().base_url()
    }
    fn known_base_urls() -> Vec<&'static str> {
        // Token-plan China is the default; offer all four regional endpoints
        // plus the general API gateway.
        vec![
            MimoEndpoint::TokenPlan(TokenPlanRegion::China).base_url(),
            MimoEndpoint::Api.base_url(),
            MimoEndpoint::TokenPlan(TokenPlanRegion::Eur).base_url(),
            MimoEndpoint::TokenPlan(TokenPlanRegion::Sgp).base_url(),
        ]
    }
}

impl MimoProvider {
    /// Preset model catalogue for the mimo provider type — metadata only.
    pub fn preset_models() -> Vec<ModelInfo> {
        <Self as ProviderPreset>::preset_models()
    }

    fn model_definitions() -> Vec<ModelInfo> {
        vec![
            // Pro series — 1M context, 128K output
            ModelInfoBuilder::new(MODEL_MIMO_V2_5_PRO)
                .context(1_000_000, 131_072)
                .capabilities(false, true, true, true)
                .pricing(1.0, 3.0)
                .build(),
            ModelInfoBuilder::new(MODEL_MIMO_V2_PRO)
                .context(1_000_000, 131_072)
                .capabilities(false, true, true, true)
                .pricing(1.0, 3.0)
                .build(),
            // Omni series — multi-modal understanding
            ModelInfoBuilder::new(MODEL_MIMO_V2_5)
                .context(1_000_000, 131_072)
                .capabilities(true, true, true, true)
                .pricing(0.4, 2.0)
                .build(),
            ModelInfoBuilder::new(MODEL_MIMO_V2_OMNI)
                .context(262_144, 131_072)
                .capabilities(true, true, true, true)
                .pricing(0.4, 2.0)
                .build(),
            // Flash series — lightweight, fast
            ModelInfoBuilder::new(MODEL_MIMO_V2_FLASH)
                .context(262_144, 65_536)
                .capabilities(false, true, true, true)
                .pricing(0.1, 0.3)
                .build(),
        ]
    }
}
