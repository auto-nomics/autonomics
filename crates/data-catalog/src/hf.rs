//! Publish data packages to a Hugging Face dataset repository.

use std::path::Path;

use async_trait::async_trait;
use hf_hub::repository::CommitOperation;
use hf_hub::{HFClient, HFClientBuilder, HFError, HFRepository, RepoTypeDataset};

use crate::error::Result;
use crate::model::{CatalogEntry, CatalogIndex, DatasetManifest};
use crate::package::{PACKAGE_MANIFEST, PAYLOAD_DIR, validate_package};
use crate::publish::{build_entry, upsert_entry};
use crate::remote::ObjectSource;

const INDEX_PATH: &str = "index.json";

/// Resolve a Hugging Face token: explicit value, then `HUGGING_FACE_TOKEN`;
/// `None` defers to the hf-hub defaults (`HF_TOKEN`, `HF_TOKEN_PATH`, or the
/// cached token file).
pub fn resolve_hf_token(explicit: Option<String>) -> Option<String> {
    explicit.or_else(|| {
        std::env::var("HUGGING_FACE_TOKEN")
            .ok()
            .filter(|token| !token.is_empty())
    })
}

/// Where and how to publish a package on the Hugging Face Hub.
#[derive(Debug, Clone)]
pub struct HfPublishTarget {
    /// Prefix for per-package dataset repositories in `owner/name` form.
    /// The index lives at this repository; each package lives at
    /// `{repo_id}-{sanitized-id}`.
    pub repo_id: String,
    /// Branch to commit to; `None` uses the repository main branch.
    pub revision: Option<String>,
    /// Bearer token. `None` falls back to the hf-hub defaults: `HF_TOKEN`,
    /// `HF_TOKEN_PATH`, or the cached token file.
    pub token: Option<String>,
    /// Create the dataset repository first when it does not exist yet.
    pub create_repository: bool,
}

/// Derive the per-package repository ID from a prefix and package ID.
///
/// Only lowercase alphanumeric characters and hyphens are preserved; all
/// other characters become hyphens. This matches HF's repo naming rules.
pub fn package_repo_id(prefix: &str, package_id: &str) -> String {
    let sanitized = package_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    format!("{prefix}-{sanitized}")
}

/// Object source that routes `owner/repo/path` keys to the correct HF dataset.
///
/// The key format uses the full HF repo ID as the first two path components:
/// `owner/repo-name/path/to/file`.
pub struct MultiRepoHfSource {
    client: HFClient,
    revision: String,
}

impl MultiRepoHfSource {
    pub fn new(
        _index_repo: &str,
        revision: Option<String>,
        token: Option<String>,
    ) -> Result<Self> {
        let mut builder = HFClientBuilder::new();
        if let Some(token) = token {
            builder = builder.token(token);
        }
        let client = builder
            .build()
            .map_err(|error| format!("build Hugging Face client: {error}"))?;
        Ok(Self {
            client,
            revision: revision.unwrap_or_else(|| "main".to_string()),
        })
    }

    fn resolve(&self, key: &str) -> Result<(HFRepository<RepoTypeDataset>, String)> {
        let parts: Vec<&str> = key.splitn(3, '/').collect();
        if parts.len() < 3 || parts[0].is_empty() || parts[1].is_empty() {
            return Err(format!(
                "HF multi-repo key must be `owner/repo/path`, got `{key}`"
            )
            .into());
        }
        let repo = self.client.dataset(parts[0], parts[1]);
        Ok((repo, parts[2].to_string()))
    }
}

#[async_trait]
impl ObjectSource for MultiRepoHfSource {
    async fn read(&self, key: &str) -> Result<Vec<u8>> {
        let (repository, path) = self.resolve(key)?;
        let bytes = repository
            .download_file_to_bytes()
            .filename(path)
            .revision(self.revision.clone())
            .send()
            .await
            .map_err(|error| format!("read Hugging Face object `{key}`: {error}"))?;
        Ok(bytes.to_vec())
    }

    async fn read_range(&self, key: &str, offset: u64, len: u64) -> Result<Vec<u8>> {
        if len == 0 {
            return Ok(Vec::new());
        }
        let (repository, path) = self.resolve(key)?;
        let bytes = repository
            .download_file_to_bytes()
            .filename(path)
            .revision(self.revision.clone())
            .range(offset..offset + len)
            .send()
            .await
            .map_err(|error| {
                format!("read Hugging Face object `{key}` at {offset}: {error}")
            })?;
        Ok(bytes.to_vec())
    }
}

/// Object source reading catalog objects from a Hugging Face dataset repo.
pub struct HfSource {
    repository: HFRepository<RepoTypeDataset>,
    revision: String,
}

