//! The `process` task backend: a transport that runs real OS processes in
//! isolated workspaces on a shared filesystem, stages uploaded artifacts
//! in, and returns an attempt receipt with glob-collected outputs.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::contract::{TaskAttemptReceipt, TaskDispatch, TaskResources, task_timeout};
use super::local::LocalTaskExecutor;
use super::resources::{LocalResourceBudget, LocalResourceLease};
use super::traits::{TaskLease, TaskTransport};
use crate::dag::DagError;
use crate::value::FileRef;

/// Nextflow-style process definition carried in [`TaskSpec::spec`].
///
/// The coordinator never depends on in-process node objects for this backend:
/// a process task is fully described by an executable script or argv,
/// environment bindings, and output file patterns relative to its workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessTaskSpec {
    /// Explicit SIF execution on a configured Slurm backend. Never ignored by
    /// the ordinary host-process transport.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<super::apptainer::ApptainerContainer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<ProcessTaskOutput>,
}

impl ProcessTaskSpec {
    fn parse(value: &serde_json::Value) -> Result<Self, DagError> {
        let process =
            serde_json::from_value::<ProcessTaskSpec>(value.clone()).map_err(|error| {
                DagError::Schedule(format!("invalid process task specification: {error}"))
            })?;
        process.validate()?;
        Ok(process)
    }

    pub fn validate(&self) -> Result<(), DagError> {
        if let Some(container) = &self.container {
            container.validate(self)?;
        }
        let uses_image_defaults = self
            .container
            .as_ref()
            .is_some_and(|container| container.mode == super::apptainer::ApptainerMode::Run);
        if self.script.is_none() && self.command.is_empty() && !uses_image_defaults {
            return Err(DagError::Schedule(
                "process task requires either script or command".into(),
            ));
        }
        if self.script.is_some() && !self.command.is_empty() {
            return Err(DagError::Schedule(
                "process task cannot define both script and command".into(),
            ));
        }
        if let Some(script) = &self.script
            && script.trim().is_empty()
        {
            return Err(DagError::Schedule("process script cannot be empty".into()));
        }
        if self.command.first().is_some_and(String::is_empty) {
            return Err(DagError::Schedule(
                "process command executable is empty".into(),
            ));
        }
        let mut ports = BTreeSet::new();
        for output in &self.outputs {
            if output.port != ports.len() as u8 {
                return Err(DagError::Schedule(format!(
                    "process outputs must use contiguous ports starting at zero; expected {}, got {}",
                    ports.len(),
                    output.port
                )));
            }
            if !ports.insert(output.port) {
                return Err(DagError::Schedule(format!(
                    "process task declares output port {} more than once",
                    output.port
                )));
            }
            if output.pattern.trim().is_empty() {
                return Err(DagError::Schedule(format!(
                    "process output on port {} has an empty pattern",
                    output.port
                )));
            }
            let pattern_path = Path::new(&output.pattern);
            if pattern_path.is_absolute()
                || pattern_path
                    .components()
                    .any(|component| matches!(component, std::path::Component::ParentDir))
            {
                return Err(DagError::Schedule(format!(
                    "process output pattern `{}` must stay inside the task workspace",
                    output.pattern
                )));
            }
            glob::Pattern::new(&output.pattern).map_err(|error| {
                DagError::Schedule(format!(
                    "process output pattern `{}` is invalid: {error}",
                    output.pattern
                ))
            })?;
        }
        Ok(())
    }
}

/// Files produced by a process task, grouped by the physical output port.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessTaskOutput {
    pub port: u8,
    pub pattern: String,
    /// A missing optional output produces no artifact for this port.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub optional: bool,
}

/// A task transport that runs real OS processes in isolated workspaces.
///
/// This is the first concrete backend behind [`TaskTransport`]. It models a
/// shared-filesystem remote worker: artifacts are copied in, the process runs
/// outside coordinator memory, and artifacts plus an attempt receipt are
/// copied back through the existing transfer protocol.
#[derive(Debug)]
pub struct ProcessTaskTransport {
    workspace_root: PathBuf,
    leases: AtomicU64,
    resource_budget: LocalResourceBudget,
    jobs: std::sync::Mutex<
        HashMap<String, tokio::task::JoinHandle<Result<TaskAttemptReceipt, DagError>>>,
    >,
}

struct PreparedProcessTask {
    dispatch: TaskDispatch,
    process: ProcessTaskSpec,
    workspace: PathBuf,
    inputs_by_name: BTreeMap<String, Vec<PathBuf>>,
}

