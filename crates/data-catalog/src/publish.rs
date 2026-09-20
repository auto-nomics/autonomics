use std::path::Path;

use crate::config::CatalogConfig;
use crate::error::Result;
use crate::model::{CatalogEntry, CatalogIndex, DatasetManifest};
use crate::package::{PAYLOAD_DIR, validate_package};
use crate::storage::{read_json_object, upload_file, write_json_object};

const PAYLOAD_UPLOAD_CONCURRENCY: usize = 32;

pub async fn publish_package(
    package: impl AsRef<Path>,
    config: &CatalogConfig,
    operator: &opendal::Operator,
) -> Result<CatalogEntry> {
    config.validate()?;
    let manifest = validate_package(package.as_ref()).map_err(|error| error.to_string())?;
    let digest = manifest
        .short_digest()
        .map_err(|error| format!("package {error}"))?
        .to_string();
    let entry_root = format!(
        "{}/{}/{}/sha256-{digest}",
        config.prefix.trim_matches('/'),
        manifest.id,
        manifest.version
    );
    let manifest_key = config.object_key(&format!("{entry_root}/manifest.json"));
    let files_prefix = entry_root.clone();
    let entry = CatalogEntry {
        id: manifest.id.clone(),
        version: manifest.version.clone(),
        kind: manifest.kind.clone(),
        digest: manifest
            .digest
            .clone()
            .expect("validated manifests have a digest"),
        manifest: format!("{entry_root}/manifest.json"),
        files: files_prefix.clone(),
        vfs_alias: format!("/bundles/{}", manifest.id),
        vfs_immutable: format!("/datasets/{}@sha256-{digest}", manifest.id),
        current: true,
        created_unix_seconds: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs() as i64)
            .unwrap_or_default(),
    };

    upload_payload_files(package.as_ref(), operator, &files_prefix, &manifest, config).await?;
    upload_object_if_needed(
        operator,
        &manifest_key,
        &serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?,
    )
    .await?;

    let index_key = config.object_key(&config.index);
    let mut index = match operator.stat(&index_key).await {
        Ok(_) => read_json_object::<CatalogIndex>(operator, &index_key).await?,
        Err(error) if error.kind() == opendal::ErrorKind::NotFound => CatalogIndex::default(),
        Err(error) => return Err(format!("stat catalog index `{index_key}`: {error}").into()),
    };
    index
        .validate()
        .map_err(|error| format!("invalid existing catalog index: {error}"))?;
    index.upsert_current(entry.clone());
    index.validate()?;
    write_json_object(operator, &index_key, &index).await?;
    Ok(entry)
}

async fn upload_payload_files(
    package: &Path,
    operator: &opendal::Operator,
    files_prefix: &str,
    manifest: &DatasetManifest,
    config: &CatalogConfig,
) -> Result<()> {
    for batch in manifest.files.chunks(PAYLOAD_UPLOAD_CONCURRENCY) {
        let mut tasks = tokio::task::JoinSet::new();
        for file in batch {
            let operator = operator.clone();
            let relative_path = file.path.clone();
            let expected_size = file.size;
            let local = package.join(PAYLOAD_DIR).join(&relative_path);
            let key = config.object_key(&format!("{files_prefix}/{relative_path}"));
            tasks.spawn(async move {
                if remote_size_matches(&operator, &key, expected_size).await? {
                    return Ok(());
                }
                upload_file(&operator, &key, &local).await
            });
        }
        while let Some(result) = tasks.join_next().await {
            result.map_err(|error| format!("payload upload task failed: {error}"))??;
        }
    }
    Ok(())
}

async fn remote_size_matches(
    operator: &opendal::Operator,
    key: &str,
    expected_size: u64,
) -> Result<bool> {
    match operator.stat(key).await {
        Ok(metadata) => Ok(metadata.content_length() == expected_size),
        Err(error) if error.kind() == opendal::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("stat object `{key}`: {error}").into()),
    }
}

