use std::{process::Command, sync::Arc};

use datafusion::prelude::{CsvReadOptions, SessionContext};
use vfs::{
    BackendConfig, BackendDefinition, MountDefinition, MountedObjectStore, OpendalFileStorage,
    VfsManifest,
};

struct RemoteTemp {
    alias: String,
    root: String,
}

impl RemoteTemp {
    fn create(alias: String) -> Self {
        assert!(!alias.is_empty() && !alias.starts_with('-'));
        let output = Command::new("ssh")
            .args([
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=10",
                "--",
                &alias,
            ])
            .arg("mktemp -d /tmp/autonomics-vfs-domain.XXXXXXXXXX")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let root = String::from_utf8(output.stdout).unwrap().trim().to_owned();
        let suffix = root.strip_prefix("/tmp/autonomics-vfs-domain.").unwrap();
        assert!(suffix.len() == 10 && suffix.bytes().all(|b| b.is_ascii_alphanumeric()));
        Self { alias, root }
    }
}

impl Drop for RemoteTemp {
    fn drop(&mut self) {
        // The path is created by this test and validated to contain no shell syntax.
        let status = Command::new("ssh")
            .args([
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=10",
                "--",
                &self.alias,
            ])
            .arg(format!("rm -rf -- {}", self.root))
            .status();
        assert!(
            status.is_ok_and(|status| status.success()),
            "remote test cleanup failed"
        );
    }
}

#[tokio::test]
#[ignore = "requires AUTONOMICS_VFS_TEST_SSH_ALIAS and a live SSH server"]
async fn domain_addresses_round_trip_over_real_ssh() {
    let remote = RemoteTemp::create(std::env::var("AUTONOMICS_VFS_TEST_SSH_ALIAS").unwrap());
    let local = tempfile::tempdir().unwrap();
    let python = std::env::var("AUTONOMICS_VFS_TEST_PYTHON").unwrap_or_else(|_| "python3".into());
    // Prepare protected data before publishing the read-only mount.
    let seed = vfs::build_ssh_operator(&remote.alias, &remote.root, &python, None).unwrap();
    seed.create_dir("readonly/").await.unwrap();
    seed.write("readonly/seed.txt", b"protected".to_vec())
        .await
        .unwrap();
    let manifest = VfsManifest {
        backend: vec![BackendDefinition {
            id: "live-ssh".into(),
            config: BackendConfig::Ssh {
                alias: remote.alias.clone(),
                root: remote.root.clone(),
                python,
                endpoint: None,
            },
        }],
        mount: vec![
            MountDefinition {
                domain: Some("ht".into()),
                path: "/data/ht".into(),
                backend: "live-ssh".into(),
                source: "/".into(),
                read_only: false,
                permissions: Default::default(),
            },
            MountDefinition {
                domain: None,
                path: "/data/ht/readonly".into(),
                backend: "live-ssh".into(),
                source: format!("{}/readonly", remote.root),
                read_only: true,
                permissions: Default::default(),
            },
        ],
    };
    let mounts = Arc::new(MountedObjectStore::from_manifest(&manifest).unwrap());
    let storage = OpendalFileStorage::with_mounts(local.path(), mounts);
    assert_eq!(
        storage
            .read_range("ht://readonly/seed.txt", 0..9)
            .await
            .unwrap()
            .to_vec(),
        b"protected"
    );
    storage
        .write_bytes("local://same.txt", b"local".to_vec())
        .await
        .unwrap();
    storage
        .write_bytes("ht://same.txt", b"remote".to_vec())
        .await
        .unwrap();
    assert_eq!(
        storage
            .read_range("local://same.txt", 0..5)
            .await
            .unwrap()
            .to_vec(),
        b"local"
    );
    assert_eq!(
        storage
            .read_range("ht://same.txt", 0..6)
            .await
            .unwrap()
            .to_vec(),
        b"remote"
    );
    assert_eq!(
        storage.canonical_uri("/data/ht/same.txt").unwrap(),
        "ht://same.txt"
    );
    let location = storage.ssh_location("ht://same.txt").unwrap().unwrap();
    assert_eq!(location.path, format!("{}/same.txt", remote.root));

    vfs::browse::copy(&storage, "local://same.txt", "ht://uploaded.txt")
        .await
        .unwrap();
    vfs::browse::copy(&storage, "ht://uploaded.txt", "local://returned.txt")
        .await
        .unwrap();
    assert_eq!(
        storage
            .read_range("local://returned.txt", 0..5)
            .await
            .unwrap()
            .to_vec(),
        b"local"
    );
    vfs::browse::move_entry(&storage, "ht://uploaded.txt", "local://moved.txt")
        .await
        .unwrap();
    assert!(storage.content_length("ht://uploaded.txt").await.is_err());
    assert_eq!(
        storage
            .read_range("local://moved.txt", 0..5)
            .await
            .unwrap()
            .to_vec(),
        b"local"
    );
    vfs::browse::move_entry(&storage, "local://moved.txt", "ht://moved.txt")
        .await
        .unwrap();
    assert!(storage.content_length("local://moved.txt").await.is_err());
    assert_eq!(
        storage
            .read_range("ht://moved.txt", 0..5)
            .await
            .unwrap()
            .to_vec(),
        b"local"
    );
    assert!(
        vfs::browse::move_entry(&storage, "local://same.txt", "ht://readonly/blocked.txt")
            .await
            .is_err()
    );
    assert_eq!(
        storage
            .read_range("local://same.txt", 0..5)
            .await
            .unwrap()
            .to_vec(),
        b"local"
    );
    assert!(
        storage
            .content_length("local://data/ht/same.txt")
            .await
            .is_err()
    );
    assert!(storage.content_length("unknown://same.txt").await.is_err());

    let csv = storage.canonical_uri("/data/ht/table %part.csv").unwrap();
    storage
        .write_bytes(&csv, b"value\n19\n".to_vec())
        .await
        .unwrap();
    let ctx = SessionContext::new();
    storage.register_domain_stores(&ctx.runtime_env()).unwrap();
    let frame = ctx
        .read_csv(vfs::uri::datafusion_uri(&csv), CsvReadOptions::default())
        .await
        .unwrap();
    let batches = frame.clone().collect().await.unwrap();
    assert_eq!(
        batches.iter().map(|batch| batch.num_rows()).sum::<usize>(),
        1
    );
    // Exercise DataFusion's ObjectStore write path, including creation of
    // previously absent parents, rather than only direct VFS byte writes.
    let parquet = "ht://parquet/deep/table%20%25.parquet";
    frame
        .write_parquet(
            &vfs::uri::datafusion_uri(parquet),
            datafusion::dataframe::DataFrameWriteOptions::new().with_single_file_output(true),
            None::<datafusion::common::config::TableParquetOptions>,
        )
        .await
        .unwrap();
    let batches = ctx
        .read_parquet(vfs::uri::datafusion_uri(parquet), Default::default())
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(
        batches.iter().map(|batch| batch.num_rows()).sum::<usize>(),
        1
    );
}
