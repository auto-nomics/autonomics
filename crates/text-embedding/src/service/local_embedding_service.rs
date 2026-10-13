use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use serde::{Deserialize, Serialize};

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
/// Completion marker written inside an installed model directory once
/// every manifest file has landed with its recorded size (the panel
/// cache's `.autonomics-panel-complete` precedent). Its presence turns
/// the readiness check from file-set existence into size verification.
pub const MODEL_COMPLETE_MARKER: &str = ".autonomics-model-complete";
/// Marker schema version; bump when the recorded shape changes.
const MARKER_VERSION: u32 = 1;

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
    /// Open the service over the default directory. The model must
    /// already be installed — `autonomics prepare` installs it; this
    /// never downloads.
    pub async fn init() -> Result<Self> {
        Self::init_at(&default_model_dir()?).await
    }

    /// Whether `model_dir` holds a complete install: every manifest
    /// file present, and — when a completion marker exists — matching
    /// the sizes it records. A directory without a marker counts as
    /// installed when the file set is complete (legacy installs and
    /// hand-copied directories); `prepare` upgrades those to marked
    /// installs.
    pub fn model_installed(model_dir: &Path) -> bool {
        matches!(
            verify_install(model_dir),
            ModelVerification::Verified | ModelVerification::CompleteWithoutMarker
        )
    }

    /// Open the embedder over an explicit model directory (host
    /// configuration and tests point this at their own layouts). The
    /// directory must already be installed — `prepare` installs it;
    /// callers that need the install should use
    /// [`ensure_installed`], not this.
    pub async fn init_at(model_dir: &Path) -> Result<Self> {
        if !is_installed(model_dir) {
            return Err(Error::Unknown(format!(
                "model not installed at {} — run `autonomics prepare`",
                model_dir.display()
            )));
        }
        let provider = Arc::new(OnnxTextEmbedder::open(model_dir)?);
        Ok(Self {
            provider: Some(provider),
        })
    }

    /// Install the default model into `~/.autonomics/model/embedding_model`
    /// following the [`ensure_installed`] policy. Returns the model
    /// directory.
    pub async fn install_default_model() -> Result<PathBuf> {
        let model_dir = default_model_dir()?;
        ensure_installed(&model_dir).await?;
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

/// The readiness state of one model directory, from [`verify_install`].
#[derive(Debug, PartialEq, Eq)]
pub enum ModelVerification {
    /// Marker present, every recorded file present at its recorded size.
    Verified,
    /// Complete file set without a marker — a legacy install or a
    /// hand-copied directory. Accepted as installed; `prepare` stamps
    /// a marker after verifying it opens.
    CompleteWithoutMarker,
    /// Absent manifest files (with or without a marker).
    Missing(Vec<String>),
    /// Marker present but a recorded file is missing or has a
    /// different size — the signature of an interrupted or damaged
    /// install.
    Inconsistent(Vec<String>),
}

/// What [`ensure_installed`] did.
#[derive(Debug, PartialEq, Eq)]
pub enum InstallAction {
    /// Already verified; nothing touched, no network.
    AlreadyVerified,
    /// A complete marker-less install opened successfully and was
    /// stamped with a marker (one-time legacy upgrade).
    MarkedLegacyInstall,
    /// Downloaded atomically (staging directory + rename + marker).
    Installed,
}

/// The install policy behind `autonomics prepare`: verify what is
/// there, heal what is broken, download only when needed.
///
/// - `Verified` → nothing to do.
/// - `CompleteWithoutMarker` → open the model once (the definitive
///   integrity check — this is the self-heal for truncated files left
///   by the pre-atomic installer, which "look installed" forever);
///   success stamps the marker, failure reinstalls cleanly.
/// - `Missing`/`Inconsistent` → reinstall atomically.
pub async fn ensure_installed(model_dir: &Path) -> Result<InstallAction> {
    match verify_install(model_dir) {
        ModelVerification::Verified => Ok(InstallAction::AlreadyVerified),
        ModelVerification::CompleteWithoutMarker => {
            match LocalEmbeddingService::init_at(model_dir).await {
                Ok(_) => {
                    write_marker(model_dir)?;
                    Ok(InstallAction::MarkedLegacyInstall)
                }
                Err(_) => {
                    install_default_model_in(model_dir).await?;
                    Ok(InstallAction::Installed)
                }
            }
        }
        ModelVerification::Missing(_) | ModelVerification::Inconsistent(_) => {
            install_default_model_in(model_dir).await?;
            Ok(InstallAction::Installed)
        }
    }
}

/// Verify an installed model directory (pure filesystem, no network).
pub fn verify_install(model_dir: &Path) -> ModelVerification {
    let marker_text = match std::fs::read_to_string(model_dir.join(MODEL_COMPLETE_MARKER)) {
        Ok(text) => text,
        Err(_) => {
            // No marker (legacy install or hand copy): the file set
            // alone decides.
            return file_set_verification(model_dir);
        }
    };
    let Ok(marker) = serde_json::from_str::<ModelCompleteMarker>(&marker_text) else {
        // A marker that cannot be parsed says nothing about the files;
        // falling back to the file set (instead of failing the install)
        // keeps a 50-byte marker corruption from forcing a ~363 MB
        // re-download. `prepare` re-stamps the marker after the open
        // verifies the payload.
        return file_set_verification(model_dir);
    };
    let mut problems = Vec::new();
    for (file, expected) in &marker.files {
        match std::fs::metadata(model_dir.join(file)) {
            Ok(meta) if meta.len() == *expected => {}
            Ok(meta) => problems.push(format!(
                "{file}: {} bytes, marker records {expected}",
                meta.len()
            )),
            Err(_) => problems.push(format!("{file}: missing")),
        }
    }
    if problems.is_empty() {
        ModelVerification::Verified
    } else {
        ModelVerification::Inconsistent(problems)
    }
}

/// The file-set-only verdict used when no (readable) marker exists.
fn file_set_verification(model_dir: &Path) -> ModelVerification {
    match missing_files(model_dir) {
        Some(missing) => ModelVerification::Missing(missing),
        None => ModelVerification::CompleteWithoutMarker,
    }
}

/// Contents of the completion marker: every manifest file's byte size.
#[derive(Debug, Serialize, Deserialize)]
pub struct ModelCompleteMarker {
    pub version: u32,
    pub files: BTreeMap<String, u64>,
}

/// Whether a model directory holds every manifest file.
fn is_installed(model_dir: &Path) -> bool {
    missing_files(model_dir).is_none()
}

/// The absent manifest files, or `None` when the set is complete.
fn missing_files(model_dir: &Path) -> Option<Vec<String>> {
    let missing: Vec<String> = DEFAULT_MODEL_FILES
        .iter()
        .filter(|file| !model_dir.join(file).is_file())
        .map(|file| (*file).to_string())
        .collect();
    if missing.is_empty() {
        None
    } else {
        Some(missing)
    }
}

/// Stamp the completion marker: every manifest file's current size.
/// Only called over a directory whose file set just verified.
fn write_marker(model_dir: &Path) -> Result<()> {
    let mut files = BTreeMap::new();
    for file in DEFAULT_MODEL_FILES {
        let path = model_dir.join(file);
        let size = std::fs::metadata(&path)
            .map_err(|error| marker_io_error("read the size of", &path, error))?
            .len();
        files.insert((*file).to_string(), size);
    }
    let text = serde_json::to_string(&ModelCompleteMarker {
        version: MARKER_VERSION,
        files,
    })
    .map_err(|error| Error::Unknown(format!("cannot serialize the model marker: {error}")))?;
    std::fs::write(model_dir.join(MODEL_COMPLETE_MARKER), text)
        .map_err(|error| marker_io_error("write", &model_dir.join(MODEL_COMPLETE_MARKER), error))?;
    Ok(())
}

fn marker_io_error(action: &str, path: &Path, error: std::io::Error) -> Error {
    Error::Unknown(format!("cannot {action} `{}`: {error}", path.display()))
}

/// Fetch the default model snapshot into `model_dir`, atomically: the
/// download lands in a sibling `.{name}.staging` directory, the
/// completion marker is written there after the full file set
/// verified, and only then does a rename move it into place. An
/// interrupted install leaves the staging directory behind (disposed
/// of on the next run) — never a half-written final directory that
/// looks installed.
///
/// The atomic dance exists because hf-hub's `local_dir` mode writes
/// directly to the final path with no temp file and skips any file
/// that already exists: a killed download leaves truncated files that
/// neither re-running the download nor the file-set check can see.
async fn install_default_model_in(model_dir: &Path) -> Result<()> {
    let file_name = model_dir
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            Error::Unknown(format!(
                "cannot derive a staging directory from {}",
                model_dir.display()
            ))
        })?;
    let parent = model_dir.parent().ok_or_else(|| {
        Error::Unknown(format!(
            "model directory {} has no parent for staging",
            model_dir.display()
        ))
    })?;
    let staging = parent.join(format!(".{file_name}.staging"));
    if staging.exists() {
        std::fs::remove_dir_all(&staging).map_err(|error| {
            Error::Unknown(format!(
                "cannot clear stale staging `{}`: {error}",
                staging.display()
            ))
        })?;
    }

    let client = hf_hub::HFClientBuilder::new().build()?;
    let repo = client.model(DEFAULT_MODEL_OWNER, DEFAULT_MODEL_NAME);
    repo.snapshot_download()
        .allow_patterns(DEFAULT_MODEL_FILES.iter().map(|f| f.to_string()).collect())
        .local_dir(staging.clone())
        .send()
        .await
        .map_err(Error::from)?;

    if let Some(missing) = missing_files(&staging) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(Error::Unknown(format!(
            "hub snapshot is incomplete (missing {})",
            missing.join(", ")
        )));
    }
    write_marker(&staging)?;

    // Swap in: callers only reach the installer when the final
    // directory is absent or broken, so replacing it outright is the
    // correct move — the payload is already staged next to it.
    if model_dir.exists() {
        std::fs::remove_dir_all(model_dir).map_err(|error| {
            Error::Unknown(format!("cannot replace `{}`: {error}", model_dir.display()))
        })?;
    }
    std::fs::rename(&staging, model_dir).map_err(|error| {
        Error::Unknown(format!(
            "cannot move the staged install into `{}`: {error}",
            model_dir.display()
        ))
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TextEmbedder;

    fn write_file_set(dir: &Path, contents: &[u8]) {
        for file in DEFAULT_MODEL_FILES {
            let path = dir.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, contents).unwrap();
        }
    }

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
        assert_eq!(
            verify_install(dir.path()),
            ModelVerification::Verified,
            "the atomic install stamps a matching completion marker"
        );
        // No staging leftovers.
        assert!(!dir.path().join(".embedding_model.staging").exists());

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

    /// `model_installed` must mirror the file-set manifest: false for
    /// an empty directory, true exactly when every manifest file is
    /// present.
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

    #[test]
    fn verify_install_matrix() {
        // Empty directory: everything missing.
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            verify_install(dir.path()),
            ModelVerification::Missing(
                DEFAULT_MODEL_FILES
                    .iter()
                    .map(|f| f.to_string())
                    .collect::<Vec<_>>()
            )
        );

        // Complete file set without a marker: legacy install, accepted.
        write_file_set(dir.path(), b"payload");
        assert_eq!(
            verify_install(dir.path()),
            ModelVerification::CompleteWithoutMarker
        );
        assert!(LocalEmbeddingService::model_installed(dir.path()));

        // Stamping the marker upgrades it to Verified.
        write_marker(dir.path()).unwrap();
        assert_eq!(verify_install(dir.path()), ModelVerification::Verified);

        // A truncated file (same set, different size) is Inconsistent.
        std::fs::write(dir.path().join("onnx/model_quantized.onnx_data"), b"short").unwrap();
        assert_eq!(
            verify_install(dir.path()),
            ModelVerification::Inconsistent(vec![
                "onnx/model_quantized.onnx_data: 5 bytes, marker records 7".to_string()
            ])
        );

        // A missing file under a marker is Inconsistent too.
        std::fs::remove_file(dir.path().join("tokenizer.json")).unwrap();
        let verification = verify_install(dir.path());
        assert!(
            matches!(verification, ModelVerification::Inconsistent(_)),
            "{verification:?}"
        );
    }

    #[test]
    fn verify_install_falls_back_to_the_file_set_on_a_corrupt_marker() {
        let dir = tempfile::tempdir().unwrap();
        write_file_set(dir.path(), b"payload");
        std::fs::write(dir.path().join(MODEL_COMPLETE_MARKER), "not json").unwrap();
        // An unparseable marker says nothing about the files: the file
        // set decides, so a 50-byte marker corruption cannot force a
        // full re-download. The daemon gate passes and the ONNX open
        // makes the final call.
        assert_eq!(
            verify_install(dir.path()),
            ModelVerification::CompleteWithoutMarker
        );
        assert!(LocalEmbeddingService::model_installed(dir.path()));
    }

    #[tokio::test]
    async fn ensure_installed_skips_a_verified_install_without_touching_disk() {
        let dir = tempfile::tempdir().unwrap();
        let model_dir = dir.path().join("embedding_model");
        write_file_set(&model_dir, b"payload");
        write_marker(&model_dir).unwrap();

        assert_eq!(
            ensure_installed(&model_dir).await.unwrap(),
            InstallAction::AlreadyVerified
        );
        // No staging directory appeared (that would mean a download
        // was attempted).
        assert!(
            !model_dir
                .parent()
                .unwrap()
                .join(".embedding_model.staging")
                .exists()
        );
    }

    #[tokio::test]
    async fn init_at_refuses_an_uninstalled_directory() {
        let dir = tempfile::tempdir().unwrap();
        let error = match LocalEmbeddingService::init_at(dir.path()).await {
            Ok(_) => panic!("init_at must refuse an uninstalled directory"),
            Err(error) => error,
        };
        let message = error.to_string();
        assert!(message.contains("not installed"), "{message}");
        assert!(message.contains("autonomics prepare"), "{message}");
    }
}