impl HfSource {
    pub fn new(repo_id: &str, revision: Option<String>, token: Option<String>) -> Result<Self> {
        let (owner, name) = split_repo_id(repo_id)?;
        let mut builder = HFClientBuilder::new();
        if let Some(token) = token {
            builder = builder.token(token);
        }
        let client = builder
            .build()
            .map_err(|error| format!("build Hugging Face client: {error}"))?;
        Ok(Self {
            repository: client.dataset(owner, name),
            revision: revision.unwrap_or_else(|| "main".to_string()),
        })
    }

    pub fn revision(&self) -> &str {
        &self.revision
    }
}

#[async_trait]
impl ObjectSource for HfSource {
    async fn read(&self, key: &str) -> Result<Vec<u8>> {
        let bytes = self
            .repository
            .download_file_to_bytes()
            .filename(key.to_string())
            .revision(self.revision.clone())
            .send()
            .await
            .map_err(|error| format!("read Hugging Face object `{key}`: {error}"))?;
        Ok(bytes.to_vec())
    }

    async fn read_range(&self, key: &str, offset: u64, len: u64) -> Result<Vec<u8>> {
        if len == 0 {
            return Ok(Vec::new());
        }
        let bytes = self
            .repository
            .download_file_to_bytes()
            .filename(key.to_string())
            .revision(self.revision.clone())
            .range(offset..offset + len)
            .send()
            .await
            .map_err(|error| format!("read Hugging Face object `{key}` at {offset}: {error}"))?;
        Ok(bytes.to_vec())
    }
}

/// Publish a validated local package to a Hugging Face dataset repository.
///
/// The Hub repository is treated as the catalog root: `index.json` lives at
/// the repository root and payload uses the same content-addressed
/// `entries/...` layout as the object-storage catalog. The manifest, payload
/// files, and updated index are committed together, so readers observe either
/// the old or the new catalog generation, never a partial upload.
///
/// Note: the Hub rejects commits with more than 1000 files, so packages with
/// extremely large payload file counts cannot be published in one commit.
pub async fn publish_package_to_hf(
    package: impl AsRef<Path>,
    target: &HfPublishTarget,
) -> Result<CatalogEntry> {
    let package = package.as_ref();
    let manifest = validate_package(package).map_err(|error| error.to_string())?;
    let mut entry = build_entry(&manifest);
    entry.repo = package_repo_id(&target.repo_id, &entry.id);

    let (pkg_owner, pkg_name) = split_repo_id(&entry.repo)?;
    let (index_owner, index_name) = split_repo_id(&target.repo_id)?;
    let mut builder = HFClientBuilder::new();
    if let Some(token) = &target.token {
        builder = builder.token(token.clone());
    }
    let client = builder
        .build()
        .map_err(|error| format!("build Hugging Face client: {error}"))?;

    if target.create_repository {
        client
            .create_repository()
            .repo_id(entry.repo.as_str())
            .repo_type(RepoTypeDataset)
            .exist_ok(true)
            .send()
            .await
            .map_err(|error| {
                format!("create package repository `{}`: {error}", entry.repo)
            })?;
        client
            .create_repository()
            .repo_id(target.repo_id.as_str())
            .repo_type(RepoTypeDataset)
            .exist_ok(true)
            .send()
            .await
            .map_err(|error| {
                format!(
                    "create Hugging Face repository `{}`: {error}",
                    target.repo_id
                )
            })?;
    }

    // Phase 1: commit manifest + payload to the per-package repository.
    let package_repository = client.dataset(pkg_owner, pkg_name);
    let revision = target
        .revision
        .clone()
        .unwrap_or_else(|| "main".to_string());
    package_repository
        .create_commit()
        .operations(package_commit_operations(package, &entry, &manifest))
        .commit_message(format!("Publish {}@{}", entry.id, entry.version))
        .revision(revision.clone())
        .send()
        .await
        .map_err(|error| {
            format!("publish package to `{}`: {error}", entry.repo)
        })?;

    // Phase 2: atomically update the index in the index repository.
    let index_repository = client.dataset(index_owner, index_name);
    let index = read_remote_index(&index_repository, target.revision.as_deref()).await?;
    let index = upsert_entry(index, &entry)?;
    let index_bytes = serde_json::to_vec_pretty(&index).map_err(|error| error.to_string())?;
    index_repository
        .create_commit()
        .operations(vec![CommitOperation::add_bytes(INDEX_PATH, index_bytes)])
        .commit_message(format!("Index {}@{}", entry.id, entry.version))
        .revision(revision)
        .send()
        .await
        .map_err(|error| {
            format!(
                "publish to Hugging Face repository `{}`: {error}",
                target.repo_id
            )
        })?;
    Ok(entry)
}

