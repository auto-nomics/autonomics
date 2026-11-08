//! Offline OCI software import into an explicitly selected private backend.
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom},
    os::{
        fd::AsRawFd,
        unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    process::{Output, Stdio},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

use crate::{ContainerRuntimeError, PodmanRuntime, connection::truncate_captured_bytes};

#[derive(Clone, Debug)]
pub struct OciImageImportRequest {
    pub archive: PathBuf,
    pub archive_sha256: String,
    pub archive_bytes: u64,
    pub image_reference: String,
    pub image_config_sha256: String,
    pub timeout_secs: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OciImageImportReceipt {
    pub archive_sha256: String,
    pub archive_bytes: u64,
    pub image_reference: String,
    pub image_config_sha256: String,
    pub manifest_digest: String,
    /// A private, unpredictable repository association, never a replacement
    /// execution reference. The requested repo@digest is verified afterwards.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository_alias: Option<String>,
    pub load_stdout: String,
    pub load_stderr: String,
}

fn invalid(message: impl Into<String>) -> ContainerRuntimeError {
    ContainerRuntimeError::Invalid(message.into())
}
fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
type FileStamp = (u64, u64, u64, u32, u32, u32, u64, i64, i64, i64, i64);
fn stamp(m: &fs::Metadata) -> FileStamp {
    (
        m.dev(),
        m.ino(),
        m.len(),
        m.uid(),
        m.gid(),
        m.mode(),
        m.nlink(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
    )
}

type DirectoryStamp = (u64, u64, u32, u32, u32);
fn private_directory(path: &Path) -> Result<DirectoryStamp, ContainerRuntimeError> {
    let metadata = fs::symlink_metadata(path).map_err(|e| invalid(e.to_string()))?;
    // SAFETY: geteuid has no memory arguments or side effects.
    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
        || fs::canonicalize(path).map_err(|e| invalid(e.to_string()))? != path
    {
        return Err(invalid(
            "OCI import backend must be owned, private and non-aliased",
        ));
    }
    Ok((
        metadata.dev(),
        metadata.ino(),
        metadata.uid(),
        metadata.gid(),
        metadata.mode(),
    ))
}

/// Serialize cooperating imports into this exact private backend. This is not
/// a Podman tag CAS or permission to mutate a shared/foreign image store.
/// Never unlink the lock: another waiter must not acquire a different inode.
struct ImportBackendLock {
    file: File,
    // Kept in the same owned task as each child; cancellation cannot close or
    // reuse the verified archive FD before Podman has actually consumed it.
    archive: Option<File>,
    path: PathBuf,
    directories: Vec<(PathBuf, DirectoryStamp)>,
}
impl ImportBackendLock {
    fn acquire(config: &crate::PodmanConfig) -> Result<Self, ContainerRuntimeError> {
        let storage = config
            .storage_root
            .as_ref()
            .ok_or_else(|| invalid("private OCI storage missing"))?;
        let run = config
            .run_root
            .as_ref()
            .ok_or_else(|| invalid("private OCI run root missing"))?;
        if storage == run || storage.starts_with(run) || run.starts_with(storage) {
            return Err(invalid(
                "OCI import requires distinct private backend directories",
            ));
        }
        let mut directories = Vec::new();
        for directory in [storage, run] {
            match fs::DirBuilder::new().mode(0o700).create(directory) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(invalid(e.to_string())),
            }
            directories.push((directory.clone(), private_directory(directory)?));
        }
        let path = storage.join(".autonomics-oci-import.lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&path)
            .map_err(|e| invalid(e.to_string()))?;
        let guard = Self {
            file,
            archive: None,
            path,
            directories,
        };
        guard.verify()?;
        // SAFETY: flock operates only on our live descriptor. Nonblocking
        // failure is explicit; no time-based stale-lock deletion or retry.
        if unsafe { libc::flock(guard.file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(invalid("OCI private backend import already locked"));
        }
        guard.verify()?;
        Ok(guard)
    }

    fn verify(&self) -> Result<(), ContainerRuntimeError> {
        for (path, before) in &self.directories {
            if &private_directory(path)? != before {
                return Err(invalid("OCI import backend directory changed"));
            }
        }
        let held = self.file.metadata().map_err(|e| invalid(e.to_string()))?;
        let named = fs::symlink_metadata(&self.path).map_err(|e| invalid(e.to_string()))?;
        // SAFETY: geteuid has no memory arguments or side effects.
        if !held.is_file()
            || held.uid() != unsafe { libc::geteuid() }
            || held.mode() & 0o077 != 0
            || held.nlink() != 1
            || stamp(&held) != stamp(&named)
        {
            return Err(invalid("OCI import backend lock identity differs"));
        }
        Ok(())
    }
}

async fn bounded_capture(
    stream: &mut (impl tokio::io::AsyncRead + Unpin),
    captured: &mut Vec<u8>,
) -> std::io::Result<()> {
    let mut buffer = [0u8; 8192];
    let limit = crate::connection::MAX_CAPTURED_OUTPUT_BYTES + 1;
    loop {
        let count = stream.read(&mut buffer).await?;
        if count == 0 {
            return Ok(());
        }
        let retained = count.min(limit.saturating_sub(captured.len()));
        captured.extend_from_slice(&buffer[..retained]);
        // Continue draining bounded buffers so verbose children cannot deadlock.
    }
}

async fn import_command(
    mut command: tokio::process::Command,
    timeout: Duration,
    backend: ImportBackendLock,
) -> Result<(Output, ImportBackendLock), ContainerRuntimeError> {
    // Dropping the caller's future only detaches this one command's waiter.
    // The task owns BOTH archive and backend lease until the actual child has
    // been waited. It cannot proceed to a later tag or issue a receipt.
    tokio::spawn(async move {
        backend.verify()?;
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(|e| invalid(e.to_string()))?;
        let (Some(mut stdout_pipe), Some(mut stderr_pipe)) =
            (child.stdout.take(), child.stderr.take())
        else {
            let _ = child.start_kill();
            child.wait().await.map_err(|e| invalid(e.to_string()))?;
            return Err(invalid("OCI import child capture unavailable"));
        };
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let waited = tokio::time::timeout(timeout, async {
            let (status, (), ()) = tokio::try_join!(
                child.wait(),
                bounded_capture(&mut stdout_pipe, &mut stdout),
                bounded_capture(&mut stderr_pipe, &mut stderr),
            )?;
            Ok::<_, std::io::Error>(status)
        })
        .await;
        let status = match waited {
            Ok(Ok(status)) => status,
            other => {
                // Even on capture/timeout failure, do not release the backend
                // merely because a kill was requested. Wait for the real exit.
                let _ = child.start_kill();
                child.wait().await.map_err(|e| invalid(e.to_string()))?;
                return Err(invalid(match other {
                    Err(_) => "OCI import command timed out; child waited".to_owned(),
                    Ok(Err(error)) => format!("OCI import capture failed; child waited: {error}"),
                    Ok(Ok(_)) => unreachable!(),
                }));
            }
        };
        backend.verify()?;
        Ok((
            Output {
                status,
                stdout,
                stderr,
            },
            backend,
        ))
    })
    .await
    .map_err(|e| invalid(format!("OCI import waiter failed: {e}")))?
}

fn inspect_command(config: &crate::PodmanConfig, image: &str) -> tokio::process::Command {
    let mut command = config.async_command();
    command
        .arg("--remote=false")
        .args(["image", "inspect", image]);
    command
}

fn unpredictable_alias(repository: &str) -> Result<String, ContainerRuntimeError> {
    // A digest-pinned input may still carry an image tag. Strip only a colon
    // after the last slash; a registry host's port belongs to the repository.
    let repository = repository
        .rsplit_once(':')
        .filter(|(_, suffix)| !suffix.contains('/'))
        .map(|(name, _)| name)
        .unwrap_or(repository);
    let mut nonce = [0u8; 32];
    let mut offset = 0;
    while offset < nonce.len() {
        // SAFETY: the pointer and remaining length address our writable buffer.
        let read = unsafe {
            libc::getrandom(nonce[offset..].as_mut_ptr().cast(), nonce.len() - offset, 0)
        };
        if read < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(invalid(format!("OCI alias entropy unavailable: {error}")));
        }
        if read == 0 {
            return Err(invalid("OCI alias entropy unavailable"));
        }
        offset += read as usize;
    }
    let nonce: String = nonce.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!("{repository}:autonomics-oci-{nonce}"))
}

fn verify_image_identity(
    output: &Output,
    request: &OciImageImportRequest,
    manifest: &str,
) -> Result<(), ContainerRuntimeError> {
    if !output.status.success() {
        return Err(invalid("imported image reference not resolvable"));
    }
    let images: Vec<serde_json::Value> =
        serde_json::from_slice(&output.stdout).map_err(|e| invalid(e.to_string()))?;
    if images.len() != 1
        || images[0]["Id"]
            .as_str()
            .map(|v| v.strip_prefix("sha256:").unwrap_or(v))
            != Some(request.image_config_sha256.as_str())
        || images[0]["Digest"] != format!("sha256:{manifest}")
    {
        return Err(invalid("imported image manifest/config identity differs"));
    }
    Ok(())
}

fn hash(file: &mut File, maximum: u64) -> Result<String, ContainerRuntimeError> {
    file.seek(SeekFrom::Start(0))
        .map_err(|e| invalid(e.to_string()))?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    let mut total = 0u64;
    loop {
        let n = file.read(&mut buffer).map_err(|e| invalid(e.to_string()))?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > maximum {
            return Err(invalid("OCI archive exceeds its declared byte limit"));
        }
        hash.update(&buffer[..n]);
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|e| invalid(e.to_string()))?;
    Ok(format!("{:x}", hash.finalize()))
}

fn remaining_import_budget(
    deadline: tokio::time::Instant,
) -> Result<Duration, ContainerRuntimeError> {
    deadline
        .checked_duration_since(tokio::time::Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| invalid("OCI import total task budget exhausted"))
}

impl PodmanRuntime {
    /// Import local, content-verified software without pulling or starting a
    /// container. The caller supplies authorization/resource admission; this
    /// operation does not provide scientific qualification or execution grants.
    pub async fn import_oci_archive(
        &self,
        request: OciImageImportRequest,
    ) -> Result<OciImageImportReceipt, ContainerRuntimeError> {
        let config = self.config();
        let manifest = request
            .image_reference
            .rsplit_once("@sha256:")
            .filter(|(name, sha)| {
                !name.is_empty()
                    && !name.starts_with('-')
                    && !name
                        .bytes()
                        .any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
                    && digest(sha)
            })
            .ok_or_else(|| invalid("immutable image reference required for OCI import"))?
            .1
            .to_owned();
        if !digest(&request.archive_sha256)
            || !digest(&request.image_config_sha256)
            || request.archive_bytes == 0
            || request.archive_bytes > 64 * 1024 * 1024 * 1024
            || !(1..=3600).contains(&request.timeout_secs)
            || !request.archive.is_absolute()
            || !config
                .storage_root
                .as_ref()
                .is_some_and(|p| p.is_absolute())
            || !config.run_root.as_ref().is_some_and(|p| p.is_absolute())
            || !matches!(config.storage_driver.as_deref(), Some("vfs" | "overlay"))
        {
            return Err(invalid(
                "bounded OCI import and explicit private backend required",
            ));
        }
        // One admission budget covers hashing, load and every identity/tag
        // step. A later step never starts with a fresh fixed timeout.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(request.timeout_secs);
        let before_request = request.clone();
        let (file, before) = tokio::task::spawn_blocking(move || {
            if fs::canonicalize(&before_request.archive).map_err(|e| invalid(e.to_string()))?
                != before_request.archive
            {
                return Err(invalid("OCI archive alias forbidden"));
            }
            let mut file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(&before_request.archive)
                .map_err(|e| invalid(e.to_string()))?;
            let m = file.metadata().map_err(|e| invalid(e.to_string()))?;
            // SAFETY: geteuid has no memory arguments or side effects.
            if !m.is_file()
                || m.nlink() != 1
                || m.uid() != unsafe { libc::geteuid() }
                || m.mode() & 0o022 != 0
                || m.len() != before_request.archive_bytes
                || hash(&mut file, before_request.archive_bytes)? != before_request.archive_sha256
            {
                return Err(invalid("OCI archive content/ownership differs"));
            }
            if stamp(&m)
                != stamp(
                    &fs::symlink_metadata(&before_request.archive)
                        .map_err(|e| invalid(e.to_string()))?,
                )
            {
                return Err(invalid("OCI archive changed during verification"));
            }
            Ok((file, m))
        })
        .await
        .map_err(|e| invalid(e.to_string()))??;
        let mut backend = ImportBackendLock::acquire(config)?;
        backend.archive = Some(file);
        let fd = backend
            .archive
            .as_ref()
            .expect("verified archive held")
            .as_raw_fd();
        let mut command = config.async_command();
        command
            .arg("--remote=false")
            .args(["load", "--input"])
            .arg(format!("/proc/self/fd/{fd}"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // SAFETY: only async-signal-safe fcntl is called after fork. Clearing
        // CLOEXEC in the child preserves the verified FD, never a reopened path.
        unsafe {
            command.pre_exec(move || {
                let flags = libc::fcntl(fd, libc::F_GETFD);
                if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let (loaded, backend) =
            import_command(command, remaining_import_budget(deadline)?, backend).await?;
        if !loaded.status.success() {
            return Err(invalid(format!(
                "OCI import failed: {}",
                truncate_captured_bytes(&loaded.stderr)
            )));
        }
        let (inspected, mut backend) = import_command(
            inspect_command(config, &request.image_reference),
            remaining_import_budget(deadline)?,
            backend,
        )
        .await?;
        let repository_alias = if inspected.status.success() {
            verify_image_identity(&inspected, &request, &manifest)?;
            None
        } else {
            // OCI archives may retain only a local ref-name annotation. Match
            // immutable content BEFORE associating the requested repository.
            let config_id = format!("sha256:{}", request.image_config_sha256);
            let (source, held) = import_command(
                inspect_command(config, &config_id),
                remaining_import_budget(deadline)?,
                backend,
            )
            .await?;
            backend = held;
            verify_image_identity(&source, &request, &manifest)?;
            let repository = request
                .image_reference
                .rsplit_once("@sha256:")
                .expect("validated immutable image reference")
                .0;
            let alias = unpredictable_alias(repository)?;
            let mut exists = config.async_command();
            exists
                .arg("--remote=false")
                .args(["image", "exists", &alias]);
            let (existing, held) =
                import_command(exists, remaining_import_budget(deadline)?, backend).await?;
            backend = held;
            // Exit1 alone means absent. Never replace an existing tag, even
            // one apparently pointing at the same content, or retry a clash.
            if existing.status.code() != Some(1) {
                return Err(invalid(
                    "OCI private repository alias already exists or absence is unproven",
                ));
            }
            // This is not an atomic tag CAS. Safety is the exclusive owned
            // private backend plus an unpredictable fresh 256-bit name. An
            // independently controlled/shared backend is not supported here.
            let mut tag = config.async_command();
            tag.arg("--remote=false")
                .args(["image", "tag", &config_id, &alias]);
            let (tagged, held) =
                import_command(tag, remaining_import_budget(deadline)?, backend).await?;
            backend = held;
            if !tagged.status.success() {
                return Err(invalid("OCI private repository association failed"));
            }
            let (associated, held) = import_command(
                inspect_command(config, &alias),
                remaining_import_budget(deadline)?,
                backend,
            )
            .await?;
            backend = held;
            verify_image_identity(&associated, &request, &manifest)?;
            let (resolved, held) = import_command(
                inspect_command(config, &request.image_reference),
                remaining_import_budget(deadline)?,
                backend,
            )
            .await?;
            backend = held;
            // Never execute by the mutable alias or rewrite the caller's
            // reference. The ORIGINAL repo@digest must now resolve correctly.
            verify_image_identity(&resolved, &request, &manifest)?;
            Some(alias)
        };
        let after_request = request.clone();
        let _backend = tokio::task::spawn_blocking(move || {
            backend.verify()?;
            let file = backend
                .archive
                .as_mut()
                .ok_or_else(|| invalid("verified archive missing"))?;
            if stamp(&before) != stamp(&file.metadata().map_err(|e| invalid(e.to_string()))?)
                || stamp(&before)
                    != stamp(
                        &fs::symlink_metadata(&after_request.archive)
                            .map_err(|e| invalid(e.to_string()))?,
                    )
                || hash(file, after_request.archive_bytes)? != after_request.archive_sha256
            {
                return Err(invalid("OCI archive changed during import"));
            }
            backend.verify()?;
            Ok(backend)
        })
        .await
        .map_err(|e| invalid(e.to_string()))??;
        remaining_import_budget(deadline)?;
        Ok(OciImageImportReceipt {
            archive_sha256: request.archive_sha256,
            archive_bytes: request.archive_bytes,
            image_reference: request.image_reference,
            image_config_sha256: request.image_config_sha256,
            manifest_digest: format!("sha256:{manifest}"),
            repository_alias,
            load_stdout: truncate_captured_bytes(&loaded.stdout),
            load_stderr: truncate_captured_bytes(&loaded.stderr),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PodmanConfig;
    use std::os::unix::fs::PermissionsExt;

    // This fake proves CLI/FD/identity enforcement only, never real OCI parsing
    // or container/method acceptance. Actual Podman must be checked separately.
    fn fixture(wrong_identity: bool) -> (tempfile::TempDir, PodmanRuntime, OciImageImportRequest) {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let archive = root.join("software.oci.tar");
        fs::write(&archive, b"UNIT_ONLY_NOT_A_REAL_OCI_ARCHIVE").unwrap();
        let program = root.join("fake-podman");
        let trace = serde_json::to_string(&root.join("calls.jsonl")).unwrap();
        let observed = serde_json::to_string(&root.join("observed-input")).unwrap();
        let config = if wrong_identity {
            "d".repeat(64)
        } else {
            "b".repeat(64)
        };
        fs::write(
            &program,
            format!(
                r#"#!/usr/bin/python3
import json,sys
from pathlib import Path
with open({trace}, 'a') as f:f.write(json.dumps(sys.argv[1:])+'\n')
if 'load' in sys.argv:
 Path({observed}).write_bytes(Path(sys.argv[sys.argv.index('--input')+1]).read_bytes())
 print('Loaded unit fixture; not an actual image')
elif 'inspect' in sys.argv:
 print(json.dumps([{{'Id':'sha256:{config}','Digest':'sha256:{manifest}'}}]))
else:sys.exit(3)
"#,
                manifest = "c".repeat(64)
            ),
        )
        .unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
        let runtime = PodmanRuntime::new(PodmanConfig {
            program: program.to_str().unwrap().to_owned(),
            storage_root: Some(root.join("private-store")),
            run_root: Some(root.join("private-run")),
            storage_driver: Some("overlay".into()),
            cgroup_manager: Some("cgroupfs".into()),
            ..PodmanConfig::default()
        });
        let request = OciImageImportRequest {
            archive_sha256: format!("{:x}", Sha256::digest(fs::read(&archive).unwrap())),
            archive_bytes: fs::metadata(&archive).unwrap().len(),
            archive,
            image_reference: format!("example.test/unit@sha256:{}", "c".repeat(64)),
            image_config_sha256: "b".repeat(64),
            timeout_secs: 5,
        };
        (directory, runtime, request)
    }

    #[tokio::test]
    async fn import_uses_verified_fd_and_same_explicit_backend_for_inspection() {
        let (directory, runtime, request) = fixture(false);
        let receipt = runtime.import_oci_archive(request.clone()).await.unwrap();
        assert_eq!(receipt.archive_sha256, request.archive_sha256);
        assert_eq!(
            fs::read(directory.path().join("observed-input")).unwrap(),
            fs::read(&request.archive).unwrap()
        );
        let calls: Vec<Vec<String>> = fs::read_to_string(directory.path().join("calls.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(calls.len(), 2);
        let first = calls[0].iter().position(|v| v == "load").unwrap();
        let second = calls[1].iter().position(|v| v == "image").unwrap();
        assert_eq!(&calls[0][..first], &calls[1][..second]);
        assert!(calls[0].iter().any(|v| v.starts_with("/proc/self/fd/")));
        assert!(!calls.iter().flatten().any(|v| v == "pull" || v == "run"));
    }

    #[tokio::test]
    async fn wrong_archive_hash_or_ambient_backend_never_starts_podman() {
        let (directory, runtime, mut request) = fixture(false);
        request.archive_sha256 = "a".repeat(64);
        assert!(runtime.import_oci_archive(request.clone()).await.is_err());
        assert!(!directory.path().join("calls.jsonl").exists());
        let mut config = runtime.config().clone();
        config.storage_root = None;
        assert!(
            PodmanRuntime::new(config)
                .import_oci_archive(request)
                .await
                .is_err()
        );
        assert!(!directory.path().join("calls.jsonl").exists());
    }

    #[tokio::test]
    async fn imported_config_mismatch_cannot_produce_a_receipt() {
        let (_directory, runtime, request) = fixture(true);
        let error = runtime.import_oci_archive(request).await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("manifest/config identity differs")
        );
    }

    fn alias_fixture(mode: &str) -> (tempfile::TempDir, PodmanRuntime, OciImageImportRequest) {
        alias_fixture_reference(
            mode,
            &format!("example.test/unit@sha256:{}", "c".repeat(64)),
        )
    }

    fn alias_fixture_reference(
        mode: &str,
        reference: &str,
    ) -> (tempfile::TempDir, PodmanRuntime, OciImageImportRequest) {
        let (directory, runtime, mut request) = fixture(false);
        request.image_reference = reference.to_owned();
        let settings = serde_json::json!({
            "root":directory.path(), "mode":mode, "pinned":request.image_reference,
            "config":"b".repeat(64), "manifest":"c".repeat(64), "archive":request.archive,
        });
        fs::write(directory.path().join("fake-podman"), format!(r#"#!/usr/bin/python3
import json,os,sys,time
from pathlib import Path
s={settings}
r=Path(s['root']); args=sys.argv[1:]
with (r/'calls.jsonl').open('a') as f:f.write(json.dumps(args)+'\n')
def image(config=None, manifest=None):
 print(json.dumps([dict(Id='sha256:'+(config or s['config']),Digest='sha256:'+(manifest or s['manifest']))]))
if 'load' in args:
 if s['mode']=='total-budget':time.sleep(0.6)
 if s['mode'] in ('cancel','timeout'):
  (r/'started').write_text(str(os.getpid()))
  time.sleep(0.5 if s['mode']=='cancel' else 2)
 (r/'observed-input').write_bytes(Path(args[args.index('--input')+1]).read_bytes())
 (r/'finished').write_text('true')
 print('Loaded local annotation only; not a repository identity')
elif 'inspect' in args:
 if s['mode']=='total-budget':
  (r/'inspect-started').write_text(str(os.getpid()))
  time.sleep(0.7)
 name=args[-1]
 if name==s['pinned']:
  if not (r/'tagged').exists() or s['mode']=='final-missing':sys.exit(125)
  image(config='d'*64 if s['mode']=='final-wrong' else None)
 elif name=='sha256:'+s['config']:
  image(config='d'*64 if s['mode']=='source-config' else None,
        manifest='d'*64 if s['mode']=='source-manifest' else None)
 elif ':autonomics-oci-' in name:
  image(manifest='d'*64 if s['mode']=='alias-wrong' else None)
 else:sys.exit(125)
elif 'exists' in args:sys.exit(0 if s['mode']=='conflict' else (125 if s['mode']=='exists-error' else 1))
elif 'tag' in args:
 if s['mode']=='tag-failure':sys.exit(125)
 if args[-2]!='sha256:'+s['config']:sys.exit(3)
 (r/'tagged').write_text(args[-1])
 if s['mode']=='archive-change':Path(s['archive']).write_bytes(b'CHANGED')
else:sys.exit(3)
"#)).unwrap();
        (directory, runtime, request)
    }

    #[tokio::test]
    async fn annotation_only_import_associates_private_repository_then_verifies_original_digest() {
        let (directory, runtime, request) = alias_fixture("success");
        let receipt = runtime.import_oci_archive(request.clone()).await.unwrap();
        assert_eq!(receipt.image_reference, request.image_reference);
        assert_eq!(receipt.image_config_sha256, request.image_config_sha256);
        let alias = receipt.repository_alias.unwrap();
        let nonce = alias
            .strip_prefix("example.test/unit:autonomics-oci-")
            .unwrap();
        assert!(digest(nonce));
        let calls: Vec<Vec<String>> = fs::read_to_string(directory.path().join("calls.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(calls.len(), 7);
        assert_eq!(calls.last().unwrap().last(), Some(&request.image_reference));
        let prefix = &calls[0][..calls[0].iter().position(|v| v == "load").unwrap()];
        for call in &calls[1..] {
            let at = call.iter().position(|v| v == "image").unwrap();
            assert_eq!(&call[..at], prefix);
        }
        assert!(calls.iter().any(|c| c.ends_with(&[
            "tag".into(),
            format!("sha256:{}", request.image_config_sha256),
            alias.clone()
        ])));
        assert!(
            !calls
                .iter()
                .flatten()
                .any(|v| v == "pull" || v == "run" || v.contains(":latest"))
        );
    }

    #[tokio::test]
    async fn alias_fallback_rejects_identity_conflicts_failures_and_archive_drift() {
        for mode in [
            "source-config",
            "source-manifest",
            "conflict",
            "exists-error",
            "tag-failure",
            "alias-wrong",
            "final-wrong",
            "final-missing",
            "archive-change",
        ] {
            let (directory, runtime, request) = alias_fixture(mode);
            assert!(runtime.import_oci_archive(request).await.is_err(), "{mode}");
            if matches!(
                mode,
                "source-config" | "source-manifest" | "conflict" | "exists-error"
            ) {
                assert!(!directory.path().join("tagged").exists(), "{mode}");
            }
        }
    }

    #[tokio::test]
    async fn repository_alias_strips_image_tag_preserves_registry_port_and_original_request() {
        for (name, repository) in [
            ("example.test/unit", "example.test/unit"),
            ("example.test/unit:locked", "example.test/unit"),
            ("example.test:5443/unit", "example.test:5443/unit"),
            ("example.test:5443/unit:locked", "example.test:5443/unit"),
            ("unit:locked", "unit"),
        ] {
            let original = format!("{name}@sha256:{}", "c".repeat(64));
            let (directory, runtime, request) = alias_fixture_reference("success", &original);
            let receipt = runtime.import_oci_archive(request).await.unwrap();
            assert_eq!(receipt.image_reference, original);
            let prefix = format!("{repository}:autonomics-oci-");
            assert!(digest(
                receipt
                    .repository_alias
                    .unwrap()
                    .strip_prefix(prefix.as_str())
                    .unwrap()
            ));
            let calls: Vec<Vec<String>> = fs::read_to_string(directory.path().join("calls.jsonl"))
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            assert_eq!(calls.last().unwrap().last(), Some(&original));
        }
    }

    #[test]
    fn repository_association_names_are_unpredictable_and_backend_lock_is_exclusive() {
        let first = unpredictable_alias("example.test/unit").unwrap();
        let second = unpredictable_alias("example.test/unit").unwrap();
        assert_ne!(first, second);
        assert!(digest(first.rsplit_once("autonomics-oci-").unwrap().1));
        let (_directory, runtime, _request) = fixture(false);
        let held = ImportBackendLock::acquire(runtime.config()).unwrap();
        assert!(ImportBackendLock::acquire(runtime.config()).is_err());
        let inode = fs::metadata(&held.path).unwrap().ino();
        drop(held);
        let next = ImportBackendLock::acquire(runtime.config()).unwrap();
        assert_eq!(fs::metadata(&next.path).unwrap().ino(), inode);
    }

    #[tokio::test]
    async fn shared_or_aliased_backend_and_foreign_lock_never_start_podman() {
        for mode in ["permissions", "directory-alias", "lock-alias"] {
            let (directory, runtime, request) = alias_fixture("success");
            let storage = runtime.config().storage_root.as_ref().unwrap();
            if mode == "directory-alias" {
                let other = directory.path().join("other");
                fs::create_dir(&other).unwrap();
                std::os::unix::fs::symlink(other, storage).unwrap();
            } else {
                fs::create_dir(storage).unwrap();
                fs::set_permissions(
                    storage,
                    fs::Permissions::from_mode(if mode == "permissions" { 0o755 } else { 0o700 }),
                )
                .unwrap();
                if mode == "lock-alias" {
                    let foreign = directory.path().join("foreign-lock");
                    fs::write(&foreign, b"FOREIGN").unwrap();
                    std::os::unix::fs::symlink(
                        &foreign,
                        storage.join(".autonomics-oci-import.lock"),
                    )
                    .unwrap();
                }
            }
            assert!(runtime.import_oci_archive(request).await.is_err(), "{mode}");
            assert!(!directory.path().join("calls.jsonl").exists(), "{mode}");
        }
    }

    #[tokio::test]
    async fn cancelled_import_holds_archive_and_backend_until_actual_child_wait() {
        let (directory, runtime, request) = alias_fixture("cancel");
        let config = runtime.config().clone();
        let original = fs::read(&request.archive).unwrap();
        let task = tokio::spawn(async move { runtime.import_oci_archive(request).await });
        for _ in 0..200 {
            if directory.path().join("started").exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(directory.path().join("started").exists());
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(ImportBackendLock::acquire(&config).is_err());
        let mut released = false;
        for _ in 0..300 {
            if ImportBackendLock::acquire(&config).is_ok() {
                released = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(released);
        assert!(directory.path().join("finished").exists());
        assert_eq!(
            fs::read(directory.path().join("observed-input")).unwrap(),
            original
        );
        assert_eq!(
            fs::read_to_string(directory.path().join("calls.jsonl"))
                .unwrap()
                .lines()
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn import_identity_steps_share_the_original_total_deadline() {
        let (directory, runtime, mut request) = alias_fixture("total-budget");
        request.timeout_secs = 1;
        let error = runtime.import_oci_archive(request).await.unwrap_err();
        assert!(error.to_string().contains("child waited"));
        assert!(
            directory.path().join("finished").exists(),
            "load completed before inspection"
        );
        assert!(!directory.path().join("tagged").exists());
        assert_eq!(
            fs::read_to_string(directory.path().join("calls.jsonl"))
                .unwrap()
                .lines()
                .count(),
            2
        );
        let pid: i32 = fs::read_to_string(directory.path().join("inspect-started"))
            .unwrap()
            .parse()
            .unwrap();
        // SAFETY: signal0 only queries the waited toy child's PID.
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
        assert!(ImportBackendLock::acquire(runtime.config()).is_ok());
    }

    #[tokio::test]
    async fn import_timeout_waits_for_killed_child_before_releasing_backend() {
        let (directory, runtime, mut request) = alias_fixture("timeout");
        request.timeout_secs = 1;
        let error = runtime.import_oci_archive(request).await.unwrap_err();
        assert!(error.to_string().contains("child waited"));
        let pid: i32 = fs::read_to_string(directory.path().join("started"))
            .unwrap()
            .parse()
            .unwrap();
        // SAFETY: signal0 only queries existence of the toy child's PID.
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
        assert!(!directory.path().join("finished").exists());
        assert!(ImportBackendLock::acquire(runtime.config()).is_ok());
    }
}