impl ProcessTaskTransport {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            leases: AtomicU64::new(1),
            resource_budget: LocalTaskExecutor::discover_resource_budget(),
            jobs: std::sync::Mutex::new(HashMap::new()),
        }
    }

    pub fn with_workspace_root_and_resource_limits(
        workspace_root: impl Into<PathBuf>,
        cpu_limit: Option<u32>,
        memory_limit_bytes: Option<u64>,
    ) -> Result<Self, DagError> {
        TaskResources {
            cpus: cpu_limit,
            memory_bytes: memory_limit_bytes,
            max_duration_ms: None,
        }
        .validate()?;
        Ok(Self {
            workspace_root: workspace_root.into(),
            leases: AtomicU64::new(1),
            resource_budget: LocalResourceBudget::new(cpu_limit, memory_limit_bytes),
            jobs: std::sync::Mutex::new(HashMap::new()),
        })
    }

    fn prepare(
        &self,
        dispatch: TaskDispatch,
        artifacts: Vec<FileRef>,
        lease_id: &str,
    ) -> Result<PreparedProcessTask, DagError> {
        let process = parse_process_task(&dispatch)?;
        if process.container.is_some() {
            return Err(DagError::Schedule(
                "Apptainer process tasks require a configured Slurm transport; host execution refused".into(),
            ));
        }
        let workspace = self
            .workspace_root
            .join(sanitize_path_component(&dispatch.spec.id))
            .join(sanitize_path_component(lease_id));
        let input_root = workspace.join("inputs");
        std::fs::create_dir_all(&input_root).map_err(|error| {
            DagError::Schedule(format!(
                "cannot create process workspace `{}`: {error}",
                input_root.display()
            ))
        })?;

        let mut artifacts = artifacts.into_iter();
        let mut inputs_by_name = BTreeMap::new();
        for binding in &dispatch.spec.inputs {
            let mut staged = Vec::with_capacity(binding.staged_paths.len());
            for (index, _) in binding.staged_paths.iter().enumerate() {
                let artifact = artifacts.next().ok_or_else(|| {
                    DagError::Schedule(format!(
                        "process task `{}` is missing uploaded artifact for input `{}` index {index}",
                        dispatch.spec.id, binding.name
                    ))
                })?;
                let source = Path::new(&artifact.path);
                let file_name = source
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| format!("artifact-{index}"));
                let destination = input_root.join(format!(
                    "{}-{index}-{}",
                    sanitize_path_component(&binding.name),
                    file_name
                ));
                std::fs::copy(source, &destination).map_err(|error| {
                    DagError::Schedule(format!(
                        "cannot stage process input `{}` -> `{}`: {error}",
                        source.display(),
                        destination.display()
                    ))
                })?;
                staged.push(destination);
            }
            inputs_by_name.insert(binding.name.clone(), staged);
        }
        if artifacts.next().is_some() {
            return Err(DagError::Schedule(format!(
                "process task `{}` received more uploaded artifacts than declared staged inputs",
                dispatch.spec.id
            )));
        }

        let manifest_path = workspace.join("task.json");
        let manifest_bytes = serde_json::to_vec_pretty(&dispatch).map_err(|error| {
            DagError::Schedule(format!("cannot serialize process task manifest: {error}"))
        })?;
        std::fs::write(&manifest_path, manifest_bytes).map_err(|error| {
            DagError::Schedule(format!(
                "cannot write process task manifest `{}`: {error}",
                manifest_path.display()
            ))
        })?;

        Ok(PreparedProcessTask {
            dispatch,
            process,
            workspace,
            inputs_by_name,
        })
    }

    async fn execute(
        prepared: PreparedProcessTask,
        _resource_lease: LocalResourceLease,
    ) -> Result<TaskAttemptReceipt, DagError> {
        let start = std::time::Instant::now();
        let PreparedProcessTask {
            dispatch,
            process,
            workspace,
            inputs_by_name,
        } = prepared;

        let mut command = if process.script.is_some() {
            tokio::process::Command::new(if cfg!(windows) { "cmd" } else { "sh" })
        } else {
            tokio::process::Command::new(&process.command[0])
        };
        command.current_dir(&workspace).kill_on_drop(true);

        let process_environment = process_task_environment(
            &dispatch.spec.id,
            &workspace.to_string_lossy(),
            &process.env,
            &inputs_by_name,
        )?;
        for (name, value) in &process_environment {
            command.env(name, value);
        }

        let stdout_path = workspace.join("stdout.log");
        let stderr_path = workspace.join("stderr.log");
        let open_log = |path: &Path| {
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(path)
        };
        let stdout = open_log(&stdout_path).map_err(|error| {
            DagError::Schedule(format!(
                "cannot create process stdout `{}`: {error}",
                stdout_path.display()
            ))
        })?;
        let stderr = open_log(&stderr_path).map_err(|error| {
            DagError::Schedule(format!(
                "cannot create process stderr `{}`: {error}",
                stderr_path.display()
            ))
        })?;
        let stdout_capture = stdout
            .try_clone()
            .map_err(|error| DagError::Schedule(error.to_string()))?;
        let stderr_capture = stderr
            .try_clone()
            .map_err(|error| DagError::Schedule(error.to_string()))?;
        command.stdout(stdout).stderr(stderr);
        if let Some(script) = &process.script {
            if cfg!(windows) {
                command.arg("/C");
            } else {
                command.arg("-c");
            }
            let script = render_process_script(script, &process_environment, &inputs_by_name);
            command.arg(script);
        } else {
            let rendered = process
                .command
                .iter()
                .map(|argument| {
                    render_process_script(argument, &process_environment, &inputs_by_name)
                })
                .collect::<Vec<_>>();
            command.arg(&rendered[0]);
            command.args(&rendered[1..]);
        }

        let mut child = command.spawn().map_err(|error| {
            DagError::Schedule(format!(
                "cannot start process task `{}`: {error}",
                dispatch.spec.id
            ))
        })?;
        let timeout = task_timeout(&dispatch.resources);
        let outcome = tokio::select! {
            status = child.wait() => (status, false),
            _ = tokio::time::sleep(timeout.unwrap_or(Duration::MAX)), if timeout.is_some() => {
                let _ = child.kill().await;
                (child.wait().await, true)
            }
        };
        let (status, timed_out) = outcome;
        let status = status.map_err(|error| {
            DagError::Schedule(format!(
                "failed to wait for process task `{}`: {error}",
                dispatch.spec.id
            ))
        })?;
        let success = !timed_out && status.success();
        let status_name = if timed_out {
            "timeout"
        } else if success {
            "success"
        } else {
            "failed"
        };

        let mut artifacts_by_port = BTreeMap::<u8, Vec<FileRef>>::new();
        if success {
            for output in &process.outputs {
                let pattern = workspace.join(&output.pattern);
                let pattern = pattern.to_str().ok_or_else(|| {
                    DagError::Schedule("process workspace is not valid UTF-8".into())
                })?;
                let matches = glob::glob(pattern).map_err(|error| {
                    DagError::Schedule(format!(
                        "invalid process output pattern `{}`: {error}",
                        output.pattern
                    ))
                })?;
                let mut files = Vec::new();
                for match_path in matches {
                    let path = match_path.map_err(|error| {
                        DagError::Schedule(format!(
                            "cannot enumerate process output `{}`: {error}",
                            output.pattern
                        ))
                    })?;
                    if path.is_file() {
                        files.push(path);
                    }
                }
                files.sort();
                if files.is_empty() {
                    if output.optional {
                        continue;
                    }
                    return Err(DagError::Schedule(format!(
                        "process output port {} matched no files for `{}`",
                        output.port, output.pattern
                    )));
                }
                let mut artifacts = Vec::with_capacity(files.len());
                let publication_port = dispatch
                    .spec
                    .spec
                    .get("typed_dispatch")
                    .and_then(|envelope| envelope.get("output_directories"))
                    .and_then(serde_json::Value::as_object)
                    .is_some_and(|directories| {
                        directories
                            .values()
                            .any(|port| port.as_u64() == Some(u64::from(output.port)))
                    });
                for path in files {
                    let mut artifact = FileRef::local(&path, format_for_path(&path))?;
                    if publication_port {
                        let (size, hash) = crate::fingerprint::file_sha256(&artifact.path, None)
                            .await
                            .ok_or_else(|| {
                                DagError::Schedule(
                                    "cannot measure process publication output".into(),
                                )
                            })?;
                        if artifact.fingerprint
                            != FileRef::local(&path, artifact.format.clone())?.fingerprint
                            || artifact
                                .fingerprint
                                .as_ref()
                                .is_none_or(|fp| fp.size != size)
                        {
                            return Err(DagError::Schedule(
                                "process publication output changed during hashing".into(),
                            ));
                        }
                        artifact.fingerprint.as_mut().unwrap().content_hash = Some(hash);
                    }
                    artifacts.push(artifact);
                }
                artifacts_by_port.insert(output.port, artifacts);
            }
        }

        let output_artifacts = artifacts_by_port
            .values()
            .flat_map(|artifacts| artifacts.iter().cloned())
            .collect::<Vec<_>>();
        let manifest = FileRef::local(workspace.join("task.json"), Some("json".into()))?;
        let receipt = TaskAttemptReceipt {
            task_id: dispatch.spec.id,
            executor: "process".into(),
            status: status_name.into(),
            elapsed_ms: start.elapsed().as_millis().min(u64::MAX as u128) as u64,
            exit_code: if timed_out {
                124
            } else {
                status.code().unwrap_or(if success { 0 } else { 1 })
            },
            workspace: workspace.to_string_lossy().into_owned(),
            task_manifest: manifest,
            stdout_log: capture_process_log(
                stdout_capture,
                &workspace.join("transport-stdout.log"),
            ),
            stderr_log: capture_process_log(
                stderr_capture,
                &workspace.join("transport-stderr.log"),
            ),
            log_capture_error: None,
            output_artifacts,
            output_artifacts_by_port: artifacts_by_port,
        };
        let receipt_path = workspace.join("attempt.json");
        let receipt_bytes = serde_json::to_vec_pretty(&receipt).map_err(|error| {
            DagError::Schedule(format!("cannot serialize process attempt receipt: {error}"))
        })?;
        std::fs::write(&receipt_path, receipt_bytes).map_err(|error| {
            DagError::Schedule(format!(
                "cannot write process attempt receipt `{}`: {error}",
                receipt_path.display()
            ))
        })?;
        Ok(receipt)
    }
}

