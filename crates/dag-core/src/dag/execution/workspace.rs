//! The per-attempt on-disk task workspace: a unique directory under the
//! executor root holding the task manifest, staged inputs, published
//! outputs, captured logs, and the attempt receipt.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use arrow::ipc::writer::FileWriter as ArrowFileWriter;
use tracing::warn;

use super::contract::{TaskAttemptReceipt, TaskSubmission};
use crate::dag::DagError;
use crate::dag::graph::PortOutputs;
use crate::dag::node_event::{EventLevel, ReportedLog};
use crate::dag::runtime::NodeRunDetails;
use crate::value::{FileRef, NodeValue};

pub(super) struct LocalTaskWorkspace {
    root: PathBuf,
    manifest: FileRef,
}

impl LocalTaskWorkspace {
    pub(super) fn create(
        root: &std::path::Path,
        submission: &TaskSubmission,
    ) -> Result<Self, DagError> {
        std::fs::create_dir_all(root).map_err(|error| {
            DagError::Schedule(format!(
                "cannot create local task workspace root `{}`: {error}",
                root.display()
            ))
        })?;

        static WORKSPACE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let sequence = WORKSPACE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let workspace_root = root.join(format!("{}-{timestamp}-{sequence}", std::process::id()));
        match std::fs::create_dir(&workspace_root) {
            Ok(()) => {}
            Err(error) => {
                return Err(DagError::Schedule(format!(
                    "cannot create local task workspace `{}`: {error}",
                    workspace_root.display()
                )));
            }
        }

        let manifest = Self::write_manifest(&workspace_root, submission)?;
        Ok(Self {
            root: workspace_root,
            manifest,
        })
    }

    fn write_manifest(
        workspace_root: &std::path::Path,
        submission: &TaskSubmission,
    ) -> Result<FileRef, DagError> {
        let manifest_path = workspace_root.join("task.json");
        let manifest_json = serde_json::to_vec_pretty(&serde_json::json!({
            "spec": submission.spec,
            "resources": submission.resources,
        }))
        .map_err(|error| {
            DagError::Schedule(format!("cannot serialize local task manifest: {error}"))
        })?;
        std::fs::write(&manifest_path, manifest_json).map_err(|error| {
            DagError::Schedule(format!(
                "cannot write local task manifest `{}`: {error}",
                manifest_path.display()
            ))
        })?;
        FileRef::local(&manifest_path, Some("json".into()))
    }

    async fn write_dataframe(
        path: &std::path::Path,
        dataframe: &datafusion::prelude::DataFrame,
    ) -> Result<FileRef, DagError> {
        let schema = dataframe.schema();
        let batches = dataframe.clone().collect().await.map_err(|error| {
            DagError::Schedule(format!("cannot materialize DataFrame output: {error}"))
        })?;
        let file = std::fs::File::create(path).map_err(|error| {
            DagError::Schedule(format!(
                "cannot create Arrow IPC artifact `{}`: {error}",
                path.display()
            ))
        })?;
        let mut writer = ArrowFileWriter::try_new(file, schema.as_ref()).map_err(|error| {
            DagError::Schedule(format!(
                "cannot initialize Arrow IPC artifact `{}`: {error}",
                path.display()
            ))
        })?;
        for batch in &batches {
            writer.write(batch).map_err(|error| {
                DagError::Schedule(format!(
                    "cannot write Arrow IPC artifact `{}`: {error}",
                    path.display()
                ))
            })?;
        }
        writer.finish().map_err(|error| {
            DagError::Schedule(format!(
                "cannot finalize Arrow IPC artifact `{}`: {error}",
                path.display()
            ))
        })?;
        FileRef::local(path, Some("arrow".into()))
    }

    async fn read_source(
        path: &str,
        storage: Option<&vfs::OpendalFileStorage>,
    ) -> Result<Vec<u8>, DagError> {
        if let Some(virtual_path) = path.strip_prefix("vfs://") {
            let storage = storage.ok_or_else(|| {
                DagError::Schedule(format!(
                    "cannot stage VFS input `{path}` because no object store is configured"
                ))
            })?;
            let length = storage
                .content_length(virtual_path)
                .await
                .map_err(|error| {
                    DagError::Schedule(format!("cannot stat VFS input `{path}`: {error}"))
                })?;
            let bytes = storage
                .read_range(virtual_path, 0..length)
                .await
                .map_err(|error| {
                    DagError::Schedule(format!("cannot read VFS input `{path}`: {error}"))
                })?;
            return Ok(bytes.to_vec());
        }

        let local_path = path.strip_prefix("file://").unwrap_or(path);
        std::fs::read(local_path).map_err(|error| {
            DagError::Schedule(format!("cannot stage local input `{local_path}`: {error}"))
        })
    }

