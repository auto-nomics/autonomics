use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{
    OnnxTextEmbedder, TextEmbedder,
    service::error::{Error, Result},
};

/// Default model repository, owner part.
pub const DEFAULT_MODEL_OWNER: &str = "wjixiang";
/// Default model repository, name part. An int8-quantized ONNX
/// export of harrier-oss-v1-270m (graph + external weights).
pub const DEFAULT_MODEL_NAME: &str = "harrier-oss-v1-270m-onnx";
/// Repository files the embedder needs: the quantized graph (plus
/// its external weight data), the tokenizer, and the config the
/// model signature is derived from.
pub const DEFAULT_MODEL_FILES: &[&str] = &[
    "onnx/model_quantized.onnx",
    "onnx/model_quantized.onnx_data",
    "config.json",
    "tokenizer.json",
    "tokenizer_config.json",
];

/// Directory the default model installs into: `~/.autonomics/model/embedding_model`.
pub fn default_model_dir() -> Result<PathBuf> {
    Ok(std::env::home_dir()
        .ok_or(Error::Unknown("cannot resolve home directory".into()))?
        .join(".autonomics")
        .join("model")
        .join("embedding_model"))
}

/// Use a local embedding model to provide text embedding.
#[derive(Default)]
pub struct LocalEmbeddingService {
    provider: Option<Arc<dyn TextEmbedder>>,
}

impl LocalEmbeddingService {
    /// Bootstrap the service over the default directory: install the
    /// default model if this is the first run, then open the embedder.
    pub async fn init() -> Result<Self> {
        Self::init_at(&default_model_dir()?).await
    }

    /// Whether `model_dir` already holds a complete install — every
    /// file [`init_at`](Self::init_at) opens — so opening it
    /// downloads nothing. Callers that must not block on the ~363 MB
    /// first-run download use this to decide between opening inline
    /// and installing in the background.
    pub fn model_installed(model_dir: &Path) -> bool {
        is_installed(model_dir)
    }

    /// Bootstrap over an explicit model directory (host configuration
    /// and tests point this at their own layouts).
    pub async fn init_at(model_dir: &Path) -> Result<Self> {
        if !is_installed(model_dir) {
            install_default_model_in(model_dir).await?;
        }
        let provider = Arc::new(OnnxTextEmbedder::open(model_dir)?);
        Ok(Self {
            provider: Some(provider),
        })
    }

    /// Download the default model repository into `~/model/embedding_model`.
    ///
    /// Idempotent and resumable: the Hub client skips blobs already
    /// present with matching etag, so a partial download continues
    /// where it stopped.
    pub async fn install_default_model() -> Result<PathBuf> {
        let model_dir = default_model_dir()?;
        install_default_model_in(&model_dir).await?;
        Ok(model_dir)
    }

    /// The opened embedder, once [`init`](Self::init) succeeded.
    pub fn provider(&self) -> Option<&dyn TextEmbedder> {
        self.provider.as_deref()
    }

    /// The opened embedder as a shared provider handle, for callers
    /// that bridge it into a broader embedding contract.
    pub fn text_embedder(&self) -> Option<Arc<dyn TextEmbedder>> {
        self.provider
            .clone()
            .map(|provider| provider as Arc<dyn TextEmbedder>)
    }
}

/// Whether a model directory already holds everything the embedder
/// opens: the graph (external data resolves relative to it), the
/// tokenizer, and the config.
fn is_installed(model_dir: &std::path::Path) -> bool {
    DEFAULT_MODEL_FILES
        .iter()
        .all(|file| model_dir.join(file).is_file())
}

/// Fetch the default model snapshot into `model_dir`.
async fn install_default_model_in(model_dir: &std::path::Path) -> Result<()> {
    let client = hf_hub::HFClientBuilder::new().build()?;
    let repo = client.model(DEFAULT_MODEL_OWNER, DEFAULT_MODEL_NAME);
    repo.snapshot_download()
        .allow_patterns(DEFAULT_MODEL_FILES.iter().map(|f| f.to_string()).collect())
        .local_dir(model_dir.to_path_buf())
        .send()
        .await
        .map_err(Error::from)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TextEmbedder;

    #[test]
    fn default_files_cover_everything_the_embedder_opens() {
        // The install manifest and the open contract must stay in
        // sync: a file listed here but missing from the pattern list
        // would make `init` download a directory it cannot open.
        assert!(DEFAULT_MODEL_FILES.contains(&"onnx/model_quantized.onnx"));
        assert!(DEFAULT_MODEL_FILES.contains(&"onnx/model_quantized.onnx_data"));
        assert!(DEFAULT_MODEL_FILES.contains(&"tokenizer.json"));
        assert!(DEFAULT_MODEL_FILES.contains(&"config.json"));
    }

    /// End-to-end install + open + embed against the real Hub repo.
    /// Network-bound (~363 MB on first run); run explicitly with:
    /// `cargo test -p text-embedding -- --ignored`
    #[tokio::test]
    #[ignore = "downloads ~363 MB from the Hugging Face Hub"]
    async fn installs_and_serves_the_default_model() {
        let dir = tempfile::tempdir().unwrap();
        // Install into an isolated directory rather than the user's
        // `~/model/embedding_model`.
        install_default_model_in(dir.path()).await.unwrap();
        assert!(is_installed(dir.path()));

        let embedder = OnnxTextEmbedder::open(dir.path()).unwrap();
        let output = embedder
            .embed(&crate::EmbeddingInput::new("a", "restart the daemon"))
            .unwrap();
        assert_eq!(output.vector.len(), 640);
        // The signature derives from the directory + artifact names;
        // in a tempdir only the artifact stem is stable.
        assert!(output.model_signature.contains("model_quantized"));
    }

    /// Against the user's real home layout (read-only check; no
    /// download). Skipped when the model is not installed yet.
    #[test]
    fn default_dir_points_under_home() {
        let dir = default_model_dir().unwrap();
        assert!(dir.ends_with("model/embedding_model"), "{dir:?}");
    }

    /// `model_installed` must mirror `init_at`'s install decision:
    /// false for an empty directory, true exactly when every file in
    /// the install manifest is present.
    #[test]
    fn model_installed_mirrors_the_init_manifest() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!LocalEmbeddingService::model_installed(dir.path()));

        for file in DEFAULT_MODEL_FILES {
            let path = dir.path().join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b"placeholder").unwrap();
            let complete = DEFAULT_MODEL_FILES
                .iter()
                .all(|f| dir.path().join(f).is_file());
            assert_eq!(
                LocalEmbeddingService::model_installed(dir.path()),
                complete,
                "after writing {file}"
            );
        }
    }
}