fn capture_process_log(mut source: std::fs::File, target: &Path) -> Option<FileRef> {
    use std::io::{Read, Seek, SeekFrom, Write};
    let mut capture = || -> std::io::Result<FileRef> {
        const LIMIT: u64 = 64 * 1024;
        let before = source.metadata()?;
        source.seek(SeekFrom::Start(before.len().saturating_sub(LIMIT)))?;
        let mut data = if before.len() > LIMIT {
            b"[truncated: final 65536 bytes follow]\n".to_vec()
        } else {
            vec![]
        };
        (&mut source).take(LIMIT).read_to_end(&mut data)?;
        let after = source.metadata()?;
        if before.len() != after.len() || before.modified()? != after.modified()? {
            return Err(std::io::Error::other("process log changed during capture"));
        }
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(target)?;
        output.write_all(&data)?;
        output.sync_all()?;
        let mut file =
            FileRef::local(target, Some("text".into())).map_err(std::io::Error::other)?;
        file.fingerprint
            .as_mut()
            .expect("local file has a fingerprint")
            .content_hash = Some(format!("{:x}", Sha256::digest(&data)));
        Ok(file)
    };
    match capture() {
        Ok(file) => Some(file),
        Err(error) => {
            tracing::warn!(%error, "cannot capture process log");
            None
        }
    }
}

