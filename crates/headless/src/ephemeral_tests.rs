use super::{
    EphemeralBackend, EphemeralMount, EphemeralRunSpec, EphemeralSetupError, RunTaskConfig,
};
use std::sync::Arc;
use vfs::{MountedObjectStore, OpendalFileStorage, VfsManifest};

#[tokio::test]
async fn ephemeral_manifest_mounts_read_only_data_and_writable_workspace() {
    let data = tempfile::tempdir().unwrap();
    std::fs::write(data.path().join("input.txt"), b"benchmark-input").unwrap();
    let workspace = tempfile::tempdir().unwrap();

    let mut spec = EphemeralRunSpec::default();
    spec.data_mounts
        .push(EphemeralMount::new(data.path(), "/data", true).unwrap());
    spec.workspace = Some(EphemeralMount::new(workspace.path(), "/app", false).unwrap());

    let (config, _state) = RunTaskConfig::ephemeral_with_mounts("benchmark", spec).unwrap();
    let manifest_path = config.runtime_config.state_dir.join("vfs.toml");
    let source = std::fs::read_to_string(&manifest_path).unwrap();
    let manifest = VfsManifest::from_toml(&source).unwrap();

    let data_mount = manifest
        .mount
        .iter()
        .find(|mount| mount.path == "/data")
        .unwrap();
    assert!(data_mount.read_only);
    let workspace_mount = manifest
        .mount
        .iter()
        .find(|mount| mount.path == "/app")
        .unwrap();
    assert!(!workspace_mount.read_only);
    assert!(!source.contains("task.json"));
    assert!(!source.contains("rubric"));

    let mounts = Arc::new(MountedObjectStore::from_manifest(&manifest).unwrap());
    let storage = Arc::new(OpendalFileStorage::with_mounts(
        &config.runtime_config.data_dir,
        mounts,
    ));
    let data_path = storage.resolve_path("/data/input.txt");
    let data_bytes = storage
        .resolve("/data/input.txt")
        .read(&data_path)
        .await
        .unwrap();
    assert_eq!(data_bytes.to_vec(), b"benchmark-input");

    storage
        .write_bytes("/app/answer.txt", b"42".to_vec())
        .await
        .unwrap();
    assert!(
        storage
            .write_bytes("/data/input.txt", b"forbidden".to_vec())
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read(workspace.path().join("answer.txt")).unwrap(),
        b"42".to_vec()
    );
}

#[test]
fn non_empty_workspace_requires_explicit_resume() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("existing.txt"), b"existing").unwrap();

    let mut spec = EphemeralRunSpec::default();
    spec.workspace = Some(EphemeralMount::new(workspace.path(), "/app", false).unwrap());
    let error = match RunTaskConfig::ephemeral_with_mounts("prompt", spec.clone()) {
        Err(error) => error,
        Ok(_) => panic!("non-empty workspace should be rejected"),
    };
    assert!(matches!(
        error,
        EphemeralSetupError::NonEmptyWorkspace { .. }
    ));

    spec.resume_workspace = true;
    assert!(RunTaskConfig::ephemeral_with_mounts("prompt", spec).is_ok());
}

#[test]
fn overlapping_and_reserved_mount_targets_are_rejected() {
    let first = tempfile::tempdir().unwrap();
    let nested = tempfile::tempdir().unwrap();
    let mut spec = EphemeralRunSpec::default();
    spec.data_mounts
        .push(EphemeralMount::new(first.path(), "/data", true).unwrap());
    spec.data_mounts
        .push(EphemeralMount::new(nested.path(), "/data/nested", true).unwrap());
    let error = match RunTaskConfig::ephemeral_with_mounts("prompt", spec) {
        Err(error) => error,
        Ok(_) => panic!("overlapping mounts should be rejected"),
    };
    assert!(matches!(error, EphemeralSetupError::MountOverlap { .. }));

    let mut reserved = EphemeralRunSpec::default();
    reserved
        .data_mounts
        .push(EphemeralMount::new(first.path(), "/literature", true).unwrap());
    let error = match RunTaskConfig::ephemeral_with_mounts("prompt", reserved) {
        Err(error) => error,
        Ok(_) => panic!("reserved mount target should be rejected"),
    };
    assert!(matches!(
        error,
        EphemeralSetupError::ReservedTargetOverlap { .. }
    ));
}

#[test]
fn ephemeral_state_cleanup_and_keep_state_are_explicit() {
    let (_config, state) = RunTaskConfig::ephemeral("prompt");
    let root = state.root().to_path_buf();
    drop(state);
    assert!(!root.exists());

    let (config, mut state) = RunTaskConfig::ephemeral("prompt");
    let kept = state.keep().to_path_buf();
    drop(state);
    assert!(kept.exists());
    assert!(std::fs::remove_dir_all(&kept).is_ok());
    assert!(config.runtime_config.state_dir.starts_with(&kept));
    assert_eq!(
        config.runtime_config.opengwas_cache_dir.as_deref(),
        Some(kept.as_path().join("cache/opengwas").as_path())
    );
}

#[test]
fn mount_manifest_parses_only_workspace_and_data_intent() {
    let source = r#"
[workspace]
source = "/absolute/work"
target = "/app"

[[data_mounts]]
source = "/absolute/data"
target = "/data"
"#;
    let spec = RunTaskConfig::parse_mount_manifest(source).unwrap();
    assert_eq!(spec.backend, EphemeralBackend::InProcess);
    assert_eq!(spec.workspace.as_ref().unwrap().target, "/app");
    assert!(spec.data_mounts[0].read_only);

    assert!(RunTaskConfig::parse_mount_manifest("unknown = true").is_err());
}
