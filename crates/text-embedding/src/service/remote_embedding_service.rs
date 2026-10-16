//! Remote Ollama embedding service.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::OllamaEmbedder;
use crate::provider::TextEmbedder;
use crate::service::error::Result;

pub const DEFAULT_BASE_URL: &str = "http://127.0.0.1:11434";
pub const DEFAULT_MODEL: &str = "qwen3-embedding:0.6b";
pub const DEFAULT_BATCH_SIZE: usize = 32;
pub const DEFAULT_TIMEOUT_SECS: u64 = 30;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_base_url")]
    pub base_url: String,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            base_url: default_base_url(),
            model: default_model(),
            batch_size: default_batch_size(),
            timeout_secs: default_timeout_secs(),
        }
    }
}

fn default_true() -> bool {
    true
}
fn default_base_url() -> String {
    DEFAULT_BASE_URL.to_string()
}
fn default_model() -> String {
    DEFAULT_MODEL.to_string()
}
fn default_batch_size() -> usize {
    DEFAULT_BATCH_SIZE
}
fn default_timeout_secs() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

#[derive(Default)]
pub struct RemoteEmbeddingService {
    provider: Option<Arc<dyn TextEmbedder>>,
}

impl RemoteEmbeddingService {
    pub async fn init(config: &EmbeddingConfig) -> Result<Self> {
        if !config.enabled {
            return Ok(Self::default());
        }
        let provider = Arc::new(OllamaEmbedder::with_options(
            &config.base_url,
            &config.model,
            config.batch_size,
            std::time::Duration::from_secs(config.timeout_secs),
        ));
        Ok(Self {
            provider: Some(provider),
        })
    }

    pub fn text_embedder(&self) -> Option<Arc<dyn TextEmbedder>> {
        self.provider.clone()
    }
}
