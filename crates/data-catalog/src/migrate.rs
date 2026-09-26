//! Read-side and one-shot remote migration of legacy catalog formats.
//!
//! Two legacy layouts are handled:
//!
//! - **Index schema v2 → v3**: v2 indexes identified packages by a legacy
//!   `id` field, with `repo` only optionally populated. v3 makes the
//!   Hugging Face repo (`owner/name`) the sole identity. Indexes are parsed
//!   with both layouts accepted and upgraded in memory before
//!   [`CatalogIndex::validate`] runs, so every read path — the local cache
//!   (`local`), the remote reader (`remote`), and the publish flow (`hf`) —
//!   accepts indexes written before the migration.
//! - **Manifest schema v1 → v2**: v1 manifests carried a legacy `id` and no
//!   `repo`. Because the manifest `digest` is a hash over the manifest's
//!   canonical serialization, a v1 manifest cannot be upgraded in memory
//!   while keeping its digest verifiable. v1 manifests are therefore only
//!   handled by the one-shot remote migration
//!   ([`crate::hf::migrate_package_repository`]): the payload is re-committed
//!   under the new digest's content-addressed path and the index is rewritten.
//!
//! Entries whose `repo` is missing or empty are recovered from the legacy
//! `id` via [`crate::hf::package_repo_id`] and a package prefix (`owner/name`
//! without the trailing `-<sanitized-id>` part). Callers supply that prefix;
//! the remote reader derives it from its registry repository via
//! [`crate::hf::package_repo_prefix_for_index`].

use crate::error::{Error, Result};
use crate::hf::package_repo_id;
use crate::model::{CatalogEntry, CatalogIndex, DatasetManifest, HfRepoId, manifest_digest};

/// Indexed view of [`CatalogIndex`] that accepts v2 payloads on read.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct RawCatalogIndex {
    #[serde(default = "default_schema")]
    schema_version: u8,
    #[serde(default)]
    generation: u64,
    #[serde(default)]
    repositories: Vec<String>,
    #[serde(default)]
    entries: Vec<RawCatalogEntry>,
}

#[derive(Debug, serde::Deserialize)]
pub(crate) struct RawCatalogEntry {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    repo: Option<String>,
    #[serde(default)]
    version: String,
    #[serde(default)]
    kind: String,
    digest: String,
    #[serde(default)]
    current: bool,
    #[serde(default)]
    created_unix_seconds: i64,
}

fn default_schema() -> u8 {
    3
}

impl RawCatalogIndex {
    /// Schema version as stored in the payload, before any upgrade.
    pub(crate) fn schema_version(&self) -> u8 {
        self.schema_version
    }

    pub(crate) fn into_v3(self, prefix: Option<&str>) -> Result<CatalogIndex> {
        if self.schema_version == 3 {
            // No id field any more; the v3 struct deserializes fine from
            // a payload that may have been written by older code paths.
            let entries = self
                .entries
                .into_iter()
                .map(|raw| raw.into_v3_entry())
                .collect::<Result<Vec<_>>>()?;
            let repositories = self
                .repositories
                .into_iter()
                .map(|repository| HfRepoId::new(&repository).map_err(Error::from))
                .collect::<Result<Vec<_>>>()?;
            return Ok(CatalogIndex {
                schema_version: 3,
                generation: self.generation,
                repositories,
                entries,
            });
        }
        if self.schema_version == 2 {
            let entries = self
                .entries
                .into_iter()
                .map(|raw| raw.into_v3_entry_from_v2(prefix))
                .collect::<Result<Vec<_>>>()?;
            let repositories = self
                .repositories
                .into_iter()
                .map(|repository| HfRepoId::new(&repository).map_err(Error::from))
                .collect::<Result<Vec<_>>>()?;
            return Ok(CatalogIndex {
                schema_version: 3,
                generation: self.generation,
                repositories,
                entries,
            });
        }
        Err(format!("unsupported catalog schema version {}", self.schema_version).into())
    }
}

impl RawCatalogEntry {
    fn into_v3_entry(self) -> Result<crate::model::CatalogEntry> {
        let repo = self
            .repo
            .ok_or_else(|| "catalog entry is missing required `repo`".to_string())?;
        if repo.is_empty() {
            return Err("catalog entry has empty `repo`".into());
        }
        let repo = HfRepoId::new(&repo).map_err(Error::from)?;
        Ok(crate::model::CatalogEntry {
            repo,
            version: self.version,
            kind: self.kind,
            digest: self.digest,
            current: self.current,
            created_unix_seconds: self.created_unix_seconds,
        })
    }

