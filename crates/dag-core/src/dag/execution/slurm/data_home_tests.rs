use super::ssh::queue_stdout_text;
use super::transport::{render_bootstrap_script, render_sbatch_script};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::super::contract::{TaskAttemptReceipt, TaskDispatch, TaskExecution};
use super::super::process::{
    ProcessTaskSpec, parse_process_task, process_task_environment, render_process_script,
    sanitize_path_component,
};
use super::super::traits::{TaskArtifactStore, TaskExecutor, TaskLease, TaskTransport};
use super::*;
use crate::dag::DagError;
use crate::dag::node_event::JobResult;
use crate::value::{DomainId, FileFingerprint, FileRef};

fn store(root: &Path) -> SshArtifactStore {
    let ssh = root.join("fixture-ssh");
    std::fs::write(
        &ssh,
        b"#!/bin/sh\nwhile [ \"$1\" = -o ]; do shift 2; done\nshift\nexec /bin/sh -c \"$*\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o700)).unwrap();
    SshArtifactStore::new(
        SshTarget::new(ssh.to_string_lossy(), "fixture"),
        root.join("attempts").to_string_lossy(),
        root.join("unused-downloads"),
    )
    .with_data_home(SshDataHome {
        ssh_aliases: vec!["fixture".into()],
        remote_root: root.to_string_lossy().into_owned(),
        vfs_prefix: "/cluster".into(),
        domain_id: Some("hpc-b".into()),
        retained_vfs_prefix: None,
        retained_remote_root: None,
    })
    .unwrap()
}
#[tokio::test]
async fn retained_files_are_measured_and_referenced_without_downloads() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let source = root.join("answer.tsv");
    std::fs::write(&source, b"value\n7\n").unwrap();
    let store = store(root);
    let reference = store
        .retained_reference(FileRef::local(&source, Some("tsv".into())).unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reference.path, "hpc-b://answer.tsv");
    assert_eq!(reference.format.as_deref(), Some("tsv"));
    // The retained publication carries the declared domain identity.
    assert_eq!(reference.domain.as_str(), "hpc-b");
    let identity = reference.fingerprint.unwrap();
    assert_eq!(
        identity.content_hash,
        Some(format!("{:x}", Sha256::digest(b"value\n7\n")))
    );
    assert!(!identity.immutable_remote);
    assert!(!root.join("unused-downloads").exists());
}
#[tokio::test]
async fn remote_inputs_use_authorized_shared_paths_and_reject_changed_content() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let source = root.join("input.tsv");
    std::fs::write(&source, b"known").unwrap();
    let manifest = vfs::VfsManifest::from_toml(&format!(
        "[[backend]]\nid='cluster'\ntype='ssh'\nalias='fixture'\nroot={}\n[[mount]]\npath='/cluster'\nbackend='cluster'\nsource='/'\nread_only=true\n", serde_json::to_string(&root.to_string_lossy()).unwrap(),
    )).unwrap();
    let mounts = Arc::new(vfs::MountedObjectStore::from_manifest(&manifest).unwrap());
    let storage = vfs::OpendalFileStorage::with_mounts(root, mounts);
    let store = store(root);
    let input = FileRef::remote(
        "vfs:///cluster/input.tsv",
        Some("tsv".into()),
        5,
        Some(format!("sha256:{:x}", Sha256::digest(b"known"))),
    );
    let remote = store
        .remote_input(input.clone(), Some(&storage))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(remote.path, source.to_string_lossy());
    // In-place references are stamped with the data home's domain id.
    assert_eq!(remote.domain.as_str(), "hpc-b");
    std::fs::write(&source, b"other").unwrap();
    assert!(store.remote_input(input, Some(&storage)).await.is_err());
    assert!(!root.join("unused-downloads").exists());
}
#[tokio::test]
async fn retained_data_rejects_symlinks_and_paths_outside_its_root() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    std::fs::write(&source, b"fixture").unwrap();
    let link = directory.path().join("link");
    symlink(&source, &link).unwrap();
    let store = store(directory.path());
    assert!(
        store
            .retained_reference(FileRef::new(link.to_string_lossy(), None))
            .await
            .is_err()
    );
    assert!(
        store
            .retained_reference(FileRef::new("/etc/passwd", None))
            .await
            .is_err()
    );
}
#[tokio::test]
async fn another_data_source_is_staged_instead_of_reusing_its_physical_path() {
    let directory = tempfile::tempdir().unwrap();
    let mut manifest = vfs::VfsManifest::from_toml("[[backend]]\nid='archive'\ntype='ssh'\nalias='archive-server'\nroot='/retained/study'\n[[mount]]\npath='/data/archive'\nbackend='archive'\nsource='/'\nread_only=true\n").unwrap();
    let storage = vfs::OpendalFileStorage::with_mounts(
        directory.path(),
        Arc::new(vfs::MountedObjectStore::from_manifest(&manifest).unwrap()),
    );
    let store = store(directory.path());
    let file = FileRef::new("vfs:///data/archive/input.tsv", Some("tsv".into()));
    assert!(
        store
            .remote_input(file.clone(), Some(&storage))
            .await
            .unwrap()
            .is_none()
    );
    // An authorized source on the same server can still live outside the
    // compute target's shared data home. Stage it through its source mount.
    if let vfs::BackendConfig::Ssh { alias, .. } = &mut manifest.backend[0].config {
        *alias = "fixture".into();
    }
    let storage = vfs::OpendalFileStorage::with_mounts(
        directory.path(),
        Arc::new(vfs::MountedObjectStore::from_manifest(&manifest).unwrap()),
    );
    assert!(
        store
            .remote_input(file.clone(), Some(&storage))
            .await
            .unwrap()
            .is_none()
    );
    // Even the same alias cannot authorize zero-copy access to a different
    // pinned endpoint. The source bytes must be staged through its own VFS.
    if let vfs::BackendConfig::Ssh { root, endpoint, .. } = &mut manifest.backend[0].config {
        *root = directory.path().to_string_lossy().into_owned();
        *endpoint = Some(vfs::SshEndpoint {
            hostname: "archive.example".into(),
            user: "owner".into(),
            port: 22,
            host_key_alias: "archive.example".into(),
        });
    }
    let storage = vfs::OpendalFileStorage::with_mounts(
        directory.path(),
        Arc::new(vfs::MountedObjectStore::from_manifest(&manifest).unwrap()),
    );
    assert!(
        store
            .remote_input(file, Some(&storage))
            .await
            .unwrap()
            .is_none()
    );
}
#[test]
fn retained_vfs_mapping_checks_root_host_and_nested_overrides() {
    let root = tempfile::tempdir().unwrap();
    let mut manifest = vfs::VfsManifest::from_toml(
        "[[backend]]\nid='ssh'\ntype='ssh'\nalias='fixture'\nroot='/cluster/data'\n[[backend]]\nid='wrong'\ntype='ssh'\nalias='other'\nroot='/other/data'\n[[mount]]\npath='/cluster'\nbackend='ssh'\nsource='/'\nread_only=true\ndomain='hpc-b'\n"
    ).unwrap();
    let home = SshDataHome {
        ssh_aliases: vec!["fixture".into()],
        remote_root: "/cluster/data".into(),
        vfs_prefix: "/cluster".into(),
        domain_id: None,
        retained_vfs_prefix: None,
        retained_remote_root: None,
    };
    let validate = |manifest: &vfs::VfsManifest| {
        let mounts = Arc::new(vfs::MountedObjectStore::from_manifest(manifest).unwrap());
        home.validate_vfs(&vfs::OpendalFileStorage::with_mounts(root.path(), mounts))
    };
    assert!(validate(&manifest).is_ok());
    let mut nested = manifest.mount[0].clone();
    nested.path = "/cluster/outputs".into();
    nested.source = "outputs".into();
    // A nested sub-partition does not carry its own domain address.
    nested.domain = None;
    manifest.mount.push(nested);
    assert!(validate(&manifest).is_ok());
    manifest.mount[1].source = "different".into();
    assert!(validate(&manifest).is_err());
    manifest.mount[1].source = "outputs".into();
    manifest.mount[1].backend = "wrong".into();
    assert!(validate(&manifest).is_err());
    manifest.mount.pop();
    manifest.mount[0].source = "shifted".into();
    assert!(validate(&manifest).is_err());
}
#[tokio::test]
async fn dedicated_retained_namespace_does_not_narrow_admitted_input_root() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let input = root.join("input.tsv");
    let output = root.join("attempts/task/output.tsv");
    std::fs::create_dir_all(output.parent().unwrap()).unwrap();
    std::fs::write(&input, b"toy input").unwrap();
    std::fs::write(&output, b"toy result").unwrap();
    let mut store = store(root);
    let mut home = store.data_home.take().unwrap();
    home.retained_vfs_prefix = Some("/cluster/attempts".into());
    home.retained_remote_root = Some(root.join("attempts").to_string_lossy().into_owned());
    store = store.with_data_home(home).unwrap();
    let mut manifest = vfs::VfsManifest::from_toml(&format!(
        "[[backend]]\nid='ssh'\ntype='ssh'\nalias='fixture'\nroot={}\n[[mount]]\npath='/cluster'\nbackend='ssh'\nsource='/'\nread_only=true\ndomain='hpc-b'\n[[mount]]\npath='/cluster/attempts'\nbackend='ssh'\nsource='attempts'\nread_only=true\n",
        serde_json::to_string(&root.to_string_lossy()).unwrap(),
    )).unwrap();
    manifest.mount[0].permissions = vfs::permission::MountPermissions::unix(
        vfs::permission::VfsOwnership { uid: 0, gid: 0 },
        vfs::permission::VfsMode::from_bits(0o700),
        vfs::permission::VfsMode::from_bits(0o600),
        vfs::permission::VfsMode::from_bits(0o700),
    );
    let mounts = Arc::new(vfs::MountedObjectStore::from_manifest(&manifest).unwrap());
    let storage = vfs::OpendalFileStorage::with_mounts(root, mounts);
    let agent = storage.with_principal(vfs::permission::VfsPrincipal::new(12_345, 100));
    assert!(agent.check_readable("/cluster/input.tsv").is_err());
    assert!(
        agent
            .check_readable("/cluster/attempts/task/output.tsv")
            .is_ok()
    );
    store
        .data_home
        .as_ref()
        .unwrap()
        .validate_vfs(&storage)
        .unwrap();
    let remote = store
        .remote_input(
            FileRef::new("vfs:///cluster/input.tsv", None),
            Some(&storage),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(remote.path, input.to_string_lossy());
    let reference = store
        .retained_reference(FileRef::local(&output, Some("tsv".into())).unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reference.path, "hpc-b://attempts/task/output.tsv");
    assert!(agent.check_readable(&reference.path).is_ok());
    let location = agent.ssh_location(&reference.path).unwrap().unwrap();
    assert_eq!(location.path, output.to_string_lossy());
    assert!(
        store
            .retained_reference(FileRef::local(&input, None).unwrap())
            .await
            .is_err()
    );
}
#[test]
fn dedicated_retained_fields_are_paired_and_confined() {
    let legacy = serde_json::json!({"ssh_aliases":["fixture"],"remote_root":"/cluster/data","vfs_prefix":"/cluster"});
    let mut home: SshDataHome = serde_json::from_value(legacy).unwrap();
    assert!(home.validate().is_ok());
    home.retained_vfs_prefix = Some("/cluster-runs".into());
    assert!(home.validate().is_err());
    home.retained_remote_root = Some("/other/data".into());
    assert!(home.validate().is_err());
    home.retained_remote_root = Some("/cluster/data/attempts".into());
    assert!(home.validate().is_ok());
    home.retained_vfs_prefix = Some("/cluster".into());
    assert!(home.validate().is_err());
}
#[test]
fn exact_completed_job_queue_absence_never_accepts_unknown_errors() {
    use std::os::unix::process::ExitStatusExt;
    let output = |code: i32, stdout: &str, stderr: &str| std::process::Output {
        status: std::process::ExitStatus::from_raw(code << 8),
        stdout: stdout.as_bytes().to_vec(),
        stderr: stderr.as_bytes().to_vec(),
    };
    let absent = "slurm_load_jobs error: Invalid job id specified";
    assert!(slurm_queue_absent(&output(0, "", "")));
    assert!(slurm_queue_absent(&output(1, "", &format!("{absent}\n"))));
    assert!(
        queue_stdout_text(output(1, "", absent), "toy queue")
            .unwrap()
            .is_empty()
    );
    for (code, stdout, stderr) in [
        (0, "RUNNING\n", ""),
        (1, "RUNNING\n", absent),
        (1, "", ""),
        (
            1,
            "",
            "slurm_load_jobs error: Socket timed out on send/recv operation",
        ),
        (
            1,
            "",
            "warning\nslurm_load_jobs error: Invalid job id specified",
        ),
        (
            1,
            "",
            "slurm_load_jobs error: Invalid job id specified\nextra diagnostic",
        ),
        (2, "", absent),
        (255, "", absent),
    ] {
        assert!(!slurm_queue_absent(&output(code, stdout, stderr)));
        if code != 0 {
            assert!(queue_stdout_text(output(code, stdout, stderr), "toy queue").is_err());
        }
    }
    let signal = std::process::Output {
        status: std::process::ExitStatus::from_raw(9),
        stdout: vec![],
        stderr: absent.as_bytes().to_vec(),
    };
    assert!(!slurm_queue_absent(&signal));
}
#[test]
fn bootstrap_heredocs_keep_task_content_out_of_the_login_shell() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let fixture = tempfile::tempdir().unwrap();
    let bin = fixture.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let sbatch = bin.join("sbatch");
    std::fs::write(&sbatch, b"#!/bin/sh\nprintf '%s\\n' 4242\n").unwrap();
    std::fs::set_permissions(&sbatch, std::fs::Permissions::from_mode(0o700)).unwrap();
    let marker = fixture.path().join("MUST_NOT_RUN_ON_LOGIN_NODE");
    let payload = format!(
        "#!/bin/bash\nAUTONOMICS_SBATCH_EOF\ntouch {}\nAUTONOMICS_SBATCH_EOF_1\n",
        sh_single_quote(marker.to_str().unwrap()),
    );
    let manifest = "{\"synthetic\":true}";
    let task = fixture.path().join("task");
    let bootstrap = render_bootstrap_script(task.to_str().unwrap(), manifest, &payload).unwrap();
    let mut child = Command::new("bash")
        .arg("-s")
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(bootstrap.as_bytes())
        .unwrap();
    let result = child.wait_with_output().unwrap();
    assert!(
        result.status.success(),
        "toy bootstrap failed: {:?}",
        result
    );
    assert_eq!(result.stdout, b"4242\n");
    assert!(
        !marker.exists(),
        "a task heredoc delimiter escaped into the login shell"
    );
    assert_eq!(
        std::fs::read_to_string(task.join("job.sbatch")).unwrap(),
        payload
    );
    assert_eq!(
        std::fs::read_to_string(task.join("task.json")).unwrap(),
        format!("{manifest}\n")
    );
    assert_eq!(
        heredoc_delimiter("MARKER", "MARKER\nMARKER_1\nMARKER_2\n"),
        "MARKER_3"
    );
    assert_eq!(heredoc_delimiter("MARKER", "some MARKER text\n"), "MARKER");
}
#[test]
fn bootstrap_rejects_nul_before_bash_can_normalize_a_delimiter() {
    for payload in [
        "#!/bin/bash\nAUTONOMICS_SBATCH_E\0OF\ntouch MUST_NOT_RUN\n",
        "export SYNTHETIC='prefix\0suffix'\necho safe\n",
    ] {
        assert!(
            render_bootstrap_script("/synthetic/task", "{}", payload)
                .unwrap_err()
                .to_string()
                .contains("NUL")
        );
    }
    assert!(render_bootstrap_script("/synthetic/ta\0sk", "{}", "echo safe\n").is_err());
    assert!(render_bootstrap_script("/synthetic/task", "{\0}", "echo safe\n").is_err());
}
#[test]
fn batch_payload_does_not_run_when_task_directory_is_gone() {
    let fixture = tempfile::tempdir().unwrap();
    let marker = fixture.path().join("MUST_NOT_RUN_IN_FALLBACK_DIRECTORY");
    let spec = serde_json::json!({"script":format!("touch {}", sh_single_quote(marker.to_str().unwrap()))});
    let dispatch: TaskDispatch = serde_json::from_value(serde_json::json!({
        "spec":{"id":"synthetic","kind":"process","spec":spec,"inputs":[],"outputs":[]},
        "resources":{},
    }))
    .unwrap();
    let process = parse_process_task(&dispatch).unwrap();
    let settings = SlurmSettings {
        apptainer: None,
        ssh: SshTarget::new("MUST_NOT_EXECUTE_SSH", "synthetic"),
        scratch_dir: fixture.path().to_str().unwrap().into(),
        local_artifact_root: fixture.path().join("artifacts"),
        partition: "synthetic".into(),
        account: None,
        gpus: None,
        qos: None,
        mem_per_cpu: "1G".into(),
        default_time: "00:01:00".into(),
        poll_interval: Duration::from_secs(1),
    };
    let script = render_sbatch_script(
        &dispatch,
        &process,
        fixture
            .path()
            .join("missing-task-directory")
            .to_str()
            .unwrap(),
        &BTreeMap::new(),
        &BTreeMap::new(),
        &settings,
        "synthetic",
    )
    .unwrap();
    let result = std::process::Command::new("bash")
        .arg("-c")
        .arg(&script)
        .current_dir(fixture.path())
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(90));
    assert!(
        !marker.exists(),
        "missing workspace executed its payload in the fallback cwd"
    );
}
#[test]
fn writable_retained_mount_is_rejected() {
    let root = tempfile::tempdir().unwrap();
    let manifest = vfs::VfsManifest::from_toml(
        "[[backend]]\nid='ssh'\ntype='ssh'\nalias='fixture'\nroot='/cluster/data'\n[[mount]]\npath='/cluster'\nbackend='ssh'\nsource='/'\nread_only=false\n",
    )
    .unwrap();
    let home = SshDataHome {
        ssh_aliases: vec!["fixture".into()],
        remote_root: "/cluster/data".into(),
        vfs_prefix: "/cluster".into(),
        domain_id: None,
        retained_vfs_prefix: None,
        retained_remote_root: None,
    };
    let mounts = Arc::new(vfs::MountedObjectStore::from_manifest(&manifest).unwrap());
    let storage = vfs::OpendalFileStorage::with_mounts(root.path(), mounts);
    let error = home.validate_vfs(&storage).unwrap_err().to_string();
    assert!(error.contains("read_only"), "{error}");
}
#[test]
fn named_domain_must_match_its_mount_partition() {
    let root = tempfile::tempdir().unwrap();
    let manifest = vfs::VfsManifest::from_toml(
        "[[backend]]\nid='ssh'\ntype='ssh'\nalias='fixture'\nroot='/cluster/data'\n[[mount]]\npath='/cluster'\nbackend='ssh'\nsource='/'\nread_only=true\ndomain='other-name'\n",
    )
    .unwrap();
    let home = SshDataHome {
        ssh_aliases: vec!["fixture".into()],
        remote_root: "/cluster/data".into(),
        vfs_prefix: "/cluster".into(),
        domain_id: Some("hpc-b".into()),
        retained_vfs_prefix: None,
        retained_remote_root: None,
    };
    let mounts = Arc::new(vfs::MountedObjectStore::from_manifest(&manifest).unwrap());
    let storage = vfs::OpendalFileStorage::with_mounts(root.path(), mounts);
    let error = home.validate_vfs(&storage).unwrap_err().to_string();
    assert!(error.contains("domain"), "{error}");
}
