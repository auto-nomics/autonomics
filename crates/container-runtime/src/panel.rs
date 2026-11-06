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
    /// Legacy panel id, or the catalog repository from schema v2 manifests.
    #[serde(alias = "repo")]
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
        if !matches!(self.schema_version, 1 | 2) {
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
    content_required: bool,
    manifest_hashes: std::collections::BTreeMap<String, String>,
}

impl Default for PanelCache {
    fn default() -> Self {
        Self::new(crate::PodmanConfig::default().panel_cache_root)
    }
}

impl PanelCache {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            content_required: false,
            manifest_hashes: Default::default(),
        }
    }
    pub fn content_required(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            content_required: true,
            manifest_hashes: Default::default(),
        }
    }
    pub fn requires_content_verification(&self) -> bool {
        self.content_required
    }
    /// Byte identities admitted in the signed method contract. A panel's
    /// opaque catalog label alone cannot bind the manifest's file checksums.
    pub fn with_manifest_hashes(
        mut self,
        hashes: std::collections::BTreeMap<String, String>,
    ) -> Result<Self, String> {
        if !self.content_required {
            return Err("manifest identities require strict panel mode".into());
        }
        for (source, hash) in hashes {
            let source = normalize_virtual_prefix(&source)?;
            if hash.len() != 64
                || !hash
                    .bytes()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
            {
                return Err("invalid admitted panel manifest hash".into());
            }
            if self.manifest_hashes.insert(source, hash).is_some() {
                return Err("duplicate admitted panel manifest source".into());
            }
        }
        Ok(self)
    }
    pub fn expected_manifest_hash(&self, source: &str) -> Option<&str> {
        let source = normalize_virtual_prefix(source).ok()?;
        self.manifest_hashes.get(&source).map(String::as_str)
    }
    async fn verify_cache_files(
        &self,
        destination: &Path,
        manifest: &PanelManifest,
    ) -> Result<(), ContainerRuntimeError> {
        use tokio::io::AsyncReadExt;
        if !self.content_required {
            return Ok(());
        }
        if manifest.files.is_empty()
            || tokio::fs::symlink_metadata(destination)
                .await?
                .file_type()
                .is_symlink()
        {
            return Err(ContainerRuntimeError::ObjectStorage(
                "strict panel cache is empty or symlinked".into(),
            ));
        }
        let root = destination.canonicalize()?;
        let expected: std::collections::BTreeSet<_> = manifest
            .files
            .iter()
            .map(|file| file.path.clone())
            .collect();
        let mut discovered = std::collections::BTreeSet::new();
        let mut directories = vec![destination.to_path_buf()];
        while let Some(directory) = directories.pop() {
            let mut entries = tokio::fs::read_dir(directory).await?;
            while let Some(entry) = entries.next_entry().await? {
                let ty = entry.file_type().await?;
                if ty.is_symlink() {
                    return Err(ContainerRuntimeError::ObjectStorage(
                        "strict panel contains a symlink".into(),
                    ));
                }
                if ty.is_dir() {
                    directories.push(entry.path());
                } else if ty.is_file() {
                    let relative = entry
                        .path()
                        .strip_prefix(destination)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned();
                    if relative != PANEL_CACHE_COMPLETE_MARKER {
                        discovered.insert(relative);
                    }
                } else {
                    return Err(ContainerRuntimeError::ObjectStorage(
                        "strict panel contains a special file".into(),
                    ));
                }
            }
        }
        if discovered != expected {
            return Err(ContainerRuntimeError::ObjectStorage(
                "strict panel file universe changed".into(),
            ));
        }
        for file in &manifest.files {
            let path = destination.join(&file.path).canonicalize()?;
            if !path.starts_with(&root) {
                return Err(ContainerRuntimeError::ObjectStorage(
                    "strict panel escaped cache root".into(),
                ));
            }
            let mut input = tokio::fs::File::open(&path).await?;
            let mut hasher = Sha256::new();
            let mut count = 0u64;
            let mut buffer = vec![0u8; 1024 * 1024];
            loop {
                let read = input.read(&mut buffer).await?;
                if read == 0 {
                    break;
                }
                hasher.update(&buffer[..read]);
                count += read as u64;
            }
            if count != file.size || sha256_digest(&hasher.finalize()) != file.sha256 {
                return Err(ContainerRuntimeError::ObjectStorage(format!(
                    "strict cached panel object {} content changed",
                    file.path
                )));
            }
        }
        Ok(())
    }

    pub async fn ensure(
        &self,
        storage: &OpendalFileStorage,
        panel: &PanelRef,
    ) -> Result<CachedPanel, ContainerRuntimeError> {
        panel.validate().map_err(ContainerRuntimeError::Invalid)?;
        let prefix =
            normalize_virtual_prefix(&panel.source).map_err(ContainerRuntimeError::Invalid)?;

        let cache_key = format!("{}@{}", panel.id, panel.digest);
        let destination = self.root.join(&cache_key);
        let marker = destination.join(PANEL_CACHE_COMPLETE_MARKER);
        // Local-first: the catalog cache already stores every bundle under
        // `<root>/<owner>/<name>@<digest>` with the same completion marker
        // and manifest schema. A present, digest-matching entry is the one
        // authoritative copy — mount it directly instead of fetching a
        // second copy from object storage.
        if marker_is_valid(&marker, &panel.digest).await {
            if let Some((manifest, manifest_bytes)) =
                self.load_local_manifest(&destination).await?
            {
                // Strict mode admits a panel only through its signed byte
                // identity; the local copy must carry the same signature
                // the object-storage manifest would have been checked
                // against.
                if manifest.digest == panel.digest && self.admits_local_manifest(&prefix, &manifest_bytes)
                {
                    self.verify_cache_files(&destination, &manifest).await?;
                    // Record the hit so the LRU sweeper removes
                    // least-recently-used entries rather than
                    // least-recently-downloaded ones.
                    crate::gc::touch_dir_mtime(&destination);
                    return Ok(self.cached_panel(panel, destination, manifest_bytes));
                }
            }
        }

        let (manifest, manifest_bytes) = self.load_manifest(storage, &prefix).await?;
        // A frozen runtime binding may use a logical alias for the catalog
        // repository. Strict mode has already verified the admitted original
        // manifest bytes; the signed binding and digest identify that content.
        if manifest.id != panel.id && !self.content_required {
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

        if marker_is_valid(&marker, &panel.digest).await {
            self.verify_cache_files(&destination, &manifest).await?;
            // Record the hit so the LRU sweeper removes least-recently-used
            // entries rather than least-recently-downloaded ones.
            crate::gc::touch_dir_mtime(&destination);
            return Ok(self.cached_panel(panel, destination, manifest_bytes));
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
        if let Some(parent) = destination.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
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
        self.verify_cache_files(&destination, &manifest).await?;
        Ok(self.cached_panel(panel, destination, manifest_bytes))
    }

    fn cached_panel(
        &self,
        panel: &PanelRef,
        host_path: PathBuf,
        manifest_bytes: Vec<u8>,
    ) -> CachedPanel {
        CachedPanel {
            id: panel.id.clone(),
            digest: panel.digest.clone(),
            host_path,
            mount_path: panel.mount_path.clone(),
            manifest_bytes: Some(manifest_bytes),
        }
    }

    /// Whether a locally stored manifest carries the signed byte identity
    /// strict mode requires. Non-strict caches admit any digest-matching
    /// manifest; strict ones only admit the exact signed bytes.
    fn admits_local_manifest(&self, prefix: &str, manifest_bytes: &[u8]) -> bool {
        if !self.content_required {
            return true;
        }
        let Some(expected) = self.expected_manifest_hash(prefix) else {
            return false;
        };
        let actual = sha256_digest(&Sha256::digest(manifest_bytes));
        actual.strip_prefix("sha256:") == Some(expected)
    }

    /// Read the manifest already stored beside a cached entry. The catalog
    /// cache and the panel cache share the same on-disk convention, so an
    /// installed bundle carries its own manifest — no object-storage round
    /// trip is needed to admit it.
    async fn load_local_manifest(
        &self,
        destination: &Path,
    ) -> Result<Option<(PanelManifest, Vec<u8>)>, ContainerRuntimeError> {
        let path = destination.join(PANEL_MANIFEST_OBJECT);
        let bytes = match tokio::fs::read(&path).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let manifest: PanelManifest = match serde_json::from_slice(&bytes) {
            Ok(manifest) => manifest,
            // A manifest this cache cannot interpret is not an admission
            // failure: the object-storage copy remains authoritative.
            Err(_) => return Ok(None),
        };
        Ok(Some((manifest, bytes)))
    }

    async fn load_manifest(
        &self,
        storage: &OpendalFileStorage,
        prefix: &str,
    ) -> Result<(PanelManifest, Vec<u8>), ContainerRuntimeError> {
        let path = format!("{prefix}/{PANEL_MANIFEST_OBJECT}");
        let bytes = storage
            .resolve(&path)
            .read(&storage.resolve_path(&path))
            .await
            .map_err(|error| ContainerRuntimeError::ObjectStorage(error.to_string()))?;
        let bytes = bytes.to_bytes();
        if self.content_required {
            let expected = self.expected_manifest_hash(prefix).ok_or_else(|| {
                ContainerRuntimeError::PanelManifest {
                    path: path.clone(),
                    message: "panel manifest has no signed byte identity".into(),
                }
            })?;
            let actual = sha256_digest(&Sha256::digest(&bytes));
            if actual.strip_prefix("sha256:") != Some(expected) {
                return Err(ContainerRuntimeError::PanelManifest {
                    path: path.clone(),
                    message: "panel manifest differs from signed byte identity".into(),
                });
            }
        }
        let manifest: PanelManifest = serde_json::from_slice(&bytes).map_err(|error| {
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
        Ok((manifest, bytes.to_vec()))
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
            tokio::io::AsyncWriteExt::flush(&mut output).await?;
            output.sync_all().await?;
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
        let cache = PanelCache::new(cache_root.path());
        let cached = cache.ensure(&storage, &panel).await.unwrap();

        assert_eq!(
            tokio::fs::read(cached.host_path.join("chr22/panel.txt"))
                .await
                .unwrap(),
            content
        );
        assert_eq!(
            cached.host_path.file_name().unwrap().to_string_lossy(),
            format!("1000g_eur@{}", manifest.digest)
        );
        assert!(cached.host_path.join(PANEL_CACHE_COMPLETE_MARKER).is_file());
    }

    /// The catalog cache already stores every bundle under
    /// `<root>/<owner>/<name>@<digest>` with the panel cache's completion
    /// marker and manifest schema. `ensure` must admit that entry as the
    /// one authoritative copy — the object storage in this test is empty, so
    /// a hit proves nothing was downloaded.
    #[tokio::test]
    async fn an_installed_catalog_entry_is_mounted_in_place_without_downloading() {
        let object_root = tempfile::tempdir().unwrap();
        let cache_root = tempfile::tempdir().unwrap();
        let storage = OpendalFileStorage::new(object_root.path());
        let content = b"catalog-panel";
        let digest = format!("sha256:{}", "4".repeat(64));
        let manifest = PanelManifest {
            schema_version: 2,
            id: "wjixiang/catalog-ldsc".into(),
            version: "v1".into(),
            digest: digest.clone(),
            files: vec![PanelFile {
                path: "LDREF/chr22.bed".into(),
                size: content.len() as u64,
                sha256: content_digest(content),
            }],
        };
        // The catalog layout: <root>/<owner>/<name>@<digest>.
        let entry = cache_root
            .path()
            .join("wjixiang")
            .join(format!("catalog-ldsc@{digest}"));
        tokio::fs::create_dir_all(entry.join("LDREF")).await.unwrap();
        tokio::fs::write(entry.join("LDREF/chr22.bed"), content)
            .await
            .unwrap();
        tokio::fs::write(
            entry.join(PANEL_MANIFEST_OBJECT),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .await
        .unwrap();
        tokio::fs::write(
            entry.join(PANEL_CACHE_COMPLETE_MARKER),
            format!("digest={digest}\n"),
        )
        .await
        .unwrap();

        let panel = PanelRef {
            id: "wjixiang/catalog-ldsc".into(),
            digest: digest.clone(),
            source: "/catalog/wjixiang/catalog-ldsc".into(),
            mount_path: "/bundles/wjixiang/catalog-ldsc".into(),
        };
        let cache = PanelCache::new(cache_root.path());
        let cached = cache.ensure(&storage, &panel).await.unwrap();

        // Mounted in place — the catalog's own directory, no copy.
        assert_eq!(cached.host_path, entry);
        assert_eq!(
            tokio::fs::read(cached.host_path.join("LDREF/chr22.bed"))
                .await
                .unwrap(),
            content
        );
    }

    #[tokio::test]
    async fn cold_and_warm_panels_keep_original_manifest_bytes_without_changing_cache_files() {
        let objects = tempfile::tempdir().unwrap();
        let cached = tempfile::tempdir().unwrap();
        let storage = OpendalFileStorage::new(objects.path());
        let content = b"reference-panel";
        let manifest = PanelManifest {
            schema_version: 1,
            id: "original-bytes".into(),
            version: "v1".into(),
            digest: format!("sha256:{}", "a".repeat(64)),
            files: vec![PanelFile {
                path: "data.txt".into(),
                size: content.len() as u64,
                sha256: content_digest(content),
            }],
        };
        // Whitespace distinguishes the authoritative bytes from reserialization.
        let raw = format!("\n{}\n", serde_json::to_string_pretty(&manifest).unwrap()).into_bytes();
        storage
            .write_bytes("/panel/manifest.json", raw.clone())
            .await
            .unwrap();
        storage
            .write_bytes("/panel/data.txt", content.to_vec())
            .await
            .unwrap();
        let panel = PanelRef {
            id: manifest.id.clone(),
            digest: manifest.digest.clone(),
            source: "/panel".into(),
            mount_path: "/panels/reference".into(),
        };
        let cache = PanelCache::content_required(cached.path())
            .with_manifest_hashes(std::collections::BTreeMap::from([(
                "/panel".into(),
                hex(&Sha256::digest(&raw)),
            )]))
            .unwrap();
        for _ in 0..2 {
            let result = cache.ensure(&storage, &panel).await.unwrap();
            assert_eq!(result.manifest_bytes.as_deref(), Some(raw.as_slice()));
            assert!(!result.host_path.join(PANEL_MANIFEST_OBJECT).exists());
            let mut files = std::fs::read_dir(&result.host_path)
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            files.sort();
            assert_eq!(files, vec![PANEL_CACHE_COMPLETE_MARKER, "data.txt"]);
        }
    }

    #[tokio::test]
    async fn strict_warm_cache_rejects_content_and_file_universe_changes() {
        let objects = tempfile::tempdir().unwrap();
        let cached = tempfile::tempdir().unwrap();
        let storage = OpendalFileStorage::new(objects.path());
        let content = b"reference-panel";
        let manifest = PanelManifest {
            schema_version: 1,
            id: "strict-panel".into(),
            version: "v1".into(),
            digest: format!("sha256:{}", "d".repeat(64)),
            files: vec![PanelFile {
                path: "nested/data.txt".into(),
                size: content.len() as u64,
                sha256: content_digest(content),
            }],
        };
        tokio::fs::create_dir_all(objects.path().join("panel/nested"))
            .await
            .unwrap();
        tokio::fs::write(objects.path().join("panel/nested/data.txt"), content)
            .await
            .unwrap();
        storage
            .write_bytes(
                "/panel/manifest.json",
                serde_json::to_vec(&manifest).unwrap(),
            )
            .await
            .unwrap();
        let panel = PanelRef {
            id: manifest.id.clone(),
            digest: manifest.digest.clone(),
            source: "/panel".into(),
            mount_path: "/panel".into(),
        };
        let unsigned_cache = PanelCache::content_required(cached.path());
        assert!(
            unsigned_cache
                .ensure(&storage, &panel)
                .await
                .unwrap_err()
                .to_string()
                .contains("no signed byte identity")
        );
        let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
        let cache = PanelCache::content_required(cached.path())
            .with_manifest_hashes(std::collections::BTreeMap::from([(
                "/panel".into(),
                content_digest(&manifest_bytes)
                    .strip_prefix("sha256:")
                    .unwrap()
                    .into(),
            )]))
            .unwrap();
        let ready = cache.ensure(&storage, &panel).await.unwrap();
        cache.ensure(&storage, &panel).await.unwrap();
        let alias = PanelRef {
            id: "runtime-alias".into(),
            ..panel.clone()
        };
        // Only an admitted source-byte identity permits a logical catalog alias.
        cache.ensure(&storage, &alias).await.unwrap();
        assert!(
            PanelCache::new(cached.path())
                .ensure(&storage, &alias)
                .await
                .unwrap_err()
                .to_string()
                .contains("expected id")
        );
        let data = ready.host_path.join("nested/data.txt");
        // Same length: metadata and the completion marker cannot establish identity.
        tokio::fs::write(&data, b"REFERENCE-PANEL").await.unwrap();
        assert!(
            cache
                .ensure(&storage, &panel)
                .await
                .unwrap_err()
                .to_string()
                .contains("content changed")
        );
        tokio::fs::write(&data, content).await.unwrap();
        let extra = ready.host_path.join("unregistered.txt");
        tokio::fs::write(&extra, b"extra").await.unwrap();
        assert!(
            cache
                .ensure(&storage, &panel)
                .await
                .unwrap_err()
                .to_string()
                .contains("universe changed")
        );
        tokio::fs::remove_file(extra).await.unwrap();
        tokio::fs::remove_file(&data).await.unwrap();
        assert!(
            cache
                .ensure(&storage, &panel)
                .await
                .unwrap_err()
                .to_string()
                .contains("universe changed")
        );
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(objects.path().join("panel/nested/data.txt"), &data)
                .unwrap();
            assert!(
                cache
                    .ensure(&storage, &panel)
                    .await
                    .unwrap_err()
                    .to_string()
                    .contains("symlink")
            );
            tokio::fs::remove_file(&data).await.unwrap();
        }
        tokio::fs::write(&data, content).await.unwrap();
        cache.ensure(&storage, &panel).await.unwrap();
        // A changed manifest with the same opaque digest must also fail.
        let mut changed = manifest;
        changed.version = "unadmitted-v2".into();
        storage
            .write_bytes(
                "/panel/manifest.json",
                serde_json::to_vec(&changed).unwrap(),
            )
            .await
            .unwrap();
        assert!(
            cache
                .ensure(&storage, &panel)
                .await
                .unwrap_err()
                .to_string()
                .contains("differs from signed byte identity")
        );
    }

    #[tokio::test]
    async fn materializes_catalog_schema_v2_manifest() {
        let object_root = tempfile::tempdir().unwrap();
        let cache_root = tempfile::tempdir().unwrap();
        let storage = OpendalFileStorage::new(object_root.path());
        let content = b"catalog-panel";
        let digest = format!("sha256:{}", "4".repeat(64));
        let manifest = serde_json::json!({
            "schema_version": 2,
            "repo": "owner/catalog-panel",
            "version": "v1",
            "kind": "reference_panel",
            "files": [{
                "path": "panel.txt",
                "size": content.len(),
                "sha256": content_digest(content),
            }],
            "digest": digest,
        });
        let source_root = object_root
            .path()
            .join("datasets/owner/catalog-panel@sha256-4444");
        tokio::fs::create_dir_all(&source_root).await.unwrap();
        tokio::fs::write(source_root.join("panel.txt"), content)
            .await
            .unwrap();
        storage
            .write_bytes(
                "/datasets/owner/catalog-panel@sha256-4444/manifest.json",
                serde_json::to_vec(&manifest).unwrap(),
            )
            .await
            .unwrap();

        let panel = PanelRef {
            id: "owner/catalog-panel".into(),
            digest,
            source: "/datasets/owner/catalog-panel@sha256-4444".into(),
            mount_path: "/panels/catalog-panel".into(),
        };
        let cached = PanelCache::new(cache_root.path())
            .ensure(&storage, &panel)
            .await
            .unwrap();

        assert_eq!(
            tokio::fs::read(cached.host_path.join("panel.txt"))
                .await
                .unwrap(),
            content
        );
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

        let error = PanelCache::new(cache_root.path())
            .ensure(&storage, &panel)
            .await
            .unwrap_err();

        assert!(error.to_string().contains("checksum mismatch"));
        assert_eq!(cache_root.path().read_dir().unwrap().count(), 0);
    }
}