fn split_repo_id(repo_id: &str) -> Result<(&str, &str)> {
    let (owner, name) = repo_id.split_once('/').ok_or_else(|| {
        format!("Hugging Face repository id must be `owner/name`, got `{repo_id}`")
    })?;
    if owner.is_empty() || name.is_empty() || name.contains('/') {
        return Err(
            format!("Hugging Face repository id must be `owner/name`, got `{repo_id}`").into(),
        );
    }
    Ok((owner, name))
}

async fn read_remote_index(
    repository: &HFRepository<RepoTypeDataset>,
    revision: Option<&str>,
) -> Result<CatalogIndex> {
    let revision = revision.unwrap_or("main");
    let request = repository
        .download_file_to_bytes()
        .filename(INDEX_PATH)
        .revision(revision.to_string());
    match request.send().await {
        Ok(bytes) => {
            let index: CatalogIndex = serde_json::from_slice(&bytes).map_err(|error| {
                format!("parse Hugging Face catalog index `{INDEX_PATH}`: {error}")
            })?;
            index.validate().map_err(|error| {
                format!("invalid Hugging Face catalog index `{INDEX_PATH}`: {error}")
            })?;
            Ok(index)
        }
        Err(HFError::EntryNotFound { .. }) => Ok(CatalogIndex::default()),
        Err(error) => {
            Err(format!("read Hugging Face catalog index `{INDEX_PATH}`: {error}").into())
        }
    }
}

fn package_commit_operations(
    package: &Path,
    entry: &CatalogEntry,
    manifest: &DatasetManifest,
) -> Vec<CommitOperation> {
    let mut operations = vec![CommitOperation::add_file(
        entry.package_manifest_key(),
        package.join(PACKAGE_MANIFEST),
    )];
    for file in &manifest.files {
        operations.push(CommitOperation::add_file(
            format!("{}/{}", entry.package_payload_prefix(), file.path),
            package.join(PAYLOAD_DIR).join(&file.path),
        ));
    }
    operations
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::{BuildOptions, build_package};
    use crate::remote::RemoteCatalog;
    use hf_hub::repository::AddSource;

    #[test]
    fn split_repo_id_requires_owner_and_name() {
        assert_eq!(
            split_repo_id("wjx/autonomics-catalog").unwrap(),
            ("wjx", "autonomics-catalog")
        );
        assert!(split_repo_id("catalog").is_err());
        assert!(split_repo_id("/catalog").is_err());
        assert!(split_repo_id("wjx/").is_err());
        assert!(split_repo_id("a/b/c").is_err());
    }

    #[test]
    fn remote_catalog_hf_construction_is_lazy() {
        let catalog = RemoteCatalog::hf("owner/catalog", None, None).unwrap();
        assert!(
            catalog
                .config()
                .object_key("entries/x")
                .starts_with("entries/")
        );
    }

    #[test]
    fn commit_operations_mirror_catalog_layout() {
        let workspace = tempfile::tempdir().unwrap();
        let input = workspace.path().join("input");
        std::fs::create_dir_all(&input).unwrap();
        std::fs::write(input.join("data.txt"), b"hf-payload").unwrap();
        let package = build_package(
            &input,
            workspace.path().join("package"),
            BuildOptions {
                id: Some("hf.panel".into()),
                version: Some("v1".into()),
                kind: Some("panel".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let manifest = validate_package(&package.path).unwrap();
        let entry = build_entry(&manifest);
        let index = upsert_entry(CatalogIndex::default(), &entry).unwrap();
        let _index_bytes = serde_json::to_vec_pretty(&index).unwrap();

        let operations = package_commit_operations(&package.path, &entry, &manifest);
        assert_eq!(operations.len(), 2);

        match &operations[0] {
            CommitOperation::Add {
                path_in_repo,
                source: AddSource::File(source),
            } => {
                assert_eq!(path_in_repo, &entry.package_manifest_key());
                assert_eq!(source, &package.path.join(PACKAGE_MANIFEST));
            }
            other => panic!("expected manifest add, got {other:?}"),
        }
        match &operations[1] {
            CommitOperation::Add {
                path_in_repo,
                source: AddSource::File(source),
            } => {
                assert_eq!(
                    path_in_repo,
                    &format!("{}/data.txt", entry.package_payload_prefix())
                );
                assert_eq!(source, &package.path.join(PAYLOAD_DIR).join("data.txt"));
            }
            other => panic!("expected payload add, got {other:?}"),
        }
    }
}
