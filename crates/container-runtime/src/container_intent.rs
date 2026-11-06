//! Durable ownership of containers in an explicitly isolated Podman store.
//! SIGKILL cannot run Drop. Stable-inode locks and kernel process start ticks
//! fence recovery; a name match alone never authorizes removal.
use crate::podman::PodmanConfig;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    os::unix::io::AsRawFd,
    path::{Path, PathBuf},
    process::Stdio,
};

pub(crate) const INTENT_LABEL: &str = "io.autonomics.container-intent";

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    schema_version: u8,
    name: String,
    owner_pid: u32,
    owner_start_ticks: u64,
    storage_root: PathBuf,
    run_root: PathBuf,
}

pub(crate) struct OwnedIntent {
    _lock: fs::File,
    path: PathBuf,
    bytes: Vec<u8>,
    pub(crate) digest: String,
    name: String,
}

fn error(message: impl ToString) -> String {
    message.to_string()
}
fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && !matches!(name, "." | "..")
        && name.len() <= 128
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}
fn root(config: &PodmanConfig) -> Result<Option<PathBuf>, String> {
    match (&config.storage_root, &config.run_root) {
        (None, None) => Ok(None),
        (Some(storage), Some(run)) if storage.is_absolute() && run.is_absolute() => {
            // The operator's state directory may legitimately be reached via
            // a symlink. Resolve that parent before checking the intent
            // directory itself, which must not redirect to another location.
            fs::create_dir_all(run).map_err(error)?;
            let path = run
                .canonicalize()
                .map_err(error)?
                .join("autonomics-container-intents");
            fs::create_dir_all(&path).map_err(error)?;
            if path.canonicalize().map_err(error)? != path {
                return Err("container intent directory must be canonical".into());
            }
            Ok(Some(path))
        }
        _ => Err(
            "container intent recovery requires paired absolute private storage/run roots".into(),
        ),
    }
}
fn lock(path: &Path) -> Result<Option<fs::File>, String> {
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(error)?;
    // Lock files are permanent: unlinking them would split the exclusion domain.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let e = std::io::Error::last_os_error();
        if e.raw_os_error() == Some(libc::EWOULDBLOCK) {
            return Ok(None);
        }
        return Err(error(e));
    }
    verify_lock(&file, path)?;
    Ok(Some(file))
}

fn verify_lock(file: &fs::File, path: &Path) -> Result<(), String> {
    let held = file.metadata().map_err(error)?;
    let named = fs::symlink_metadata(path).map_err(error)?;
    if !named.is_file()
        || named.file_type().is_symlink()
        || (held.dev(), held.ino()) != (named.dev(), named.ino())
    {
        return Err("container intent lock inode changed".into());
    }
    Ok(())
}
fn process(pid: u32) -> Result<Option<(char, u64)>, String> {
    let text = match fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(error(e)),
    };
    let (_, rest) = text
        .rsplit_once(") ")
        .ok_or("cannot parse container owner process")?;
    let fields: Vec<_> = rest.split_whitespace().collect();
    let state = fields
        .first()
        .and_then(|s| s.chars().next())
        .ok_or("process state missing")?;
    let ticks = fields
        .get(19)
        .ok_or("process start ticks missing")?
        .parse()
        .map_err(error)?;
    Ok(Some((state, ticks)))
}
fn read(path: &Path) -> Result<Vec<u8>, String> {
    let metadata = fs::symlink_metadata(path).map_err(error)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 16384 {
        return Err("container intent must be a bounded regular file".into());
    }
    fs::read(path).map_err(error)
}

fn present(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(error(e)),
    }
}

async fn output(config: &PodmanConfig, args: &[&str]) -> Result<std::process::Output, String> {
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        config
            .async_command()
            .args(args)
            .stdin(Stdio::null())
            .output(),
    )
    .await
    .map_err(|_| "container ownership command timed out".to_string())?
    .map_err(error)
}

