//! Remote compute enrollment and its progress events.

use super::{
    ComputeBackend, ComputeRegistry, ComputeRegistryState, DataBackend, DataDomain, DataSource,
    SaveTarget, SelectTarget,
    cluster_connection::ClusterConnection,
    identifier, is_domain_absolute, private_dir, source_address_domain,
    worker::{bundled_worker, worker_profile},
};
use crate::config::SlurmConfig;
use dag_core::dag::execution::typed_worker::TypedPodmanSettings;
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

/// Fixed enrollment/repair step identifier. Serialized in snake_case so the
/// wire contract shared with CLI and graphical clients stays unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnrollStage {
    Connection,
    Authenticate,
    Validate,
    RemoteHome,
    PrepareDataDirectory,
    PrepareDirectory,
    CheckPython,
    Chmod,
    ProbeWorker,
    Upload,
    CommitUpload,
    WorkerProfile,
    WorkerPackage,
    WorkerUpdate,
    DataSource,
    SaveTarget,
    SelectTarget,
    Ready,
}

impl EnrollStage {
    /// Human-facing step label reported through progress events.
    pub fn label(self) -> &'static str {
        match self {
            Self::Connection => "Connection",
            Self::Authenticate => "Connecting cluster and verifing credential",
            Self::Validate => "Validating enrollment parameters",
            Self::RemoteHome => "Resolving remote home directory",
            Self::PrepareDataDirectory => "Preparing remote default data directory",
            Self::PrepareDirectory => "Preparing remote worker directory",
            Self::CheckPython => "Checking remote Python",
            Self::Chmod => "Setting worker execution permissions",
            Self::ProbeWorker => "Probing worker executable and SHA256",
            Self::Upload => "Uploading worker binary",
            Self::CommitUpload => "Committing uploaded worker binary",
            Self::WorkerProfile => "Generating worker profile",
            Self::WorkerPackage => "Checking system bundled matching worker",
            Self::WorkerUpdate => "Restoring remote worker",
            Self::DataSource => "Registering compute target data source",
            Self::SaveTarget => "Saving compute target",
            Self::SelectTarget => "Selecting compute target",
            Self::Ready => "Ready",
        }
    }

    /// Wire name of this stage; must stay identical to the serde rename.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Connection => "connection",
            Self::Authenticate => "authenticate",
            Self::Validate => "validate",
            Self::RemoteHome => "remote_home",
            Self::PrepareDataDirectory => "prepare_data_directory",
            Self::PrepareDirectory => "prepare_directory",
            Self::CheckPython => "check_python",
            Self::Chmod => "chmod",
            Self::ProbeWorker => "probe_worker",
            Self::Upload => "upload",
            Self::CommitUpload => "commit_upload",
            Self::WorkerProfile => "worker_profile",
            Self::WorkerPackage => "worker_package",
            Self::WorkerUpdate => "worker_update",
            Self::DataSource => "data_source",
            Self::SaveTarget => "save_target",
            Self::SelectTarget => "select_target",
            Self::Ready => "ready",
        }
    }
}

impl std::fmt::Display for EnrollStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Structured progress shared by CLI and graphical clients.
#[derive(Clone, Serialize)]
pub struct EnrollProgressEvent {
    pub stage: EnrollStage,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uploaded_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_bytes: Option<u64>,
}

/// Per-request progress sink, independent of daemon tracing subscribers.
#[derive(Clone, Default)]
pub struct EnrollProgress(Option<Arc<dyn Fn(&EnrollProgressEvent) + Send + Sync>>);

impl EnrollProgress {
    pub fn new(callback: impl Fn(EnrollStage, &str) + Send + Sync + 'static) -> Self {
        Self::with_events(move |event| callback(event.stage, &event.message))
    }

    pub fn with_events(callback: impl Fn(&EnrollProgressEvent) + Send + Sync + 'static) -> Self {
        Self(Some(Arc::new(callback)))
    }

    pub(super) fn report(&self, stage: EnrollStage, message: &str) {
        self.emit(stage, message, None, None);
    }

    pub(super) fn upload(&self, message: &str, uploaded: u64, total: u64) {
        self.emit(EnrollStage::Upload, message, Some(uploaded), Some(total));
    }

