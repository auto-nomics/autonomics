use super::{cached_file_changed, hex_lower};
use crate::value::{FileFingerprint, FileRef};
use sha2::{Digest, Sha256};
use std::sync::Arc;

fn shadow_storage(host: &std::path::Path, shadow: &std::path::Path) -> vfs::OpendalFileStorage {
    let manifest = vfs::VfsManifest {
        backend: vec![vfs::BackendDefinition {
            id: "shadow".into(),
            config: vfs::BackendConfig::local(shadow.to_string_lossy().to_string()),
        }],
        mount: vec![vfs::MountDefinition {
            domain: None,
            path: host.to_string_lossy().to_string(),
            backend: "shadow".into(),
            source: shadow.to_string_lossy().to_string(),
            read_only: true,
            permissions: Default::default(),
        }],
    };
    vfs::OpendalFileStorage::with_mounts(
        shadow,
        Arc::new(vfs::MountedObjectStore::from_manifest(&manifest).unwrap()),
    )
}

#[tokio::test]
async fn resolved_host_hash_must_not_be_shadowed_by_virtual_mount() {
    let host = tempfile::tempdir().unwrap();
    let shadow = tempfile::tempdir().unwrap();
    let path = host.path().join("input.bin");
    std::fs::write(&path, b"host physical bytes").unwrap();
    std::fs::write(shadow.path().join("input.bin"), b"virtual shadow bytes").unwrap();
    let storage = shadow_storage(host.path(), shadow.path());
    assert!(storage.is_mounted(path.to_str().unwrap()));
    let expected = hex_lower(&Sha256::digest(b"host physical bytes"));
    for spelling in [
        path.to_string_lossy().to_string(),
        format!("file://{}", path.display()),
    ] {
        let (size, digest) = super::file_sha256(&spelling, Some(&storage)).await.unwrap();
        assert_eq!(size, b"host physical bytes".len() as u64);
        assert_eq!(digest, expected, "resolved host namespace: {spelling}");
    }
    let (_, digest) = super::file_sha256(&vfs::uri::uri(path.display()), Some(&storage))        .await
        .unwrap();
    assert_eq!(digest, hex_lower(&Sha256::digest(b"virtual shadow bytes")));
}

#[tokio::test]
async fn virtual_shadow_cannot_hide_host_content_tampering() {
    let host = tempfile::tempdir().unwrap();
    let shadow = tempfile::tempdir().unwrap();
    let path = host.path().join("input.bin");
    std::fs::write(&path, b"original").unwrap();
    std::fs::write(shadow.path().join("input.bin"), b"original").unwrap();
    let storage = shadow_storage(host.path(), shadow.path());
    let mut fingerprint = FileFingerprint::from_path(&path).unwrap();
    fingerprint.content_hash = Some(hex_lower(&Sha256::digest(b"original")));
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
    std::fs::write(&path, b"tampered").unwrap();
    std::fs::File::open(&path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(modified))
        .unwrap();
    for spelling in [
        path.to_string_lossy().to_string(),
        format!("file://{}", path.display()),
    ] {
        let file = FileRef {
            path: spelling,
            format: None,
            fingerprint: Some(fingerprint.clone()),
        };
        assert!(
            cached_file_changed(&file, Some(&storage)).await,
            "unchanged shadow bytes cannot justify reuse after host tampering"
        );
    }
    std::fs::remove_file(&path).unwrap();
    assert!(
        super::file_sha256(path.to_str().unwrap(), Some(&storage))
            .await
            .is_none(),
        "missing host bytes cannot be replaced by a virtual object"
    );
}

#[tokio::test]
async fn required_content_identity_cannot_accept_virtual_shadow_for_host_input() {
    let host = tempfile::tempdir().unwrap();
    let shadow = tempfile::tempdir().unwrap();
    let path = host.path().join("input.bin");
    std::fs::write(&path, b"tampered").unwrap();
    std::fs::write(shadow.path().join("input.bin"), b"original").unwrap();
    let storage = shadow_storage(host.path(), shadow.path());
    let expected = FileFingerprint {
        size: 8,
        mtime_ns: 0,
        content_hash: Some(hex_lower(&Sha256::digest(b"original"))),
        immutable_remote: false,
    };
    for spelling in [
        path.to_string_lossy().to_string(),
        format!("file://{}", path.display()),
    ] {
        for value in [
            super::IdentityValue::File {
                path: spelling.clone(),
                fingerprint: Some(expected.clone()),
            },
            super::IdentityValue::FileSet(vec![(spelling.clone(), Some(expected.clone()))]),
        ] {
            let mut identities = [super::InputIdentity {
                from: "source".into(),
                from_port: 0,
                to_port: 0,
                value,
            }];
            let error = super::require_content_hashes(&mut identities, Some(&storage))
                .await
                .unwrap_err();
            assert!(error.contains("required content hash mismatch"), "{error}");
        }
    }
    let mut virtual_input = [super::InputIdentity {
        from: "source".into(),
        from_port: 0,
        to_port: 0,
        value: super::IdentityValue::File {
            path: format!("vfs://{}", path.display()),
            fingerprint: Some(expected.clone()),
        },
    }];
    super::require_content_hashes(&mut virtual_input, Some(&storage))
        .await
        .unwrap();
    // Restore the real host bytes to show the gate accepts their own identity.
    std::fs::write(&path, b"original").unwrap();
    let mut host_input = [super::InputIdentity {
        from: "source".into(),
        from_port: 0,
        to_port: 0,
        value: super::IdentityValue::File {
            path: format!("file://{}", path.display()),
            fingerprint: Some(expected),
        },
    }];
    super::require_content_hashes(&mut host_input, Some(&storage))
        .await
        .unwrap();
    std::fs::remove_file(&path).unwrap();
    let error = super::require_content_hashes(&mut host_input, Some(&storage))
        .await
        .unwrap_err();
    assert!(
        error.contains("required content hash unavailable"),
        "{error}"
    );
}

