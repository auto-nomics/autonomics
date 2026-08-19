use std::path::{Path, PathBuf};
use std::sync::Arc;

use dag_core::node::DataBundle;
use dag_core::registry::NodeCtx;
use futures::io::AsyncReadExt;
use serde::Deserialize;
use sha2::{Digest, Sha256};

const DEFAULT_REFERENCE_ID: &str = "g1000_eur";
const BUNDLE_MANIFEST: &str = "bundle.json";

#[derive(Debug)]
pub(crate) struct MixerReferenceBundle {
    pub(crate) mixer_home: PathBuf,
    pub(crate) bim_template: String,
    pub(crate) ld_template: String,
    pub(crate) extract_template: String,
    _staging: tempfile::TempDir,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MixerPanelBundle {
    schema_version: u8,
    id: String,
    source_commit: String,
    genome_build: String,
    population: String,
    engine_path: String,
    bim_template: String,
    ld_template: String,
    extract_template: String,
    engine_sha256: String,
}

pub(crate) fn default_reference() -> String {
    DEFAULT_REFERENCE_ID.to_string()
}

fn valid_id(id: &str) -> bool {
    let mut chars = id.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

fn relative_path(value: &str) -> Result<PathBuf, String> {
    if value.is_empty()
        || value.starts_with('/')
        || value.contains('\\')
        || value
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(format!(
            "bundle path '{value}' must be relative and cannot escape the bundle"
        ));
    }
    Ok(PathBuf::from(value))
}

fn validate_template(value: &str) -> Result<(), String> {
    if value.matches('@').count() != 1 {
        return Err(format!(
            "template '{value}' must contain exactly one chromosome placeholder (@)"
        ));
    }
    relative_path(value)?;
    Ok(())
}

fn vpath(root: &str, relative: &str) -> String {
    format!("{}/{}", root.trim_end_matches('/'), relative)
}

async fn read_vfs_bytes(node_ctx: &NodeCtx, path: &str) -> Result<Vec<u8>, String> {
    let storage = node_ctx
        .opendal
        .as_ref()
        .ok_or_else(|| format!("read {path}: no VFS storage is configured"))?;
    if !storage.is_mounted(path) {
        return Err(format!("read {path}: path is not covered by a VFS mount"));
    }
    let operator = storage.resolve(path);
    let key = storage.resolve_path(path);
    let reader = operator
        .reader_with(&key)
        .concurrent(8)
        .chunk(8 * 1024 * 1024)
        .await
        .map_err(|error| format!("open {path}: {error}"))?;
    let mut reader = reader
        .into_futures_async_read(..)
        .await
        .map_err(|error| format!("open {path}: {error}"))?;
    let mut bytes = Vec::new();
    reader
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| format!("read {path}: {error}"))?;
    Ok(bytes)
}

async fn stage_vfs_file(node_ctx: &NodeCtx, source: &str, target: &Path) -> Result<(), String> {
    let bytes = read_vfs_bytes(node_ctx, source).await?;
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", parent.display()))?;
    }
    std::fs::write(target, bytes).map_err(|error| format!("write {}: {error}", target.display()))
}

async fn stage_bundle(node_ctx: &NodeCtx, root: &str, local_root: &Path) -> Result<(), String> {
    let storage = node_ctx
        .opendal
        .as_ref()
        .ok_or_else(|| "no VFS storage is configured".to_string())?;
    let root_key = storage.resolve_path(root);
    let operator = storage.resolve(root);
    let list_key = if root_key.is_empty() {
        "/".to_string()
    } else {
        format!("{root_key}/")
    };
    let entries = operator
        .list_with(&list_key)
        .recursive(true)
        .await
        .map_err(|error| format!("list {root}: {error}"))?;

    for entry in entries {
        if entry.metadata().is_dir() {
            continue;
        }
        let relative = entry.path().trim_start_matches('/');
        let source = vpath(root, relative);
        let target = local_root.join(relative);
        stage_vfs_file(node_ctx, &source, &target).await?;
    }
    Ok(())
}