    fn emit(
        &self,
        stage: EnrollStage,
        message: &str,
        uploaded_bytes: Option<u64>,
        total_bytes: Option<u64>,
    ) {
        if let Some(callback) = &self.0 {
            callback(&EnrollProgressEvent {
                stage,
                message: message.into(),
                uploaded_bytes,
                total_bytes,
            });
        }
    }
}

/// Remote execution mode. Omitted by legacy clients means Slurm.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnrollBackend {
    Ssh,
    #[default]
    Slurm,
}

impl EnrollBackend {
    fn target(self, settings: SlurmConfig) -> ComputeBackend {
        match self {
            Self::Ssh => ComputeBackend::Ssh { settings },
            Self::Slurm => ComputeBackend::Slurm { settings },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum EnrollContainerRuntime {
    Podman {
        plugins_root: PathBuf,
        plugins_sha256: String,
        podman: TypedPodmanSettings,
    },
    Sif {
        image_manifest: PathBuf,
        #[serde(default)]
        runtime: dag_core::SifRuntime,
        singularity_executable: Option<PathBuf>,
    },
}

/// Minimal operator input for one-shot enrollment.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnrollRequest {
    #[serde(default)]
    pub backend: EnrollBackend,
    pub container_runtime: Option<EnrollContainerRuntime>,
    pub id: String,
    /// Display name; defaults to the id.
    #[serde(default)]
    pub name: Option<String>,
    /// Login-node ssh alias (as configured in ~/.ssh/config).
    pub login_alias: String,
    /// Data-home ssh alias. Supplied with `data_root`, it enables
    /// zero-copy in-place references through a derived DataDomain and a
    /// read-only mount at `/data/<id>`.
    #[serde(default)]
    pub data_alias: Option<String>,
    /// Absolute root of the shared filesystem.
    /// The compatibility field for a legacy shared data root.
    /// An omitted data_alias uses login_alias.
    #[serde(default)]
    pub data_root: Option<String>,
    /// Root of the unified remote directory layout.
    #[serde(default)]
    pub storage_root: Option<String>,
    /// Absolute path of the worker binary on the cluster. Existence,
    /// executability and SHA256 are probed over ssh — no hand-authored
    /// worker profile is involved.
    /// Omit to install the bundled worker under the remote home directory.
    #[serde(default)]
    pub worker_executable: String,
    /// Local standalone `autonomics-worker` binary to deploy to `worker_executable` before probing:
    /// streamed over the login alias through the framed staged-write
    /// channel (atomic rename, no torn destination), then `chmod +x`.
    /// With an explicit remote path, omission means verify the existing worker.
    /// With no remote path, omission selects the bundled worker.
    #[serde(default)]
    pub deploy_worker: Option<String>,
    #[serde(default)]
    pub partition: Option<String>,
    #[serde(default)]
    pub account: Option<String>,
    /// Overrides the ssh program (defaults to the site configuration).
    #[serde(default)]
    pub ssh_command: Option<String>,
    /// Remote python interpreter for the file-channel helpers (defaults
    /// to `python3`). Clusters whose system python is older than 3.7 can
    /// point at any standalone build, e.g. one managed by uv:
    /// `uv python install 3.12 && uv python find 3.12`.
    #[serde(default)]
    pub python: Option<String>,
    /// Absolute scratch directory on the cluster; defaults to
    /// `<data_root>/scratch`.
    #[serde(default, alias = "scratch_dir")]
    pub data_dir: Option<String>,
    /// Switch the global compute policy to this target afterwards.
    #[serde(default)]
    pub select: bool,
}

impl EnrollRequest {
    fn validate(&self) -> Result<Option<(&str, &str)>, String> {
        identifier(&self.id)?;
        if let Some(root) = &self.storage_root {
            if !is_domain_absolute(root) {
                return Err("storage_root must be a clean absolute path".into());
            }
            if self.data_root.is_some() || self.data_dir.is_some() {
                return Err("storage_root cannot be combined with data_root or scratch_dir".into());
            }
        }
        if !valid_alias(&self.login_alias) {
            return Err("login_alias must be a configured ssh host alias".into());
        }
        if self.backend == EnrollBackend::Ssh
            && (self.partition.is_some() || self.account.is_some())
        {
            return Err("partition and account require a Slurm target".into());
        }
        if self
            .data_dir
            .as_ref()
            .is_some_and(|path| !is_domain_absolute(path))
        {
            return Err("scratch_dir must be a clean absolute path".into());
        }
        let data = match (self.data_alias.as_deref(), self.data_root.as_deref()) {
            (alias, Some(root)) => {
                let alias = alias.unwrap_or(&self.login_alias);
                if !valid_alias(alias) || !is_domain_absolute(root) {
                    return Err(
                        "data_alias must be a valid ssh alias and data_root a clean absolute path"
                            .into(),
                    );
                }
                Some((alias, root))
            }
            (None, None) => None,
            (Some(alias), None) if self.storage_root.is_some() && valid_alias(alias) => None,
            (Some(_), None) => {
                return Err(
                    "data_alias and data_root must be supplied together (or both omitted)".into(),
                );
            }
        };
        if !self.worker_executable.is_empty() && !is_domain_absolute(&self.worker_executable) {
            return Err("worker_executable must be an absolute cluster path".into());
        }
        // The local deploy path crosses a process boundary: the daemon —
        // not the calling shell — resolves it, so a relative path silently
        // points somewhere else. Reject it with the reason instead.
        if let Some(local) = &self.deploy_worker
            && !local.starts_with('/')
        {
            return Err(
                "deploy_worker must be an absolute local path; the daemon (not your shell) \
                 resolves it"
                    .into(),
            );
        }
        Ok(data)
    }
}

fn valid_alias(alias: &str) -> bool {
    !alias.is_empty()
        && !alias.starts_with('-')
        && alias
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.@:-".contains(&b))
}

impl ComputeRegistry {
    /// Enroll a remote target, register its data source, and optionally select it.
    /// A failure after data-source registration can leave the mount registered.
    #[tracing::instrument(skip_all, fields(cluster_id = %request.id, alias = %request.login_alias))]
    pub async fn enroll(&self, request: EnrollRequest) -> Result<ComputeRegistryState, String> {
        self.enroll_with_progress(request, EnrollProgress::default())
            .await
    }

    #[tracing::instrument(skip_all, fields(cluster_id = %request.id, alias = %request.login_alias))]
    pub async fn enroll_with_progress(
        &self,
        mut request: EnrollRequest,
        progress: EnrollProgress,
    ) -> Result<ComputeRegistryState, String> {
        progress.report(EnrollStage::Validate, "正在检查注册参数");
        tracing::info!(stage = ?EnrollStage::Validate, "cluster enrollment started");
        let explicit_data = request.validate()?;

        let runtime = request
            .container_runtime
            .as_ref()
            .ok_or("compute enrollment requires a container_runtime configuration")?;
        match (request.backend, runtime) {
            (
                EnrollBackend::Ssh,
                EnrollContainerRuntime::Podman {
                    plugins_root,
                    plugins_sha256,
                    podman,
                },
            ) => {
                crate::host::hpc_placement::validate_ssh_podman(
                    plugins_root,
                    plugins_sha256,
                    podman,
                )
                .map_err(|e| e.to_string())?;
            }
            (EnrollBackend::Slurm, EnrollContainerRuntime::Sif { image_manifest, .. }) => {
                if !image_manifest.is_absolute() {
                    return Err("SIF image manifest must be an absolute gateway path".into());
                }
                let bytes = super::read_owned(image_manifest, false)?;
                let manifest: dag_core::dag::execution::ApptainerSettings =
                    serde_json::from_slice(&bytes)
                        .map_err(|e| format!("invalid SIF image manifest: {e}"))?;
                manifest.validate().map_err(|e| e.to_string())?;
            }
            _ => return Err("SSH requires Podman and Slurm requires a SIF runtime".into()),
        }

        let ssh_command = request
            .ssh_command
            .clone()
            .unwrap_or_else(|| self.config.slurm.ssh_command.clone());
        let connection = ClusterConnection::new(
            &request.id,
            &request.login_alias,
            &ssh_command,
            request.python.as_deref().unwrap_or("python3"),
        )
        .with_progress(progress.clone());
        // Resolve and validate the local package before any remote mutation.
        let deploy_worker =
            if request.worker_executable.is_empty() && request.deploy_worker.is_none() {
                Some(bundled_worker().await?.to_string_lossy().into_owned())
            } else {
                request.deploy_worker.clone()
            };
        connection.authenticate().await?;
        let unified = request.storage_root.is_some()
            || (explicit_data.is_none() && request.data_dir.is_none());
        let use_default_data_root = explicit_data.is_none();
        let home = if (use_default_data_root && request.storage_root.is_none())
            || request.worker_executable.is_empty()
        {
            Some(connection.home_directory().await?)
        } else {
            None
        };
        let storage_root = if unified {
            Some(match request.storage_root.clone() {
                Some(root) => root,
                None => format!(
                    "{}/.autonomics/compute/{}",
                    home.as_deref().ok_or("remote home was not resolved")?,
                    request.id
                ),
            })
        } else {
            None
        };
        let (data_alias, data_root) = match explicit_data {
            Some((alias, root)) => (alias.to_owned(), root.to_owned()),
            None if unified => (
                request
                    .data_alias
                    .clone()
                    .unwrap_or_else(|| request.login_alias.clone()),
                format!("{}/data", storage_root.as_deref().unwrap()),
            ),
            None => {
                let home = home.as_deref().ok_or("remote home was not resolved")?;
                let root = format!("{home}/autonomics-data/{}", request.id);
                (request.login_alias.clone(), root)
            }
        };
        let scratch_dir = request.data_dir.unwrap_or_else(|| match &storage_root {
            Some(root) => format!("{root}/dag-tasks"),
            None => format!("{data_root}/scratch"),
        });
        if !unified
            && (scratch_dir == data_root || !Path::new(&scratch_dir).starts_with(&data_root))
        {
            return Err("scratch_dir must be a subdirectory of the mounted data root".into());
        }
        if request.worker_executable.is_empty() {
            let home = home.as_deref().ok_or("remote home was not resolved")?;
            request.worker_executable = format!(
                "{home}/.local/share/autonomics/workers/{}/autonomics-worker",
                uuid::Uuid::new_v4()
            );
        }
        if let Some(local) = &deploy_worker {
            connection
                .deploy_worker(local, &request.worker_executable)
                .await?;
        }
        let sha256 = connection.probe_worker(&request.worker_executable).await?;
        connection
            .verify_worker_identity(&request.worker_executable, &sha256)
            .await?;

        progress.report(EnrollStage::WorkerProfile, "正在生成 worker 配置");
        tracing::info!(stage = ?EnrollStage::WorkerProfile, "cluster enrollment step started");
        let mut profile = worker_profile(&request.worker_executable, &sha256);

        let mut settings = SlurmConfig {
            enabled: true,
            ssh_host: request.login_alias,
            ssh_command,
            typed_worker_config: None,
            typed_worker: None,
            data_ssh_host: Some(data_alias.clone()),
            pi_ssh_host: None,
            apptainer_manifest: None,
            account: request.account,
            scratch_remote_dir: scratch_dir,
            storage_root_v1: storage_root.clone(),
            ..SlurmConfig::default()
        };
        match runtime {
            EnrollContainerRuntime::Podman {
                plugins_root,
                plugins_sha256,
                podman,
            } => {
                profile["worker"]["plugins_root"] = serde_json::json!(plugins_root);
                profile["worker"]["plugins_sha256"] = serde_json::json!(plugins_sha256);
                profile["worker"]["podman"] =
                    serde_json::to_value(podman).map_err(|e| e.to_string())?;
            }
            EnrollContainerRuntime::Sif {
                image_manifest,
                runtime,
                singularity_executable,
            } => {
                settings.apptainer_manifest = Some(image_manifest.clone());
                settings.sif_runtime = *runtime;
                settings.singularity_executable = singularity_executable.clone();
            }
        }
        settings.typed_worker = Some(profile);
        if let Some(python) = request.python {
            settings.remote_python = python;
        }
        if let Some(partition) = request.partition {
            settings.partition = partition;
        }

        let retained_id = if let Some(root) = &storage_root {
            for subdir in [
                "data",
                "dag-tasks",
                "work",
                "images/sif",
                "runtime/apptainer",
                "panels",
            ] {
                connection
                    .prepare_data_directory(&format!("{root}/{subdir}"))
                    .await?;
            }
            settings.panel_cache_remote_dir = format!("{root}/panels");
            Some(format!("dag-tasks-{}", request.id))
        } else {
            if use_default_data_root {
                connection.prepare_data_directory(&data_root).await?;
            }
            None
        };
        // The domain's read-only mount must exist before target validation.
        progress.report(EnrollStage::DataSource, "正在注册计算目标数据源");
        let domain = DataDomain {
            id: source_address_domain(&request.id),
            ssh_aliases: vec![data_alias.clone()],
            remote_root: data_root,
            vfs_prefix: format!("/data/{}", request.id),
            retained_vfs_prefix: retained_id.as_ref().map(|id| format!("/data/{id}")),
            retained_remote_root: retained_id
                .as_ref()
                .map(|_| settings.scratch_remote_dir.clone()),
        };
        let source = DataSource {
            id: request.id.clone(),
            name: format!("{} shared filesystem", request.id),
            backend: DataBackend::Ssh {
                alias: data_alias,
                root: domain.remote_root.clone(),
                python: settings.remote_python.clone(),
            },
            read_only: !unified,
            domain: Some(domain.id.clone()),
            endpoint: None,
            permissions: Default::default(),
        };
        let source = self.prepare_source(source).await?;
        let retained = if let Some(id) = retained_id {
            Some(
                self.prepare_source(DataSource {
                    name: format!("{} task artifacts", request.id),
                    backend: DataBackend::Ssh {
                        alias: settings.data_ssh_host.clone().unwrap(),
                        root: settings.scratch_remote_dir.clone(),
                        python: settings.remote_python.clone(),
                    },
                    read_only: true,
                    domain: Some(id.clone()),
                    endpoint: None,
                    permissions: Default::default(),
                    id,
                })
                .await?,
            )
        } else {
            None
        };

        progress.report(EnrollStage::SaveTarget, "正在保存计算目标");
        tracing::info!(stage = ?EnrollStage::SaveTarget, "cluster enrollment step started");
        let mut state = self
            .save_target_with_source(
                SaveTarget {
                    id: request.id.clone(),
                    name: request.name.unwrap_or_else(|| request.id.clone()),
                    enabled: true,
                    backend: request.backend.target(settings),
                    data_domains: vec![domain],
                },
                Some((source, retained)),
                None,
            )
            .map_err(|error| format!("target save failed: {error}"))?;

        if request.select {
            progress.report(EnrollStage::SelectTarget, "正在选择计算目标");
            tracing::info!(stage = ?EnrollStage::SelectTarget, "cluster enrollment step started");
            state = self
                .select(SelectTarget {
                    target_id: request.id.clone(),
                })
                .map_err(|error| format!("policy switch failed: {error}"))?;
        }
        tracing::info!("cluster enrollment finished");
        Ok(state)
    }
}

#[cfg(test)]
mod tests {
    use super::EnrollStage;

    /// The manual `as_str` table must stay identical to the serde wire names.
    #[test]
    fn stage_as_str_matches_serde_rename() {
        let stages = [
            EnrollStage::Connection,
            EnrollStage::Authenticate,
            EnrollStage::Validate,
            EnrollStage::RemoteHome,
            EnrollStage::PrepareDataDirectory,
            EnrollStage::PrepareDirectory,
            EnrollStage::CheckPython,
            EnrollStage::Chmod,
            EnrollStage::ProbeWorker,
            EnrollStage::Upload,
            EnrollStage::CommitUpload,
            EnrollStage::WorkerProfile,
            EnrollStage::WorkerPackage,
            EnrollStage::WorkerUpdate,
            EnrollStage::DataSource,
            EnrollStage::SaveTarget,
            EnrollStage::SelectTarget,
            EnrollStage::Ready,
        ];
        for stage in stages {
            assert_eq!(
                serde_json::to_value(stage).unwrap(),
                serde_json::json!(stage.as_str()),
                "wire name drifted for {stage}"
            );
        }
    }
}
