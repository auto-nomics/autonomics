use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::Result;

pub const CATALOG_SCHEMA_VERSION: u8 = 3;
pub const DATASET_SCHEMA_VERSION: u8 = 2;

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Deserialize, Serialize)]
pub struct DatasetFile {
    /// Path relative to the package payload root.
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Deserialize, Serialize)]
pub struct DatasetManifest {
    #[serde(default = "default_dataset_schema_version")]
    pub schema_version: u8,
    /// Hugging Face dataset repository in `owner/name` form. Required and
    /// durable: the catalog uses it as the primary identity for both
    /// indexes and VFS paths. Replaces the legacy `id` field.
    pub repo: String,
    pub version: String,
    pub kind: String,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
    #[serde(default)]
    pub files: Vec<DatasetFile>,
    #[serde(default)]
    pub payload: serde_json::Map<String, serde_json::Value>,
    /// Content digest of the unsigned canonical manifest JSON.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
}

fn default_dataset_schema_version() -> u8 {
    DATASET_SCHEMA_VERSION
}

impl DatasetManifest {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != DATASET_SCHEMA_VERSION {
            return Err(
                format!("unsupported dataset schema version {}", self.schema_version).into(),
            );
        }
        validate_repo_ref(&self.repo)?;
        validate_version(&self.version)?;
        validate_kind(&self.kind)?;
        let digest = self
            .digest
            .as_deref()
            .ok_or_else(|| "digest is missing".to_string())?;
        if !is_sha256(digest) {
            return Err(format!("invalid manifest digest `{digest}`").into());
        }
        if self.digest.as_deref() != Some(&manifest_digest(self)) {
            return Err("manifest digest does not match its canonical content".into());
        }

        let mut paths = std::collections::BTreeSet::new();
        for file in &self.files {
            validate_relative_path(&file.path)?;
            if !is_sha256(&file.sha256) {
                return Err(format!("invalid sha256 for `{}`", file.path).into());
            }
            if !paths.insert(file.path.clone()) {
                return Err(format!("duplicate payload file `{}`", file.path).into());
            }
        }
        if self.files.is_empty() {
            return Err("dataset package must contain at least one payload file".into());
        }
        Ok(())
    }
}

/// One immutable dataset version listed by a catalog index.
///
/// Each entry is identified by its Hugging Face repo (`owner/name`); that
/// field is the catalog's primary key. The repository namespace guarantees
/// global uniqueness across owners: two packages with identical short names
/// under different owners remain distinct entries.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Deserialize, Serialize)]
pub struct CatalogEntry {
    /// Hugging Face dataset repository in `owner/name` form. The durable
    /// primary identity of the entry.
    pub repo: String,
    pub version: String,
    pub kind: String,
    /// Canonical manifest digest, including the `sha256:` prefix.
    pub digest: String,
    pub current: bool,
    pub created_unix_seconds: i64,
}

impl CatalogEntry {
    /// Digest without the `sha256:` prefix, as used in package and VFS paths.
    pub fn short_digest(&self) -> &str {
        self.digest.strip_prefix("sha256:").unwrap_or(&self.digest)
    }

    /// Content-addressed root within a per-package HF repository:
    /// `<version>/sha256-<digest>`.
    pub fn package_root(&self) -> String {
        format!("{}/sha256-{}", self.version, self.short_digest())
    }

    pub fn package_manifest_key(&self) -> String {
        format!("{}/manifest.json", self.package_root())
    }

    pub fn package_payload_prefix(&self) -> String {
        self.package_root()
    }

    /// Full ObjectSource key for the manifest, routing to the correct repo.
    pub fn source_manifest_key(&self) -> String {
        format!("{}/{}", self.repo, self.package_manifest_key())
    }

    /// Full ObjectSource key for a payload file, routing to the correct repo.
    pub fn source_payload_path(&self, relative: &str) -> String {
        format!(
            "{}/{}/{}",
            self.repo,
            self.package_payload_prefix(),
            relative
        )
    }