#[async_trait::async_trait]
impl TaskTransport for ProcessTaskTransport {
    async fn submit(
        &self,
        dispatch: TaskDispatch,
        artifacts: Vec<FileRef>,
    ) -> Result<TaskLease, DagError> {
        dispatch.resources.validate()?;
        let resource_lease = self.resource_budget.acquire(&dispatch.resources).await?;
        let lease_sequence = self.leases.fetch_add(1, Ordering::Relaxed);
        let lease_id = format!("lease-{lease_sequence}");
        let prepared = self.prepare(dispatch, artifacts, &lease_id)?;
        let task_id = prepared.dispatch.spec.id.clone();
        let handle = tokio::spawn(Self::execute(prepared, resource_lease));
        self.jobs
            .lock()
            .map_err(|_| DagError::Schedule("process transport state poisoned".into()))?
            .insert(lease_id.clone(), handle);
        Ok(TaskLease { lease_id, task_id })
    }

    async fn wait(&self, lease: &TaskLease) -> Result<TaskAttemptReceipt, DagError> {
        let handle = self
            .jobs
            .lock()
            .map_err(|_| DagError::Schedule("process transport state poisoned".into()))?
            .remove(&lease.lease_id)
            .ok_or_else(|| {
                DagError::Schedule(format!(
                    "process lease `{}` is unknown, cancelled, or already consumed",
                    lease.lease_id
                ))
            })?;
        handle.await.map_err(|error| {
            DagError::Schedule(format!(
                "process task for lease `{}` failed: {error}",
                lease.lease_id
            ))
        })?
    }

