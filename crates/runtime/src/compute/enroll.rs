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
    /// Local binary to deploy to `worker_executable` before probing:
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

fn valid_alias(alias: &str) -> bool {
    !alias.is_empty()
        && !alias.starts_with('-')
        && alias
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.@:-".contains(&b))
}

fn shq(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

impl ComputeRegistry {
    /// Enroll (or upgrade) one cluster target from minimal input. The
    /// sequence is: worker probe → profile generation → data-source
    /// mount → target with derived domain → optional policy switch.
    /// Failures name the failing step; already-applied steps stay
    /// applied and re-running the same request is idempotent.
    pub async fn enroll(&self, request: EnrollRequest) -> Result<ComputeRegistryState, String> {
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

        // 0. Deploy the worker binary when a local one is supplied.
        if let Some(local) = &request.deploy_worker {
            deploy_worker_binary(local, &request.worker_executable, &request).await?;
        }

        // 1. Probe the worker binary on the login node.
        let ssh_command = request
            .ssh_command
            .clone()
            .unwrap_or_else(|| self.config.slurm.ssh_command.clone());
        let ssh = dag_core::SshTarget::new(ssh_command.clone(), &request.login_alias);
        let probe = format!(
            "test -x {path} && sha256sum {path}",
            path = shq(&request.worker_executable)
        );
        let output = ssh.run_script_once(&probe).await.map_err(|error| {
            format!(
                "worker probe could not reach `{}`: {error}",
                request.login_alias
            )
        })?;
        if !output.status.success() {
            return Err(format!(
                "worker executable `{}` is missing or not executable on `{}`",
                request.worker_executable, request.login_alias
            ));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let sha256 = stdout
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !valid_sha256(&sha256) {
            return Err("worker probe returned an unparsable digest".into());
        }

        // 2. Generate the worker profile where the freezer expects it.
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
            "local_kinds": ["channel", "dynamic_fanout"],
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
            state = self
                .select(SelectTarget {
                    expected_revision: state.revision,
                    target_id: request.id.clone(),
                })
                .map_err(|error| format!("policy switch failed: {error}"))?;
        }
        Ok(state)
    }
}

/// Stream the local worker binary to its cluster path through the login
/// alias: framed staged writes (atomic same-directory rename — an aborted
/// transfer never leaves a torn binary) followed by `chmod +x`. The
/// destination's parent directory is created on demand.
async fn deploy_worker_binary(
    local: &str,
    destination: &str,
    request: &EnrollRequest,
) -> Result<(), String> {
    let local = std::path::Path::new(local);
    let metadata = std::fs::metadata(local)
        .map_err(|error| format!("deploy_worker `{}`: {error}", local.display()))?;
    if !metadata.is_file() {
        return Err(format!(
            "deploy_worker `{}` is not a regular file",
            local.display()
        ));
    }
    let ssh_command = request.ssh_command.clone().unwrap_or_else(|| "ssh".into());
    let python = "python3".to_string();

    // Ensure the destination directory exists, then stream the binary.
    let ssh = dag_core::SshTarget::new(ssh_command.clone(), &request.login_alias);
    let parent = std::path::Path::new(destination)
        .parent()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "/".into());
    let mkdir = format!("mkdir -p {}", shq(&parent));
    ssh.run_script_once(&mkdir)
        .await
        .map_err(|error| format!("worker deploy mkdir failed: {error}"))?;

    // The ssh backend refuses a root of `/`, so the operator is rooted at
    // the destination's parent directory and the file name is the object.
    let destination_path = std::path::Path::new(destination);
    let (root, file_name) = (
        destination_path
            .parent()
            .map(|parent| parent.display().to_string())
            .unwrap_or_else(|| "/public".into()),
        destination_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("worker deploy destination must end in a file name")?
            .to_string(),
    );
    if root == "/" || file_name.is_empty() {
        return Err("worker deploy destination must be a file inside a directory".into());
    }
    let operator = vfs::build_ssh_operator(&request.login_alias, &root, &python, None)
        .map_err(|error| format!("worker deploy channel: {error}"))?;
    let object_path = file_name;
    let mut writer = operator
        .writer(&object_path)
        .await
        .map_err(|error| format!("worker deploy open failed: {error}"))?;
    let file = tokio::fs::File::open(local)
        .await
        .map_err(|error| format!("deploy_worker `{}`: {error}", local.display()))?;
    let mut reader = tokio::io::BufReader::with_capacity(1024 * 1024, file);
    let mut chunk = vec![0u8; 1024 * 1024];
    loop {
        let read = tokio::io::AsyncReadExt::read(&mut reader, &mut chunk)
            .await
            .map_err(|error| format!("worker deploy read failed: {error}"))?;
        if read == 0 {
            break;
        }
        let buffer = opendal::Buffer::from(chunk[..read].to_vec());
        opendal::Writer::write(&mut writer, buffer)
            .await
            .map_err(|error| format!("worker deploy upload failed: {error}"))?;
    }
    opendal::Writer::close(&mut writer)
        .await
        .map_err(|error| format!("worker deploy commit failed: {error}"))?;
    let chmod = format!("chmod +x {}", shq(destination));
    ssh.run_script_once(&chmod)
        .await
        .map_err(|error| format!("worker deploy chmod failed: {error}"))?;
    tracing::info!(
        destination,
        bytes = metadata.len(),
        "deployed worker binary"
    );
    Ok(())
}
