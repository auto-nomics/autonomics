//! The artifact store: staged uploads/downloads over the same ssh
//! channel, in-place references through the declared data home, and
//! retained publications.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::super::contract::{TaskAttemptReceipt, TaskDispatch, TaskExecution};
use super::super::process::{
    ProcessTaskSpec, parse_process_task, process_task_environment, render_process_script,
    sanitize_path_component,
};
use super::super::traits::{TaskArtifactStore, TaskExecutor, TaskLease, TaskTransport};
use super::ssh::{SshTarget, ensure_ssh_success, hex_encode, sh_single_quote, ssh_stdout_text};
use super::transport::{
    attempt_key, remote_task_dir, render_collect_script, resolve_remote_prefix,
};
use crate::dag::DagError;
use crate::dag::node_event::JobResult;
use crate::value::{DomainId, FileFingerprint, FileRef};

// ── artifact store ───────────────────────────────────────────────────────
/// A retained cluster filesystem exposed through a read-only VFS mount.
/// Aliases are explicit operator assertions that data/login endpoints share
/// this filesystem; the login endpoint reopens and measures every reference.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SshDataHome {
    pub ssh_aliases: Vec<String>,
    pub remote_root: String,
    pub vfs_prefix: String,
    /// Catalog identity of this domain, flowing into every placement the
    /// store publishes so downstream consumers can compare domains by id.
    /// Legacy profiles (frozen before domains existed) carry none: their
    /// placements stay unmarked (`local`), preserving old behavior.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain_id: Option<String>,
    /// Optional dedicated output namespace. Both fields must be supplied;
    /// inputs remain confined to `remote_root` and its admitted SSH aliases.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retained_vfs_prefix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retained_remote_root: Option<String>,
}
fn clean_absolute(path: &str) -> bool {
    path.starts_with('/')
        && path != "/"
        && !path.ends_with('/')
        && !path
            .split('/')
            .skip(1)
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
        && !path.chars().any(char::is_control)
}
impl SshDataHome {
    fn retained_mapping(&self) -> (&str, &str) {
        match (&self.retained_vfs_prefix, &self.retained_remote_root) {
            (Some(prefix), Some(root)) => (prefix, root),
            _ => (&self.vfs_prefix, &self.remote_root),
        }
    }
    pub(super) fn validate(&self) -> Result<(), DagError> {
        if self
            .domain_id
            .as_deref()
            .is_some_and(|id| !vfs::uri::valid_domain_name(id) || id == "local")
        {
            return Err(DagError::Schedule(
                "invalid remote filesystem domain URI scheme".into(),
            ));
        }
        if !clean_absolute(&self.remote_root)
            || !clean_absolute(&self.vfs_prefix)
            || self.ssh_aliases.is_empty()
            || self.ssh_aliases.iter().any(|alias| {
                alias.is_empty()
                    || alias.starts_with('-')
                    || !alias
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"_.@:-".contains(&c))
            })
        {
            return Err(DagError::Schedule("invalid retained SSH data home".into()));
        }
        match (&self.retained_vfs_prefix, &self.retained_remote_root) {
            (None, None) => {}
            (Some(prefix), Some(root)) if clean_absolute(prefix)
                && clean_absolute(root)
                && prefix != &self.vfs_prefix
                && root != &self.remote_root
                && Path::new(root).starts_with(&self.remote_root) => {}
            _ => return Err(DagError::Schedule(
                "retained output prefix/root must be a paired, distinct namespace below the data home".into()
            )),
        }
        Ok(())
    }
    /// Bind the operator's retained-output namespace to the actual VFS.
    /// Check nested overrides too: a deeper local or different SSH mount must
    /// never silently reinterpret a generated retained artifact reference.
    pub fn validate_vfs(&self, storage: &vfs::OpendalFileStorage) -> Result<(), DagError> {
        self.validate()?;
        self.validate_vfs_mapping(storage, &self.vfs_prefix, &self.remote_root)?;
        if let (Some(_), Some(prefix), Some(root)) = (
            &self.domain_id,
            &self.retained_vfs_prefix,
            &self.retained_remote_root,
        ) {
            let relative = Path::new(root)
                .strip_prefix(&self.remote_root)
                .map_err(|_| DagError::Schedule("retained root outside domain".into()))?;
            let expected = format!("{}/{}", self.vfs_prefix, relative.to_string_lossy());
            if *prefix != expected {
                return Err(DagError::Schedule(format!(
                    "named domain retained mount must preserve its primary projection: expected '{expected}'"
                )));
            }
        }
        if let (Some(prefix), Some(root)) = (&self.retained_vfs_prefix, &self.retained_remote_root)
        {
            self.validate_vfs_mapping(storage, prefix, root)?;
        }
        // A named domain must agree with its *primary* mount partition: the
        // `<domain>://` address layer resolves against the mount's declared
        // domain, so a mismatch would make addresses and placements
        // disagree about which filesystem they name. The retained output
        // namespace is a sub-partition of the same domain and does not
        // carry its own address.
        if let Some(declared) = &self.domain_id
            && storage.mount_domain(&self.vfs_prefix).as_deref() != Some(declared.as_str())
        {
            return Err(DagError::Schedule(
                "data domain id must match the domain declared by its VFS mount".into(),
            ));
        }
        Ok(())
    }
    fn validate_vfs_mapping(
        &self,
        storage: &vfs::OpendalFileStorage,
        vfs_prefix: &str,
        remote_root: &str,
    ) -> Result<(), DagError> {
        // The published namespace must be mounted read-only: placements in
        // this domain are measured once and trusted thereafter, and a
        // writable mount would let silent overwrites defeat that trust.
        // Writable work areas belong to a *different* mount partition of
        // the same filesystem, not to the published domain namespace.
        if storage.mount_read_only(vfs_prefix) == Some(false) {
            return Err(DagError::Schedule(
                "retained VFS namespace must be mounted read_only".into(),
            ));
        }
        let mut prefixes = vec![vfs_prefix.to_string()];
        if let Some(mounts) = &storage.mounts {
            prefixes.extend(
                mounts
                    .mount_paths()
                    .into_iter()
                    .filter(|path| path.starts_with(&format!("{vfs_prefix}/"))),
            );
        }
        for prefix in prefixes {
            let virtual_path = format!("{prefix}/__autonomics_retained_mapping_probe");
            let expected = format!(
                "{}{}{}",
                remote_root,
                prefix.strip_prefix(vfs_prefix).unwrap_or_default(),
                "/__autonomics_retained_mapping_probe"
            );
            let location = storage
                .ssh_location(&virtual_path)
                .map_err(|_| {
                    DagError::Schedule(
                        "retained VFS mapping is not readable by the operator".into(),
                    )
                })?
                .ok_or_else(|| {
                    DagError::Schedule("retained VFS namespace must resolve to SSH storage".into())
                })?;
            if !self.ssh_aliases.contains(&location.alias) || location.path != expected {
                return Err(DagError::Schedule(
                    "retained VFS namespace does not match its SSH data home".into(),
                ));
            }
        }
        Ok(())
    }
}
// Data bytes stay on the shared filesystem. Measure through a held no-follow
// descriptor and refuse changed files; only the bounded identity JSON returns.
const MEASURE_DATA_HOME_FILE: &str = r#"
import os,sys,json,stat,hashlib
r=json.loads(sys.argv[1])
root=r['root']; path=r['path']
if not path.startswith(root+'/'): raise ValueError('outside data root')
parts=path.split('/')[1:]
if any(p in ('','.','..') for p in parts): raise ValueError('invalid path')
fd=os.open('/',os.O_PATH|os.O_DIRECTORY|os.O_NOFOLLOW)
try:
 for p in parts[:-1]:
  nxt=os.open(p,os.O_PATH|os.O_DIRECTORY|os.O_NOFOLLOW,dir_fd=fd)
  os.close(fd); fd=nxt
 data=os.open(parts[-1],os.O_RDONLY|os.O_NONBLOCK|os.O_NOFOLLOW,dir_fd=fd)
 try:
  before=os.fstat(data)
  if not stat.S_ISREG(before.st_mode): raise ValueError('regular file required')
  h=hashlib.sha256()
  while True:
   chunk=os.read(data,1024*1024)
   if not chunk: break
   h.update(chunk)
  key=lambda s:(s.st_dev,s.st_ino,s.st_mode,s.st_size,s.st_mtime_ns,s.st_ctime_ns)
  if key(before)!=key(os.fstat(data)) or key(before)!=key(os.stat(parts[-1],dir_fd=fd,follow_symlinks=False)): raise ValueError('changed file')
  print(json.dumps({'size':before.st_size,'sha256':h.hexdigest()}))
 finally: os.close(data)