    async fn stage_file(
        input_root: &std::path::Path,
        sequence: &mut u64,
        source: &FileRef,
        storage: Option<&vfs::OpendalFileStorage>,
    ) -> Result<FileRef, DagError> {
        let bytes = Self::read_source(&source.path, storage).await?;
        let staged_path = input_root.join(format!("input-{}.bin", *sequence));
        *sequence += 1;
        std::fs::write(&staged_path, bytes).map_err(|error| {
            DagError::Schedule(format!(
                "cannot write staged input `{}`: {error}",
                staged_path.display()
            ))
        })?;
        let mut staged = FileRef::local(&staged_path, source.format.clone())?;
        if source.fingerprint.is_some() {
            staged.fingerprint = source.fingerprint.clone();
        }
        Ok(staged)
    }

    fn write_json_artifact(
        path: &std::path::Path,
        value: &serde_json::Value,
    ) -> Result<FileRef, DagError> {
        let bytes = serde_json::to_vec_pretty(value).map_err(|error| {
            DagError::Schedule(format!("cannot serialize JSON artifact: {error}"))
        })?;
        std::fs::write(path, bytes).map_err(|error| {
            DagError::Schedule(format!(
                "cannot write JSON artifact `{}`: {error}",
                path.display()
            ))
        })?;
        FileRef::local(path, Some("json".into()))
    }

    pub(super) async fn stage_inputs(
        &mut self,
        submission: &mut TaskSubmission,
        storage: Option<&vfs::OpendalFileStorage>,
    ) -> Result<(), DagError> {
        let input_root = self.root.join("inputs");
        std::fs::create_dir_all(&input_root).map_err(|error| {
            DagError::Schedule(format!(
                "cannot create input staging directory `{}`: {error}",
                input_root.display()
            ))
        })?;
        let mut sequence = 0u64;
        let mut staged_by_port = std::collections::BTreeMap::<u8, Vec<String>>::new();

        for input in &mut submission.inputs {
            if let NodeValue::DataFrame(dataframe) = &input.data {
                let path = input_root.join(format!("input-{sequence}.arrow"));
                sequence += 1;
                let artifact = Self::write_dataframe(&path, dataframe).await?;
                staged_by_port
                    .entry(input.port)
                    .or_default()
                    .push(artifact.path.clone());
                continue;
            }
            if let NodeValue::Channel(channel) = &input.data {
                let path = input_root.join(format!("input-{sequence}.json"));
                sequence += 1;
                let artifact = Self::write_json_artifact(&path, &serde_json::json!(channel.items))?;
                staged_by_port
                    .entry(input.port)
                    .or_default()
                    .push(artifact.path.clone());
                continue;
            }
            let (sources, is_file_set) = match &input.data {
                NodeValue::File(file) => (vec![file.clone()], false),
                NodeValue::FileSet(files) => (files.clone(), true),
                NodeValue::DataFrame(_) | NodeValue::Channel(_) => continue,
            };
            let mut staged_files = Vec::with_capacity(sources.len());
            for source in &sources {
                let staged = Self::stage_file(&input_root, &mut sequence, source, storage).await?;
                staged_by_port
                    .entry(input.port)
                    .or_default()
                    .push(staged.path.clone());
                staged_files.push(staged);
            }
            input.data = if is_file_set {
                NodeValue::FileSet(staged_files)
            } else {
                NodeValue::File(
                    staged_files
                        .into_iter()
                        .next()
                        .ok_or_else(|| DagError::Schedule("file input had no source".into()))?,
                )
            };
        }
        for binding in &mut submission.spec.inputs {
            if let Some(paths) = staged_by_port.get(&binding.port) {
                binding.staged_paths = paths.clone();
            }
        }
        self.manifest = Self::write_manifest(&self.root, submission)?;
        Ok(())
    }

    pub(super) async fn publish_outputs(
        &mut self,
        submission: &mut TaskSubmission,
        outputs: &PortOutputs,
    ) -> Result<BTreeMap<u8, Vec<FileRef>>, DagError> {
        let output_root = self.root.join("outputs");
        std::fs::create_dir_all(&output_root).map_err(|error| {
            DagError::Schedule(format!(
                "cannot create output artifact directory `{}`: {error}",
                output_root.display()
            ))
        })?;
        let mut artifacts_by_port = BTreeMap::<u8, Vec<FileRef>>::new();
        let mut ports = outputs.iter().map(|(port, _)| *port).collect::<Vec<_>>();
        ports.sort_unstable();
        ports.dedup();

        for port in ports {
            let value = outputs
                .get(&port)
                .ok_or_else(|| DagError::Schedule(format!("output port {port} disappeared")))?;
            let mut port_artifacts = Vec::new();
            match value {
                NodeValue::DataFrame(dataframe) => {
                    let path = output_root.join(format!("port-{port}.arrow"));
                    let artifact = Self::write_dataframe(&path, dataframe).await?;
                    port_artifacts.push(artifact);
                }
                NodeValue::File(file) => port_artifacts.push(file.clone()),
                NodeValue::FileSet(files) => port_artifacts.extend(files.iter().cloned()),
                NodeValue::Channel(channel) => {
                    let path = output_root.join(format!("port-{port}.json"));
                    let artifact =
                        Self::write_json_artifact(&path, &serde_json::json!(channel.items))?;
                    port_artifacts.push(artifact);
                }
            }
            artifacts_by_port.insert(port, port_artifacts);
        }
        for output in &mut submission.spec.outputs {
            if let Some(paths) = artifacts_by_port.get(&output.port) {
                output.artifact_paths =
                    paths.iter().map(|artifact| artifact.path.clone()).collect();
            }
        }
        self.manifest = Self::write_manifest(&self.root, submission)?;
        Ok(artifacts_by_port)
    }

