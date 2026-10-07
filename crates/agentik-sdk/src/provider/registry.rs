//! Static registry that maps [`ProviderType`] → preset models and default URLs.
//!
//! All lookup is compile-time dispatch via `match` — no runtime registration.

use crate::http::auth::AuthMethod;
use crate::model::{ModelInfo, ProviderType};
use crate::provider::{
    ProviderPreset, bailian::BailianProvider, deepseek::DeepseekProvider, mimo::MimoProvider,
    minimax::MinimaxProvider, moonshot::MoonshotProvider, openai::OpenaiProvider,
    openrouter::OpenrouterProvider, sensenova::SensenovaProvider, stepfun::StepfunProvider,
    zai::ZaiProvider,
};
use crate::wire::WireProtocolKind;

/// Returns preset models for a known [`ProviderType`], or `None` for
/// [`ProviderType::Custom(_)`](ProviderType::Custom) (which has no baked-in
/// catalogue).
pub fn preset_models(provider_type: &ProviderType) -> Option<Vec<ModelInfo>> {
    match provider_type {
        ProviderType::Bailian => Some(BailianProvider::preset_models()),
        ProviderType::Deepseek => Some(DeepseekProvider::preset_models()),
        ProviderType::Mimo => Some(MimoProvider::preset_models()),
        ProviderType::Minimax => Some(MinimaxProvider::preset_models()),
        ProviderType::Moonshot => Some(MoonshotProvider::preset_models()),
        ProviderType::Openai => Some(OpenaiProvider::preset_models()),
        ProviderType::Openrouter => Some(OpenrouterProvider::preset_models()),
        ProviderType::Sensenova => Some(SensenovaProvider::preset_models()),
        ProviderType::Stepfun => Some(StepfunProvider::preset_models()),
        ProviderType::Zai => Some(ZaiProvider::preset_models()),
        ProviderType::Custom(_) => None,
    }
}

/// Returns the default base URL for a known provider type, or `None` for
/// [`ProviderType::Custom(_)`](ProviderType::Custom).
pub fn default_base_url(provider_type: &ProviderType) -> Option<&'static str> {
    match provider_type {
        ProviderType::Bailian => Some(BailianProvider::default_base_url()),
        ProviderType::Deepseek => Some(DeepseekProvider::default_base_url()),
        ProviderType::Mimo => Some(MimoProvider::default_base_url()),
        ProviderType::Minimax => Some(MinimaxProvider::default_base_url()),
        ProviderType::Moonshot => Some(MoonshotProvider::default_base_url()),
        ProviderType::Openai => Some(OpenaiProvider::default_base_url()),
        ProviderType::Openrouter => Some(OpenrouterProvider::default_base_url()),
        ProviderType::Sensenova => Some(SensenovaProvider::default_base_url()),
        ProviderType::Stepfun => Some(StepfunProvider::default_base_url()),
        ProviderType::Zai => Some(ZaiProvider::default_base_url()),
        ProviderType::Custom(_) => None,
    }
}

/// Returns all known base URLs for a known provider type (default first).
/// Returns an empty vec for [`ProviderType::Custom(_)`](ProviderType::Custom).
pub fn known_base_urls(provider_type: &ProviderType) -> Vec<&'static str> {
    match provider_type {
        ProviderType::Bailian => BailianProvider::known_base_urls(),
        ProviderType::Deepseek => DeepseekProvider::known_base_urls(),
        ProviderType::Mimo => MimoProvider::known_base_urls(),
        ProviderType::Minimax => MinimaxProvider::known_base_urls(),
        ProviderType::Moonshot => MoonshotProvider::known_base_urls(),
        ProviderType::Openai => OpenaiProvider::known_base_urls(),
        ProviderType::Openrouter => OpenrouterProvider::known_base_urls(),
        ProviderType::Sensenova => SensenovaProvider::known_base_urls(),
        ProviderType::Stepfun => StepfunProvider::known_base_urls(),
        ProviderType::Zai => ZaiProvider::known_base_urls(),
        ProviderType::Custom(_) => Vec::new(),
    }
}

/// Returns the default authentication method for a known provider type.
/// Falls back to [`AuthMethod::Anthropic`] for custom providers.
pub fn default_auth_method(provider_type: &ProviderType) -> AuthMethod {
    match provider_type {
        ProviderType::Bailian => BailianProvider::default_auth_method(),
        ProviderType::Deepseek => DeepseekProvider::default_auth_method(),
        ProviderType::Mimo => MimoProvider::default_auth_method(),
        ProviderType::Minimax => MinimaxProvider::default_auth_method(),
        ProviderType::Moonshot => MoonshotProvider::default_auth_method(),
        ProviderType::Openai => OpenaiProvider::default_auth_method(),
        ProviderType::Openrouter => OpenrouterProvider::default_auth_method(),
        ProviderType::Sensenova => SensenovaProvider::default_auth_method(),
        ProviderType::Stepfun => StepfunProvider::default_auth_method(),
        ProviderType::Zai => ZaiProvider::default_auth_method(),
        ProviderType::Custom(_) => AuthMethod::Anthropic,
    }
}

/// Returns the [`WireProtocolKind`] advertised by a known provider type.
///
/// Most built-in presets speak an Anthropic-compatible wire; the OpenAI-native
/// presets (openai via the ChatGPT backend, openrouter via the
/// OpenAI-compatible gateway) declare [`WireProtocolKind::ChatgptResponses`]
/// and [`WireProtocolKind::OpenaiChat`] respectively. Custom providers fall
/// back to the Anthropic-compatible wire.
pub fn wire_protocol(provider_type: &ProviderType) -> WireProtocolKind {
    match provider_type {
        ProviderType::Bailian => BailianProvider::wire_protocol(),
        ProviderType::Deepseek => DeepseekProvider::wire_protocol(),
        ProviderType::Mimo => MimoProvider::wire_protocol(),
        ProviderType::Minimax => MinimaxProvider::wire_protocol(),
        ProviderType::Moonshot => MoonshotProvider::wire_protocol(),
        ProviderType::Openai => OpenaiProvider::wire_protocol(),
        ProviderType::Openrouter => OpenrouterProvider::wire_protocol(),
        ProviderType::Sensenova => SensenovaProvider::wire_protocol(),
        ProviderType::Stepfun => StepfunProvider::wire_protocol(),
        ProviderType::Zai => ZaiProvider::wire_protocol(),
        ProviderType::Custom(_) => WireProtocolKind::Anthropic,
    }
}

/// Whether a known provider type exposes a pollable remote model catalogue.
/// Custom providers are assumed not to.
#[must_use]
pub fn supports_remote_catalog(provider_type: &ProviderType) -> bool {
    match provider_type {
        ProviderType::Openrouter => OpenrouterProvider::supports_remote_catalog(),
        ProviderType::Openai => OpenaiProvider::supports_remote_catalog(),
        _ => false,
    }
}

/// Lists all built-in provider type names that have presets.
pub fn known_provider_types() -> Vec<&'static str> {
    vec![
        "bailian",
        "deepseek",
        "mimo",
        "minimax",
        "moonshot",
        "openai",
        "openrouter",
        "sensenova",
        "stepfun",
        "zai",
    ]
}