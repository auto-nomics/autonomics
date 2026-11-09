//! One-shot composite enrollment: minimal operator input, everything
//! derivable derived, every cross-consistency rule internalized.
//!
//! A raw enrollment touches three APIs with two hand-authored JSON
//! artifacts and three invariants (domain/mount agreement, read-only
//! publication mount, scratch inside the data root). [`EnrollRequest`]
//! takes the handful of facts an operator actually knows about a
//! cluster and performs the whole sequence: probe the worker binary
//! over ssh, generate and pin its profile, create the domain's
//! read-only data-source mount, save the target with a derived
//! `DataDomain`, and optionally switch the global policy to it.

use super::*;

/// Structured progress shared by CLI and graphical clients.
#[derive(Clone, Serialize)]
pub struct EnrollProgressEvent {
    pub stage: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uploaded_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_bytes: Option<u64>,
}

/// Per-request progress sink, independent of daemon tracing subscribers.
#[derive(Clone, Default)]
pub struct EnrollProgress(Option<std::sync::Arc<dyn Fn(&EnrollProgressEvent) + Send + Sync>>);

impl EnrollProgress {
    pub fn new(callback: impl Fn(&str, &str) + Send + Sync + 'static) -> Self {
        Self::with_events(move |event| callback(&event.stage, &event.message))
    }

    pub fn with_events(callback: impl Fn(&EnrollProgressEvent) + Send + Sync + 'static) -> Self {
        Self(Some(std::sync::Arc::new(callback)))
    }

    pub(super) fn report(&self, stage: &str, message: &str) {
        self.emit(stage, message, None, None);
    }

    pub(super) fn upload(&self, message: &str, uploaded: u64, total: u64) {
        self.emit("upload", message, Some(uploaded), Some(total));
    }

    fn emit(
        &self,
        stage: &str,
        message: &str,
        uploaded_bytes: Option<u64>,
        total_bytes: Option<u64>,
    ) {
        if let Some(callback) = &self.0 {
            callback(&EnrollProgressEvent {
                stage: stage.into(),
                message: message.into(),
                uploaded_bytes,
                total_bytes,
            });
        }
    }
}

/// Minimal operator input for one-shot enrollment.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnrollRequest {
    pub expected_revision: u64,
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
    /// Absolute root of the shared filesystem on that alias.
    #[serde(default)]
    pub data_root: Option<String>,
    /// Absolute path of the worker binary on the cluster. Existence,
    /// executability and SHA256 are probed over ssh — no hand-authored
    /// worker profile is involved.
    pub worker_executable: String,
    /// Local standalone `autonomics-worker` binary to deploy to `worker_executable` before probing:
    /// streamed over the login alias through the framed staged-write
    /// channel (atomic rename, no torn destination), then `chmod +x`.
    /// When omitted the cluster path must already exist.
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
    /// `<data_root>/scratch` when a data home is declared.
    #[serde(default)]
    pub scratch_dir: Option<String>,
    /// Switch the global compute policy to this target afterwards.
    #[serde(default)]
    pub select: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepairWorkerRequest {
    pub expected_revision: u64,
    pub target_id: String,
}

/// The coordinator distribution includes its matching standalone worker beside it.
async fn bundled_worker() -> Result<PathBuf, String> {
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let path = executable.with_file_name("autonomics-worker");
    validate_worker_package(&path).await?;
    Ok(path)
}

async fn validate_worker_package(path: &Path) -> Result<(), String> {
    let bytes = tokio::fs::read(path).await.map_err(|e| format!("matching worker package unavailable at {}: {e}; install autonomics-worker alongside the gateway from the same build", path.display()))?;
    let sha = format!("{:x}", Sha256::digest(&bytes));
    drop(bytes);
    let output = tokio::process::Command::new(path).arg("identity")
        .kill_on_drop(true).output();
    let output = tokio::time::timeout(std::time::Duration::from_secs(30), output).await
        .map_err(|_| "local worker identity check timed out")?
        .map_err(|e| format!("local worker identity check failed: {e}"))?;
    if !output.status.success() { return Err("local worker identity check failed".into()); }
    let identity: dag_core::dag::execution::typed_worker::WorkerIdentity =
        serde_json::from_slice(&output.stdout).map_err(|e| format!("invalid local worker identity: {e}"))?;
    identity.validate(&sha).map_err(|e| e.to_string())
}