    /// Stable mutable alias: `/bundles/<owner>/<name>`.
    pub fn vfs_alias(&self) -> String {
        format!("/bundles/{}", self.repo)
    }

    /// Immutable versioned path: `/datasets/<owner>/<name>@sha256-<digest>`.
    pub fn vfs_immutable(&self) -> String {
        format!("/datasets/{}@sha256-{}", self.repo, self.short_digest())
    }

    /// Local cache directory name: `<owner>/<name>@<digest>`.
    ///
    /// The owner part forms a subdirectory so multiple owners can coexist on
    /// disk without any package ever sharing its parent path with another
    /// package from the same owner.
    pub fn cache_dir_name(&self) -> String {
        format!("{}@{}", self.repo, self.digest)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Deserialize, Serialize)]
pub struct CatalogIndex {
    #[serde(default = "default_catalog_schema_version")]
    pub schema_version: u8,
    pub generation: u64,
    /// Package repository references in `owner/name` form.
    ///
    /// A registry index normally contains only this list. A package-local
    /// index contains resolved `entries` instead. Local cache keeps both: the
    /// repository list is the user-facing dependency declaration, while entries
    /// are the resolved, verified installations used by the runtime.
    #[serde(default)]
    pub repositories: Vec<String>,
    #[serde(default)]
    pub entries: Vec<CatalogEntry>,
}

fn default_catalog_schema_version() -> u8 {
    CATALOG_SCHEMA_VERSION
}

impl Default for CatalogIndex {
    fn default() -> Self {
        Self {
            schema_version: CATALOG_SCHEMA_VERSION,
            generation: 1,
            repositories: Vec::new(),
            entries: Vec::new(),
        }
    }
}

impl CatalogIndex {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != CATALOG_SCHEMA_VERSION {
            return Err(
                format!("unsupported catalog schema version {}", self.schema_version).into(),
            );
        }
        if self.generation == 0 {
            return Err("catalog generation must be greater than zero".into());
        }

        let mut repositories = std::collections::BTreeSet::new();
        for repository in &self.repositories {
            let (owner, name) = repository
                .split_once('/')
                .ok_or_else(|| format!("catalog repository `{repository}` must be `owner/name`"))?;
            if owner.is_empty() || name.is_empty() || name.contains('/') {
                return Err(
                    format!("catalog repository `{repository}` must be `owner/name`").into(),
                );
            }
            if !repositories.insert(repository.clone()) {
                return Err(format!("duplicate catalog repository `{repository}`").into());
            }
        }