impl OwnedIntent {
    pub(crate) fn create(config: &PodmanConfig, name: &str) -> Result<Option<Self>, String> {
        let Some(root) = root(config)? else {
            return Ok(None);
        };
        if !safe_name(name) {
            return Err("unsafe container intent name".into());
        }
        let held = lock(&root.join(format!("{name}.lock")))?
            .ok_or("container intent is held by another process")?;
        let path = root.join(format!("{name}.json"));
        if present(&path)? {
            return Err("container intent already exists".into());
        }
        let owner_pid = std::process::id();
        let (_, owner_start_ticks) = process(owner_pid)?.ok_or("owner process unavailable")?;
        let intent = Intent {
            schema_version: 1,
            name: name.into(),
            owner_pid,
            owner_start_ticks,
            storage_root: config.storage_root.clone().unwrap(),
            run_root: config.run_root.clone().unwrap(),
        };
        let bytes = serde_json::to_vec(&intent).map_err(error)?;
        let temporary = root.join(format!(".{name}.pending"));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temporary)
            .map_err(error)?;
        // Only this successful exclusive-create owns the temporary path.
        let publication = (|| -> Result<(), String> {
            file.write_all(&bytes).map_err(error)?;
            file.sync_all().map_err(error)?;
            fs::hard_link(&temporary, &path).map_err(error)?;
            fs::File::open(&root)
                .and_then(|f| f.sync_all())
                .map_err(error)?;
            Ok(())
        })();
        let _ = fs::remove_file(&temporary);
        publication?;
        let digest = format!("{:x}", Sha256::digest(&bytes));
        Ok(Some(Self {
            _lock: held,
            path,
            bytes,
            digest,
            name: name.into(),
        }))
    }

    /// Remove by verified immutable container ID, under this intent's lock.
    /// No stale name-based delete can remove a successor or foreign container.
    pub(crate) async fn cleanup(&self, config: &PodmanConfig) -> Result<(), String> {
        verify_lock(&self._lock, &self.path.with_extension("lock"))?;
        if read(&self.path)? != self.bytes {
            return Err("container intent identity changed".into());
        }
        let exists = output(config, &["container", "exists", &self.name]).await?;
        match exists.status.code() {
            Some(1) => {} // Podman's documented not-found exit; other errors block.
            Some(0) => {
                let inspected = output(config, &["inspect", &self.name]).await?;
                if !inspected.status.success() {
                    return Err("cannot inspect owned container".into());
                }
                let rows: Vec<serde_json::Value> =
                    serde_json::from_slice(&inspected.stdout).map_err(error)?;
                if rows.len() != 1
                    || rows[0]
                        .pointer("/Config/Labels")
                        .and_then(|v| v.get(INTENT_LABEL))
                        .and_then(|v| v.as_str())
                        != Some(&self.digest)
                {
                    return Err("container ownership label differs; refusing removal".into());
                }
                let id = rows[0]
                    .get("Id")
                    .and_then(|v| v.as_str())
                    .ok_or("container ID missing")?;
                if id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Err("invalid immutable container ID".into());
                }
                let removed = output(config, &["rm", "--force", "--time=0", id]).await?;
                if !removed.status.success() {
                    return Err("owned container removal failed".into());
                }
                let after = output(config, &["container", "exists", id]).await?;
                if after.status.code() != Some(1) {
                    return Err("owned container still exists after removal".into());
                }
            }
            _ => return Err("cannot determine container existence".into()),
        }
        if read(&self.path)? != self.bytes {
            return Err("container intent identity changed".into());
        }
        verify_lock(&self._lock, &self.path.with_extension("lock"))?;
        fs::remove_file(&self.path).map_err(error)?;
        fs::File::open(self.path.parent().unwrap())
            .and_then(|f| f.sync_all())
            .map_err(error)?;
        Ok(())
    }
}

