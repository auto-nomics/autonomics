use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::io::AsyncReadExt;
use lava::input::PlinkRef;

use dag_core::node::DataBundle;
use dag_core::registry::NodeCtx;

pub(crate) const PLINK_BUNDLE: &str = "plink.1000g_eur";

pub(crate) fn storage_template(bundle: &DataBundle) -> Result<String, String> {
    if !bundle.vpath.contains("{N}") {
        return Err(format!(
            "PLINK bundle vpath must contain '{{N}}': got {}",
            bundle.vpath
        ));
    }
    Ok(format!("vfs://{}", bundle.vpath))
}

#[derive(Debug)]
pub(crate) struct LavaReference {
    pub(crate) reference: PlinkRef,
    pub(crate) local_prefix: PathBuf,
    _staging: Option<tempfile::TempDir>,
}

fn vfs_path(raw_path: &str) -> Result<String, String> {
    let rest = raw_path
        .strip_prefix("vfs://")
        .ok_or_else(|| format!("expected a vfs:// path, got {raw_path}"))?;
    Ok(vfs::OpendalFileStorage::normalize_path(rest))
}

async fn stage_vfs_file(node_ctx: &NodeCtx, raw_path: &str, target: &Path) -> Result<(), String> {
    let path = vfs_path(raw_path)?;
    let storage = node_ctx
        .opendal
        .as_ref()
        .ok_or_else(|| format!("cannot read {raw_path}: no VFS storage is configured"))?;
    if !storage.is_mounted(&path) {
        return Err(format!(
            "cannot read {raw_path}: path is not covered by a VFS mount"
        ));
    }

    let op = storage.resolve(&path);
    let key = storage.resolve_path(&path);
    let mut reader = op
        .reader_with(&key)
        .concurrent(8)
        .chunk(8 * 1024 * 1024)
        .await
        .map_err(|e| format!("open {raw_path}: {e}"))?
        .into_futures_async_read(..)
        .await
        .map_err(|e| format!("open {raw_path}: {e}"))?;

    let mut output =
        std::fs::File::create(target).map_err(|e| format!("create {}: {e}", target.display()))?;
    let mut buffer = vec![0u8; 8 * 1024 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .await
            .map_err(|e| format!("read {raw_path}: {e}"))?;
        if read == 0 {
            break;
        }
        output
            .write_all(&buffer[..read])
            .map_err(|e| format!("write {}: {e}", target.display()))?;
    }
    Ok(())
}

async fn load_vfs_reference_template(
    node_ctx: &NodeCtx,
    template: &str,
    chroms: &[i64],
) -> Result<LavaReference, String> {
    if !template.contains("{N}") {
        return Err(format!(
            "VFS PLINK template must contain '{{N}}': got {template:?}"
        ));
    }

    let staging = tempfile::tempdir().map_err(|e| format!("create PLINK staging dir: {e}"))?;
    let relative_template = template.strip_prefix("vfs://").unwrap_or(template);
    for chrom in chroms {
        let resolved = template.replace("{N}", &chrom.to_string());
        let prefix = PathBuf::from(relative_template.replace("{N}", &chrom.to_string()));
        let parent = prefix
            .parent()
            .ok_or_else(|| format!("VFS PLINK prefix has no parent: {resolved}"))?;
        let local_parent = staging
            .path()
            .join(parent.strip_prefix("/").unwrap_or(parent));
        std::fs::create_dir_all(&local_parent)
            .map_err(|e| format!("create {}: {e}", local_parent.display()))?;
        for extension in ["bed", "bim", "fam"] {
            let source = format!("{resolved}.{extension}");
            let mut target = local_parent.join(prefix.file_name().unwrap_or_default());
            // Append rather than replace: PLINK prefixes may themselves contain
            // extensions (for example, chr1/1000G.EUR.chr1.qc).
            target.as_mut_os_string().push(".");
            target.as_mut_os_string().push(extension);
            stage_vfs_file(node_ctx, &source, &target).await?;
        }
    }

    let local_prefix = staging
        .path()
        .join(
            relative_template
                .strip_prefix('/')
                .unwrap_or(relative_template),
        )
        .to_string_lossy()
        .into_owned();
    let reference =
        lava::plink::load_reference_template(&local_prefix, chroms).map_err(|e| e.to_string())?;
    Ok(LavaReference {
        reference,
        local_prefix: PathBuf::from(local_prefix),
        _staging: Some(staging),
    })
}