        let mut identities = std::collections::BTreeSet::new();
        let mut current_by_repo: BTreeMap<String, usize> = BTreeMap::new();
        for (index, entry) in self.entries.iter().enumerate() {
            validate_repo_ref(&entry.repo)?;
            validate_version(&entry.version)?;
            validate_kind(&entry.kind)?;
            if !is_sha256(&entry.digest) {
                return Err(format!(
                    "catalog entry `{}/{}` has invalid digest",
                    entry.repo, entry.version
                )
                .into());
            }
            if !identities.insert((entry.repo.clone(), entry.digest.clone())) {
                return Err(format!(
                    "duplicate catalog entry `{}@{}`",
                    entry.repo, entry.digest
                )
                .into());
            }
            if entry.current {
                if let Some(previous) = current_by_repo.insert(entry.repo.clone(), index) {
                    return Err(format!(
                        "catalog repo `{}` has multiple current entries at indexes {previous} and {index}",
                        entry.repo
                    )
                    .into());
                }
            }
        }
        Ok(())
    }

    pub fn record_repository(&mut self, repository: &str) {
        if !self.repositories.iter().any(|value| value == repository) {
            self.repositories.push(repository.to_string());
        }
    }

    pub fn current_entries(&self) -> impl Iterator<Item = &CatalogEntry> {
        self.entries.iter().filter(|entry| entry.current)
    }

    /// Select the single entry matched by HF repo and optional version/digest.
    pub fn select(
        &self,
        repo: &str,
        version: Option<&str>,
        digest: Option<&str>,
    ) -> Result<CatalogEntry> {
        let mut matched = self
            .entries
            .iter()
            .filter(|entry| entry.repo == repo)
            .filter(|entry| version.is_none_or(|value| entry.version == value))
            .filter(|entry| digest.is_none_or(|value| entry.digest == value))
            .cloned()
            .collect::<Vec<_>>();
        match matched.len() {
            0 => Err(format!("catalog dataset `{repo}` was not found").into()),
            1 => Ok(matched.remove(0)),
            _ => {
                matched.retain(|entry| entry.current);
                match matched.len() {
                    1 => Ok(matched.remove(0)),
                    0 => Err(format!(
                        "catalog dataset `{repo}` has multiple versions; specify version or digest"
                    )
                    .into()),
                    _ => Err(format!("catalog dataset `{repo}` has multiple current entries").into()),
                }
            }
        }
    }

    /// Select the sole current entry, typically from a package-local index.
    pub fn select_current(&self) -> Result<CatalogEntry> {
        let mut current = self.current_entries();
        let entry = current
            .next()
            .ok_or_else(|| "catalog has no current entry".to_string())?;
        if current.next().is_some() {
            return Err("catalog has multiple current entries".into());
        }
        Ok(entry.clone())
    }

    /// Find the current entry whose Hugging Face repo is `repo`.
    ///
    /// `repo` is the canonical `owner/name` identifier of the per-package
    /// dataset repository. Multiple historical versions in the same repo are
    /// allowed; only the current one is returned.
    pub fn find_current_by_repo(&self, repo: &str) -> Option<&CatalogEntry> {
        self.current_entries().find(|entry| entry.repo == repo)
    }

    /// Select the current entry for an HF repo.
    pub fn select_by_repo(&self, repo: &str) -> Result<CatalogEntry> {
        self.find_current_by_repo(repo)
            .cloned()
            .ok_or_else(|| format!("catalog has no current entry for HF repo `{repo}`").into())
    }

    /// Search current entries by free-text terms over their identity fields.
    pub fn search(&self, query: &str, kind: Option<&str>, limit: usize) -> Vec<CatalogEntry> {
        self.search_entries(self.current_entries(), query, kind, limit)
    }

    /// Search every entry, including historical versions.
    pub fn search_all(&self, query: &str, kind: Option<&str>, limit: usize) -> Vec<CatalogEntry> {
        self.search_entries(self.entries.iter(), query, kind, limit)
    }

    fn search_entries<'a>(
        &self,
        entries: impl Iterator<Item = &'a CatalogEntry>,
        query: &str,
        kind: Option<&str>,
        limit: usize,
    ) -> Vec<CatalogEntry> {
        let terms = query
            .split_whitespace()
            .map(str::to_ascii_lowercase)
            .collect::<Vec<_>>();
        let mut matched = entries
            .filter(|entry| kind.is_none_or(|value| entry.kind == value))
            .filter(|entry| {
                let haystack = format!(
                    "{} {} {} {} {}",
                    entry.repo,
                    entry.version,
                    entry.kind,
                    entry.digest,
                    entry.short_digest()
                )
                .to_ascii_lowercase();
                terms.iter().all(|term| haystack.contains(term))
            })
            .take(limit)
            .cloned()
            .collect::<Vec<_>>();
        matched.sort_by(|left, right| {
            left.repo
                .cmp(&right.repo)
                .then_with(|| right.created_unix_seconds.cmp(&left.created_unix_seconds))
        });
        matched
    }

    pub fn upsert_current(&mut self, entry: CatalogEntry) {
        self.record_repository(&entry.repo);
        self.schema_version = CATALOG_SCHEMA_VERSION;
        self.entries
            .retain(|existing| existing.repo != entry.repo || existing.digest != entry.digest);
        for existing in &mut self.entries {
            if existing.repo == entry.repo {
                existing.current = false;
            }
        }
        self.entries.push(entry);
        self.entries.sort_by(|left, right| {
            left.repo
                .cmp(&right.repo)
                .then_with(|| left.created_unix_seconds.cmp(&right.created_unix_seconds))
        });
        self.generation = self.generation.saturating_add(1);
    }
}

