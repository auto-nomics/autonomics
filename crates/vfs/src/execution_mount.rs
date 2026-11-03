//! Read-only local evidence mounts, without exposing their parent directories.
use std::path::{Component, Path};

use crate::mount::{BackendConfig, BackendDefinition, MountDefinition, MountError, VfsManifest};

impl VfsManifest {
    /// Make host-absolute execution references readable in the same VFS
    /// namespace. An existing mount must already identify this exact directory;
    /// its permissions are preserved and writes are disabled.
    pub fn ensure_read_only_local_identity_mount(&mut self, root: &Path) -> Result<(), MountError> {
        if !root.is_absolute()
            || root == Path::new("/")
            || root
                .components()
                .any(|part| matches!(part, Component::ParentDir))
        {
            return Err(MountError::Config(
                "execution evidence requires a dedicated absolute directory".into(),
            ));
        }
        let path = root.to_string_lossy().into_owned();
        if let Some(existing) = self.mount.iter_mut().find(|mount| mount.path == path) {
            let actual = self
                .backend
                .iter()
                .find(|backend| backend.id == existing.backend)
                .and_then(|backend| match &backend.config {
                    BackendConfig::Local { root } => {
                        let source = Path::new(&existing.source);
                        Some(if existing.source.is_empty() || source == Path::new("/") {
                            Path::new(root).to_path_buf()
                        } else if source.is_absolute() {
                            source.to_path_buf()
                        } else {
                            Path::new(root).join(source)
                        })
                    }
                    _ => None,
                });
            if actual.as_deref() != Some(root) {
                return Err(MountError::Config(format!(
                    "execution evidence mount `{path}` conflicts with its host directory"
                )));
            }
            existing.read_only = true;
            return Ok(());
        }
        let backend =
            if let Some(backend) = self.backend.iter().find(
                |backend| matches!(&backend.config, BackendConfig::Local { root } if root == "/"),
            ) {
                backend.id.clone()
            } else {
                let mut id = "execution-evidence".to_owned();
                while self.backend.iter().any(|backend| backend.id == id) {
                    id.push('_');
                }
                self.backend.push(BackendDefinition {
                    id: id.clone(),
                    config: BackendConfig::local("/"),
                });
                id
            };
        self.mount.push(MountDefinition {
            path: path.clone(),
            backend,
            source: path,
            read_only: true,
            permissions: Default::default(),
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permission::{VfsMode, VfsPrincipal};
    use crate::{MountedObjectStore, OpendalFileStorage};
    use std::sync::Arc;

    #[tokio::test]
    async fn execution_evidence_is_readable_but_not_writable_without_exposing_parent() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("state");
        let mut manifest = VfsManifest::local_root(temp.path().join("data").to_string_lossy());
        for root in [state.join("slurm-tasks"), state.join("slurm-artifacts")] {
            manifest
                .ensure_read_only_local_identity_mount(&root)
                .unwrap();
            std::fs::create_dir_all(root.join("attempt")).unwrap();
            std::fs::write(root.join("attempt/stderr.log"), b"worker failure").unwrap();
        }
        let mounts = Arc::new(MountedObjectStore::from_manifest(&manifest).unwrap());
        let storage = OpendalFileStorage::with_mounts(&temp.path().join("data"), mounts)
            .with_principal(VfsPrincipal::new(64482, 100));
        let path = state
            .join("slurm-tasks/attempt/stderr.log")
            .to_string_lossy()
            .into_owned();
        assert_eq!(
            storage.read_range(&path, 0..14).await.unwrap().to_vec(),
            b"worker failure"
        );
        assert!(storage.check_writable(&path).is_err());
        let history = state.join("agent.db");
        assert_ne!(
            storage.local_host_path(&history.to_string_lossy()),
            Some(history)
        );
    }

    #[test]
    fn execution_evidence_refuses_broad_roots_and_conflicting_mounts() {
        let temp = tempfile::tempdir().unwrap();
        let mut manifest = VfsManifest::local_root(temp.path().to_string_lossy());
        for path in ["/", "relative", "/tmp/../private"] {
            assert!(
                manifest
                    .ensure_read_only_local_identity_mount(Path::new(path))
                    .is_err()
            );
        }
        let root = temp.path().join("artifacts");
        manifest.mount.push(MountDefinition {
            path: root.to_string_lossy().into_owned(),
            backend: "default".into(),
            source: "unrelated".into(),
            read_only: false,
            permissions: Default::default(),
        });
        assert!(
            manifest
                .ensure_read_only_local_identity_mount(&root)
                .is_err()
        );
        assert!(
            !manifest.mount.last().unwrap().read_only,
            "a conflict is not silently rebound"
        );
    }

    #[test]
    fn existing_identity_mount_keeps_its_permission_policy() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("artifacts");
        let mut manifest = VfsManifest::local_root(temp.path().to_string_lossy());
        let mut permissions = crate::permission::MountPermissions::default();
        permissions.file_mode = VfsMode::from_bits(0o600);
        manifest.mount.push(MountDefinition {
            path: root.to_string_lossy().into_owned(),
            backend: "default".into(),
            source: root.to_string_lossy().into_owned(),
            read_only: false,
            permissions: permissions.clone(),
        });
        manifest
            .ensure_read_only_local_identity_mount(&root)
            .unwrap();
        assert!(manifest.mount.last().unwrap().read_only);
        assert_eq!(manifest.mount.last().unwrap().permissions, permissions);
    }
}