    async fn cancel(&self, lease: &TaskLease) -> Result<(), DagError> {
        if let Some(handle) = self
            .jobs
            .lock()
            .map_err(|_| DagError::Schedule("process transport state poisoned".into()))?
            .remove(&lease.lease_id)
        {
            handle.abort();
        }
        Ok(())
    }
}

pub(super) fn sanitize_path_component(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric()
                || character == '.'
                || character == '_'
                || character == '-'
            {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() || sanitized == "." || sanitized == ".." {
        "task".into()
    } else {
        sanitized
    }
}

pub(super) fn sanitize_env_name(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' {
                character.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

pub(super) fn format_for_path(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .filter(|extension| !extension.is_empty())
}

pub(super) fn render_process_script(
    script: &str,
    environment: &BTreeMap<String, String>,
    inputs_by_name: &BTreeMap<String, Vec<PathBuf>>,
) -> String {
    let mut rendered = script.to_string();
    for (name, paths) in inputs_by_name {
        let joined = paths
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(":");
        rendered = rendered.replace(&format!("{{{{input.{name}}}}}"), &joined);
        rendered = rendered.replace(&format!("{{{{input.{name}[*]}}}}"), &joined);
        for (index, path) in paths.iter().enumerate() {
            rendered = rendered.replace(
                &format!("{{{{input.{name}[{index}]}}}}"),
                &path.to_string_lossy(),
            );
        }
    }
    for (name, value) in environment {
        rendered = rendered.replace(&format!("{{{{env.{name}}}}}"), value);
    }
    rendered
}

/// Parse and cross-check the `process` payload of a dispatched task: the
/// kind must be `process`, the spec must validate, and the declared output
/// ports must match the task contract. Shared by the local process
/// transport and the Slurm transport so both backends enforce the same
/// payload contract.
pub(super) fn parse_process_task(dispatch: &TaskDispatch) -> Result<ProcessTaskSpec, DagError> {
    if dispatch.spec.kind != "process" {
        return Err(DagError::Schedule(format!(
            "process transport cannot execute task kind `{}`",
            dispatch.spec.kind
        )));
    }
    let process = ProcessTaskSpec::parse(&dispatch.spec.spec)?;

    let expected_ports = dispatch
        .spec
        .outputs
        .iter()
        .map(|output| output.port)
        .collect::<BTreeSet<_>>();
    let actual_ports = process
        .outputs
        .iter()
        .map(|output| output.port)
        .collect::<BTreeSet<_>>();
    if expected_ports != actual_ports {
        return Err(DagError::Schedule(format!(
            "process outputs must match task output ports; expected {expected_ports:?}, got {actual_ports:?}"
        )));
    }
    Ok(process)
}

/// Build the environment a `process` task runs under: the spec's own
/// bindings plus the `AUTONOMICS_*` task-identity variables. Shared by the
/// local process transport and the Slurm transport so a remote job sees the
/// identical environment contract.
pub(super) fn process_task_environment(
    task_id: &str,
    workspace: &str,
    env: &BTreeMap<String, String>,
    inputs_by_name: &BTreeMap<String, Vec<PathBuf>>,
) -> Result<BTreeMap<String, String>, DagError> {
    let mut environment = env.clone();
    environment.insert("AUTONOMICS_TASK_ID".into(), task_id.to_string());
    environment.insert("AUTONOMICS_WORKSPACE".into(), workspace.to_string());
    environment.insert(
        "AUTONOMICS_INPUTS".into(),
        serde_json::to_string(inputs_by_name).map_err(|error| {
            DagError::Schedule(format!("cannot serialize process inputs: {error}"))
        })?,
    );
    environment.insert(
        "AUTONOMICS_INPUT_COUNT".into(),
        inputs_by_name
            .values()
            .map(|paths| paths.len())
            .sum::<usize>()
            .to_string(),
    );
    for (name, paths) in inputs_by_name {
        let joined = paths
            .iter()
            .map(|path| path.to_string_lossy())
            .collect::<Vec<_>>()
            .join(":");
        environment.insert(
            format!("AUTONOMICS_INPUT_{}", sanitize_env_name(name)),
            joined,
        );
        for (index, path) in paths.iter().enumerate() {
            environment.insert(
                format!("AUTONOMICS_INPUT_{}_{index}", sanitize_env_name(name)),
                path.to_string_lossy().into_owned(),
            );
        }
    }
    Ok(environment)
}