#[tokio::test]
async fn changed_same_size_preserved_mtime_must_not_override_sha256() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("content.bin");
    std::fs::write(&path, b"original").unwrap();
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
    let mut expected = FileFingerprint::from_path(&path).unwrap();
    expected.content_hash = Some(format!(
        "sha256:{}",
        hex_lower(&Sha256::digest(b"original"))
    ));
    std::fs::write(&path, b"tampered").unwrap();
    std::fs::File::open(&path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(modified))
        .unwrap();
    let current = FileFingerprint::from_path(&path).unwrap();
    assert_eq!(current.size, expected.size);
    assert_eq!(current.mtime_ns, expected.mtime_ns);
    println!("MATCHING_METADATA_TAMPER_CONTROL_READY");
    let file = FileRef {
        path: path.to_string_lossy().into_owned(),
        format: None,
        fingerprint: Some(expected),
    };
    assert!(cached_file_changed(&file, None).await);
}

#[tokio::test]
async fn incorrect_recorded_hash_is_not_ignored_when_metadata_matches() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("content.bin");
    std::fs::write(&path, b"original").unwrap();
    let mut expected = FileFingerprint::from_path(&path).unwrap();
    expected.content_hash = Some(format!(
        "sha256:{}",
        hex_lower(&Sha256::digest(b"tampered"))
    ));
    let file = FileRef {
        path: path.to_string_lossy().into_owned(),
        format: None,
        fingerprint: Some(expected),
    };
    assert!(cached_file_changed(&file, None).await);
}

#[tokio::test]
async fn unsupported_content_hash_is_not_downgraded_to_metadata_success() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("content.bin");
    std::fs::write(&path, b"original").unwrap();
    let mut expected = FileFingerprint::from_path(&path).unwrap();
    expected.content_hash = Some("unknown:identity".into());
    let file = FileRef {
        path: path.to_string_lossy().into_owned(),
        format: None,
        fingerprint: Some(expected),
    };
    assert!(cached_file_changed(&file, None).await);
}

#[tokio::test]
async fn unchanged_content_with_matching_metadata_and_hash_remains_clean() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("content.bin");
    std::fs::write(&path, b"original").unwrap();
    let mut expected = FileFingerprint::from_path(&path).unwrap();
    expected.content_hash = Some(format!(
        "sha256:{}",
        hex_lower(&Sha256::digest(b"original"))
    ));
    let file = FileRef {
        path: path.to_string_lossy().into_owned(),
        format: None,
        fingerprint: Some(expected),
    };
    assert!(!cached_file_changed(&file, None).await);
}

#[tokio::test]
async fn bare_checkpoint_sha256_reuses_identical_bytes_and_detects_metadata_preserving_tamper() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("checkpoint.bin");
    std::fs::write(&path, b"original").unwrap();
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
    let mut expected = FileFingerprint::from_path(&path).unwrap();
    expected.content_hash = Some(hex_lower(&Sha256::digest(b"original")));
    let file = FileRef {
        path: path.to_string_lossy().into_owned(),
        format: None,
        fingerprint: Some(expected),
    };
    assert!(!cached_file_changed(&file, None).await);
    std::fs::write(&path, b"tampered").unwrap();
    std::fs::File::open(&path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(modified))
        .unwrap();
    let current = FileFingerprint::from_path(&path).unwrap();
    let recorded = file.fingerprint.as_ref().unwrap();
    assert_eq!(current.size, recorded.size);
    assert_eq!(current.mtime_ns, recorded.mtime_ns);
    assert!(cached_file_changed(&file, None).await);
}

#[tokio::test]
async fn malformed_or_other_hash_dialects_never_reuse_matching_metadata() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("checkpoint.bin");
    std::fs::write(&path, b"original").unwrap();
    let digest = hex_lower(&Sha256::digest(b"original"));
    for hash in [
        format!("sha512:{digest}"),
        format!("SHA256:{digest}"),
        format!("sha256:{}", &digest[..63]),
        format!("{digest}0"),
        "g".repeat(64),
        String::new(),
    ] {
        let mut expected = FileFingerprint::from_path(&path).unwrap();
        expected.content_hash = Some(hash.clone());
        let file = FileRef {
            path: path.to_string_lossy().into_owned(),
            format: None,
            fingerprint: Some(expected),
        };
        assert!(cached_file_changed(&file, None).await, "{hash}");
    }
}
