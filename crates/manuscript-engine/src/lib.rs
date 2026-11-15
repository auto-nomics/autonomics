//! Isolated, embedded Tectonic compilation. No system TeX installation is used.
mod worker;

/// Call before initializing threads/runtimes in an executable embedding this crate.
pub fn run_worker_if_requested() -> bool {
    let mut args: Vec<String> = std::env::args().collect();
    if args
        .get(1)
        .is_none_or(|arg| arg != "--internal-manuscript-worker")
    {
        return false;
    }
    args.remove(1);
    worker::run_worker(args);
    true
}

use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
    time::Duration,
};
use tokio::process::Command;
static COMPILE_SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("{0}")]
    Invalid(String),
    #[error("compiler I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("compiler protocol: {0}")]
    Json(#[from] serde_json::Error),
    #[error("compilation timed out")]
    Timeout,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub severity: String,
    pub message: String,
    pub file: Option<String>,
    pub line: Option<u32>,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct CompileOutput {
    pub success: bool,
    pub pdf: Option<Vec<u8>>,
    pub log: String,
    pub diagnostics: Vec<Diagnostic>,
    pub synctex: Option<Vec<u8>>,
}
#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub worker: PathBuf,
    pub cache_dir: PathBuf,
    pub bundle: Option<String>,
    pub only_cached: bool,
    pub timeout: Duration,
}
impl EngineConfig {
    pub fn from_env() -> Self {
        let worker = std::env::var_os("AUTONOMICS_MANUSCRIPT_WORKER")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::current_exe().unwrap_or_default());
        let cache_dir = std::env::var_os("AUTONOMICS_TECTONIC_CACHE")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(std::env::temp_dir)
                    .join(".cache/autonomics/tectonic")
            });
        Self {
            worker,
            cache_dir,
            bundle: std::env::var("AUTONOMICS_TECTONIC_BUNDLE").ok(),
            only_cached: std::env::var("AUTONOMICS_TECTONIC_OFFLINE").is_ok_and(|v| v == "1"),
            timeout: Duration::from_secs(
                std::env::var("AUTONOMICS_TECTONIC_TIMEOUT_SECS")
                    .ok()
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(300)
                    .clamp(5, 600),
            ),
        }
    }
    pub async fn compile(
        &self,
        files: BTreeMap<String, Vec<u8>>,
        main_file: &str,
    ) -> Result<CompileOutput, EngineError> {
        let _permit = COMPILE_SLOTS
            .acquire()
            .await
            .map_err(|_| EngineError::Invalid("compiler unavailable".into()))?;
        validate_path(main_file)?;
        if !files.contains_key(main_file) {
            return Err(EngineError::Invalid("main file missing".into()));
        }
        if files.len() > 512 || files.values().map(Vec::len).sum::<usize>() > 64 * 1024 * 1024 {
            return Err(EngineError::Invalid(
                "project exceeds compilation limits".into(),
            ));
        }
        let dir = tempfile::tempdir()?;
        for (name, data) in files {
            validate_path(&name)?;
            let path = dir.path().join(name);
            if let Some(p) = path.parent() {
                tokio::fs::create_dir_all(p).await?;
            }
            tokio::fs::write(path, data).await?;
        }
        tokio::fs::create_dir_all(&self.cache_dir).await?;
        let cache = tokio::fs::canonicalize(&self.cache_dir).await?;
        let result_path = dir.path().join(".compile-result.json");
        let mut command = Command::new(&self.worker);
        command
            .arg("--internal-manuscript-worker")
            .arg(dir.path())
            .arg(main_file)
            .arg(&cache)
            .arg(self.bundle.as_deref().unwrap_or(""))
            .arg(if self.only_cached { "1" } else { "0" })
            .env_clear()
            .env("HOME", dir.path())
            .env("TMPDIR", dir.path())
            .env("XDG_CACHE_HOME", &cache)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        let mut child = command.spawn()?;
        let status = match tokio::time::timeout(self.timeout, child.wait()).await {
            Ok(s) => s?,
            Err(_) => {
                child.kill().await?;
                return Err(EngineError::Timeout);
            }
        };
        if !result_path.exists() {
            return Err(EngineError::Invalid(format!(
                "embedded compiler worker exited with {status}; verify worker installation and Linux Landlock support"
            )));
        }
        if tokio::fs::metadata(&result_path).await?.len() > 96 * 1024 * 1024 {
            return Err(EngineError::Invalid(
                "compiler result exceeds 96 MiB limit".into(),
            ));
        }
        let bytes = tokio::fs::read(result_path).await?;
        Ok(serde_json::from_slice(&bytes)?)
    }
}
pub fn validate_path(name: &str) -> Result<(), EngineError> {
    if name.is_empty()
        || name.starts_with('.')
        || name.contains('\\')
        || name.contains('\0')
        || !Path::new(name)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
    {
        return Err(EngineError::Invalid("invalid project file path".into()));
    }
    Ok(())
}