pub fn validate_repo_ref(value: &str) -> Result<()> {
    let (owner, name) = value
        .split_once('/')
        .ok_or_else(|| format!("catalog repository `{value}` must be `owner/name`"))?;
    if owner.is_empty() || name.is_empty() || name.contains('/') {
        return Err(format!("catalog repository `{value}` must be `owner/name`").into());
    }
    Ok(())
}

pub fn manifest_digest(manifest: &DatasetManifest) -> String {
    let unsigned = DatasetManifest {
        digest: None,
        ..manifest.clone()
    };
    let bytes = serde_json::to_vec(&unsigned).expect("manifest is JSON serializable");
    format!("sha256:{}", hex(&Sha256::digest(&bytes)))
}

pub fn validate_version(value: &str) -> Result<()> {
    valid_token(value, "version")
}

pub fn validate_kind(value: &str) -> Result<()> {
    if value.is_empty()
        || !value.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
        })
    {
        return Err(format!("kind `{value}` must use lowercase snake_case").into());
    }
    Ok(())
}

fn valid_token(value: &str, field: &str) -> Result<()> {
    let mut characters = value.chars();
    let valid = characters
        .next()
        .is_some_and(|character| character.is_ascii_alphanumeric())
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
        })
        && value.len() <= 128;
    if !valid {
        return Err(format!("invalid {field} `{value}`").into());
    }
    Ok(())
}

pub fn validate_relative_path(value: &str) -> Result<()> {
    let path = std::path::Path::new(value);
    if value.is_empty()
        || value.contains('\\')
        || value.contains('\0')
        || path.is_absolute()
        || !path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
    {
        return Err(format!("unsafe relative path `{value}`").into());
    }
    Ok(())
}