pub(crate) async fn resolve_reference(
    node_ctx: &NodeCtx,
    runtime_bundle: &DataBundle,
    reference: &str,
) -> Result<MixerReferenceBundle, String> {
    if !valid_id(reference) {
        return Err(format!(
            "invalid reference ID '{reference}': IDs may contain only ASCII letters, digits, '_', '-', and '.'"
        ));
    }

    let root = runtime_bundle.vpath.trim_end_matches('/').to_string();
    let manifest_bytes = read_vfs_bytes(node_ctx, &vpath(&root, BUNDLE_MANIFEST)).await?;
    let bundle: MixerPanelBundle = serde_json::from_slice(&manifest_bytes)
        .map_err(|error| format!("invalid {BUNDLE_MANIFEST} for {reference}: {error}"))?;

    if bundle.schema_version != 1 {
        return Err(format!(
            "unsupported bundle schema version {}",
            bundle.schema_version
        ));
    }
    if bundle.id != reference {
        return Err(format!(
            "bundle ID '{}' does not match requested reference '{reference}'",
            bundle.id
        ));
    }
    for (name, value) in [
        ("source_commit", &bundle.source_commit),
        ("genome_build", &bundle.genome_build),
        ("population", &bundle.population),
    ] {
        if value.trim().is_empty() {
            return Err(format!("{name} cannot be empty"));
        }
    }
    validate_template(&bundle.bim_template)?;
    validate_template(&bundle.ld_template)?;
    validate_template(&bundle.extract_template)?;

    let staging = tempfile::tempdir().map_err(|error| format!("create staging dir: {error}"))?;
    stage_bundle(node_ctx, &root, staging.path()).await?;

    let mixer_home = staging.path().join(&bundle.engine_path);
    let mixer_py = mixer_home.join("precimed").join("mixer.py");
    let library = mixer_home.join("libbgmg.so");
    if !mixer_py.is_file() {
        return Err(format!("missing mixer engine: {}", mixer_py.display()));
    }
    let library_bytes =
        std::fs::read(&library).map_err(|error| format!("read {}: {error}", library.display()))?;
    let actual_checksum = Sha256::digest(&library_bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if bundle.engine_sha256.len() != 64
        || bundle
            .engine_sha256
            .bytes()
            .any(|byte| !byte.is_ascii_hexdigit())
    {
        return Err("engine_sha256 must contain 64 hexadecimal characters".into());
    }
    if !actual_checksum.eq_ignore_ascii_case(&bundle.engine_sha256) {
        return Err(format!(
            "libbgmg.so checksum mismatch: expected {}, got {actual_checksum}",
            bundle.engine_sha256
        ));
    }

    Ok(MixerReferenceBundle {
        bim_template: staging
            .path()
            .join(&bundle.bim_template)
            .to_string_lossy()
            .into_owned(),
        ld_template: staging
            .path()
            .join(&bundle.ld_template)
            .to_string_lossy()
            .into_owned(),
        extract_template: staging
            .path()
            .join(&bundle.extract_template)
            .to_string_lossy()
            .into_owned(),
        mixer_home,
        _staging: staging,
    })
}

pub(crate) fn python_executable(mixer_home: &Path) -> String {
    std::env::var("MIXER_PYTHON")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| {
            mixer_home
                .join(".venv/bin/python")
                .to_string_lossy()
                .into_owned()
        })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use datafusion::prelude::SessionContext;
    use vfs::{BackendConfig, BackendDefinition, MountDefinition, MountedObjectStore, VfsManifest};

    fn bundle() -> DataBundle {
        DataBundle::new(
            "mixer.g1000_eur",
            "MiXeR reference",
            "/bundles/mixer/g1000_eur",
        )
    }

    pub(crate) fn vfs_ctx(root: &Path) -> (NodeCtx, tempfile::TempDir) {
        let manifest = VfsManifest {
            backend: vec![BackendDefinition {
                id: "mixer".into(),
                config: BackendConfig::local(root.to_string_lossy().to_string()),
            }],
            mount: vec![MountDefinition {
                path: "/bundles/mixer/g1000_eur".into(),
                backend: "mixer".into(),
                source: "/".into(),
                read_only: true,
            }],
        };
        let mounted = Arc::new(MountedObjectStore::from_manifest(&manifest).unwrap());
        let scratch = tempfile::tempdir().unwrap();
        let opendal = Arc::new(vfs::OpendalFileStorage::with_mounts(
            scratch.path(),
            mounted,
        ));
        (
            NodeCtx::new(SessionContext::new().runtime_env(), Some(opendal)),
            scratch,
        )
    }

    #[tokio::test]
    async fn stages_mixer_bundle_through_opendal() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("engine/precimed")).unwrap();
        std::fs::create_dir_all(root.path().join("data")).unwrap();
        std::fs::write(
            root.path().join("engine/precimed/mixer.py"),
            b"print('mixer')",
        )
        .unwrap();
        std::fs::write(root.path().join("engine/libbgmg.so"), b"engine").unwrap();
        std::fs::write(root.path().join("data/bim21.bim"), b"bim21").unwrap();
        std::fs::write(root.path().join("data/ld21.ld"), b"ld21").unwrap();
        std::fs::write(root.path().join("data/extract21.gz"), b"extract21").unwrap();
        let checksum = Sha256::digest(b"engine")
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        std::fs::write(
            root.path().join(BUNDLE_MANIFEST),
            format!(
                r#"{{
                  "schema_version": 1,
                  "id": "g1000_eur",
                  "source_commit": "test",
                  "genome_build": "GRCh37",
                  "population": "EUR",
                  "engine_path": "engine",
                  "bim_template": "data/bim@.bim",
                  "ld_template": "data/ld@.ld",
                  "extract_template": "data/extract@.gz",
                  "engine_sha256": "{checksum}"
                }}"#
            ),
        )
        .unwrap();

        let (ctx, _scratch) = vfs_ctx(root.path());
        let resolved = resolve_reference(&ctx, &bundle(), "g1000_eur")
            .await
            .unwrap();

        assert!(resolved.mixer_home.join("precimed/mixer.py").is_file());
        assert_eq!(
            std::fs::read(resolved.bim_template.replace('@', "21")).unwrap(),
            b"bim21".to_vec()
        );
    }

    #[tokio::test]
    async fn missing_vfs_storage_is_rejected() {
        let ctx = NodeCtx::new(SessionContext::new().runtime_env(), None);

        let error = resolve_reference(&ctx, &bundle(), "g1000_eur")
            .await
            .unwrap_err();

        assert!(error.contains("no VFS storage"));
    }
}
