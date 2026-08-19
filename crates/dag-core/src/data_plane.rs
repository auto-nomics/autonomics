use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use vfs::OpendalFileStorage;

use crate::dag::DagError;

/// Runtime-local staging for logical VFS artifacts.
///
/// Analysis nodes that require native tools request a staged path from this
/// service instead of embedding physical backend paths in their specifications.
pub struct DataPlane {
    vfs: Arc<OpendalFileStorage>,
    run_root: PathBuf,
}

const STAGE_BUFFER_BYTES: usize = 1024 * 1024;

impl DataPlane {
    pub fn new(vfs: Arc<OpendalFileStorage>, run_root: impl Into<PathBuf>) -> Self {
        Self {
            vfs,
            run_root: run_root.into(),
        }
    }

    pub async fn stage(&self, node_id: &str, vpath: &str) -> Result<PathBuf, DagError> {
        let operator = self.vfs.resolve(vpath);
        let key = self.vfs.resolve_path(vpath);
        let metadata = operator.stat(&key).await.map_err(|error| {
            DagError::Schedule(format!("cannot stat staged input `{vpath}`: {error}"))
        })?;
        if metadata.is_dir() {
            return Err(DagError::Schedule(format!(
                "cannot stage directory `{vpath}`; stage concrete files"
            )));
        }

        let target = self.staged_path(node_id, vpath)?;
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|error| {
                DagError::Schedule(format!(
                    "cannot create staging directory `{}`: {error}",
                    parent.display()
                ))
            })?;
        }

        let reader = operator
            .reader_with(&key)
            .concurrent(8)
            .chunk(8 * 1024 * 1024)
            .await
            .map_err(|error| {
                DagError::Schedule(format!("cannot open staged input `{vpath}`: {error}"))
            })?;
        let mut reader = reader.into_futures_async_read(..).await.map_err(|error| {
            DagError::Schedule(format!("cannot open staged input `{vpath}`: {error}"))
        })?;
        let mut staged_file = tokio::fs::File::create(&target).await.map_err(|error| {
            DagError::Schedule(format!(
                "cannot create staged file `{}`: {error}",
                target.display()
            ))
        })?;
        let mut buffer = vec![0; STAGE_BUFFER_BYTES];
        loop {
            let bytes_read = reader.read(&mut buffer).await.map_err(|error| {
                DagError::Schedule(format!("cannot read staged input `{vpath}`: {error}"))
            })?;
            if bytes_read == 0 {
                break;
            }
            staged_file
                .write_all(&buffer[..bytes_read])
                .await
                .map_err(|error| {
                    DagError::Schedule(format!(
                        "cannot write staged file `{}`: {error}",
                        target.display()
                    ))
                })?;
        }
        staged_file.flush().await.map_err(|error| {
            DagError::Schedule(format!(
                "cannot flush staged file `{}`: {error}",
                target.display()
            ))
        })?;

        Ok(target)
    }

    fn staged_path(&self, node_id: &str, vpath: &str) -> Result<PathBuf, DagError> {
        let node = safe_component(node_id);
        let normalized = OpendalFileStorage::normalize_path(vpath);
        let relative = normalized.trim_start_matches('/');
        if relative.is_empty() {
            return Err(DagError::Schedule(format!(
                "cannot stage VFS root `{vpath}`"
            )));
        }

        let path = Path::new(relative);
        if path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::Prefix(_)
            )
        }) {
            return Err(DagError::Schedule(format!(
                "staging path `{vpath}` escaped the run root"
            )));
        }

        Ok(self
            .run_root
            .join("nodes")
            .join(node)
            .join("input")
            .join(path))
    }
}

fn safe_component(value: &str) -> String {
    let component: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();

    if component.is_empty() {
        "node".to_string()
    } else {
        component
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::test_support::mounted_vfs;

    #[tokio::test]
    async fn stages_virtual_input_in_node_workspace() {
        let harness = mounted_vfs(&[("nested/panel.txt", b"panel-data")]);
        let run_root = tempfile::tempdir().expect("run root");
        let plane = DataPlane::new(harness.storage.clone(), run_root.path());

        let staged = plane
            .stage("plink", "/bundles/panels/nested/panel.txt")
            .await
            .unwrap();

        assert_eq!(
            staged,
            run_root
                .path()
                .join("nodes/plink/input/bundles/panels/nested/panel.txt")
        );
        assert_eq!(
            tokio::fs::read(&staged).await.unwrap(),
            b"panel-data".to_vec()
        );
        assert!(!harness.data_dir.path().join("panel.txt").exists());
    }

    #[tokio::test]
    async fn node_ids_are_sanitized_before_staging() {
        let harness = mounted_vfs(&[("panel.txt", b"panel-data")]);
        let run_root = tempfile::tempdir().expect("run root");
        let plane = DataPlane::new(harness.storage.clone(), run_root.path());

        let staged = plane
            .stage("../plink", "/bundles/panels/panel.txt")
            .await
            .unwrap();

        assert!(staged.starts_with(run_root.path().join("nodes/___plink/input")));
    }
}
