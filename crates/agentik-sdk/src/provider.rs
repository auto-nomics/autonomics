//! Provider type presets.
//!
//! Each submodule exposes `preset_models()` — the catalogue of models a given
//! provider type offers, as metadata-only [`ModelInfo`](crate::model::ModelInfo)
//! entries (capabilities + pricing, `provider_id` left nil). Connection config
//! is *not* baked in here; it lives on a user-configured
//! [`ProviderConfig`](crate::model::ProviderConfig), which a model references
//! via `provider_id`. The server's `provider_registry` joins these presets with
//! a provider instance when the user creates one.

pub mod client;
pub mod deepseek;
pub mod mimo;
pub mod minimax;
pub mod moonshot;
pub mod registry;
pub mod sensenova;
pub mod zai;

use crate::http::auth::AuthMethod;
use crate::model::{ModelInfo, ProviderType};
use crate::wire::WireProtocolKind;

/// Implemented by each built-in provider module to expose its preset model
/// catalogue and default connection endpoint.
///
/// All methods are associated functions (no `&self`) — the trait formalises the
/// interface that every provider struct already follows ad-hoc.
pub trait ProviderPreset {
    /// The [`ProviderType`] variant this preset corresponds to.
    fn provider_type() -> ProviderType;

    /// Canonical preset models. All entries have `provider_id = Uuid::nil()`.
    /// The caller assigns a real `provider_id` when binding to a `ProviderConfig`.
    fn preset_models() -> Vec<ModelInfo>;

    /// Default base URL for this provider type, if one exists.
    /// Returns `""` for providers that require explicit configuration
    /// (e.g. minimax).
    fn default_base_url() -> &'static str;

    /// All known base URL endpoints for this provider type.
    ///
    /// The first entry is always the default (same as
    /// [`default_base_url`](Self::default_base_url)); subsequent entries are
    /// alternative endpoints the user may switch between (e.g. regional
    /// gateways). Providers with a single endpoint inherit the default
    /// implementation, which returns just the default URL.
    fn known_base_urls() -> Vec<&'static str> {
        vec![Self::default_base_url()]
    }

    /// Default authentication method for this provider type.
    /// Most Anthropic-compatible endpoints use [`AuthMethod::Anthropic`]
    /// (`x-api-key` header). Providers whose gateway requires a Bearer token
    /// override this to return [`AuthMethod::Bearer`].
    fn default_auth_method() -> AuthMethod {
        AuthMethod::Anthropic
    }

    /// Which wire protocol this provider's gateway speaks.
    ///
    /// All built-in presets default to [`WireProtocolKind::Anthropic`] since
    /// every current partner (DeepSeek, MiMo, MiniMax, Moonshot, SenseNova,
    /// ZAI) exposes an Anthropic-compatible Messages gateway. Providers that
    /// speak OpenAI Chat Completions or the OpenAI Responses API override this
    /// to select the matching adapter.
    fn wire_protocol() -> WireProtocolKind {
        WireProtocolKind::Anthropic
    }
}
