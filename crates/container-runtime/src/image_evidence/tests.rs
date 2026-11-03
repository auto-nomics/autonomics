use super::*;
use crate::PrivateStorageDriver;
use std::{
    fs,
    os::unix::{
        ffi::OsStrExt,
        fs::{PermissionsExt, symlink},
    },
};

struct Fixture {
    _temporary: tempfile::TempDir,
    config: PodmanConfig,
    index: std::path::PathBuf,
    image: std::path::PathBuf,
    manifest_file: String,
    reference: String,
    manifest: String,
    image_config: String,
}

impl Fixture {
    fn new(driver: PrivateStorageDriver) -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let config = PodmanConfig::for_state_dir_with_storage_driver(temporary.path(), driver);
        let diff = format!("sha256:{}", "a".repeat(64));
        let image_config = format!(
            "{{\n  \"architecture\":\"amd64\",\"rootfs\":{{\"type\":\"layers\",\"diff_ids\":[\"{diff}\"]}}\n}}\n"
        );
        let config_digest = sha(image_config.as_bytes());
        let id = &config_digest[7..];
        let manifest = format!(
            "{{\n\"schemaVersion\":2,\"mediaType\":\"application/vnd.oci.image.manifest.v1+json\",\"config\":{{\"digest\":\"{config_digest}\",\"size\":{}}},\"layers\":[{{\"mediaType\":\"application/vnd.oci.image.layer.v1.tar\",\"digest\":\"{diff}\",\"size\":1024}}]\n}}\n",
            image_config.len()
        );
        let digest = sha(manifest.as_bytes());
        let key = format!("manifest-{digest}");
        let images = config.storage_root.as_ref().unwrap().join(format!(
            "{}-images",
            config.storage_driver.as_ref().unwrap()
        ));
        let image = images.join(id);
        fs::create_dir_all(&image).unwrap();
        let manifest_file = key_filename(&key);
        fs::write(image.join(&manifest_file), &manifest).unwrap();
        // The mutable default can point at a different serialization. Always
        // use the digest-specific original when the store records it.
        fs::write(image.join("manifest"), "SYNTHETIC OTHER SERIALIZATION").unwrap();
        fs::write(image.join(key_filename(&config_digest)), &image_config).unwrap();
        let index = images.join("images.json");
        fs::write(
            &index,
            serde_json::to_vec(&json!([{
                "id":id,"digest":digest,"names":["localhost:5000/owner/demo:renamed"],
                "big-data-names":[key,"manifest",config_digest],"big-data-digests":{key:digest}
            }]))
            .unwrap(),
        )
        .unwrap();
        Self {
            _temporary: temporary,
            config,
            index,
            image,
            manifest_file,
            reference: format!("localhost:5000/owner/demo@{digest}"),
            manifest,
            image_config,
        }
    }

    fn inspect(&self) -> Result<ImageEvidence, String> {
        inspect(&self.config, &self.reference)
    }

    fn rewrite_index(&self, change: impl FnOnce(&mut Value)) {
        let mut rows: Value = serde_json::from_slice(&fs::read(&self.index).unwrap()).unwrap();
        change(&mut rows);
        fs::write(&self.index, serde_json::to_vec(&rows).unwrap()).unwrap();
    }

    fn replace_manifest(&mut self, change: impl FnOnce(&mut Value)) {
        let mut manifest: Value = serde_json::from_str(&self.manifest).unwrap();
        change(&mut manifest);
        self.manifest = serde_json::to_string(&manifest).unwrap();
        let digest = sha(self.manifest.as_bytes());
        let key = format!("manifest-{digest}");
        self.manifest_file = key_filename(&key);
        fs::write(self.image.join(&self.manifest_file), &self.manifest).unwrap();
        self.reference = format!("localhost:5000/owner/demo@{digest}");
        self.rewrite_index(|rows| {
            rows[0]["digest"] = json!(digest);
            rows[0]["big-data-names"] = json!([key]);
        });
    }
}

#[test]
fn exact_original_bytes_for_both_private_backends_without_initializing_podman() {
    for driver in [PrivateStorageDriver::Vfs, PrivateStorageDriver::Overlay] {
        let mut f = Fixture::new(driver);
        f.config.program = "/SYNTHETIC-MUST-NOT-EXECUTE".into();
        let before = fs::read(&f.index).unwrap();
        let evidence = f.inspect().unwrap();
        assert_eq!(evidence.manifest_json, f.manifest);
        assert_eq!(evidence.config_json, f.image_config);
        assert_eq!(
            evidence.receipt["manifest"]["sha256"],
            &sha(f.manifest.as_bytes())[7..]
        );
        assert_eq!(evidence.receipt["layer_count"], 1);
        assert_eq!(
            evidence.receipt["source_repository_mapping"]["names"],
            json!(["localhost:5000/owner/demo:renamed"])
        );
        assert_eq!(evidence.receipt["manifest_config_binding_verified"], true);
        for field in [
            "layer_bytes_verified",
            "archive_export_verified",
            "sif_mapping_verified",
            "dependencies_qualified",
            "store_mutated",
        ] {
            assert_eq!(evidence.receipt[field], false, "{field}");
        }
        assert_eq!(fs::read(&f.index).unwrap(), before);
        assert!(!f.config.run_root.as_ref().unwrap().exists());
        assert!(
            !f.config
                .storage_root
                .as_ref()
                .unwrap()
                .join("db.sql")
                .exists()
        );
        let encoded = serde_json::to_vec(&evidence).unwrap();
        let decoded: ImageEvidence = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded.manifest_json.as_bytes(), f.manifest.as_bytes());
        #[cfg(target_os = "linux")]
        {
            let ancestor = f._temporary.path();
            let original = fs::metadata(ancestor).unwrap().permissions();
            fs::set_permissions(ancestor, fs::Permissions::from_mode(0o111)).unwrap();
            let through_search_only_ancestor = f.inspect();
            // Restore before unwrapping so a failing regression remains
            // self-cleaning even when the process cannot list this directory.
            fs::set_permissions(ancestor, original).unwrap();
            let evidence = through_search_only_ancestor.unwrap();
            assert_eq!(evidence.manifest_json.as_bytes(), f.manifest.as_bytes());
        }
    }
}

