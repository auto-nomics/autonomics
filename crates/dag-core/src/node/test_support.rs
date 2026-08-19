use std::sync::Arc;

use vfs::{
    BackendConfig, BackendDefinition, MountDefinition, MountedObjectStore, OpendalFileStorage,
    VfsManifest,
};

pub(crate) struct MountedVfsHarness {
    pub storage: Arc<OpendalFileStorage>,
    pub data_dir: tempfile::TempDir,
    // Keeps the source backend alive even though tests read only through VFS.
    pub _source_dir: tempfile::TempDir,
}

pub(crate) fn mounted_vfs(files: &[(&str, &[u8])]) -> MountedVfsHarness {
    let data_dir = tempfile::tempdir().expect("runtime data directory");
    let source_dir = tempfile::tempdir().expect("bundle source directory");
    for (relative_path, content) in files {
        let path = source_dir
            .path()
            .join(relative_path.trim_start_matches('/'));
        std::fs::create_dir_all(path.parent().expect("file has parent")).unwrap();
        std::fs::write(path, content).unwrap();
    }

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
    let storage = Arc::new(OpendalFileStorage::with_mounts(data_dir.path(), mounted));

    MountedVfsHarness {
        storage,
        data_dir,
        _source_dir: source_dir,
    }
}
