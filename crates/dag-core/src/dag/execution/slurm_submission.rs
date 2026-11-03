//! Durable pre-submission metadata. This contains no command/environment bytes.
//! Reading it is not authorization to cancel, resubmit or accept a result.
use super::*;
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

const SCHEMA: &str = "SLURM_SUBMISSION_INTENT_V1";

/// Trusted host integration, called synchronously after the private intent is
/// durable and before SSH can submit anything. An error forbids submission.
/// Ordinary transports do not install this hook.
pub trait SlurmSubmissionRecorder: std::fmt::Debug + Send + Sync {
    /// Private runtimes must refuse preparation (including remote mkdir and
    /// input transfer) before their running context is bound. This is an early
    /// check, not a substitute for the durable atomic before_submit callback.
    fn ensure_ready(&self) -> Result<(), DagError> {
        Ok(())
    }

    fn before_submit(
        &self,
        intent: &SlurmSubmissionIntent,
    ) -> Result<std::sync::Arc<dyn SlurmRecordedSubmission>, DagError>;
}

/// Owns the submission's journal/lease scope. Stop facts are raw scheduler
/// observations; the trusted consumer must independently validate them.
pub trait SlurmRecordedSubmission: std::fmt::Debug + Send + Sync {
    fn stopped(&self, facts: &serde_json::Value) -> Result<(), DagError>;
    /// Synchronous notification on error/cancellation, including dropped
    /// futures. It never clears the durable outstanding obligation.
    fn unresolved(&self);
}