    fn into_v3_entry_from_v2(self, prefix: Option<&str>) -> Result<crate::model::CatalogEntry> {
        let repo = if let Some(repo) = self.repo.filter(|value| !value.is_empty()) {
            repo
        } else {
            let id = self.id.ok_or_else(|| {
                "v2 catalog entry is missing both `id` and `repo`; cannot migrate".to_string()
            })?;
            let prefix = prefix.ok_or_else(|| {
                "v2 catalog entry has empty `repo` and no prefix is configured to derive one"
                    .to_string()
            })?;
            package_repo_id(prefix, &id)
        };
        let repo = HfRepoId::new(&repo).map_err(Error::from)?;
        Ok(crate::model::CatalogEntry {
            repo,
            version: self.version,
            kind: self.kind,
            digest: self.digest,
            current: self.current,
            created_unix_seconds: self.created_unix_seconds,
        })
    }
}

/// Manifest view that accepts v1 payloads (legacy `id`, no `repo`) on read.
///
/// Unlike index migration, v1 manifests cannot be upgraded transparently:
/// the digest hashes the manifest's own canonical bytes, so rewriting the
/// shape changes the digest and with it the content-addressed path. Use
/// [`plan_entry_migration`] to compute the new manifest and entry pair that
/// [`crate::hf::migrate_package_repository`] commits remotely.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct RawDatasetManifest {
    #[serde(default = "default_manifest_schema")]
    schema_version: u8,
    /// Legacy v1 identity; accepted for tolerance, never read — the
    /// containing repository is the authoritative `repo`.
    #[serde(default)]
    #[allow(dead_code)]
    id: Option<String>,
    #[serde(default)]
    repo: Option<String>,
    #[serde(default)]
    version: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    metadata: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    files: Vec<crate::model::DatasetFile>,
    #[serde(default)]
    payload: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    digest: Option<String>,
}

fn default_manifest_schema() -> u8 {
    1
}

impl RawDatasetManifest {
    fn into_v2(self, repo: &str) -> Result<DatasetManifest> {
        if let Some(manifest) = self.as_current(repo)? {
            return Ok(manifest);
        }
        if self.schema_version != 1 {
            return Err(
                format!("unsupported dataset schema version {}", self.schema_version).into(),
            );
        }
        // v1: the containing repository is the authoritative `repo`; a
        // populated legacy `repo` field that disagrees is a red flag.
        if let Some(legacy_repo) = self.repo.as_deref().filter(|value| !value.is_empty()) {
            if legacy_repo != repo {
                return Err(format!(
                    "v1 manifest declares repo `{legacy_repo}` but lives in `{repo}`"
                )
                .into());
            }
        }
        let mut manifest = DatasetManifest {
            schema_version: crate::model::DATASET_SCHEMA_VERSION,
            repo: HfRepoId::new(repo).map_err(Error::from)?,
            version: self.version,
            kind: self.kind,
            metadata: self.metadata,
            files: self.files,
            payload: self.payload,
            digest: None,
        };
        manifest.digest = Some(manifest_digest(&manifest));
        manifest.validate()?;
        Ok(manifest)
    }

    /// A manifest that is already current-format passes through untouched
    /// (digest included); `None` means the payload is legacy v1.
    fn as_current(&self, repo: &str) -> Result<Option<DatasetManifest>> {
        if self.schema_version != crate::model::DATASET_SCHEMA_VERSION {
            return Ok(None);
        }
        let manifest_repo = self
            .repo
            .as_deref()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "v2 manifest is missing required `repo`".to_string())?;
        if manifest_repo != repo {
            return Err(
                format!("manifest declares repo `{manifest_repo}` but lives in `{repo}`").into(),
            );
        }
        let manifest = DatasetManifest {
            schema_version: self.schema_version,
            repo: HfRepoId::new(manifest_repo).map_err(Error::from)?,
            version: self.version.clone(),
            kind: self.kind.clone(),
            metadata: self.metadata.clone(),
            files: self.files.clone(),
            payload: self.payload.clone(),
            digest: self.digest.clone(),
        };
        manifest.validate()?;
        Ok(Some(manifest))
    }
}

/// One entry's computed transition from a legacy layout to the current one.
#[derive(Debug)]
pub(crate) struct PlannedEntryMigration {
    /// Entry rewritten with the migrated manifest's digest (unchanged when
    /// `changed` is `false`).
    pub new_entry: CatalogEntry,
    /// Current-format manifest for the entry.
    pub new_manifest: DatasetManifest,
    /// Digest the entry had before migration, for reporting.
    pub from_digest: String,
    /// Manifest key of the legacy layout, for cleanup deletes.
    pub old_manifest_key: String,
    /// Payload path prefix of the legacy layout, for cleanup deletes.
    pub old_payload_prefix: String,
    /// Whether the remote layout must change (`false` = already current).
    pub changed: bool,
}

