use std::collections::BTreeSet;

use dag_core::DataBundle;
use vfs::MountDefinition;

use crate::config::CatalogConfig;
use crate::model::CatalogIndex;
use crate::storage::{catalog_object_key, operator_for_backend, read_json_object};

#[derive(Debug, Clone)]
pub struct CatalogRuntime {
    pub index: CatalogIndex,
}

impl CatalogRuntime {
    pub async fn load(manifest: &vfs::VfsManifest, config: &CatalogConfig) -> Result<Self, String> {
        config.validate()?;
        let operator = operator_for_backend(manifest, &config.backend)?;
        let key = config.object_key(&config.index);
        let index: CatalogIndex = read_json_object(&operator, &key).await?;
        index.validate().map_err(|error| {
            format!(
                "invalid catalog index `{}`: {error}",
                config.object_key(&config.index)
            )
        })?;
        Ok(Self { index })
    }

    pub fn data_bundles(&self) -> dag_core::DataBundleCatalog {
        let mut bundles = Vec::new();
        for entry in self.index.current_entries() {
            let mut bundle = DataBundle::new(
                entry.id.clone(),
                format!("{} {} catalog dataset", entry.id, entry.version),
                entry.vfs_alias.clone(),
            );
            let entry_root = entry
                .manifest
                .trim_end_matches("/manifest.json")
                .to_string();
            bundle.source = Some(format!("/catalog/{entry_root}"));
            bundle.digest = Some(entry.digest.clone());
            bundles.push(bundle);
        }
        dag_core::DataBundleCatalog::from_bundles(bundles)
            .expect("validated catalog entries have unique current ids")
    }
}

pub fn catalog_mount_definitions(
    manifest: &vfs::VfsManifest,
    index: &CatalogIndex,
    config: &CatalogConfig,
) -> Result<Vec<MountDefinition>, String> {
    crate::storage::backend_definition(manifest, &config.backend)?;
    let mut existing = BTreeSet::new();
    for mount in &manifest.mount {
        if !existing.insert(mount.path.clone()) {
            return Err(format!("duplicate VFS mount path `{}`", mount.path));
        }
    }

    let mut mounts = Vec::new();
    if config.agent_visible && existing.insert("/catalog".into()) {
        mounts.push(MountDefinition {
            path: "/catalog".into(),
            backend: config.backend.clone(),
            source: config.source_prefix(),
            read_only: true,
        });
    }
    for entry in index.current_entries() {
        let source = catalog_object_key(&config.source, &entry.files);
        if existing.insert(entry.vfs_immutable.clone()) {
            mounts.push(MountDefinition {
                path: entry.vfs_immutable.clone(),
                backend: config.backend.clone(),
                source: source.clone(),
                read_only: true,
            });
        }
        if existing.insert(entry.vfs_alias.clone()) {
            mounts.push(MountDefinition {
                path: entry.vfs_alias.clone(),
                backend: config.backend.clone(),
                source,
                read_only: true,
            });
        }
    }
    Ok(mounts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::CatalogEntry;
    use vfs::{BackendConfig, VfsManifest};

    fn config() -> CatalogConfig {
        CatalogConfig {
            backend: "warehouse".into(),
            source: "/warehouse/catalog".into(),
            index: "index.json".into(),
            prefix: "entries".into(),
            enabled: true,
            agent_visible: true,
        }
    }

    fn manifest() -> VfsManifest {
        VfsManifest {
            backend: vec![vfs::BackendDefinition {
                id: "warehouse".into(),
                config: BackendConfig::local("/"),
            }],
            mount: Vec::new(),
        }
    }

    fn index() -> CatalogIndex {
        let mut index = CatalogIndex::default();
        index.upsert_current(CatalogEntry {
            id: "1000g_eur".into(),
            version: "v3".into(),
            kind: "vcf".into(),
            digest: format!("sha256:{}", "a".repeat(64)),
            manifest: "entries/1000g_eur/v3/manifest.json".into(),
            files: "entries/1000g_eur/v3".into(),
            vfs_alias: "/bundles/1000g_eur".into(),
            vfs_immutable: "/datasets/1000g_eur@sha256-aaa".into(),
            current: true,
            created_unix_seconds: 1,
        });
        index
    }

    #[test]
    fn generates_catalog_aliases_and_immutable_mounts() {
        let mounts = catalog_mount_definitions(&manifest(), &index(), &config()).unwrap();
        assert!(mounts.iter().any(|mount| mount.path == "/catalog"));
        assert!(
            mounts
                .iter()
                .any(|mount| mount.path == "/bundles/1000g_eur" && mount.read_only)
        );
        assert!(
            mounts
                .iter()
                .any(|mount| mount.path == "/datasets/1000g_eur@sha256-aaa")
        );
        assert!(
            mounts
                .iter()
                .any(|mount| mount.source == "warehouse/catalog/entries/1000g_eur/v3")
        );
    }

    #[test]
    fn existing_static_alias_wins_without_duplicate_mount() {
        let mut manifest = manifest();
        manifest.mount.push(MountDefinition {
            path: "/bundles/1000g_eur".into(),
            backend: "warehouse".into(),
            source: "/legacy/files".into(),
            read_only: true,
        });
        let mounts = catalog_mount_definitions(&manifest, &index(), &config()).unwrap();
        assert_eq!(
            mounts
                .iter()
                .filter(|mount| mount.path == "/bundles/1000g_eur")
                .count(),
            0
        );
    }
}
