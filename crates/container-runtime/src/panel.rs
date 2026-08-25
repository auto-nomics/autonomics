//! Immutable reference-panel manifests and a shared POSIX cache.
//!
//! Object storage is the authority. A cache entry becomes visible only after
//! every listed object has been downloaded and checksum-verified.

use std::path::{Component, Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use vfs::OpendalFileStorage;

use crate::error::ContainerRuntimeError;
use crate::types::{CachedPanel, PanelRef};

pub const PANEL_MANIFEST_OBJECT: &str = "manifest.json";
pub const PANEL_CACHE_COMPLETE_MARKER: &str = ".autonomics-panel-complete";

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Deserialize, Serialize)]
pub struct PanelFile {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Deserialize, Serialize)]
pub struct PanelManifest {
    #[serde(default = "default_schema_version")]
    pub schema_version: u8,
    pub id: String,
    pub version: String,
    pub digest: String,
    #[serde(default)]
    pub files: Vec<PanelFile>,
}

fn default_schema_version() -> u8 {
    1
}

impl PanelManifest {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1 {
            return Err(format!(
                "unsupported schema version {}",
                self.schema_version
            ));
        }
        if self.id.trim().is_empty() || self.version.trim().is_empty() {
            return Err("`id` and `version` cannot be empty".into());
        }
        if !is_sha256_digest(&self.digest) {
            return Err(format!("invalid bundle digest `{}`", self.digest));
        }
        let mut paths = std::collections::BTreeSet::new();
        for file in &self.files {
            validate_object_path(&file.path)?;
            if !is_sha256_digest(&file.sha256) {
                return Err(format!("invalid sha256 for `{}`", file.path));
            }
            if !paths.insert(file.path.clone()) {
                return Err(format!("duplicate panel file `{}`", file.path));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct PanelCache {
    pub root: PathBuf,
    pub pvc_prefix: String,
}

impl Default for PanelCache {
    fn default() -> Self {
        let config = crate::K3sConfig::default();
        Self::new(config.panel_cache_root, config.panel_pvc_prefix)
    }
}

impl PanelCache {
    pub fn new(root: impl Into<PathBuf>, pvc_prefix: impl Into<String>) -> Self {
        Self {
            root: root.into(),
            pvc_prefix: pvc_prefix.into(),
        }
    }

    pub async fn ensure(
        &self,
        storage: &OpendalFileStorage,
        panel: &PanelRef,
    ) -> Result<CachedPanel, ContainerRuntimeError> {
        panel.validate().map_err(ContainerRuntimeError::Invalid)?;
        let prefix =
            normalize_virtual_prefix(&panel.source).map_err(ContainerRuntimeError::Invalid)?;
        let manifest = self.load_manifest(storage, &prefix).await?;
        if manifest.id != panel.id {
            return Err(ContainerRuntimeError::PanelManifest {
                path: panel.source.clone(),
                message: format!("expected id `{}`, found `{}`", panel.id, manifest.id),
            });
        }
        if manifest.digest != panel.digest {
            return Err(ContainerRuntimeError::PanelManifest {
                path: panel.source.clone(),
                message: format!(
                    "expected digest `{}`, found `{}`",
                    panel.digest, manifest.digest
                ),
            });
        }

        let cache_key = format!("{}@{}", panel.id, panel.digest);
        let destination = self.root.join(&cache_key);
        let marker = destination.join(PANEL_CACHE_COMPLETE_MARKER);
        if marker_is_valid(&marker, &panel.digest).await {
            return Ok(self.cached_panel(panel, destination, cache_key));
        }

        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let temporary = self.root.join(format!(
            ".downloading-{cache_key}-{}-{nanos}",
            std::process::id()
        ));
        tokio::fs::create_dir_all(&temporary).await?;
        let result = self
            .download_files(storage, &prefix, &manifest, &temporary)
            .await;
        if let Err(error) = result {
            let _ = tokio::fs::remove_dir_all(&temporary).await;
            return Err(error);
        }

        tokio::fs::write(
            temporary.join(PANEL_CACHE_COMPLETE_MARKER),
            format!("digest={}\n", panel.digest),
        )
        .await?;
        tokio::fs::create_dir_all(&self.root).await?;
        match tokio::fs::rename(&temporary, &destination).await {
            Ok(()) => {}
            Err(_error) if destination.exists() => {
                let _ = tokio::fs::remove_dir_all(&temporary).await;
            }
            Err(error) => {
                let _ = tokio::fs::remove_dir_all(&temporary).await;
                return Err(error.into());
            }
        }

        if !marker_is_valid(&marker, &panel.digest).await {
            return Err(ContainerRuntimeError::ObjectStorage(format!(
                "panel cache entry `{}` did not become valid",
                destination.display()
            )));
        }
        Ok(self.cached_panel(panel, destination, cache_key))
    }

    fn cached_panel(&self, panel: &PanelRef, host_path: PathBuf, cache_key: String) -> CachedPanel {
        let prefix = self.pvc_prefix.trim_matches('/');
        CachedPanel {
            id: panel.id.clone(),
            digest: panel.digest.clone(),
            host_path,
            pvc_sub_path: if prefix.is_empty() {
                cache_key
            } else {
                format!("{prefix}/{cache_key}")
            },
            mount_path: panel.mount_path.clone(),
        }
    }

    async fn load_manifest(
        &self,
        storage: &OpendalFileStorage,
        prefix: &str,
    ) -> Result<PanelManifest, ContainerRuntimeError> {
        let path = format!("{prefix}/{PANEL_MANIFEST_OBJECT}");
        let bytes = storage
            .resolve(&path)
            .read(&storage.resolve_path(&path))
            .await
            .map_err(|error| ContainerRuntimeError::ObjectStorage(error.to_string()))?;
        let manifest: PanelManifest =
            serde_json::from_slice(&bytes.to_bytes()).map_err(|error| {
                ContainerRuntimeError::PanelManifest {
                    path: path.clone(),
                    message: error.to_string(),
                }
            })?;
        manifest
            .validate()
            .map_err(|message| ContainerRuntimeError::PanelManifest {
                path: path.clone(),
                message,
            })?;
        Ok(manifest)
    }

    async fn download_files(
        &self,
        storage: &OpendalFileStorage,
        prefix: &str,
        manifest: &PanelManifest,
        destination: &Path,
    ) -> Result<(), ContainerRuntimeError> {
        for file in &manifest.files {
            let virtual_path = format!("{prefix}/{}", file.path);
            let object_path = storage.resolve_path(&virtual_path);
            let operator = storage.resolve(&virtual_path);
            let target = destination.join(&file.path);
            if let Some(parent) = target.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }

            let reader = operator
                .reader(&object_path)
                .await
                .map_err(|error| ContainerRuntimeError::ObjectStorage(error.to_string()))?;
            let mut output = tokio::fs::File::create(&target).await?;
            let mut hasher = Sha256::new();
            let mut offset = 0_u64;
            const CHUNK: u64 = 8 * 1024 * 1024;
            while offset < file.size {
                let end = (offset + CHUNK).min(file.size);
                let buffer = reader
                    .read(offset..end)
                    .await
                    .map_err(|error| ContainerRuntimeError::ObjectStorage(error.to_string()))?;
                if buffer.is_empty() {
                    break;
                }
                let bytes = buffer.to_bytes();
                tokio::io::AsyncWriteExt::write_all(&mut output, &bytes).await?;
                hasher.update(&bytes);
                offset += bytes.len() as u64;
            }

            if offset != file.size {
                return Err(ContainerRuntimeError::ObjectStorage(format!(
                    "panel object `{virtual_path}` changed size while downloading: expected {}, got {offset}",
                    file.size
                )));
            }
            let actual = sha256_digest(&hasher.finalize());
            if actual != file.sha256 {
                return Err(ContainerRuntimeError::ObjectStorage(format!(
                    "panel object `{virtual_path}` checksum mismatch: expected {}, got {actual}",
                    file.sha256
                )));
            }
        }
        Ok(())
    }
}

async fn marker_is_valid(path: &Path, digest: &str) -> bool {
    matches!(
        tokio::fs::read_to_string(path).await,
        Ok(content) if content.trim() == format!("digest={digest}")
    )
}

fn normalize_virtual_prefix(source: &str) -> Result<String, String> {
    if source.contains('\0') {
        return Err("panel source cannot contain NUL bytes".into());
    }
    let value = source.strip_prefix("vfs://").unwrap_or(source);
    let normalized = vfs::OpendalFileStorage::normalize_path(value);
    let trimmed = normalized.trim_end_matches('/');
    if trimmed == "/" || trimmed.is_empty() {
        return Err("panel source cannot be the VFS root".into());
    }
    Ok(trimmed.to_string())
}

fn validate_object_path(path: &str) -> Result<(), String> {
    if path.is_empty() || path.contains('\0') {
        return Err("panel file paths cannot be empty".into());
    }
    let candidate = Path::new(path);
    if candidate.is_absolute()
        || !candidate
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(format!("unsafe panel file path `{path}`"));
    }
    Ok(())
}

fn is_sha256_digest(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn sha256_digest(bytes: &[u8]) -> String {
    format!("sha256:{}", hex(bytes))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content_digest(content: &[u8]) -> String {
        sha256_digest(&Sha256::digest(content))
    }

    #[tokio::test]
    async fn materializes_verified_panel_atomically() {
        let object_root = tempfile::tempdir().unwrap();
        let cache_root = tempfile::tempdir().unwrap();
        let storage = OpendalFileStorage::new(object_root.path());
        let content = b"reference-panel";
        let manifest = PanelManifest {
            schema_version: 1,
            id: "1000g_eur".into(),
            version: "v3".into(),
            digest: format!("sha256:{}", "3".repeat(64)),
            files: vec![PanelFile {
                path: "chr22/panel.txt".into(),
                size: content.len() as u64,
                sha256: content_digest(content),
            }],
        };
        tokio::fs::create_dir_all(object_root.path().join("panels/1000g/chr22"))
            .await
            .unwrap();
        tokio::fs::write(
            object_root.path().join("panels/1000g/chr22/panel.txt"),
            content,
        )
        .await
        .unwrap();
        storage
            .write_bytes(
                "/panels/1000g/manifest.json",
                serde_json::to_vec(&manifest).unwrap(),
            )
            .await
            .unwrap();

        let panel = PanelRef {
            id: "1000g_eur".into(),
            digest: manifest.digest.clone(),
            source: "/panels/1000g".into(),
            mount_path: "/panels/1000g_eur".into(),
        };
        let cache = PanelCache::new(cache_root.path(), "panels");
        let cached = cache.ensure(&storage, &panel).await.unwrap();

        assert_eq!(
            tokio::fs::read(cached.host_path.join("chr22/panel.txt"))
                .await
                .unwrap(),
            content
        );
        assert_eq!(
            cached.pvc_sub_path,
            format!("panels/1000g_eur@{}", manifest.digest)
        );
        assert!(cached.host_path.join(PANEL_CACHE_COMPLETE_MARKER).is_file());
    }

    #[tokio::test]
    async fn rejects_checksum_mismatch_without_publishing_cache_entry() {
        let object_root = tempfile::tempdir().unwrap();
        let cache_root = tempfile::tempdir().unwrap();
        let storage = OpendalFileStorage::new(object_root.path());
        tokio::fs::create_dir_all(object_root.path().join("panel"))
            .await
            .unwrap();
        tokio::fs::write(object_root.path().join("panel/data.txt"), b"bad")
            .await
            .unwrap();
        let manifest = PanelManifest {
            schema_version: 1,
            id: "panel".into(),
            version: "v1".into(),
            digest: format!("sha256:{}", "a".repeat(64)),
            files: vec![PanelFile {
                path: "data.txt".into(),
                size: 3,
                sha256: format!("sha256:{}", "b".repeat(64)),
            }],
        };
        storage
            .write_bytes(
                "/panel/manifest.json",
                serde_json::to_vec(&manifest).unwrap(),
            )
            .await
            .unwrap();
        let panel = PanelRef {
            id: "panel".into(),
            digest: manifest.digest,
            source: "/panel".into(),
            mount_path: "/panel".into(),
        };

        let error = PanelCache::new(cache_root.path(), "panels")
            .ensure(&storage, &panel)
            .await
            .unwrap_err();

        assert!(error.to_string().contains("checksum mismatch"));
        assert_eq!(cache_root.path().read_dir().unwrap().count(), 0);
    }
}