/// Read intents under permanent locks. Unknown ownership is retained and
/// blocks; live/locked owners are never displaced. No registry-wide prune.
pub(crate) async fn recover(config: &PodmanConfig) -> Result<Vec<String>, String> {
    let Some(root) = root(config)? else {
        return Ok(Vec::new());
    };
    let mut recovered = Vec::new();
    for entry in fs::read_dir(&root).map_err(error)? {
        let path = entry.map_err(error)?.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or("invalid intent path")?;
        if !safe_name(name) {
            return Err("unsafe intent path".into());
        }
        let Some(held) = lock(&root.join(format!("{name}.lock")))? else {
            continue;
        };
        if !present(&path)? {
            continue;
        }
        let bytes = read(&path)?;
        let intent: Intent = serde_json::from_slice(&bytes).map_err(error)?;
        if intent.schema_version != 1
            || intent.name != name
            || intent.owner_pid == 0
            || intent.owner_start_ticks == 0
            || Some(&intent.storage_root) != config.storage_root.as_ref()
            || Some(&intent.run_root) != config.run_root.as_ref()
        {
            return Err("container intent scope or identity mismatch".into());
        }
        if process(intent.owner_pid)?.is_some_and(|(state, start)| {
            start == intent.owner_start_ticks && !matches!(state, 'Z' | 'X')
        }) {
            continue;
        }
        let owned = OwnedIntent {
            _lock: held,
            path: path.clone(),
            digest: format!("{:x}", Sha256::digest(&bytes)),
            bytes,
            name: name.into(),
        };
        owned.cleanup(config).await?;
        recovered.push(owned.name.clone());
    }
    Ok(recovered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn fake(config: &PodmanConfig, label: &str) {
        fs::create_dir_all(config.storage_root.as_ref().unwrap()).unwrap();
        fs::write(
            config
                .storage_root
                .as_ref()
                .unwrap()
                .join("TEST-state.json"),
            serde_json::to_vec(&serde_json::json!({"exists":true,"label":label,"removed":[]}))
                .unwrap(),
        )
        .unwrap();
        fs::write(&config.program,r#"#!/usr/bin/python3
import json,pathlib,sys
a=sys.argv[1:];p=pathlib.Path(a[a.index('--root')+1])/'TEST-state.json';s=json.loads(p.read_text())
if 'container' in a:sys.exit(0 if s['exists'] else 1)
if 'inspect' in a:
 print(json.dumps([{'Id':'a'*64,'Config':{'Labels':{'io.autonomics.container-intent':s['label']}}}]))
elif 'rm' in a:
 s['removed'].append(a[-1]);s['exists']=False;p.write_text(json.dumps(s))
else:sys.exit(125)
"#).unwrap();
        fs::set_permissions(&config.program, fs::Permissions::from_mode(0o700)).unwrap();
    }
    fn config(dir: &Path) -> PodmanConfig {
        let mut config = PodmanConfig::for_state_dir(dir);
        config.program = dir.join("TEST-podman.py").to_string_lossy().into();
        config
    }
    #[test]
    fn state_directory_alias_is_supported_but_intent_directory_symlink_is_rejected() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        fs::create_dir(&real).unwrap();
        let alias = dir.path().join("alias");
        symlink(&real, &alias).unwrap();
        let alias_config = config(&alias);
        let intent = OwnedIntent::create(&alias_config, "TEST-owned")
            .unwrap()
            .unwrap();
        assert!(intent.path.starts_with(&real));
        drop(intent);

        let other = dir.path().join("other");
        fs::create_dir(&other).unwrap();
        let bad = config(&dir.path().join("bad"));
        let run = bad.run_root.as_ref().unwrap();
        fs::create_dir_all(run).unwrap();
        symlink(&other, run.join("autonomics-container-intents")).unwrap();
        assert_eq!(
            root(&bad).unwrap_err(),
            "container intent directory must be canonical"
        );
    }
    async fn eventually_recover_fake(config: &PodmanConfig, path: &Path) -> Vec<String> {
        let original = fs::read(path).unwrap();
        let state_path = config
            .storage_root
            .as_ref()
            .unwrap()
            .join("TEST-state.json");
        let state = fs::read(&state_path).unwrap();
        // Parallel test subprocesses can briefly inherit either the CLOEXEC
        // lock or the writer that created the fake executable. The production
        // code must defer/error without changing ownership or container state.
        // Only these two refusals are retried by this bounded test driver.
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                match recover(config).await {
                    Ok(recovered) if !recovered.is_empty() => return recovered,
                    Ok(_) => {}
                    Err(message)
                        if message
                            == std::io::Error::from_raw_os_error(libc::ETXTBSY).to_string() => {}
                    Err(message) => panic!("unexpected recovery error: {message}"),
                }
                assert_eq!(fs::read(path).unwrap(), original);
                assert_eq!(fs::read(&state_path).unwrap(), state);
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("stale identity remained after transient kernel refusal")
    }
    fn stale(config: &PodmanConfig, pid: u32, start: u64) -> (PathBuf, String) {
        let root = root(config).unwrap().unwrap();
        let intent = Intent {
            schema_version: 1,
            name: "TEST-owned".into(),
            owner_pid: pid,
            owner_start_ticks: start,
            storage_root: config.storage_root.clone().unwrap(),
            run_root: config.run_root.clone().unwrap(),
        };
        let bytes = serde_json::to_vec(&intent).unwrap();
        let path = root.join("TEST-owned.json");
        fs::write(&path, &bytes).unwrap();
        (path, format!("{:x}", Sha256::digest(bytes)))
    }
    #[tokio::test]
    async fn dead_owner_recovery_removes_only_label_verified_immutable_id() {
        let dir = tempfile::tempdir().unwrap();
        let config = config(dir.path());
        let (path, digest) = stale(&config, u32::MAX, 1);
        fake(&config, &digest);
        assert_eq!(recover(&config).await.unwrap(), vec!["TEST-owned"]);
        assert!(!path.exists());
        assert!(path.with_extension("lock").exists());
        let state: serde_json::Value = serde_json::from_slice(
            &fs::read(config.storage_root.unwrap().join("TEST-state.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(state["removed"], serde_json::json!(["a".repeat(64)]));
    }
    #[tokio::test]
    async fn live_or_locked_owner_is_not_displaced_and_lock_inode_survives_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let config = config(dir.path());
        let intent = OwnedIntent::create(&config, "TEST-owned").unwrap().unwrap();
        fake(&config, &intent.digest);
        assert!(OwnedIntent::create(&config, "TEST-owned").is_err());
        assert!(recover(&config).await.unwrap().is_empty());
        use std::os::unix::fs::MetadataExt;
        let inode = intent._lock.metadata().unwrap().ino();
        intent.cleanup(&config).await.unwrap();
        drop(intent);
        let next = OwnedIntent::create(&config, "TEST-owned").unwrap().unwrap();
        assert_eq!(next._lock.metadata().unwrap().ino(), inode);
        assert!(recover(&config).await.unwrap().is_empty());
        next.cleanup(&config).await.unwrap();
    }
    #[tokio::test]
    async fn unknown_label_malformed_record_or_wrong_store_blocks_without_deleting() {
        for kind in ["label", "malformed", "scope"] {
            let dir = tempfile::tempdir().unwrap();
            let config = config(dir.path());
            let (path, digest) = stale(&config, u32::MAX, 1);
            fake(&config, if kind == "label" { "foreign" } else { &digest });
            if kind == "malformed" {
                fs::write(&path, b"foreign bytes").unwrap();
            }
            if kind == "scope" {
                let mut value: serde_json::Value =
                    serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                value["storage_root"] = serde_json::json!("/foreign-store");
                fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
            }
            let before = fs::read(&path).unwrap();
            assert!(recover(&config).await.is_err(), "{kind}");
            assert_eq!(fs::read(path).unwrap(), before);
            let state: serde_json::Value = serde_json::from_slice(
                &fs::read(config.storage_root.unwrap().join("TEST-state.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(state["removed"], serde_json::json!([]));
        }
    }
    #[tokio::test]
    async fn reused_pid_cannot_impersonate_original_owner_and_live_unlocked_owner_is_retained() {
        let dir = tempfile::tempdir().unwrap();
        let config = config(dir.path());
        let start = process(std::process::id()).unwrap().unwrap().1;
        let (path, digest) = stale(&config, std::process::id(), start);
        fake(&config, &digest);
        assert!(recover(&config).await.unwrap().is_empty());
        assert!(path.exists());
        fs::remove_file(path).unwrap();
        let (path, digest) = stale(&config, std::process::id(), start + 1);
        fake(&config, &digest);
        assert_ne!(process(std::process::id()).unwrap().unwrap().1, start + 1);
        // Other tests spawn processes while this suite runs. A fork can
        // temporarily inherit our CLOEXEC lock descriptor before exec closes
        // it. Recovery correctly defers while any descriptor holds the lock;
        // PID reuse alone never licenses displacement of that kernel lock.
        assert_eq!(
            eventually_recover_fake(&config, &path).await,
            vec!["TEST-owned"]
        );
        assert!(!path.exists());
        assert!(process(std::process::id()).unwrap().is_some());
    }

    #[tokio::test]
    async fn inherited_kernel_lock_defers_stale_recovery_until_the_child_releases_it() {
        // The child only performs async-signal-safe syscalls after fork, and
        // the parent's guard releases/reaps it even if an assertion panics.
        struct Child {
            pid: libc::pid_t,
            signal: i32,
        }
        impl Drop for Child {
            fn drop(&mut self) {
                unsafe {
                    libc::write(self.signal, b"x".as_ptr().cast(), 1);
                    libc::close(self.signal);
                    while libc::waitpid(self.pid, std::ptr::null_mut(), 0) == -1
                        && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR)
                    {
                    }
                }
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let config = config(dir.path());
        let (path, digest) = stale(&config, u32::MAX, 1);
        fake(&config, &digest);
        let held = lock(&path.with_extension("lock")).unwrap().unwrap();
        let mut pipe = [-1; 2];
        assert_eq!(
            unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC) },
            0
        );
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0);
        if pid == 0 {
            unsafe {
                libc::close(pipe[1]);
                let mut byte = 0_u8;
                libc::read(pipe[0], (&mut byte as *mut u8).cast(), 1);
                libc::_exit(0);
            }
        }
        unsafe { libc::close(pipe[0]) };
        let child = Child {
            pid,
            signal: pipe[1],
        };
        drop(held);
        let original = fs::read(&path).unwrap();
        assert!(recover(&config).await.unwrap().is_empty());
        assert_eq!(fs::read(&path).unwrap(), original);
        drop(child);
        assert_eq!(
            eventually_recover_fake(&config, &path).await,
            vec!["TEST-owned"]
        );
        assert!(!path.exists());
        let state: serde_json::Value = serde_json::from_slice(
            &fs::read(config.storage_root.unwrap().join("TEST-state.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(state["removed"], serde_json::json!(["a".repeat(64)]));
    }
    #[tokio::test]
    async fn busy_fake_executable_preserves_intent_and_container_until_writer_closes() {
        let dir = tempfile::tempdir().unwrap();
        let config = config(dir.path());
        let (path, digest) = stale(&config, u32::MAX, 1);
        fake(&config, &digest);
        let original = fs::read(&path).unwrap();
        let state_path = config
            .storage_root
            .as_ref()
            .unwrap()
            .join("TEST-state.json");
        let state = fs::read(&state_path).unwrap();
        // Deterministically reproduce Linux's exec refusal for a file with an
        // open writer, rather than relying on the parallel fork timing window.
        let writer = fs::OpenOptions::new()
            .write(true)
            .open(&config.program)
            .unwrap();
        let failure = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                match recover(&config).await {
                    Err(message) => break message,
                    Ok(recovered) => assert!(recovered.is_empty()),
                }
                assert_eq!(fs::read(&path).unwrap(), original);
                assert_eq!(fs::read(&state_path).unwrap(), state);
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("busy executable was not attempted after transient inherited lock release");
        assert_eq!(
            failure,
            std::io::Error::from_raw_os_error(libc::ETXTBSY).to_string()
        );
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(fs::read(&state_path).unwrap(), state);
        drop(writer);
        assert_eq!(
            eventually_recover_fake(&config, &path).await,
            vec!["TEST-owned"]
        );
        assert!(!path.exists());
        let state: serde_json::Value =
            serde_json::from_slice(&fs::read(state_path).unwrap()).unwrap();
        assert_eq!(state["removed"], serde_json::json!(["a".repeat(64)]));
    }
    #[test]
    fn replaced_lock_inode_blocks_cleanup_before_any_container_command() {
        let dir = tempfile::tempdir().unwrap();
        let config = config(dir.path());
        let intent = OwnedIntent::create(&config, "TEST-owned").unwrap().unwrap();
        let path = intent.path.with_extension("lock");
        fs::rename(&path, path.with_extension("old-lock")).unwrap();
        fs::write(&path, b"foreign lock").unwrap();
        // No fake Podman program exists: refusal must precede a CLI call.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        assert!(
            runtime
                .block_on(intent.cleanup(&config))
                .unwrap_err()
                .contains("lock inode changed")
        );
        assert_eq!(fs::read(path).unwrap(), b"foreign lock");
        assert_eq!(fs::read(&intent.path).unwrap(), intent.bytes);
    }

    #[test]
    fn foreign_temporary_or_unsafe_name_is_never_removed() {
        let dir = tempfile::tempdir().unwrap();
        let config = config(dir.path());
        let root = root(&config).unwrap().unwrap();
        let temporary = root.join(".TEST-owned.pending");
        fs::write(&temporary, b"foreign pending bytes").unwrap();
        assert!(OwnedIntent::create(&config, "TEST-owned").is_err());
        assert_eq!(fs::read(temporary).unwrap(), b"foreign pending bytes");
        for name in ["../escape", "/absolute", "..", "", "name\nvalue"] {
            assert!(OwnedIntent::create(&config, name).is_err());
        }
    }
}