/// Do not keep a failed or dropped future looking like a live coordinator.
pub(super) struct SubmissionScope {
    record: Option<std::sync::Arc<dyn SlurmRecordedSubmission>>,
    armed: bool,
}
impl SubmissionScope {
    pub(super) fn new(record: Option<std::sync::Arc<dyn SlurmRecordedSubmission>>) -> Self {
        Self {
            record,
            armed: true,
        }
    }
    pub(super) fn disarm(&mut self) {
        self.armed = false;
    }
}
impl Drop for SubmissionScope {
    fn drop(&mut self) {
        if self.armed {
            if let Some(record) = &self.record {
                record.unresolved();
            }
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlurmSubmissionIntent {
    pub schema: String,
    pub task_id: String,
    pub job_name: String,
    pub nonce: String,
    pub dispatch_sha256: String,
    pub manifest_sha256: String,
    pub manifest_bytes: u64,
    pub remote_task_dir: String,
    pub submission_identity: String,
    pub transport_binding: String,
    pub attempted_at_unix_ms: u128,
}

fn invalid(message: &str) -> DagError {
    DagError::Schedule(message.into())
}

pub(super) fn identity(directory: &str, manifest: &str) -> String {
    hex_encode(
        &Sha256::new()
            .chain_update(directory.as_bytes())
            .chain_update([0])
            .chain_update(manifest.as_bytes())
            .finalize(),
    )
}

/// Only non-secret, configured transport choices. A recovery caller must also
/// retain its operator-pinned SSH endpoint/target revision; an alias alone is
/// not a physical endpoint identity.
pub(super) fn transport_binding(settings: &SlurmSettings) -> Result<String, DagError> {
    settings.ssh.verify_endpoints()?;
    let mut value = serde_json::json!({
        "ssh_program":settings.ssh.program,"ssh_host":settings.ssh.host,
        "scratch_dir":settings.scratch_dir,"partition":settings.partition,
        "account":settings.account,"gpus":settings.gpus,"qos":settings.qos,
        "mem_per_cpu":settings.mem_per_cpu,"default_time":settings.default_time,
        "apptainer":settings.apptainer
    });
    // Preserve ordinary legacy identities byte-for-byte when no profile was
    // installed. A pinned profile binds its non-secret source files too.
    if let Some(profile) = &settings.ssh.pinned_configuration {
        value["ssh_pinned_configuration"] =
            serde_json::to_value(profile).map_err(|_| invalid("cannot encode SSH profile"))?;
    }
    if let Some(endpoint) = &settings.ssh.endpoint {
        value["ssh_endpoint"] = serde_json::to_value(endpoint)
            .map_err(|_| invalid("cannot encode frozen SSH endpoint"))?;
    }
    if !settings.ssh.shared_endpoints.is_empty() {
        value["ssh_shared_endpoints"] = serde_json::to_value(&settings.ssh.shared_endpoints)
            .map_err(|_| invalid("cannot encode shared SSH endpoint identities"))?;
    }
    let raw = serde_json::to_vec(&value)
        .map_err(|_| invalid("cannot encode configured Slurm binding"))?;
    Ok(hex_encode(&Sha256::digest(raw)))
}

impl SlurmSubmissionIntent {
    pub(super) fn new(
        settings: &SlurmSettings,
        task: &str,
        job: &str,
        nonce: &str,
        directory: &str,
        manifest: &str,
    ) -> Result<Self, DagError> {
        let v = Self {
            schema: SCHEMA.into(),
            task_id: task.into(),
            job_name: job.into(),
            nonce: nonce.into(),
            dispatch_sha256: hex_encode(&Sha256::digest(manifest.as_bytes())),
            manifest_sha256: hex_encode(
                &Sha256::new()
                    .chain_update(manifest.as_bytes())
                    .chain_update(b"\n")
                    .finalize(),
            ),
            manifest_bytes: manifest.len() as u64 + 1,
            remote_task_dir: directory.into(),
            submission_identity: identity(directory, manifest),
            transport_binding: transport_binding(settings)?,
            attempted_at_unix_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|_| invalid("submission clock precedes epoch"))?
                .as_millis(),
        };
        v.validate()?;
        Ok(v)
    }

    pub fn validate(&self) -> Result<(), DagError> {
        let hex = |s: &str, n| {
            s.len() == n
                && s.bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        };
        let path = Path::new(&self.remote_task_dir);
        if self.schema != SCHEMA
            || self.task_id.is_empty()
            || self.task_id.len() > 4096
            || self.job_name != slurm_job_name(&self.task_id, &self.nonce)
            || !hex(&self.nonce, 16)
            || !hex(&self.dispatch_sha256, 64)
            || !hex(&self.manifest_sha256, 64)
            || !hex(&self.submission_identity, 64)
            || !hex(&self.transport_binding, 64)
            || self.manifest_bytes == 0
            || self.attempted_at_unix_ms == 0
            || self.attempted_at_unix_ms > u64::MAX as u128
            || !path.is_absolute()
            || self.remote_task_dir.len() > 8192
            || self
                .remote_task_dir
                .chars()
                .any(|c| c.is_whitespace() || c.is_control())
            || self
                .remote_task_dir
                .split('/')
                .skip(1)
                .any(|s| s.is_empty() || matches!(s, "." | ".."))
        {
            return Err(invalid(
                "incomplete or unsupported Slurm recovery intent; retain evidence",
            ));
        }
        Ok(())
    }

    /// Read a complete owned record without following a final symlink or FIFO.
    /// Legacy records without content/placement binding remain unsupported.
    #[cfg(unix)]
    pub fn read(path: &Path) -> Result<Self, DagError> {
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
            .map_err(|_| invalid("cannot open a regular Slurm intent"))?;
        let info = file
            .metadata()
            .map_err(|_| invalid("intent metadata unavailable"))?;
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        // SAFETY: geteuid reads process credentials and takes no pointers.
        let current_uid = unsafe { libc::geteuid() };
        if !info.is_file()
            || info.len() > 16384
            || info.uid() != current_uid
            || info.permissions().mode() & 0o077 != 0
        {
            return Err(invalid(
                "Slurm intent is not a bounded owner-private regular file",
            ));
        }
        let mut raw = Vec::new();
        std::io::Read::by_ref(&mut file)
            .take(16385)
            .read_to_end(&mut raw)
            .map_err(|_| invalid("cannot read Slurm intent"))?;
        if raw.len() > 16384 {
            return Err(invalid("oversized Slurm intent"));
        }
        // Reject duplicate keys as well as unknown fields.
        let mut decoder = serde_json::Deserializer::from_slice(&raw);
        let value: Self = serde::Deserialize::deserialize(&mut decoder)
            .map_err(|_| invalid("unsupported or ambiguous Slurm intent"))?;
        decoder
            .end()
            .map_err(|_| invalid("trailing Slurm intent bytes"))?;
        value.validate()?;
        if path.file_name().and_then(|p| p.to_str())
            != Some(format!(".intent-{}.json", value.nonce).as_str())
        {
            return Err(invalid("intent filename differs from its nonce"));
        }
        Ok(value)
    }

    #[cfg(not(unix))]
    pub fn read(_path: &Path) -> Result<Self, DagError> {
        Err(invalid(
            "secure Slurm intent recovery is unsupported on this platform",
        ))
    }

    pub(super) fn persist(&self, root: &Path) -> Result<PathBuf, DagError> {
        self.validate()?;
        let dir = root.join(sanitize_path_component(&self.task_id));
        durable_directory(&dir)?;
        let path = dir.join(format!(".intent-{}.json", self.nonce));
        let raw = serde_json::to_vec_pretty(self)
            .map_err(|_| invalid("cannot serialize Slurm intent"))?;
        if raw.len() > 16384 {
            return Err(invalid("oversized Slurm intent"));
        }
        let mut staged = tempfile::NamedTempFile::new_in(&dir)
            .map_err(|_| invalid("cannot stage Slurm submission intent"))?;
        staged
            .write_all(&raw)
            .and_then(|_| staged.as_file().sync_all())
            .map_err(|_| invalid("cannot durably write Slurm submission intent"))?;
        staged
            .persist_noclobber(&path)
            .map_err(|_| invalid("Slurm submission intent exists or cannot be published"))?;
        std::fs::File::open(&dir)
            .and_then(|file| file.sync_all())
            .map_err(|_| {
                invalid("cannot sync Slurm submission intent directory; no submission permitted")
            })?;
        Ok(path)
    }

    /// Preserve the first transport outcome before reconciliation can obscure
    /// it. Private diagnostics are evidence only, never replay authorization.
    pub(super) fn persist_response(
        &self,
        root: &Path,
        response: &Result<Output, DagError>,
    ) -> Result<PathBuf, DagError> {
        self.validate()?;
        let dir = root.join(sanitize_path_component(&self.task_id));
        durable_directory(&dir)?;
        let path = dir.join(format!(".submission-response-{}.json", self.nonce));
        let bounded = |bytes: &[u8]| serde_json::json!({
            "bytes": bytes.len(), "sha256": hex_encode(&Sha256::digest(bytes)),
            "prefix": String::from_utf8_lossy(&bytes[..bytes.len().min(16384)]),
            "truncated": bytes.len() > 16384,
        });
        let outcome = match response {
            Ok(output) => serde_json::json!({"exit_code": output.status.code(),
                "stdout": bounded(&output.stdout), "stderr": bounded(&output.stderr)}),
            Err(error) => serde_json::json!({"transport_error": bounded(error.to_string().as_bytes())}),
        };
        let raw = serde_json::to_vec_pretty(&serde_json::json!({
            "schema": "SLURM_SUBMISSION_RESPONSE_V1", "nonce": self.nonce,
            "submission_identity": self.submission_identity,
            "manifest_sha256": self.manifest_sha256, "outcome": outcome,
            "resubmission_authorized": false,
        })).map_err(|_| invalid("cannot serialize submission response"))?;
        let mut staged = tempfile::NamedTempFile::new_in(&dir)
            .map_err(|_| invalid("cannot stage submission response"))?;
        staged.write_all(&raw).and_then(|_| staged.as_file().sync_all())
            .map_err(|_| invalid("cannot durably write submission response"))?;
        staged.persist_noclobber(&path)
            .map_err(|_| invalid("submission response exists or cannot be published"))?;
        std::fs::File::open(&dir).and_then(|file| file.sync_all())
            .map_err(|_| invalid("cannot sync submission response directory"))?;
        Ok(path)
    }
}

#[cfg(unix)]
fn durable_directory(path: &Path) -> Result<(), DagError> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .map_err(|_| invalid("intent base unavailable"))?
            .join(path)
    };
    let mut current = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::RootDir => {
                current.push(component);
                continue;
            }
            std::path::Component::Normal(_) => current.push(component),
            _ => return Err(invalid("canonical intent directory required")),
        }
        match std::fs::symlink_metadata(&current) {
            Ok(info) if info.is_dir() && !info.file_type().is_symlink() => continue,
            Ok(_) => return Err(invalid("Slurm intent directory is not a real directory")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => return Err(invalid("Slurm intent directory metadata unavailable")),
        }
        match std::fs::DirBuilder::new().mode(0o700).create(&current) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let info = std::fs::symlink_metadata(&current)
                    .map_err(|_| invalid("intent directory raced"))?;
                if !info.is_dir() || info.file_type().is_symlink() {
                    return Err(invalid("intent directory raced"));
                }
            }
            Err(_) => return Err(invalid("cannot create Slurm intent directory")),
        }
        for p in [current.as_path(), current.parent().unwrap()] {
            std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
                .open(p)
                .and_then(|f| f.sync_all())
                .map_err(|_| invalid("cannot sync new Slurm intent directory"))?;
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn durable_directory(_path: &Path) -> Result<(), DagError> {
    Err(invalid(
        "durable Slurm intent publication is unsupported on this platform",
    ))
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlurmSubmissionObservation {
    pub schema: String,
    pub submission_identity: String,
    pub manifest_sha256: String,
    pub remote_task_dir: String,
    pub state: String,
    pub job_id: Option<String>,
    pub queue: serde_json::Value,
    pub accounting: serde_json::Value,
    /// Observation never grants retry, cancellation, result acceptance or G0.
    pub resubmission_authorized: bool,
}

impl SlurmTaskTransport {
    /// Non-secret configured binding for preparation of an operator-approved
    /// formal target. It grants no execution authority and submits no work.
    pub fn configured_submission_binding(&self) -> Result<String, DagError> {
        transport_binding(&self.settings)
    }

    /// Request only the original, freshly re-observed owned job's cancellation.
    /// No retry of the command and no interpretation of its exit as stop proof.
    pub async fn request_submission_cancellation(
        &self,
        intent: &SlurmSubmissionIntent,
        expected_job: &str,
    ) -> Result<serde_json::Value, DagError> {
        let observed = self.inspect_submission_intent(intent).await?;
        if observed.job_id.as_deref() != Some(expected_job) {
            return Err(invalid(
                "original job ownership changed before cancellation",
            ));
        }
        let output = self
            .settings
            .ssh
            .run_script_once(&format!("scancel {expected_job}"))
            .await?;
        if output.stdout.len() > 65536 || output.stderr.len() > 65536 {
            return Err(invalid(
                "oversized cancellation response; stop remains unknown",
            ));
        }
        Ok(serde_json::json!({"schema":"SLURM_OWNED_CANCEL_REQUEST_V1",
            "submission_identity":intent.submission_identity,"job_id":expected_job,
            "exit":output.status.code(),"stdout":String::from_utf8_lossy(&output.stdout),
            "stderr":String::from_utf8_lossy(&output.stderr)}))
    }

    /// Read-only reconciliation. This does not run sbatch or recreate pending
    /// in-memory state, and missing records never become retry permission.
    pub async fn inspect_submission_intent(
        &self,
        intent: &SlurmSubmissionIntent,
    ) -> Result<SlurmSubmissionObservation, DagError> {
        let raw = self.observe_submission_raw(intent, false).await?;
        let value: SlurmSubmissionObservation =
            serde_json::from_str(&raw).map_err(|_| invalid("invalid recovery observation"))?;
        let found = value.state == "IDENTITY_VERIFIED_JOB_FOUND";
        if value.schema != "SLURM_SUBMISSION_OBSERVATION_V1"
            || value.submission_identity != intent.submission_identity
            || value.manifest_sha256 != intent.manifest_sha256
            || value.remote_task_dir != intent.remote_task_dir
            || value.resubmission_authorized
            || (!found && value.state != "UNRESOLVED")
            || (found
                && !value
                    .job_id
                    .as_ref()
                    .is_some_and(|id| !id.is_empty() && id.bytes().all(|c| c.is_ascii_digit())))
            || (!found && value.job_id.is_some())
        {
            return Err(invalid(
                "recovery observation is not bound to the original submission",
            ));
        }
        Ok(value)
    }
    async fn observe_submission_raw(
        &self,
        intent: &SlurmSubmissionIntent,
        collect_stop: bool,
    ) -> Result<String, DagError> {
        intent.validate()?;
        if intent.transport_binding != transport_binding(&self.settings)? {
            return Err(invalid(
                "Slurm recovery transport differs from recorded binding",
            ));
        }
        let scratch = resolve_remote_prefix(
            &self.settings.ssh,
            &self.settings.scratch_dir,
            &self.scratch_home,
        )
        .await?;
        if Path::new(&intent.remote_task_dir).parent() != Some(Path::new(&scratch)) {
            return Err(invalid(
                "Slurm recovery directory is outside the recorded scratch root",
            ));
        }
        let request =
            serde_json::to_string(intent).map_err(|_| invalid("cannot encode recovery request"))?;
        let command = format!(
            "python3 -I -S -B -c {} {}{}",
            sh_single_quote(include_str!("slurm_submission_observer.py")),
            sh_single_quote(&request),
            if collect_stop { " collect-stop" } else { "" }
        );
        let raw = ssh_stdout_text(
            self.settings.ssh.run(&command).await?,
            "Slurm recovery remains unknown",
        )?;
        if raw.len() > 1024 * 1024 {
            return Err(invalid("oversized Slurm recovery observation"));
        }
        Ok(raw)
    }

    /// Collect raw, identity-bound terminal evidence. The returned object is
    /// not a stop verdict: the trusted Store validates every scheduler fact.
    pub async fn collect_submission_stop_evidence(
        &self,
        intent: &SlurmSubmissionIntent,
    ) -> Result<serde_json::Value, DagError> {
        let raw = self.observe_submission_raw(intent, true).await?;
        let value: serde_json::Value =
            serde_json::from_str(&raw).map_err(|_| invalid("invalid stop evidence"))?;
        if value["schema"] != "SLURM_OWNED_STOP_PROOF_V1"
            || value["ownership"]["submission_identity"] != intent.submission_identity
            || value["ownership"]["manifest_sha256"] != intent.manifest_sha256
            || value["ownership"]["remote_task_dir"] != intent.remote_task_dir
        {
            return Err(invalid(
                "collected stop facts differ from the original operation",
            ));
        }
        Ok(value)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn dropped_observation_scope_marks_unknown_but_transferred_scope_does_not() {
        #[derive(Debug)]
        struct Record(std::sync::Arc<std::sync::atomic::AtomicUsize>);
        impl SlurmRecordedSubmission for Record {
            fn stopped(&self, _: &serde_json::Value) -> Result<(), DagError> {
                Ok(())
            }
            fn unresolved(&self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let unknown = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let record: std::sync::Arc<dyn SlurmRecordedSubmission> =
            std::sync::Arc::new(Record(unknown.clone()));
        let mut transferred = SubmissionScope::new(Some(record.clone()));
        transferred.disarm();
        drop(transferred);
        assert_eq!(unknown.load(Ordering::SeqCst), 0);
        // async cancellation drops this same RAII scope, without needing a
        // running executor to perform a best-effort asynchronous journal write.
        drop(SubmissionScope::new(Some(record)));
        assert_eq!(unknown.load(Ordering::SeqCst), 1);
    }

    fn intent() -> SlurmSubmissionIntent {
        let settings = SlurmSettings {
            apptainer: None,
            ssh: SshTarget::new("/usr/bin/false", "synthetic.invalid"),
            scratch_dir: "/synthetic/jobs".into(),
            local_artifact_root: "/unused".into(),
            partition: "test".into(),
            account: None,
            gpus: None,
            qos: None,
            mem_per_cpu: "1G".into(),
            default_time: "00:01:00".into(),
            poll_interval: Duration::from_secs(1),
        };
        let nonce = "0123456789abcdef";
        SlurmSubmissionIntent::new(
            &settings,
            "toy",
            &slurm_job_name("toy", nonce),
            nonce,
            "/synthetic/jobs/toy",
            r#"{"environment":"synthetic-private-test-value"}"#,
        )
        .unwrap()
    }

    #[test]
    fn durable_intent_reopens_exact_bindings_without_copying_environment() {
        let temp = tempfile::tempdir().unwrap();
        let v = intent();
        let path = v.persist(temp.path()).unwrap();
        let reopened = SlurmSubmissionIntent::read(&path).unwrap();
        assert_eq!(
            serde_json::to_value(&reopened).unwrap(),
            serde_json::to_value(&v).unwrap()
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(
            !std::fs::read_to_string(path)
                .unwrap()
                .contains("synthetic-private-test-value")
        );
        assert_eq!(v.manifest_bytes, 47);
        assert_eq!(
            v.manifest_sha256,
            "7b052573c4370615abe72a75f344111f076fdf83a5e660e8915c2e347effd676"
        );
        assert_eq!(
            v.submission_identity,
            "28ccdf5b722b45ee2e23764e0c4efdc0e230fcc30eedcc7df1f6db117358603e"
        );
    }

    #[test]
    fn concurrent_intent_publication_has_one_winner_and_preserves_foreign_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let v = intent();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let root = temp.path().to_owned();
        let results = std::thread::scope(|scope| {
            let a = scope.spawn(|| {
                barrier.wait();
                v.persist(&root)
            });
            let b = scope.spawn(|| {
                barrier.wait();
                v.persist(&root)
            });
            [a.join().unwrap(), b.join().unwrap()]
        });
        assert_eq!(results.iter().filter(|v| v.is_ok()).count(), 1);
        let path = results.into_iter().find_map(Result::ok).unwrap();
        let before = std::fs::read(&path).unwrap();
        assert!(v.persist(&root).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1
        );
    }

    #[test]
    fn legacy_ambiguous_world_readable_and_symlink_intents_are_not_recovered() {
        let temp = tempfile::tempdir().unwrap();
        let v = intent();
        let path = v.persist(temp.path()).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        for bad in [
            r#"{"task_id":"toy"}"#.to_string(),
            raw.replacen('{', "{\"schema\":\"duplicate\",", 1),
            format!("{raw}{{}}"),
        ] {
            std::fs::write(&path, bad).unwrap();
            assert!(SlurmSubmissionIntent::read(&path).is_err());
        }
        std::fs::write(&path, &raw).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(SlurmSubmissionIntent::read(&path).is_err());
        std::fs::remove_file(&path).unwrap();
        let outside = temp.path().join("outside");
        std::fs::write(&outside, b"keep").unwrap();
        std::os::unix::fs::symlink(&outside, &path).unwrap();
        assert!(v.persist(temp.path()).is_err());
        assert!(SlurmSubmissionIntent::read(&path).is_err());
        assert_eq!(std::fs::read(&outside).unwrap(), b"keep");
    }

    #[test]
    fn existing_task_directory_beneath_a_symlink_cannot_publish_an_intent() {
        let temp = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        std::fs::create_dir(other.path().join("toy")).unwrap();
        let link = temp.path().join("link");
        std::os::unix::fs::symlink(other.path(), &link).unwrap();
        assert!(intent().persist(&link).is_err());
        assert_eq!(
            std::fs::read_dir(other.path().join("toy")).unwrap().count(),
            0
        );
    }
}