/// Compute how one index entry and its manifest move to the current format.
///
/// The file list is identical between old and new manifests — migration only
/// touches identity fields and the digest — so callers pair
/// [`PlannedEntryMigration::old_payload_prefix`] with `new_manifest.files`
/// to derive the per-file old/new repo keys.
pub(crate) fn plan_entry_migration(
    repo: &str,
    entry: CatalogEntry,
    raw: RawDatasetManifest,
) -> Result<PlannedEntryMigration> {
    let manifest = raw.into_v2(repo)?;
    let (new_entry, changed) = if manifest.digest.as_deref() == Some(entry.digest.as_str()) {
        (entry.clone(), false)
    } else {
        let digest = manifest.digest.clone().unwrap_or_default();
        (
            CatalogEntry {
                digest,
                ..entry.clone()
            },
            true,
        )
    };
    Ok(PlannedEntryMigration {
        new_entry,
        new_manifest: manifest,
        from_digest: entry.digest.clone(),
        old_manifest_key: entry.package_manifest_key(),
        old_payload_prefix: entry.package_payload_prefix(),
        changed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index_json(schema_version: u8, entry: &str) -> String {
        format!(
            r#"{{
                "schema_version": {schema_version},
                "generation": 1,
                "repositories": [],
                "entries": [{entry}]
            }}"#
        )
    }

    const DIGEST: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    /// Extra entry fields every fixture needs so `CatalogIndex::validate`
    /// passes after migration (version and kind default to empty strings).
    const ENTRY_TAIL: &str = r#", "version": "v1", "kind": "panel""#;

    #[test]
    fn v2_entry_with_repo_passes_through() {
        let entry = format!(
            r#"{{ "id": "panel.a", "repo": "owner/catalog-panel-a", "digest": "{DIGEST}"{ENTRY_TAIL} }}"#
        );
        let raw: RawCatalogIndex = serde_json::from_str(&index_json(2, &entry)).unwrap();
        let index = raw.into_v3(None).unwrap();
        assert_eq!(index.schema_version, 3);
        assert_eq!(index.entries[0].repo.as_str(), "owner/catalog-panel-a");
        index.validate().unwrap();
    }

    #[test]
    fn v2_entry_without_repo_uses_prefix_and_legacy_id() {
        let entry = format!(r#"{{ "id": "hdl.ref.ukb_eur", "digest": "{DIGEST}"{ENTRY_TAIL} }}"#);
        let raw: RawCatalogIndex = serde_json::from_str(&index_json(2, &entry)).unwrap();
        let index = raw.into_v3(Some("wjixiang/catalog")).unwrap();
        assert_eq!(
            index.entries[0].repo.as_str(),
            "wjixiang/catalog-hdl-ref-ukb-eur"
        );
        index.validate().unwrap();
    }

    #[test]
    fn v2_entry_without_repo_and_prefix_is_rejected() {
        let entry = format!(r#"{{ "id": "panel.a", "digest": "{DIGEST}"{ENTRY_TAIL} }}"#);
        let raw: RawCatalogIndex = serde_json::from_str(&index_json(2, &entry)).unwrap();
        let error = raw.into_v3(None).unwrap_err().to_string();
        assert!(error.contains("no prefix is configured"), "{error}");
    }

    #[test]
    fn v2_entry_without_id_and_repo_is_rejected() {
        let entry = format!(r#"{{ "digest": "{DIGEST}"{ENTRY_TAIL} }}"#);
        let raw: RawCatalogIndex = serde_json::from_str(&index_json(2, &entry)).unwrap();
        let error = raw
            .into_v3(Some("wjixiang/catalog"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("missing both `id` and `repo`"), "{error}");
    }

    #[test]
    fn v3_payload_passes_through_without_repo_rewrite() {
        let entry =
            format!(r#"{{ "repo": "owner/catalog-panel-a", "digest": "{DIGEST}"{ENTRY_TAIL} }}"#);
        let raw: RawCatalogIndex = serde_json::from_str(&index_json(3, &entry)).unwrap();
        let index = raw.into_v3(None).unwrap();
        assert_eq!(index.entries[0].repo.as_str(), "owner/catalog-panel-a");
        index.validate().unwrap();
    }

    #[test]
    fn unknown_schema_version_is_rejected() {
        let entry =
            format!(r#"{{ "repo": "owner/catalog-panel-a", "digest": "{DIGEST}"{ENTRY_TAIL} }}"#);
        let raw: RawCatalogIndex = serde_json::from_str(&index_json(4, &entry)).unwrap();
        let error = raw.into_v3(None).unwrap_err().to_string();
        assert!(
            error.contains("unsupported catalog schema version 4"),
            "{error}"
        );
    }

    fn v1_manifest_bytes() -> Vec<u8> {
        serde_json::json!({
            "schema_version": 1,
            "id": "plink.ref.1000g_eur.binary",
            "version": "v1",
            "kind": "plink_ref_binary",
            "files": [{ "path": "panel.bed", "size": 3, "sha256": DIGEST }],
        })
        .to_string()
        .into_bytes()
    }

    fn entry_with(digest: &str) -> CatalogEntry {
        CatalogEntry {
            repo: HfRepoId::new("owner/catalog-plink-ref").unwrap(),
            version: "v1".into(),
            kind: "plink_ref_binary".into(),
            digest: digest.into(),
            current: true,
            created_unix_seconds: 1789930111,
        }
    }

    #[test]
    fn v1_manifest_migrates_with_recomputed_verifiable_digest() {
        let raw: RawDatasetManifest = serde_json::from_slice(&v1_manifest_bytes()).unwrap();
        let old_digest = format!("sha256:{}", "c".repeat(64));
        let plan =
            plan_entry_migration("owner/catalog-plink-ref", entry_with(&old_digest), raw).unwrap();

        assert!(plan.changed);
        assert_eq!(plan.from_digest, old_digest);
        assert_eq!(
            plan.new_entry.digest,
            plan.new_manifest.digest.clone().unwrap()
        );
        assert_eq!(plan.new_manifest.repo.as_str(), "owner/catalog-plink-ref");
        assert_eq!(
            plan.new_manifest.schema_version,
            crate::model::DATASET_SCHEMA_VERSION
        );
        // The rewritten manifest must validate, including its own digest.
        plan.new_manifest.validate().unwrap();
        // Content-addressed path moves to the new digest.
        assert_eq!(
            plan.old_payload_prefix,
            format!("v1/sha256-{}", old_digest.strip_prefix("sha256:").unwrap())
        );
        assert_ne!(
            plan.old_payload_prefix,
            plan.new_entry.package_payload_prefix()
        );
    }

    #[test]
    fn current_manifest_and_matching_entry_is_a_no_op_plan() {
        let raw: RawDatasetManifest = serde_json::from_slice(&v1_manifest_bytes()).unwrap();
        let migrated = raw.into_v2("owner/catalog-plink-ref").unwrap();
        let digest = migrated.digest.clone().unwrap();
        // Re-parse the migrated (current-format) manifest.
        let bytes = serde_json::to_vec(&migrated).unwrap();
        let raw: RawDatasetManifest = serde_json::from_slice(&bytes).unwrap();
        let plan =
            plan_entry_migration("owner/catalog-plink-ref", entry_with(&digest), raw).unwrap();

        assert!(!plan.changed);
        assert_eq!(plan.new_entry.digest, digest);
        assert_eq!(plan.old_manifest_key, plan.new_entry.package_manifest_key());
    }

    #[test]
    fn v2_manifest_in_the_wrong_repo_is_rejected() {
        let raw: RawDatasetManifest = serde_json::from_slice(&v1_manifest_bytes()).unwrap();
        let migrated = raw.into_v2("owner/catalog-plink-ref").unwrap();
        let bytes = serde_json::to_vec(&migrated).unwrap();
        let raw: RawDatasetManifest = serde_json::from_slice(&bytes).unwrap();
        let error = raw.into_v2("owner/somewhere-else").unwrap_err().to_string();
        assert!(error.contains("lives in"), "{error}");
    }

    #[test]
    fn v1_manifest_declaring_a_foreign_repo_is_rejected() {
        let mut value: serde_json::Value = serde_json::from_slice(&v1_manifest_bytes()).unwrap();
        value["repo"] = "owner/other".into();
        let raw: RawDatasetManifest =
            serde_json::from_slice(&serde_json::to_vec(&value).unwrap()).unwrap();
        let error = raw
            .into_v2("owner/catalog-plink-ref")
            .unwrap_err()
            .to_string();
        assert!(error.contains("lives in"), "{error}");
    }

    #[test]
    fn unknown_manifest_schema_version_is_rejected() {
        let mut value: serde_json::Value = serde_json::from_slice(&v1_manifest_bytes()).unwrap();
        value["schema_version"] = 3.into();
        let raw: RawDatasetManifest =
            serde_json::from_slice(&serde_json::to_vec(&value).unwrap()).unwrap();
        let error = raw
            .into_v2("owner/catalog-plink-ref")
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("unsupported dataset schema version 3"),
            "{error}"
        );
    }
}
