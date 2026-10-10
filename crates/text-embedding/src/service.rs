use crate::error::Result;
use std::{env, os};

use crate::OnnxTextEmbedder;

/// Use local embedding model to provide text embedding service
pub struct LocalEmbeddingService {
    provider: Option<OnnxTextEmbedder>,
}

impl LocalEmbeddingService {
    pub async fn init() -> Result<()> {
        let model_path = env::home_dir()
            .ok_or("fail to get home dir")?
            .join("model")
            .join("embedding_model");
        todo!()
    }

    pub async fn install_default_model() -> Result<()> {
        todo!()
    }
}

impl Default for LocalEmbeddingService {
    fn default() -> Self {
        Self {
            provider: Default::default(),
        }
    }
}