finally: os.close(fd)
"#;
/// A [`TaskArtifactStore`] that moves single files over the same ssh
/// channel as the transport (`cat > remote` to upload, `cat remote` to
/// download), keeping the ssh surface at exactly one program.
#[derive(Debug)]
pub struct SshArtifactStore {
    ssh: SshTarget,
    scratch_dir: String,
    local_root: PathBuf,
    scratch_home: Mutex<Option<String>>,
    pub(super) data_home: Option<SshDataHome>,
}
impl SshArtifactStore {
    /// The catalog identity of the domain this store's in-place references
    /// and retained publications belong to. Legacy profiles without a
    /// declared domain keep placements unmarked (`local`).
    fn placement_domain(&self) -> DomainId {
        self.data_home
            .as_ref()
            .and_then(|home| home.domain_id.as_deref())
            .map(DomainId::new)
            .unwrap_or_default()
    }
    pub fn new(
        ssh: SshTarget,
        scratch_dir: impl Into<String>,
        local_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            ssh,
            scratch_dir: scratch_dir.into(),
            local_root: local_root.into(),
            scratch_home: Mutex::new(None),
            data_home: None,
        }
    }
    pub fn with_data_home(mut self, data_home: SshDataHome) -> Result<Self, DagError> {
        data_home.validate()?;
        if !clean_absolute(&self.scratch_dir)
            || !Path::new(&self.scratch_dir).starts_with(&data_home.remote_root)
            || self.scratch_dir == data_home.remote_root
        {
            return Err(DagError::Schedule(
                "remote task work must be below its retained data home".into(),
            ));
        }
        let (_, retained_root) = data_home.retained_mapping();
        if !Path::new(&self.scratch_dir).starts_with(retained_root) {
            return Err(DagError::Schedule(
                "task work must be inside the retained output root".into(),
            ));
        }
        self.data_home = Some(data_home);
        Ok(self)
    }
    async fn measure_data_file(&self, path: &str, original: &FileRef) -> Result<FileRef, DagError> {
        let home = self
            .data_home
            .as_ref()
            .ok_or_else(|| DagError::Schedule("SSH data home is disabled".into()))?;
        if !clean_absolute(path)
            || !Path::new(path).starts_with(&home.remote_root)
            || path == home.remote_root
        {
            return Err(DagError::Schedule(
                "remote file is outside its retained data home".into(),
            ));
        }
        let request = serde_json::json!({"root":home.remote_root,"path":path}).to_string();
        let command = format!(
            "python3 -c {} {}",
            sh_single_quote(MEASURE_DATA_HOME_FILE),
            sh_single_quote(&request)
        );
        let raw = ssh_stdout_text(
            self.ssh.run_script_once(&command).await?,
            "remote file identity",
        )?;
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Identity {
            size: u64,
            sha256: String,
        }
        let identity: Identity = serde_json::from_str(&raw)
            .map_err(|_| DagError::Schedule("invalid remote file identity".into()))?;
        if identity.sha256.len() != 64 || !identity.sha256.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err(DagError::Schedule("invalid remote file SHA256".into()));
        }
        if let Some(expected) = &original.fingerprint {
            if expected.size != identity.size
                || expected.content_hash.as_ref().is_some_and(|hash| {
                    hash.strip_prefix("sha256:").unwrap_or(hash) != identity.sha256
                })
            {
                return Err(DagError::Schedule(
                    "retained remote file fingerprint changed".into(),
                ));
            }
        }
        let mut result = FileRef::remote(
            path,
            original.format.clone(),
            identity.size,
            Some(identity.sha256),
        )
        .in_domain(self.placement_domain());
        // A POSIX file is not an immutable object store, even after hashing.
        if let Some(fingerprint) = &mut result.fingerprint {
            fingerprint.immutable_remote = false;
        }
        Ok(result)
    }
    /// Absolute remote path for one artifact inside the task's scratch dir.
    async fn remote_path(&self, task_id: &str, name: &str) -> Result<String, DagError> {
        let scratch =
            resolve_remote_prefix(&self.ssh, &self.scratch_dir, &self.scratch_home).await?;
        Ok(format!(
            "{}/{}/{}",
            scratch.trim_end_matches('/'),
            sanitize_path_component(task_id),
            name
        ))
    }
}
#[async_trait::async_trait]
impl TaskArtifactStore for SshArtifactStore {
    async fn remote_input(
        &self,
        artifact: FileRef,
        storage: Option<&vfs::OpendalFileStorage>,
    ) -> Result<Option<FileRef>, DagError> {
        let Some(home) = &self.data_home else {
            return Ok(None);
        };
        let Some(storage) = storage else {
            return Ok(None);
        };
        // Placement algebra decides first: a tagged placement belonging to
        // a different domain skips physical admission and stages; a tagged
        // same-domain placement (and every legacy untagged record) proceeds
        // to the alias/endpoint admission below, which remains the trust
        // step for in-place references.
        match super::super::placement::placement_decision(
            &artifact.domain,
            &self.placement_domain(),
        ) {
            super::super::placement::Movement::Stage => return Ok(None),
            super::super::placement::Movement::Reject(reason) => {
                return Err(DagError::Schedule(reason.to_string()));
            }
            super::super::placement::Movement::ReferenceInPlace => {}
        }
        let Some(location) = storage
            .ssh_location(&artifact.path)
            .map_err(|error| DagError::Schedule(format!("remote input authorization: {error}")))?
        else {
            return Ok(None);
        };
        let expected_endpoint = self
            .ssh
            .shared_endpoints
            .get(&location.alias)
            .cloned()
            .or_else(|| {
                (location.alias == self.ssh.host)
                    .then(|| self.ssh.endpoint.clone())
                    .flatten()
            });
        let actual_endpoint = if location.endpoint.is_none() && expected_endpoint.is_some() {
            // An existing vfs.toml mount can still use an enrolled shared data
            // endpoint. Resolve its current local SSH configuration, never
            // assume that an unpinned alias still names the original server.
            vfs::SshEndpoint::capture("ssh", &location.alias).ok()
        } else {
            location.endpoint.clone()
        };
        if !home.ssh_aliases.contains(&location.alias)
            || actual_endpoint != expected_endpoint
            || !Path::new(&location.path).starts_with(&home.remote_root)
        {
            // Another authorized data source needs ordinary VFS staging. Its
            // physical path is never interpreted on this compute target.
            return Ok(None);
        }
        self.measure_data_file(&location.path, &artifact)
            .await
            .map(Some)
    }
    async fn retained_reference(&self, artifact: FileRef) -> Result<Option<FileRef>, DagError> {
        let Some(home) = &self.data_home else {
            return Ok(None);
        };
        let (prefix, root) = home.retained_mapping();
        let relative = Path::new(&artifact.path)
            .strip_prefix(root)
            .map_err(|_| DagError::Schedule("remote output is outside its retained root".into()))?;
        if relative.as_os_str().is_empty() {
            return Err(DagError::Schedule(
                "retained output must be a file below its root".into(),
            ));
        }
        let mut reference = self.measure_data_file(&artifact.path, &artifact).await?;
        reference.path = if let Some(domain) = &home.domain_id {
            // Domain addresses are relative to the primary filesystem root,
            // including the retained subdirectory when one is configured.
            let domain_relative = Path::new(&artifact.path)
                .strip_prefix(&home.remote_root)
                .map_err(|_| DagError::Schedule("output is outside its domain root".into()))?;
            format!(
                "{domain}://{}",
                vfs::uri::encode_path(&domain_relative.to_string_lossy())
            )
        } else {
            vfs::uri::uri(format!("{prefix}/{}", relative.to_string_lossy()))
        };
        // measure_data_file already stamps the domain; assert it stays put
        // when the path is rewritten into the VFS projection.
        debug_assert_eq!(reference.domain, self.placement_domain());
        Ok(Some(reference))
    }
    async fn upload(
        &self,
        artifact: FileRef,
        task_id: &str,
        name: &str,
    ) -> Result<FileRef, DagError> {
        self.upload_observed(
            artifact,
            task_id,
            name,
            &crate::dag::node_event::NodeReporter::noop(),
        )
        .await
    }
    async fn upload_observed(
        &self,
        artifact: FileRef,
        task_id: &str,
        name: &str,
        reporter: &crate::dag::node_event::NodeReporter,
    ) -> Result<FileRef, DagError> {
        let phase = format!("hashing_upload:{name}");
        reporter.phase(&phase, "verifying upload source");
        // Reject FIFO/socket/directory/link/missing inputs before even the
        // first SSH home lookup or mkdir, then keep this exact file descriptor.
        let mut source = super::ssh::admit_upload_source(Path::new(&artifact.path))?;
        let before = source
            .metadata()
            .map_err(|error| DagError::Schedule(error.to_string()))?;
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        let mut size = 0u64;
        let mut last = std::time::Instant::now();
        reporter.phase_progress(&phase, 0, Some(before.len()), "bytes");
        loop {
            let count = std::io::Read::read(&mut source, &mut buffer)
                .map_err(|error| DagError::Schedule(error.to_string()))?;
            if count == 0 {
                break;
            }
            size += count as u64;
            hasher.update(&buffer[..count]);
            if last.elapsed() >= Duration::from_secs(1) {
                reporter.phase_progress(&phase, size, Some(before.len()), "bytes");
                last = std::time::Instant::now();
                tokio::task::yield_now().await;
            }
        }
        reporter.phase_progress(&phase, size, Some(before.len()), "bytes");
        let after = source
            .metadata()
            .map_err(|error| DagError::Schedule(error.to_string()))?;
        if before.len() != size
            || before.len() != after.len()
            || before.modified().ok() != after.modified().ok()
        {
            return Err(DagError::Schedule(
                "upload input changed during hashing".into(),
            ));
        }
        let digest = hex_encode(&hasher.finalize());
        if let Some(expected) = &artifact.fingerprint {
            if expected.size != size
                || expected
                    .content_hash
                    .as_ref()
                    .is_some_and(|hash| hash.strip_prefix("sha256:").unwrap_or(hash) != digest)
            {
                return Err(DagError::Schedule(
                    "upload input fingerprint changed".into(),
                ));
            }
        }
        std::io::Seek::seek(&mut source, std::io::SeekFrom::Start(0))
            .map_err(|error| DagError::Schedule(error.to_string()))?;
        let remote = self.remote_path(task_id, name).await?;
        let parent = match remote.rfind('/') {
            Some(index) => remote[..index].to_string(),
            None => ".".into(),
        };
        ssh_stdout_text(
            self.ssh
                .run(&format!("umask 077; mkdir -p {}", sh_single_quote(&parent)))
                .await?,
            "slurm artifact staging",
        )?;
        for attempt in 0..2 {
            std::io::Seek::seek(&mut source, std::io::SeekFrom::Start(0))
                .map_err(|error| DagError::Schedule(error.to_string()))?;
            let output = self
                .ssh
                .run_with_admitted_file_stdin_observed(
                    &format!("umask 077; set -C; cat > {}", sh_single_quote(&remote)),
                    source
                        .try_clone()
                        .map_err(|error| DagError::Schedule(error.to_string()))?,
                    reporter,
                    &format!("uploading:{name}"),
                )
                .await?;
            if output.status.success() {
                reporter.phase_progress(&format!("uploading:{name}"), size, Some(size), "bytes");
                break;
            }
            if output.status.code() != Some(255) || attempt != 0 {
                ensure_ssh_success(output, "slurm artifact upload")?;
                unreachable!();
            }
            // A lost SSH reply does not tell us whether cat ran. Inspect the
            // exact target before deciding: complete bytes are usable input,
            // absence permits one retry, and partial/foreign bytes stay held.
            // This never retries sbatch or invents a remote command exit.
            let target = sh_single_quote(&remote);
            let observed = ssh_stdout_text(self.ssh.run(&format!(
                "set -e; if [ -L {target} ] || {{ [ -e {target} ] && [ ! -f {target} ]; }}; then printf 'FOREIGN\\n'; elif [ -f {target} ]; then printf 'FILE\\n'; stat -c %s -- {target}; sha256sum -- {target}; else printf 'ABSENT\\n'; fi"
            )).await?, "slurm interrupted upload readback")?;
            if observed == "ABSENT\n" {
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
            let lines = observed.lines().collect::<Vec<_>>();
            if lines.len() == 3
                && lines[0] == "FILE"
                && lines[1].parse::<u64>().ok() == Some(size)
                && lines[2].split_whitespace().next() == Some(digest.as_str())
            {
                break;
            }
            return Err(DagError::Schedule(
                "interrupted upload left partial or foreign bytes; target retained without retry or overwrite".into(),
            ));
        }
        Ok(FileRef {
            path: remote,
            format: artifact.format.clone(),
            fingerprint: Some(FileFingerprint {
                size,
                mtime_ns: 0,
                content_hash: Some(digest),
                immutable_remote: false,
            }),
            // The uploaded copy physically lives in the target's scratch
            // domain; the coordinator-side original keeps its own domain.
            domain: self.placement_domain(),
        })
    }
    async fn download(
        &self,
        artifact: FileRef,
        task_id: &str,
        name: &str,
    ) -> Result<FileRef, DagError> {
        self.download_observed(
            artifact,
            task_id,
            name,
            &crate::dag::node_event::NodeReporter::noop(),
        )
        .await
    }
    async fn download_observed(
        &self,
        artifact: FileRef,
        task_id: &str,
        name: &str,
        reporter: &crate::dag::node_event::NodeReporter,
    ) -> Result<FileRef, DagError> {
        reporter.phase(
            &format!("downloading:{name}"),
            "verifying remote artifact identity",
        );
        let destination_dir = self.local_root.join(sanitize_path_component(task_id));
        std::fs::create_dir_all(&destination_dir).map_err(|error| {
            DagError::Schedule(format!(
                "cannot create slurm artifact directory `{}`: {error}",
                destination_dir.display()
            ))
        })?;
        if Path::new(name).components().count() != 1 || name == "." || name == ".." {
            return Err(DagError::Schedule(
                "invalid artifact destination name".into(),
            ));
        }
        let destination = destination_dir.join(name);
        // Artifact paths normally come from our own receipts (absolute), but
        // resolve defensively so a tilde never reaches a quoted ssh command.
        let remote_source =
            resolve_remote_prefix(&self.ssh, &artifact.path, &self.scratch_home).await?;
        let metadata = ssh_stdout_text(
            self.ssh
                .run(&format!(
                    "stat -c %s -- {source} && sha256sum -- {source}",
                    source = sh_single_quote(&remote_source)
                ))
                .await?,
            "slurm artifact metadata",
        )?;
        let mut lines = metadata.lines();
        let expected_size = lines
            .next()
            .and_then(|s| s.parse::<u64>().ok())
            .ok_or_else(|| DagError::Schedule("invalid artifact size".into()))?;
        let expected_hash = lines
            .next()
            .and_then(|s| s.split_whitespace().next())
            .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or_else(|| DagError::Schedule("invalid artifact SHA256".into()))?;
        if lines.next().is_some() {
            return Err(DagError::Schedule("ambiguous artifact metadata".into()));
        }
        let fingerprint = match &artifact.fingerprint {
            Some(expected) => {
                if expected.size != expected_size
                    || expected
                        .content_hash
                        .as_deref()
                        .is_some_and(|hash| !hash.eq_ignore_ascii_case(expected_hash))
                {
                    return Err(DagError::Schedule(
                        "artifact fingerprint mismatch for slurm download".into(),
                    ));
                }
                let mut verified = expected.clone();
                if verified.content_hash.is_none() {
                    verified.content_hash = Some(expected_hash.to_ascii_lowercase());
                }
                Some(verified)
            }
            None => Some(FileFingerprint::remote(
                expected_size,
                Some(expected_hash.to_ascii_lowercase()),
            )),
        };
        stream_download_observed(
            &self.ssh,
            &remote_source,
            &destination,
            expected_size,
            expected_hash,
            reporter,
            &format!("downloading:{name}"),
        )
        .await?;
        Ok(FileRef {
            domain: DomainId::local(),
            path: destination.to_string_lossy().into_owned(),
            format: artifact.format.clone(),
            fingerprint,
        })
    }
}
// Anonymous Linux staging has no pathname to replace or accidentally delete.
// Closing its held FD on failure/cancellation is the complete cleanup operation.
struct StagedDownload(std::fs::File);
impl StagedDownload {
    #[cfg(target_os = "linux")]
    fn create(destination: &Path) -> std::io::Result<Self> {
        use std::os::unix::fs::OpenOptionsExt;
        let parent = destination.parent().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "missing destination directory",
            )
        })?;
        // Linux O_TMPFILE | O_NOFOLLOW. No named-file fallback: unsupported
        // filesystems must fail without touching an existing destination.
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(0x410000 | 0x20000)
            .open(parent)
            .map(Self)
    }
    #[cfg(not(target_os = "linux"))]
    fn create(_destination: &Path) -> std::io::Result<Self> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "safe anonymous Slurm downloads require Linux O_TMPFILE",
        ))
    }
    #[cfg(target_os = "linux")]
    fn publish(&self, destination: &Path) -> std::io::Result<()> {
        use std::ffi::CString;
        use std::os::fd::AsRawFd;
        use std::os::unix::ffi::OsStrExt;
        unsafe extern "C" {
            fn linkat(
                oldfd: i32,
                oldpath: *const std::ffi::c_char,
                newfd: i32,
                newpath: *const std::ffi::c_char,
                flags: i32,
            ) -> i32;
        }
        let source = CString::new(format!("/proc/self/fd/{}", self.0.as_raw_fd()))?;
        let destination = CString::new(destination.as_os_str().as_bytes())?;
        // AT_FDCWD / AT_SYMLINK_FOLLOW: follow only our held proc FD, never a
        // mutable staging pathname. linkat is atomic and never replaces EEXIST.
        // SAFETY: both C strings live across the call and the FD remains owned.
        let result = unsafe { linkat(-100, source.as_ptr(), -100, destination.as_ptr(), 0x400) };
        if result == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
    #[cfg(not(target_os = "linux"))]
    fn publish(&self, _destination: &Path) -> std::io::Result<()> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "safe anonymous Slurm downloads require Linux",
        ))
    }
}
async fn stream_download_observed(
    ssh: &SshTarget,
    source: &str,
    destination: &Path,
    expected_size: u64,
    expected_hash: &str,
    reporter: &crate::dag::node_event::NodeReporter,
    phase: &str,
) -> Result<(), DagError> {
    let started = std::time::Instant::now();
    for attempt in 0..2 {
        let remaining = ssh.transfer_timeout.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Err(DagError::Schedule(
                "artifact transfer timed out; no retry budget remains".into(),
            ));
        }
        let connection = ssh.clone().with_transfer_timeout(remaining);
        match stream_download_once_observed(
            &connection,
            source,
            destination,
            expected_size,
            expected_hash,
            reporter,
            phase,
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(DagError::Schedule(message))
                if attempt == 0 && message == "artifact ssh transfer interrupted (exit 255)" =>
            {
                // Reading an authenticated artifact is idempotent. The failed
                // local child has been reaped and its anonymous staging FD is
                // closed; start a fresh full stream and verify all bytes again.
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!()
}
async fn stream_download_once_observed(
    ssh: &SshTarget,
    source: &str,
    destination: &Path,
    expected_size: u64,
    expected_hash: &str,
    reporter: &crate::dag::node_event::NodeReporter,
    phase: &str,
) -> Result<(), DagError> {
    let failure = |error| DagError::Schedule(format!("slurm artifact transfer failed: {error}"));
    let staging = StagedDownload::create(destination).map_err(failure)?;
    let mut file = tokio::fs::File::from_std(staging.0.try_clone().map_err(failure)?);
    let mut child = ssh
        .command()?
        .arg(format!("cat -- {}", sh_single_quote(source)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(failure)?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| DagError::Schedule("missing ssh stdout".into()))?;
    let transfer = async {
        let mut hash = Sha256::new();
        let mut size = 0u64;
        let mut last = std::time::Instant::now();
        reporter.phase_progress(phase, 0, Some(expected_size), "bytes");
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let count = stdout.read(&mut buffer).await.map_err(failure)?;
            if count == 0 {
                break;
            }
            size = size
                .checked_add(count as u64)
                .ok_or_else(|| DagError::Schedule("artifact size overflow".into()))?;
            if size > expected_size {
                return Err(DagError::Schedule("artifact size mismatch".into()));
            }
            hash.update(&buffer[..count]);
            file.write_all(&buffer[..count]).await.map_err(failure)?;
            if last.elapsed() >= Duration::from_secs(1) {
                reporter.phase_progress(phase, size, Some(expected_size), "bytes");
                last = std::time::Instant::now();
            }
        }
        let status = child.wait().await.map_err(failure)?;
        if !status.success() {
            return Err(DagError::Schedule(format!(
                "artifact ssh transfer interrupted (exit {})",
                status
                    .code()
                    .map_or_else(|| "signal".into(), |code| code.to_string())
            )));
        }
        if size != expected_size
            || !format!("{:x}", hash.finalize()).eq_ignore_ascii_case(expected_hash)
        {
            return Err(DagError::Schedule(
                "artifact size or SHA256 mismatch".into(),
            ));
        }
        file.sync_all().await.map_err(failure)?;
        staging.publish(destination).map_err(failure)?;
        reporter.phase_progress(phase, size, Some(expected_size), "bytes");
        Ok(())
    };
    let result = tokio::time::timeout(ssh.transfer_timeout, transfer)
        .await
        .unwrap_or_else(|_| {
            Err(DagError::Schedule(
                "artifact transfer timed out; destination retained".into(),
            ))
        });
    if let Err(error) = result {
        // An early size/read/write error or deadline can occur before wait().
        // kill_on_drop only requests termination; do not return a handled
        // failure while relying on a runtime which may immediately shut down
        // to reap the local SSH process later. No remote job stop is inferred.
        drop(stdout);
        let _ = child.start_kill();
        match tokio::time::timeout(Duration::from_secs(5), child.wait()).await {
            Ok(Ok(_)) => return Err(error),
            Ok(Err(wait_error)) => {
                return Err(DagError::Schedule(format!(
                    "{error}; local SSH child wait failed: {wait_error}; local stop UNKNOWN"
                )));
            }
            Err(_) => {
                return Err(DagError::Schedule(format!(
                    "{error}; local SSH child wait timed out; local stop UNKNOWN"
                )));
            }
        }
    }
    Ok(())
}