#[cfg(test)]
mod repair_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[tokio::test]
    async fn bundled_worker_identity_must_match_coordinator_and_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("autonomics-worker");
        for revision in ["old-source", dag_core::source_revision()] {
            let script = format!(r#"#!/bin/sh
hash=$(sha256sum -- "$0"); hash=${{hash%% *}}
printf '{{"artifact":"autonomics-worker","protocol":1,"source_revision":"{}","executable_sha256":"%s","capabilities":["task-worker"]}}\n' "$hash"
"#, revision);
            fs::write(&path, script).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            let result = validate_worker_package(&path).await;
            if revision == "old-source" { assert!(result.unwrap_err().contains("differs from coordinator")); }
            else { result.unwrap(); }
        }
        assert!(validate_worker_package(&dir.path().join("missing")).await.unwrap_err().contains("matching worker package unavailable"));
    }
}

fn valid_alias(alias: &str) -> bool {
    !alias.is_empty()
        && !alias.starts_with('-')
        && alias
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.@:-".contains(&b))
}

pub(super) fn shq(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub(super) fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

impl ComputeRegistry {
    /// Replace only the worker pin, preserving the target's remaining configuration.
    pub async fn repair_worker(&self, request: RepairWorkerRequest) -> Result<ComputeRegistryState, String> {
        self.repair_worker_with_progress(request, EnrollProgress::default()).await
    }

    pub async fn repair_worker_with_progress(&self, request: RepairWorkerRequest, progress: EnrollProgress) -> Result<ComputeRegistryState, String> {
        progress.report("validate", "正在读取目标已有配置");
        let state = self.state();
        if state.revision != request.expected_revision { return Err("stale compute revision; reload settings".into()); }
        let target = state.targets.iter().rev().find(|t| t.id == request.target_id)
            .ok_or("unknown target")?.clone();
        if state.deleted_target_ids.contains(&target.id) { return Err("target is deleted".into()); }
        let mut settings = match &target.backend {
            ComputeBackend::Slurm { settings } => settings.clone(),
            _ => return Err("worker repair currently requires a Slurm target".into()),
        };
        if let Some(pinned) = target.endpoints.get(&settings.ssh_host) {
            let current = vfs::SshEndpoint::capture(&settings.ssh_command, &settings.ssh_host)?;
            if &current != pinned { return Err("SSH endpoint changed since this target revision; review the target configuration before repair".into()); }
        }
        let profile_path = settings.typed_worker_config.as_ref().ok_or("target has no worker profile")?;
        let profile_bytes = read_owned(profile_path, false)?;
        if target.capsule_sha256.get("worker.json").is_some_and(|expected| *expected != format!("{:x}", Sha256::digest(&profile_bytes))) {
            return Err("pinned worker profile hash mismatch".into());
        }
        let mut profile: serde_json::Value = serde_json::from_slice(&profile_bytes).map_err(|e| e.to_string())?;
        let old_path = profile["worker"]["executable"].as_str().ok_or("worker executable missing from profile")?;
        if !old_path.starts_with('/') { return Err("worker executable must be absolute".into()); }
        let connection = super::cluster_connection::ClusterConnection::new(&target.id, &settings.ssh_host, &settings.ssh_command, &settings.remote_python).with_progress(progress.clone());
        connection.authenticate().await?;
        // A healthy worker only needs a fresh connection and identity observation.
        // Repair transport failures or stale pins using the bundled worker below.
        let existing_worker = async {
            let sha = connection.probe_worker(old_path).await?;
            connection.verify_worker_identity(old_path, &sha).await?;
            if profile["worker"]["executable_sha256"] != sha
                || profile["worker"]["expected_source_revision"] != dag_core::source_revision() {
                return Err("worker pin needs an updated target revision".to_string());
            }
            Ok::<_, String>(())
        }.await;
        match existing_worker {
            Ok(()) => {
                if self.state().revision != request.expected_revision {
                    return Err("compute registry changed during repair; reload the dashboard".into());
                }
                if state.policy.target_id == target.id && state.policy.target_revision != target.revision {
                    progress.report("select_target", "正在激活已验证的目标修订");
                    return self.select(SelectTarget { expected_revision: state.revision, target_id: target.id });
                }
                progress.report("ready", "连接及 worker 校验通过，无需更新");
                return Ok(state);
            }
            Err(error) => progress.report("worker_update", &format!("需要恢复 worker：{error}")),
        }
        progress.report("worker_package", "正在检查系统随附的匹配 worker");
        let package = bundled_worker().await?;
        let parent = Path::new(old_path).parent().ok_or("worker directory missing")?;
        let destination = parent.join(format!("autonomics-worker-repair-{}", uuid::Uuid::new_v4())).to_string_lossy().into_owned();
        connection.deploy_worker(package.to_str().ok_or("worker package path is not UTF-8")?, &destination).await?;
        let sha = connection.probe_worker(&destination).await?;
        connection.verify_worker_identity(&destination, &sha).await?;
        profile["worker"]["executable"] = serde_json::json!(destination);
        profile["worker"]["executable_sha256"] = serde_json::json!(sha);
        profile["worker"]["expected_source_revision"] = serde_json::json!(dag_core::source_revision());
        let staging = self.root.join("targets").join(format!("{}-repair-{}", target.id, uuid::Uuid::new_v4()));
        private_dir(&staging)?;
        let path = staging.join("worker.json");
        write_bytes_new(&path, &serde_json::to_vec(&profile).map_err(|e| e.to_string())?)?;
        settings.typed_worker_config = Some(path);
        progress.report("save_target", "正在保存已验证的 worker 新修订");
        let saved = self.save_target(SaveTarget { expected_revision: request.expected_revision, id: target.id.clone(), name: target.name, enabled: target.enabled, backend: ComputeBackend::Slurm { settings }, data_domains: target.data_domains })?;
        if state.policy.target_id == target.id {
            progress.report("select_target", "正在激活修复后的目标");
            self.select(SelectTarget { expected_revision: saved.revision, target_id: target.id }).map_err(|e| format!("worker revision saved but activation failed: {e}"))
        } else { Ok(saved) }
    }
    /// Enroll (or upgrade) one cluster target from minimal input. The
    /// sequence is: worker probe → profile generation → data-source
    /// mount → target with derived domain → optional policy switch.
    /// Failures name the failing step; already-applied steps stay
    /// applied and re-running the same request is idempotent.
    #[tracing::instrument(skip_all, fields(cluster_id = %request.id, alias = %request.login_alias))]
    pub async fn enroll(&self, request: EnrollRequest) -> Result<ComputeRegistryState, String> {
        self.enroll_with_progress(request, EnrollProgress::default())
            .await
    }

    #[tracing::instrument(skip_all, fields(cluster_id = %request.id, alias = %request.login_alias))]
    pub async fn enroll_with_progress(
        &self,
        request: EnrollRequest,
        progress: EnrollProgress,
    ) -> Result<ComputeRegistryState, String> {
        progress.report("validate", "正在检查注册参数");
        tracing::info!(stage = "validate", "cluster enrollment started");
        identifier(&request.id)?;
        if !valid_alias(&request.login_alias) {
            return Err("login_alias must be a configured ssh host alias".into());
        }
        let data = match (&request.data_alias, &request.data_root) {
            (Some(alias), Some(root)) => {
                if !valid_alias(alias) || !is_domain_absolute(root) {
                    return Err(
                        "data_alias must be a valid ssh alias and data_root a clean absolute path"
                            .into(),
                    );
                }
                Some((alias.clone(), root.clone()))
            }
            (None, None) => None,
            _ => {
                return Err(
                    "data_alias and data_root must be supplied together (or both omitted)".into(),
                );
            }
        };
        if !request.worker_executable.starts_with('/') {
            return Err("worker_executable must be an absolute cluster path".into());
        }
        // The local deploy path crosses a process boundary: the daemon —
        // not the calling shell — resolves it, so a relative path silently
        // points somewhere else. Reject it with the reason instead.
        if let Some(local) = &request.deploy_worker
            && !local.starts_with('/')
        {
            return Err(
                "deploy_worker must be an absolute local path; the daemon (not your shell) \
                 resolves it"
                    .into(),
            );
        }

        let ssh_command = request
            .ssh_command
            .clone()
            .unwrap_or_else(|| self.config.slurm.ssh_command.clone());
        let connection = super::cluster_connection::ClusterConnection::new(
            &request.id,
            &request.login_alias,
            &ssh_command,
            request.python.as_deref().unwrap_or("python3"),
        )
        .with_progress(progress.clone());
        connection.authenticate().await?;
        if let Some(local) = &request.deploy_worker {
            connection
                .deploy_worker(local, &request.worker_executable)
                .await?;
        }
        let sha256 = connection.probe_worker(&request.worker_executable).await?;
        connection
            .verify_worker_identity(&request.worker_executable, &sha256)
            .await?;

        // 2. Generate the worker profile where the freezer expects it.
        progress.report("worker_profile", "正在生成 worker 配置");
        tracing::info!(stage = "worker_profile", "cluster enrollment step started");
        let staging = self.root.join("targets").join(format!(
            "{}-enroll-{}",
            request.id,
            uuid::Uuid::new_v4()
        ));
        private_dir(&staging)?;
        let profile_path = staging.join("worker.json");
        let profile = serde_json::json!({
            "worker": {
                "executable": request.worker_executable,
                "executable_sha256": sha256,
                "expected_source_revision": dag_core::source_revision(),
            },
            "local_kinds": [
                "channel", "dynamic_fanout", "table_from_df", "figure_embed",
                "source_literature", "source_literature_fetch", "literature_fulltext",
                "source_literature_citations", "source_s2_recommendations", "bib_save",
                "evidence_merge", "evidence_export",
            ],
        });
        write_bytes_new(&profile_path, profile.to_string().as_bytes())?;

        // 3. Settings: site defaults where the operator did not speak.
        let mut settings = SlurmConfig {
            ssh_host: request.login_alias.clone(),
            ssh_command,
            typed_worker_config: Some(profile_path),
            ..SlurmConfig::default()
        };
        if let Some(python) = &request.python {
            settings.remote_python = python.clone();
        }
        settings.enabled = true;
        if let Some(partition) = &request.partition {
            settings.partition = partition.clone();
        }
        if let Some(account) = &request.account {
            settings.account = Some(account.clone());
        }
        if let Some(scratch) = &request.scratch_dir {
            if !is_domain_absolute(scratch) {
                return Err("scratch_dir must be a clean absolute path".into());
            }
            settings.scratch_remote_dir = scratch.clone();
        } else if let Some((_, root)) = &data {
            settings.scratch_remote_dir = format!("{root}/scratch");
        }

        // 4. The domain's read-only mount must exist before the target's
        //    domain validation runs, so the data source goes first.
        let mut revision = request.expected_revision;
        if let Some((alias, root)) = &data {
            progress.report("data_source", "正在注册集群数据源");
            tracing::info!(
                stage = "data_source",
                alias,
                root,
                "cluster enrollment step started"
            );
            let source = DataSource {
                id: request.id.clone(),
                name: format!("{} shared filesystem", request.id),
                backend: DataBackend::Ssh {
                    alias: alias.clone(),
                    root: root.clone(),
                    python: settings.remote_python.clone(),
                },
                read_only: true,
                domain: Some(request.id.clone()),
                endpoint: None,
                permissions: Default::default(),
            };
            let state = self
                .add_source(SaveDataSource {
                    expected_revision: revision,
                    source,
                })
                .await
                .map_err(|error| format!("data source creation failed: {error}"))?;
            revision = state.revision;
        }

        // 5. The target, with every derived field filled in.
        progress.report("save_target", "正在保存计算目标");
        tracing::info!(stage = "save_target", "cluster enrollment step started");
        let data_domains = data
            .map(|(alias, root)| {
                vec![DataDomain {
                    id: request.id.clone(),
                    ssh_aliases: vec![alias],
                    remote_root: root,
                    vfs_prefix: format!("/data/{}", request.id),
                    retained_vfs_prefix: None,
                    retained_remote_root: None,
                }]
            })
            .unwrap_or_default();
        let mut state = self
            .save_target(SaveTarget {
                expected_revision: revision,
                id: request.id.clone(),
                name: request.name.clone().unwrap_or_else(|| request.id.clone()),
                enabled: true,
                backend: ComputeBackend::Slurm { settings },
                data_domains,
            })
            .map_err(|error| format!("target save failed: {error}"))?;

        // 6. Optional policy switch.
        if request.select {
            progress.report("select_target", "正在选择计算目标");
            tracing::info!(stage = "select_target", "cluster enrollment step started");
            state = self
                .select(SelectTarget {
                    expected_revision: state.revision,
                    target_id: request.id.clone(),
                })
                .map_err(|error| format!("policy switch failed: {error}"))?;
        }
        tracing::info!(revision = state.revision, "cluster enrollment finished");
        Ok(state)
    }
}