#[test]
fn wrong_repository_digest_mutable_reference_and_store_never_fall_back() {
    let f = Fixture::new(PrivateStorageDriver::Overlay);
    for reference in [
        f.reference.replace("owner/demo", "other/demo"),
        format!("localhost:5000/owner/demo@sha256:{}", "b".repeat(64)),
        "localhost:5000/owner/demo:renamed".into(),
        "../../images.json".into(),
    ] {
        assert!(inspect(&f.config, &reference).is_err());
    }
    let wrong = PodmanConfig::for_state_dir(f._temporary.path());
    assert!(inspect(&wrong, &f.reference).is_err());
}

#[test]
fn missing_or_ambiguous_repository_mapping_is_not_a_successful_inventory() {
    let f = Fixture::new(PrivateStorageDriver::Vfs);
    f.rewrite_index(|rows| {
        let duplicate = rows[0].clone();
        rows.as_array_mut().unwrap().push(duplicate);
    });
    assert!(f.inspect().unwrap_err().contains("ambiguous"));
    fs::write(&f.index, b"malformed").unwrap();
    assert!(f.inspect().unwrap_err().contains("invalid image index"));
}

#[test]
fn matching_store_labels_do_not_authenticate_wrong_manifest_or_config_bytes() {
    let f = Fixture::new(PrivateStorageDriver::Vfs);
    let path = f.image.join(&f.manifest_file);
    fs::write(&path, format!("{} ", f.manifest)).unwrap();
    assert!(f.inspect().unwrap_err().contains("manifest bytes"));
    fs::write(path, &f.manifest).unwrap();
    let config_digest = sha(f.image_config.as_bytes());
    fs::write(f.image.join(key_filename(&config_digest)), b"{}").unwrap();
    assert!(f.inspect().unwrap_err().contains("config bytes"));
}

#[test]
fn image_id_traversal_and_symlinked_directories_are_rejected() {
    let f = Fixture::new(PrivateStorageDriver::Overlay);
    f.rewrite_index(|rows| rows[0]["id"] = json!("../escape"));
    assert!(f.inspect().unwrap_err().contains("invalid image ID"));
    let f = Fixture::new(PrivateStorageDriver::Overlay);
    let original = f.image.with_extension("original");
    fs::rename(&f.image, &original).unwrap();
    symlink(original, &f.image).unwrap();
    assert!(f.inspect().is_err());
}

#[test]
fn symlinks_fifo_and_oversized_blobs_are_refused_without_blocking_or_following() {
    let f = Fixture::new(PrivateStorageDriver::Overlay);
    let blob = f.image.join(&f.manifest_file);
    fs::remove_file(&blob).unwrap();
    symlink(f.image.join("manifest"), &blob).unwrap();
    assert!(f.inspect().is_err());
    fs::remove_file(&blob).unwrap();
    let name = CString::new(blob.as_os_str().as_bytes()).unwrap();
    // SAFETY: valid NUL-terminated fixture path; creates a toy FIFO only.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    assert!(f.inspect().unwrap_err().contains("regular store file"));
    fs::remove_file(&blob).unwrap();
    File::create(&blob).unwrap().set_len(MAX_BLOB + 1).unwrap();
    assert!(f.inspect().unwrap_err().contains("bounded regular"));
}

#[test]
fn generic_manifest_is_allowed_only_when_its_actual_bytes_match() {
    let f = Fixture::new(PrivateStorageDriver::Overlay);
    f.rewrite_index(|rows| rows[0]["big-data-names"] = json!(["manifest"]));
    assert!(f.inspect().unwrap_err().contains("manifest bytes"));
    fs::write(f.image.join("manifest"), &f.manifest).unwrap();
    assert_eq!(
        f.inspect().unwrap().receipt["source_manifest_file"],
        "manifest"
    );
}

#[test]
fn content_addressed_but_inconsistent_descriptors_do_not_pass_binding() {
    let changes: Vec<Box<dyn FnOnce(&mut Value)>> = vec![
        Box::new(|m| m["config"]["size"] = json!(1)),
        Box::new(|m| m["config"]["digest"] = json!(format!("sha256:{}", "b".repeat(64)))),
        Box::new(|m| m["layers"] = json!([])),
        Box::new(|m| m["layers"][0]["digest"] = json!(format!("sha256:{}", "b".repeat(64)))),
        Box::new(|m| m["layers"][0]["size"] = json!(-1)),
        Box::new(|m| m["mediaType"] = json!("application/vnd.oci.image.index.v1+json")),
    ];
    for change in changes {
        let mut f = Fixture::new(PrivateStorageDriver::Overlay);
        f.replace_manifest(change);
        assert!(f.inspect().is_err());
    }
}
