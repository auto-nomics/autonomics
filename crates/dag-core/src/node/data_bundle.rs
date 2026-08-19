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
    use crate::node::test_support::mounted_vfs;

    #[tokio::test]
    async fn read_file_resolves_through_a_vfs_mount() {
        let harness = mounted_vfs(&[("panel.txt", b"panel-data")]);
        let vfs = harness.storage.as_ref();
        let bundle = DataBundle::new(
            "panels",
            "Reference panel bundle",
            "/bundles/panels/panel.txt",
        );

        let bytes = bundle.read_file(vfs).await.unwrap();

        assert_eq!(bytes, b"panel-data");
        assert!(!harness.data_dir.path().join("panel.txt").exists());
    }
}
