//! Ollama provider preset and live model discovery.

use crate::http::auth::AuthMethod;
use crate::model::ModelInfo;
use crate::model::ProviderType;
use crate::model::model_info::ModelInfoBuilder;
use crate::provider::ProviderPreset;
use crate::wire::WireProtocolKind;
use agentik_types::ReasoningEffort;

pub const DEFAULT_BASE_URL: &str = "http://localhost:11434";

pub struct OllamaProvider;

impl ProviderPreset for OllamaProvider {
    fn provider_type() -> ProviderType {
        ProviderType::Ollama
    }

    fn preset_models() -> Vec<ModelInfo> {
        Vec::new()
    }

    fn default_base_url() -> &'static str {
        DEFAULT_BASE_URL
    }

    fn default_auth_method() -> AuthMethod {
        AuthMethod::None
    }

    fn wire_protocol() -> WireProtocolKind {
        WireProtocolKind::OllamaChat
    }

    fn supports_remote_catalog() -> bool {
        true
    }
}

impl OllamaProvider {
    /// Fetch installed models from Ollama's `GET /api/tags` endpoint.
    ///
    /// # Errors
    ///
    /// Returns a display-ready message when Ollama is unreachable, returns a
    /// non-success status, or sends malformed JSON.
    pub async fn fetch_remote_catalog_with_auth(
        base_url: &str,
        api_key: &str,
    ) -> std::result::Result<Vec<ModelInfo>, String> {
        let url = format!("{}/api/tags", base_url.trim_end_matches('/'));
        let mut request = reqwest::Client::new().get(url);
        if !api_key.is_empty() {
            request = request.bearer_auth(api_key);
        }
        let response = request
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| format!("Ollama catalogue request failed: {e}"))?;
        if !response.status().is_success() {
            return Err(format!(
                "Ollama catalogue endpoint returned {}",
                response.status()
            ));
        }
        let catalogue = response
            .json::<RemoteCatalogue>()
            .await
            .map_err(|e| format!("Ollama catalogue decode failed: {e}"))?;
        Ok(catalogue
            .models
            .into_iter()
            .map(|model| map_remote_model(&model))
            .collect())
    }

    /// Fetch the local Ollama catalogue without credentials.
    ///
    /// # Errors
    /// See [`Self::fetch_remote_catalog_with_auth`].
    pub async fn fetch_remote_catalog(
        base_url: &str,
    ) -> std::result::Result<Vec<ModelInfo>, String> {
        Self::fetch_remote_catalog_with_auth(base_url, "").await
    }
}

#[derive(serde::Deserialize)]
struct RemoteCatalogue {
    models: Vec<RemoteModel>,
}

#[derive(serde::Deserialize)]
struct RemoteModel {
    name: String,
    #[serde(default)]
    details: Option<RemoteDetails>,
}

#[derive(serde::Deserialize)]
struct RemoteDetails {
    #[serde(default)]
    families: Vec<String>,
    #[serde(default)]
    family: Option<String>,
}

fn map_remote_model(model: &RemoteModel) -> ModelInfo {
    let families = model
        .details
        .as_ref()
        .map(|details| {
            details
                .family
                .as_deref()
                .map(str::to_string)
                .into_iter()
                .chain(details.families.iter().cloned())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default()
        .to_lowercase();
    let vision = ["llava", "minicpm-v", "moondream", "multimodal"]
        .iter()
        .any(|family| families.contains(family));
    let thinking = ["qwen3", "deepseek-r1", "gpt-oss", "granite3"]
        .iter()
        .any(|family| families.contains(family));
    let mut builder = ModelInfoBuilder::new(&model.name)
        .context(8_192, 4_096)
        .capabilities(vision, true, true, thinking)
        .pricing(0.0, 0.0);
    if thinking {
        builder = builder
            .thinking_enabled(None)
            .max_reasoning_effort(ReasoningEffort::Medium);
    }
    builder.build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ollama_defaults_are_local_and_unauthenticated() {
        assert_eq!(OllamaProvider::default_base_url(), DEFAULT_BASE_URL);
        assert_eq!(OllamaProvider::default_auth_method(), AuthMethod::None);
        assert_eq!(
            OllamaProvider::wire_protocol(),
            WireProtocolKind::OllamaChat
        );
    }

    #[test]
    fn remote_tags_map_capabilities() {
        let model = RemoteModel {
            name: "qwen3:4b".into(),
            details: Some(RemoteDetails {
                families: vec!["qwen3".into()],
                family: None,
            }),
        };
        let mapped = map_remote_model(&model);
        assert_eq!(mapped.model_name, "qwen3:4b");
        assert!(mapped.supports_function_calling);
        assert!(mapped.supports_streaming);
        assert!(mapped.supports_thinking);
        assert!(!mapped.vision_ability);
    }
}
