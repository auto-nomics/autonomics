use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::Result;

pub const CATALOG_SCHEMA_VERSION: u8 = 1;
pub const DATASET_SCHEMA_VERSION: u8 = 1;

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
    pub id: String,
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
        validate_id(&self.id)?;
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

    pub fn short_digest(&self) -> Result<&str> {
        self.digest
            .as_deref()
            .and_then(|digest| digest.strip_prefix("sha256:"))
            .ok_or_else(|| "dataset digest is missing".into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Deserialize, Serialize)]
pub struct CatalogEntry {
    pub id: String,
    pub version: String,
    pub kind: String,
    pub digest: String,
    /// Object key/path of the version manifest relative to the catalog backend root.
    pub manifest: String,
    /// Object prefix containing payload files, relative to the catalog backend root.
    pub files: String,
    /// Stable compatibility path, normally `/bundles/<id>`.
    pub vfs_alias: String,
    /// Immutable path, normally `/datasets/<id>@sha256-<digest>`.
    pub vfs_immutable: String,
    pub current: bool,
    pub created_unix_seconds: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Deserialize, Serialize)]
pub struct CatalogIndex {
    #[serde(default = "default_catalog_schema_version")]
    pub schema_version: u8,
    pub generation: u64,
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

        let mut identities = std::collections::BTreeSet::new();
        let mut current_by_id: BTreeMap<String, usize> = BTreeMap::new();
        for (index, entry) in self.entries.iter().enumerate() {
            validate_id(&entry.id)?;
            validate_version(&entry.version)?;
            validate_kind(&entry.kind)?;
            if !is_sha256(&entry.digest) {
                return Err(format!("catalog entry `{}` has invalid digest", entry.id).into());
            }
            validate_object_path(&entry.manifest)?;
            validate_object_path(&entry.files)?;
            validate_absolute_vfs_path(&entry.vfs_alias)?;
            validate_absolute_vfs_path(&entry.vfs_immutable)?;
            if !entry.manifest.ends_with("manifest.json") {
                return Err(format!(
                    "catalog entry `{}` manifest must end with manifest.json",
                    entry.id
                )
                .into());
            }
            if !identities.insert((entry.id.clone(), entry.digest.clone())) {
                return Err(
                    format!("duplicate catalog entry `{}@{}`", entry.id, entry.digest).into(),
                );
            }
            if entry.current {
                if let Some(previous) = current_by_id.insert(entry.id.clone(), index) {
                    return Err(format!(
                        "catalog id `{}` has multiple current entries at indexes {previous} and {index}",
                        entry.id
                    )
                    .into());
                }
            }
        }
        Ok(())
    }

    pub fn current_entries(&self) -> impl Iterator<Item = &CatalogEntry> {
        self.entries.iter().filter(|entry| entry.current)
    }

    pub fn upsert_current(&mut self, entry: CatalogEntry) {
        self.entries
            .retain(|existing| existing.id != entry.id || existing.digest != entry.digest);
        for existing in &mut self.entries {
            if existing.id == entry.id {
                existing.current = false;
            }
        }
        self.entries.push(entry);
        self.entries.sort_by(|left, right| {
            left.id
                .cmp(&right.id)
                .then_with(|| left.created_unix_seconds.cmp(&right.created_unix_seconds))
        });
        self.generation = self.generation.saturating_add(1);
    }
}

pub fn manifest_digest(manifest: &DatasetManifest) -> String {
    let unsigned = DatasetManifest {
        digest: None,
        ..manifest.clone()
    };
    let bytes = serde_json::to_vec(&unsigned).expect("manifest is JSON serializable");
    format!("sha256:{}", hex(&Sha256::digest(&bytes)))
}

pub fn validate_id(value: &str) -> Result<()> {
    valid_token(value, "id")
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

pub fn validate_object_path(value: &str) -> Result<()> {
    validate_relative_path(value.trim_start_matches('/'))?;
    if value.is_empty() || value.contains('\\') || value.contains('\0') {
        return Err(format!("unsafe object path `{value}`").into());
    }
    Ok(())
}

pub fn validate_absolute_vfs_path(value: &str) -> Result<()> {
    if !value.starts_with('/')
        || value.contains('\\')
        || value.contains('\0')
        || value.contains("/../")
        || value.ends_with("/..")
        || value == "/"
    {
        return Err(format!("unsafe VFS path `{value}`").into());
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
            id: "1000g_eur".into(),
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
    fn catalog_rejects_multiple_current_versions() {
        let mut index = CatalogIndex::default();
        for version in ["v1", "v2"] {
            index.upsert_current(CatalogEntry {
                id: "panel".into(),
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
                manifest: format!("entries/panel/{version}/manifest.json"),
                files: format!("entries/panel/{version}/files"),
                vfs_alias: "/bundles/panel".into(),
                vfs_immutable: format!("/datasets/panel@sha256-{version}"),
                current: true,
                created_unix_seconds: 1,
            });
        }
        assert_eq!(index.current_entries().count(), 1);
        assert!(index.validate().is_ok());

        index.entries[0].current = true;
        assert!(index.validate().is_err());
    }
}
