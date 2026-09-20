//! Backend-agnostic helpers shared by the object-storage catalog services.
//!
//! The Hugging Face and S3 backends both serve the same logical catalog
//! layout: a root `CatalogIndex` plus a manifest per entry. The only thing
//! that differs between them is the [`opendal::Operator`] used to address
//! the bytes, so the manifest IO, validation, search, and entry selection
//! helpers all live here.

use opendal::Operator;

use crate::config::CatalogConfig;
use crate::error::Result;
use crate::model::{CatalogEntry, DatasetManifest};
use crate::storage::read_json_object;

use super::model::CatalogRecord;

/// Read and validate a dataset manifest stored relative to the catalog root.
pub(crate) async fn read_manifest(
    operator: &Operator,
    config: &CatalogConfig,
    relative_key: &str,
) -> Result<DatasetManifest> {
    let key = config.object_key(relative_key);
    let manifest: DatasetManifest = read_json_object(operator, &key).await?;
    manifest
        .validate()
        .map_err(|error| format!("invalid manifest `{key}`: {error}"))?;
    Ok(manifest)
}

/// Confirm a manifest's identifying fields agree with its catalog entry.
pub(crate) fn validate_entry_manifest(
    entry: &CatalogEntry,
    manifest: &DatasetManifest,
) -> Result<()> {
    if manifest.id != entry.id
        || manifest.version != entry.version
        || manifest.kind != entry.kind
        || manifest.digest.as_deref() != Some(entry.digest.as_str())
    {
        return Err(format!("catalog entry `{}` does not match its manifest", entry.id).into());
    }
    Ok(())
}

/// Build the lower-cased haystack used for free-text matching.
pub(crate) fn search_haystack(record: &CatalogRecord) -> String {
    let mut text = format!(
        "{} {} {} {} {} {} ",
        record.id,
        record.version,
        record.kind,
        record.digest,
        record.vfs_alias,
        record.vfs_immutable
    );
    if let Some(description) = &record.description {
        text.push_str(description);
        text.push(' ');
    }
    text.push_str(&record.tags.join(" "));
    text.push(' ');
    for (key, value) in &record.metadata {
        text.push_str(key);
        text.push(' ');
        text.push_str(value);
        text.push(' ');
    }
    if let Ok(payload) = serde_json::to_string(&record.payload) {
        text.push_str(&payload);
    }
    text.to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_haystack_lowercases_and_includes_metadata_payload_tags() {
        let record = CatalogRecord {
            id: "Search.Panel".into(),
            version: "V1".into(),
            kind: "panel".into(),
            digest: "sha256:abc".into(),
            current: true,
            created_unix_seconds: 0,
            vfs_alias: "/bundles/search.panel".into(),
            vfs_immutable: "/datasets/search.panel@sha256-abc".into(),
            description: Some("European reference panel".into()),
            tags: vec!["genomics".into(), "EUR".into()],
            metadata: [
                ("species".to_string(), "Homo sapiens".to_string()),
                ("study".to_string(), "1000G".to_string()),
            ]
            .into_iter()
            .collect(),
            payload: serde_json::Map::from_iter([(
                "build".to_string(),
                serde_json::Value::String("GRCh38".into()),
            )]),
            file_count: 1,
            total_size_bytes: 0,
        };
        let hay = search_haystack(&record);
        assert!(hay.contains("search.panel"));
        assert!(hay.contains("european reference panel"));
        assert!(hay.contains("homo sapiens"));
        assert!(hay.contains("grch38"));
        assert!(hay.contains("eur"));
    }
}
