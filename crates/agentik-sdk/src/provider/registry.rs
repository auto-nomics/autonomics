//! Static registry that maps [`ProviderType`] → preset models and default URLs.
//!
//! All lookup is compile-time dispatch via `match` — no runtime registration.

use crate::http::auth::AuthMethod;
use crate::model::{ModelInfo, ProviderType};
use crate::provider::{
    ProviderPreset, deepseek::DeepseekProvider, mimo::MimoProvider, minimax::MinimaxProvider,
    moonshot::MoonshotProvider, sensenova::SensenovaProvider, zai::ZaiProvider,
};

/// Returns preset models for a known [`ProviderType`], or `None` for
/// [`ProviderType::Custom(_)`](ProviderType::Custom) (which has no baked-in
/// catalogue).
pub fn preset_models(provider_type: &ProviderType) -> Option<Vec<ModelInfo>> {
    match provider_type {
        ProviderType::Deepseek => Some(DeepseekProvider::preset_models()),
        ProviderType::Mimo => Some(MimoProvider::preset_models()),
        ProviderType::Minimax => Some(MinimaxProvider::preset_models()),
        ProviderType::Moonshot => Some(MoonshotProvider::preset_models()),
        ProviderType::Sensenova => Some(SensenovaProvider::preset_models()),
        ProviderType::Zai => Some(ZaiProvider::preset_models()),
        ProviderType::Custom(_) => None,
    }
}

/// Returns the default base URL for a known provider type, or `None` for
/// [`ProviderType::Custom(_)`](ProviderType::Custom).
pub fn default_base_url(provider_type: &ProviderType) -> Option<&'static str> {
    match provider_type {
        ProviderType::Deepseek => Some(DeepseekProvider::default_base_url()),
        ProviderType::Mimo => Some(MimoProvider::default_base_url()),
        ProviderType::Minimax => Some(MinimaxProvider::default_base_url()),
        ProviderType::Moonshot => Some(MoonshotProvider::default_base_url()),
        ProviderType::Sensenova => Some(SensenovaProvider::default_base_url()),
        ProviderType::Zai => Some(ZaiProvider::default_base_url()),
        ProviderType::Custom(_) => None,
    }
}

/// Returns the default authentication method for a known provider type.
/// Falls back to [`AuthMethod::Anthropic`] for custom providers.
pub fn default_auth_method(provider_type: &ProviderType) -> AuthMethod {
    match provider_type {
        ProviderType::Deepseek => DeepseekProvider::default_auth_method(),
        ProviderType::Mimo => MimoProvider::default_auth_method(),
        ProviderType::Minimax => MinimaxProvider::default_auth_method(),
        ProviderType::Moonshot => MoonshotProvider::default_auth_method(),
        ProviderType::Sensenova => SensenovaProvider::default_auth_method(),
        ProviderType::Zai => ZaiProvider::default_auth_method(),
        ProviderType::Custom(_) => AuthMethod::Anthropic,
    }
}

/// Lists all built-in provider type names that have presets.
pub fn known_provider_types() -> Vec<&'static str> {
    vec![
        "deepseek",
        "mimo",
        "minimax",
        "moonshot",
        "sensenova",
        "zai",
    ]
}
