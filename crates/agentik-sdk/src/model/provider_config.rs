use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::http::auth::AuthMethod;

/// Known provider type — used to resolve model presets and protocol adapters.
///
/// Serde round-trips via lowercase strings (e.g. `"deepseek"`, `"mimo"`).
/// Unknown strings deserialize to [`ProviderType::Custom`] so the database never
/// rejects a value — even a future one.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderType {
    Deepseek,
    Mimo,
    Minimax,
    Moonshot,
    Openrouter,
    Zai,
    Sensenova,
    /// A user-defined or externally-registered provider type.
    Custom(String),
}

impl ProviderType {
    /// The canonical lowercase name (e.g. `"deepseek"`, `"mimo"`).
    /// For `Custom(s)` returns `s` unchanged.
    pub fn as_str(&self) -> &str {
        match self {
            ProviderType::Deepseek => "deepseek",
            ProviderType::Mimo => "mimo",
            ProviderType::Minimax => "minimax",
            ProviderType::Moonshot => "moonshot",
            ProviderType::Openrouter => "openrouter",
            ProviderType::Zai => "zai",
            ProviderType::Sensenova => "sensenova",
            ProviderType::Custom(s) => s.as_str(),
        }
    }
}

impl std::fmt::Display for ProviderType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<&str> for ProviderType {
    fn from(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "deepseek" => ProviderType::Deepseek,
            "mimo" => ProviderType::Mimo,
            "minimax" => ProviderType::Minimax,
            "moonshot" => ProviderType::Moonshot,
            "openrouter" => ProviderType::Openrouter,
            "zai" => ProviderType::Zai,
            "sensenova" => ProviderType::Sensenova,
            custom => ProviderType::Custom(custom.to_string()),
        }
    }
}

impl From<String> for ProviderType {
    fn from(s: String) -> Self {
        ProviderType::from(s.as_str())
    }
}

/// A user-configured provider **instance** — the "master" side of the
/// provider→model relationship.
///
/// One `ProviderConfig` corresponds to a single endpoint: a provider *type*
/// (e.g. `deepseek`, `mimo`) combined with a concrete `base_url`, credentials,
/// and auth scheme. A provider type that exposes multiple endpoints (e.g. mimo
/// with regional token-plan endpoints) is represented as several distinct
/// `ProviderConfig` rows.
///
/// Models reference a provider via [`ProviderConfig::id`] (`ModelInfo::provider_id`),
/// so credentials and base URL live in exactly one place and can be rotated by
/// editing a single row.
#[derive(Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Stable primary key. Referenced by `ModelInfo::provider_id`.
    pub id: Uuid,
    /// Human-readable label, e.g. `"deepseek-prod"` or `"mimo-cn"`.
    pub name: String,
    /// Provider type key — matches a known preset (`deepseek`, `mimo`, `zai`, …).
    /// Used to resolve model presets and protocol adapters.
    pub provider_type: ProviderType,
    pub base_url: String,
    pub api_key: String,
    pub auth_method: AuthMethod,
}

impl ProviderConfig {
    /// Create a new instance with a freshly generated id.
    pub fn new(
        name: impl Into<String>,
        provider_type: impl Into<ProviderType>,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        auth_method: AuthMethod,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            provider_type: provider_type.into(),
            base_url: base_url.into(),
            api_key: api_key.into(),
            auth_method,
        }
    }
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            id: Uuid::nil(),
            name: String::new(),
            provider_type: ProviderType::Custom(String::new()),
            base_url: String::new(),
            api_key: String::new(),
            auth_method: AuthMethod::default(),
        }
    }
}

impl std::fmt::Debug for ProviderConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderConfig")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("provider_type", &self.provider_type)
            .field("base_url", &self.base_url)
            .field("api_key", &"[REDACTED]")
            .field("auth_method", &self.auth_method)
            .finish()
    }
}

impl PartialEq for ProviderConfig {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}