async fn upload_object_if_needed(
    operator: &opendal::Operator,
    key: &str,
    bytes: &[u8],
) -> Result<()> {
    if remote_size_matches(operator, key, bytes.len() as u64).await? {
        return Ok(());
    }
    crate::storage::write_object(operator, key, bytes).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::{BuildOptions, build_package};
    use crate::storage::operator_for_backend;
    use crate::{CatalogRuntime, catalog_mount_definitions};
    use std::path::Path;
    use std::path::PathBuf;
    use vfs::{
        BackendConfig, BackendDefinition, MountDefinition, MountedObjectStore, OpendalFileStorage,
        VfsManifest,
    };

    fn make_package(parent: &Path, id: &str, version: &str, content: &[u8]) -> PathBuf {
        let input = parent.join(format!("{id}-{version}-input"));
        std::fs::create_dir_all(&input).unwrap();
        std::fs::write(input.join("data.txt"), content).unwrap();
        let output = parent.join(format!("{id}-{version}-package"));
        build_package(
            input,
            output,
            BuildOptions {
                id: Some(id.into()),
                version: Some(version.into()),
                kind: Some("table".into()),
                ..Default::default()
            },
        )
        .unwrap()
        .path
    }

    #[tokio::test]
    async fn publish_load_and_mount_catalog_package() {
        let workspace = tempfile::tempdir().unwrap();
        let warehouse = tempfile::tempdir().unwrap();
        let manifest = VfsManifest {
            backend: vec![BackendDefinition {
                id: "warehouse".into(),
                config: BackendConfig::local(warehouse.path().to_string_lossy().to_string()),
            }],
            mount: Vec::new(),
        };
        let config = CatalogConfig {
            backend: "warehouse".into(),
            source: "/".into(),
            ..Default::default()
        };
        let operator = operator_for_backend(&manifest, &config.backend).unwrap();
        let package = make_package(workspace.path(), "panel", "v1", b"panel-v1");

        let entry = publish_package(&package, &config, &operator).await.unwrap();
        assert_eq!(entry.vfs_alias, "/bundles/panel");

        let runtime = CatalogRuntime::load(&manifest, &config).await.unwrap();
        assert_eq!(runtime.index.entries.len(), 1);
        assert_eq!(
            runtime
                .bundle_registry()
                .get("panel")
                .map(|bundle| bundle.vpath.as_str())
                .unwrap(),
            "/bundles/panel"
        );

        let mut mounted_manifest = manifest.clone();
        mounted_manifest
            .mount
            .extend(catalog_mount_definitions(&manifest, &runtime.index, &config).unwrap());
        let mounted = MountedObjectStore::from_manifest(&mounted_manifest).unwrap();
        let storage =
            OpendalFileStorage::with_mounts(workspace.path(), std::sync::Arc::new(mounted));
        let bytes = storage
            .resolve("/bundles/panel/data.txt")
            .read(&storage.resolve_path("/bundles/panel/data.txt"))
            .await
            .unwrap();
        assert_eq!(bytes.to_vec(), b"panel-v1");
    }

    #[tokio::test]
    async fn publishing_new_version_advances_current_alias() {
        let workspace = tempfile::tempdir().unwrap();
        let warehouse = tempfile::tempdir().unwrap();
        let manifest = VfsManifest {
            backend: vec![BackendDefinition {
                id: "warehouse".into(),
                config: BackendConfig::local(warehouse.path().to_string_lossy().to_string()),
            }],
            mount: Vec::new(),
        };
        let config = CatalogConfig {
            backend: "warehouse".into(),
            source: "/".into(),
            ..Default::default()
        };
        let operator = operator_for_backend(&manifest, &config.backend).unwrap();
        publish_package(
            make_package(workspace.path(), "panel", "v1", b"one"),
            &config,
            &operator,
        )
        .await
        .unwrap();
        publish_package(
            make_package(workspace.path(), "panel", "v2", b"two"),
            &config,
            &operator,
        )
        .await
        .unwrap();

        let runtime = CatalogRuntime::load(&manifest, &config).await.unwrap();
        assert_eq!(runtime.index.entries.len(), 2);
        assert_eq!(runtime.index.current_entries().count(), 1);
        assert_eq!(
            runtime.index.current_entries().next().unwrap().version,
            "v2"
        );
    }
}
