use opendal::Operator;
use vfs::{
    BackendConfig, BackendDefinition, MountDefinition, MountedObjectStore, OpendalFileStorage,
    VfsManifest,
};

/// A logical reference to data required by a DAG node.
///
/// `DataBundle` intentionally does not know whether its path is backed by a
/// local filesystem, S3, OSS, or another OpenDAL service. The runtime VFS is
/// responsible for resolving the virtual path to a backend operator and key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataBundle {
    pub ident: String,
    pub desc: String,
    pub vpath: String,
}

/// The backend-specific form of a [`DataBundle`] after VFS resolution.
#[derive(Debug, Clone)]
pub struct ResolvedDataBundle {
    pub ident: String,
    pub operator: Operator,
    pub key: String,
}

impl DataBundle {
    pub fn new(
        ident: impl Into<String>,
        desc: impl Into<String>,
        vpath: impl Into<String>,
    ) -> Self {
        Self {
            ident: ident.into(),
            desc: desc.into(),
            vpath: vpath.into(),
        }
    }

    /// Resolve the bundle through the runtime VFS.
    ///
    /// The returned operator is cheap to clone and can be used independently
    /// of `OpendalFileStorage`.
    pub fn resolve(&self, vfs: &OpendalFileStorage) -> ResolvedDataBundle {
        ResolvedDataBundle {
            ident: self.ident.clone(),
            operator: vfs.resolve(&self.vpath),
            key: vfs.resolve_path(&self.vpath),
        }
    }

    pub async fn read_file(&self, vfs: &OpendalFileStorage) -> Result<Vec<u8>, opendal::Error> {
        let resolved = self.resolve(vfs);
        let bytes = resolved.operator.read(&resolved.key).await?;

        Ok(bytes.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn read_file_resolves_through_a_vfs_mount() {
        let data_dir = tempfile::tempdir().expect("data directory");
        let source_dir = tempfile::tempdir().expect("source directory");
        std::fs::write(source_dir.path().join("panel.txt"), b"panel-data").unwrap();

        let manifest = VfsManifest {
            backend: vec![
                BackendDefinition {
                    id: "runtime".into(),
                    config: BackendConfig::local(data_dir.path().to_string_lossy().to_string()),
                },
                BackendDefinition {
                    id: "external".into(),
                    config: BackendConfig::local(source_dir.path().to_string_lossy().to_string()),
                },
            ],
            mount: vec![
                MountDefinition {
                    path: "/".into(),
                    backend: "runtime".into(),
                    source: "/".into(),
                    read_only: false,
                },
                MountDefinition {
                    path: "/bundles/panels".into(),
                    backend: "external".into(),
                    source: "/".into(),
                    read_only: true,
                },
            ],
        };
        let mounted = Arc::new(MountedObjectStore::from_manifest(&manifest).unwrap());
        let vfs = OpendalFileStorage::with_mounts(data_dir.path(), mounted);
        let bundle = DataBundle::new(
            "panels",
            "Reference panel bundle",
            "/bundles/panels/panel.txt",
        );

        let bytes = bundle.read_file(&vfs).await.unwrap();

        assert_eq!(bytes, b"panel-data");
        assert!(!data_dir.path().join("panel.txt").exists());
    }
}