pub fn is_sha256(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> DatasetManifest {
        DatasetManifest {
            schema_version: DATASET_SCHEMA_VERSION,
            repo: "wjixiang/catalog-1000g-eur".into(),
            version: "v3".into(),
            kind: "vcf".into(),
            metadata: BTreeMap::new(),
            files: vec![DatasetFile {
                path: "chr22.vcf.gz".into(),
                size: 3,
                sha256: format!("sha256:{}", "a".repeat(64)),
            }],
            payload: serde_json::Map::new(),
            digest: None,
        }
    }

    #[test]
    fn manifest_digest_is_canonical_and_verifiable() {
        let mut value = manifest();
        value.digest = Some(manifest_digest(&value));
        assert!(value.validate().is_ok());

        let mut tampered = value.clone();
        tampered.kind = "plink".into();
        assert!(tampered.validate().is_err());
    }

    #[test]
    fn catalog_rejects_multiple_current_versions_in_same_repo() {
        let mut index = CatalogIndex::default();
        for version in ["v1", "v2"] {
            index.upsert_current(CatalogEntry {
                repo: "owner/catalog-panel".into(),
                version: version.into(),
                kind: "plink".into(),
                digest: format!(
                    "sha256:{}",
                    if version == "v1" {
                        "a".repeat(64)
                    } else {
                        "b".repeat(64)
                    }
                ),
                current: true,
                created_unix_seconds: 1,
            });
        }
        assert_eq!(index.current_entries().count(), 1);
        assert!(index.validate().is_ok());

        index.entries[0].current = true;
        assert!(index.validate().is_err());
    }

    #[test]
    fn entry_paths_follow_the_layout_convention() {
        let entry = CatalogEntry {
            repo: "wjixiang/catalog-panel".into(),
            version: "v1".into(),
            kind: "plink".into(),
            digest: format!("sha256:{}", "a".repeat(64)),
            current: true,
            created_unix_seconds: 1,
        };
        assert_eq!(entry.short_digest(), "a".repeat(64));
        assert_eq!(
            entry.package_root(),
            format!("v1/sha256-{}", "a".repeat(64))
        );
        assert_eq!(
            entry.source_manifest_key(),
            format!(
                "wjixiang/catalog-panel/v1/sha256-{}/manifest.json",
                "a".repeat(64)
            )
        );
        assert_eq!(
            entry.source_payload_path("data.txt"),
            format!(
                "wjixiang/catalog-panel/v1/sha256-{}/data.txt",
                "a".repeat(64)
            )
        );
        assert_eq!(entry.vfs_alias(), "/bundles/wjixiang/catalog-panel");
        assert_eq!(
            entry.vfs_immutable(),
            format!(
                "/datasets/wjixiang/catalog-panel@sha256-{}",
                "a".repeat(64)
            )
        );
        assert_eq!(
            entry.cache_dir_name(),
            format!("wjixiang/catalog-panel@sha256:{}", "a".repeat(64))
        );
    }

    #[test]
    fn select_and_search_current_entries() {
        let mut index = CatalogIndex::default();
        index.upsert_current(CatalogEntry {
            repo: "wjixiang/catalog-panel-eur".into(),
            version: "v1".into(),
            kind: "panel".into(),
            digest: format!("sha256:{}", "a".repeat(64)),
            current: true,
            created_unix_seconds: 1,
        });
        index.upsert_current(CatalogEntry {
            repo: "wjixiang/catalog-panel-afr".into(),
            version: "v2".into(),
            kind: "panel".into(),
            digest: format!("sha256:{}", "b".repeat(64)),
            current: true,
            created_unix_seconds: 2,
        });

        let selected = index
            .select("wjixiang/catalog-panel-afr", None, None)
            .unwrap();
        assert_eq!(selected.version, "v2");
        assert!(index.select("missing", None, None).is_err());
        let found = index.search("catalog-panel-afr", Some("panel"), 10);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].repo, "wjixiang/catalog-panel-afr");
        assert!(index.search("missing", None, 10).is_empty());

        index.upsert_current(CatalogEntry {
            repo: "wjixiang/catalog-panel-eur".into(),
            version: "v2".into(),
            kind: "panel".into(),
            digest: format!("sha256:{}", "c".repeat(64)),
            current: true,
            created_unix_seconds: 3,
        });
        assert_eq!(index.search("catalog-panel-eur", None, 10).len(), 1);
        assert_eq!(
            index.search_all("catalog-panel-eur", None, 10).len(),
            2,
            "search_all includes historical versions"
        );
    }

    #[test]
    fn lookup_by_hf_repo_resolves_to_current_entry() {
        let mut index = CatalogIndex::default();
        index.upsert_current(CatalogEntry {
            repo: "wjixiang/catalog-panel-eur".into(),
            version: "v2".into(),
            kind: "panel".into(),
            digest: format!("sha256:{}", "c".repeat(64)),
            current: true,
            created_unix_seconds: 3,
        });
        // Older version shares the same repo but is no longer current.
        index.entries.push(CatalogEntry {
            repo: "wjixiang/catalog-panel-eur".into(),
            version: "v1".into(),
            kind: "panel".into(),
            digest: format!("sha256:{}", "a".repeat(64)),
            current: false,
            created_unix_seconds: 1,
        });

        let found = index
            .find_current_by_repo("wjixiang/catalog-panel-eur")
            .expect("repo present");
        assert_eq!(found.version, "v2");
        assert_eq!(found.digest, format!("sha256:{}", "c".repeat(64)));
        assert!(index.find_current_by_repo("wjixiang/missing").is_none());

        let selected = index
            .select_by_repo("wjixiang/catalog-panel-eur")
            .unwrap();
        assert_eq!(selected.version, "v2");
        assert!(index.select_by_repo("wjixiang/missing").is_err());
    }
}