    fn capture_logs(&self, logs: Vec<ReportedLog>) -> (Option<FileRef>, Option<FileRef>) {
        let write = |name: &str, entries: Vec<&ReportedLog>| -> Option<FileRef> {
            if entries.is_empty() {
                return None;
            }
            let path = self.root.join(name);
            let rendered = entries
                .iter()
                .map(|entry| {
                    format!(
                        "{} {}",
                        serde_json::to_string(&entry.level).unwrap_or_default(),
                        entry.message
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            std::fs::write(path, format!("{rendered}\n")).ok()?;
            FileRef::local(self.root.join(name), Some("text".into())).ok()
        };
        let stdout = write(
            "stdout.log",
            logs.iter()
                .filter(|entry| !matches!(entry.level, EventLevel::Warn | EventLevel::Error))
                .collect(),
        );
        let stderr = write(
            "stderr.log",
            logs.iter()
                .filter(|entry| matches!(entry.level, EventLevel::Warn | EventLevel::Error))
                .collect(),
        );
        (stdout, stderr)
    }

    pub(super) fn complete(
        &self,
        task_id: &str,
        executor: &str,
        details: &mut Option<crate::dag::runtime::NodeRunDetails>,
        logs: Vec<ReportedLog>,
        exit_code: i32,
        status: &str,
        duration: Duration,
    ) {
        let (stdout_log, stderr_log) = self.capture_logs(logs);
        let details = details.get_or_insert_with(Default::default);
        if details.stdout_log.is_none() {
            details.stdout_log = stdout_log;
        }
        if details.stderr_log.is_none() {
            details.stderr_log = stderr_log;
        }
        details.workspace = Some(self.root.to_string_lossy().into_owned());
        details.task_manifest = Some(self.manifest.clone());
        if details.exit_code.is_none() {
            details.exit_code = Some(exit_code);
        }
        self.finish(task_id, executor, details, status, duration);
    }

    fn finish(
        &self,
        task_id: &str,
        executor: &str,
        details: &crate::dag::runtime::NodeRunDetails,
        status: &str,
        duration: Duration,
    ) {
        let status_json = serde_json::json!({
            "status": status,
            "elapsed_ms": duration.as_millis().min(u64::MAX as u128) as u64,
        });
        let write_result = std::fs::write(
            self.root.join("status.json"),
            serde_json::to_vec_pretty(&status_json).unwrap_or_default(),
        );
        if let Err(error) = write_result {
            warn!(
                workspace = %self.root.display(),
                error = %error,
                "cannot write local task status"
            );
        }

        let receipt = TaskAttemptReceipt {
            task_id: task_id.to_string(),
            executor: executor.to_string(),
            status: status.to_string(),
            elapsed_ms: duration.as_millis().min(u64::MAX as u128) as u64,
            exit_code: details
                .exit_code
                .unwrap_or(if status == "success" { 0 } else { 1 }),
            workspace: self.root.to_string_lossy().into_owned(),
            task_manifest: self.manifest.clone(),
            output_artifacts: details.output_artifacts.clone(),
            output_artifacts_by_port: details.output_artifacts_by_port.clone(),
        };
        let receipt_path = self.root.join("attempt.json");
        let write_result = serde_json::to_vec_pretty(&receipt)
            .map_err(|error| {
                DagError::Schedule(format!("cannot serialize task attempt receipt: {error}"))
            })
            .and_then(|bytes| {
                std::fs::write(&receipt_path, bytes).map_err(|error| {
                    DagError::Schedule(format!(
                        "cannot write task attempt receipt `{}`: {error}",
                        receipt_path.display()
                    ))
                })
            });
        if let Err(error) = write_result {
            warn!(
                workspace = %self.root.display(),
                error = %error,
                "cannot write local task attempt receipt"
            );
        }
    }
}