/// Resolve and stage a PLINK reference bundle through OpenDAL.
pub(crate) async fn load_lava_reference(
    node_ctx: &NodeCtx,
    bundle: &DataBundle,
    chroms: &[i64],
) -> Result<LavaReference, String> {
    let template = storage_template(bundle)?;
    load_vfs_reference_template(node_ctx, &template, chroms).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use datafusion::prelude::SessionContext;
    use vfs::{BackendDefinition, MountDefinition, VfsManifest};

    fn bundle() -> DataBundle {
        DataBundle::new(
            PLINK_BUNDLE,
            "1000G EUR PLINK reference",
            "/data/mixer/resources/g1000_eur/stage/chr{N}/1000G.EUR.chr{N}.qc",
        )
    }

    fn write_prefix(root: &Path, prefix: &str, marker: &str) {
        let prefix_path = root.join(prefix);
        std::fs::create_dir_all(prefix_path.parent().unwrap()).unwrap();
        std::fs::write(format!("{}.bed", prefix_path.display()), [0x6c, 0x1e, 0x01]).unwrap();
        std::fs::write(
            format!("{}.bim", prefix_path.display()),
            format!("1 {marker} 0 1 A G\n"),
        )
        .unwrap();
        std::fs::write(
            format!("{}.fam", prefix_path.display()),
            format!("0 {marker} 0 0 1 -9\n"),
        )
        .unwrap();
    }

    fn vfs_ctx_at(source: &Path, mount_path: &str) -> NodeCtx {
        let manifest = VfsManifest {
            backend: vec![BackendDefinition {
                id: "plink-test".into(),
                // Production local mounts use a backend rooted at `/` and an
                // absolute source directory. Preserve that key layout in tests.
                config: vfs::BackendConfig::local("/"),
            }],
            mount: vec![MountDefinition {
                path: mount_path.into(),
                backend: "plink-test".into(),
                source: source.to_string_lossy().into_owned(),
                read_only: true,
            }],
        };
        let mounts = Arc::new(vfs::MountedObjectStore::from_manifest(&manifest).unwrap());
        let scratch = tempfile::tempdir().unwrap();
        let opendal = Arc::new(vfs::OpendalFileStorage::with_mounts(scratch.path(), mounts));
        NodeCtx::new(SessionContext::new().runtime_env(), Some(opendal))
    }

    fn vfs_ctx(source: &Path) -> NodeCtx {
        vfs_ctx_at(source, "/data/mixer/resources")
    }

    #[tokio::test]
    async fn lava_reference_loads_from_configured_vfs_mount() {
        let source = tempfile::tempdir().unwrap();
        write_prefix(
            source.path(),
            "g1000_eur/stage/chr1/1000G.EUR.chr1.qc",
            "panel",
        );

        let loaded = load_lava_reference(&vfs_ctx(source.path()), &bundle(), &[1])
            .await
            .expect("VFS PLINK reference should load");
        assert_eq!(loaded.reference.sample_size, 1);
        assert!(
            loaded
                .reference
                .chr_prefix
                .get(&1)
                .is_some_and(|path| path.ends_with("1000G.EUR.chr1.qc"))
        );
    }

    #[tokio::test]
    async fn lava_reference_loads_rsid_panel_from_production_layout() {
        let source = Path::new("/mnt/data/lava/resources");
        if !source.join("chr21/1000G.EUR.chr21.qc.bim").is_file() {
            return;
        }
        let bundle = DataBundle::new(
            PLINK_BUNDLE,
            "1000G EUR rsID PLINK reference",
            "/bundles/plink/1000g_eur/chr{N}/1000G.EUR.chr{N}.qc",
        );

        let loaded = load_lava_reference(
            &vfs_ctx_at(source, "/bundles/plink/1000g_eur"),
            &bundle,
            &[21],
        )
        .await
        .expect("rsID PLINK reference should load");

        assert_eq!(loaded.reference.sample_size, 503);
        assert!(
            loaded
                .reference
                .snp_info
                .snp
                .first()
                .is_some_and(|snp| snp.starts_with("rs")),
            "LAVA requires a reference panel keyed by rsID"
        );
    }

    #[tokio::test]
    async fn lava_reference_reports_missing_vfs_panel() {
        let source = tempfile::tempdir().unwrap();
        let error = load_lava_reference(&vfs_ctx(source.path()), &bundle(), &[1])
            .await
            .unwrap_err();
        assert!(error.contains("vfs:///data/mixer/resources"), "{error}");
    }

    #[tokio::test]
    #[ignore = "needs the deployed 1000G EUR MiXeR panel"]
    async fn lava_reference_loads_deployed_vfs_panel() {
        let source = Path::new("/mnt/data/mixer/resources");
        if !source.join("g1000_eur/stage/chr22").is_dir() {
            return;
        }

        let loaded = load_lava_reference(&vfs_ctx(source), &bundle(), &[22])
            .await
            .expect("deployed VFS panel should load");
        assert!(loaded.reference.sample_size > 0);
        assert!(!loaded.reference.snp_info.snp.is_empty());
    }
}
