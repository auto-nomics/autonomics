//! Standalone worker package validation, profile generation, and repair.

use super::{
    ComputeBackend, ComputeRegistry, ComputeRegistryState, EnrollProgress, EnrollStage, SaveTarget,
    cluster_connection::ClusterConnection, is_domain_absolute,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepairWorkerRequest {
    pub target_id: String,
}

/// The coordinator distribution includes its matching standalone worker beside it.
pub(super) async fn bundled_worker() -> Result<PathBuf, String> {
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let path = executable.with_file_name("autonomics-worker");
    validate_worker_package(&path).await?;
    Ok(path)
}

async fn validate_worker_package(path: &Path) -> Result<(), String> {
    let bytes = tokio::fs::read(path).await.map_err(|e| format!("matching worker package unavailable at {}: {e}; install autonomics-worker alongside the gateway from the same build", path.display()))?;
    let sha = format!("{:x}", Sha256::digest(&bytes));
    drop(bytes);
    let output = tokio::process::Command::new(path)
        .arg("identity")
        .kill_on_drop(true)
        .output();
    let output = tokio::time::timeout(std::time::Duration::from_secs(30), output)
        .await
        .map_err(|_| "local worker identity check timed out")?
        .map_err(|e| format!("local worker identity check failed: {e}"))?;
    if !output.status.success() {
        return Err("local worker identity check failed".into());
    }
    let identity: dag_core::dag::execution::typed_worker::WorkerIdentity =
        serde_json::from_slice(&output.stdout)
            .map_err(|e| format!("invalid local worker identity: {e}"))?;
    identity.validate(&sha).map_err(|e| e.to_string())
}

impl ComputeRegistry {
    /// Replace only the worker pin, preserving the target's remaining configuration.
    pub async fn repair_worker(
        &self,
        request: RepairWorkerRequest,
    ) -> Result<ComputeRegistryState, String> {
        self.repair_worker_with_progress(request, EnrollProgress::default())
            .await
    }

    pub async fn repair_worker_with_progress(
        &self,
        request: RepairWorkerRequest,
        progress: EnrollProgress,
    ) -> Result<ComputeRegistryState, String> {
        progress.report(EnrollStage::Validate, "正在读取目标已有配置");
        let state = self.state();
        let target = state
            .targets
            .iter()
            .rev()
            .find(|t| t.id == request.target_id)
            .ok_or("unknown target")?
            .clone();
        if state.deleted_target_ids.contains(&target.id) {
            return Err("target is deleted".into());
        }
        let mut settings = match &target.backend {
            ComputeBackend::Slurm { settings } | ComputeBackend::Ssh { settings } => {
                settings.clone()
            }
            _ => return Err("worker repair requires a remote target".into()),
        };
        if let Some(pinned) = target.endpoints.get(&settings.ssh_host) {
            let current = vfs::SshEndpoint::capture(&settings.ssh_command, &settings.ssh_host)?;
            if &current != pinned {
                return Err("SSH endpoint changed since this target was saved; review the target configuration before repair".into());
            }
        }
        let mut profile = settings
            .typed_worker
            .take()
            .unwrap_or_else(|| worker_profile("", ""));
        let old_path = profile["worker"]["executable"]
            .as_str()
            .ok_or("worker executable missing from profile")?
            .to_owned();
        if !old_path.is_empty() && !is_domain_absolute(&old_path) {
            return Err("worker executable must be absolute".into());
        }
        let connection = ClusterConnection::new(
            &target.id,
            &settings.ssh_host,
            &settings.ssh_command,
            &settings.remote_python,
        )
        .with_progress(progress.clone());
        connection.authenticate().await?;
        // A healthy worker only needs a fresh connection and identity observation.
        // Repair transport failures or stale pins using the bundled worker below.
        let existing_worker = async {
            if old_path.is_empty() {
                return Err("target has no worker profile".to_string());
            }
            let sha = connection.probe_worker(&old_path).await?;
            connection.verify_worker_identity(&old_path, &sha).await?;
            if profile["worker"]["executable_sha256"] != sha
                || profile["worker"]["expected_source_revision"] != dag_core::source_revision()
            {
                return Err("worker pin needs updating".to_string());
            }
            Ok::<_, String>(())
        }
        .await;
        match existing_worker {
            Ok(()) => {
                progress.report(EnrollStage::Ready, "连接及 worker 校验通过，无需更新");
                return Ok(state);
            }
            Err(error) => progress.report(
                EnrollStage::WorkerUpdate,
                &format!("需要恢复 worker：{error}"),
            ),
        }
        progress.report(EnrollStage::WorkerPackage, "正在检查系统随附的匹配 worker");
        let package = bundled_worker().await?;
        let parent = if old_path.is_empty() {
            PathBuf::from(connection.home_directory().await?)
                .join(".local/share/autonomics/workers")
        } else {
            Path::new(&old_path)
                .parent()
                .ok_or("worker directory missing")?
                .to_path_buf()
        };
        let destination = parent
            .join(format!("autonomics-worker-repair-{}", uuid::Uuid::new_v4()))
            .to_string_lossy()
            .into_owned();
        connection
            .deploy_worker(
                package.to_str().ok_or("worker package path is not UTF-8")?,
                &destination,
            )
            .await?;
        let sha = connection.probe_worker(&destination).await?;
        connection
            .verify_worker_identity(&destination, &sha)
            .await?;
        profile["worker"]["executable"] = serde_json::json!(destination);
        profile["worker"]["executable_sha256"] = serde_json::json!(sha);
        profile["worker"]["expected_source_revision"] =
            serde_json::json!(dag_core::source_revision());
        settings.typed_worker = Some(profile);
        progress.report(EnrollStage::SaveTarget, "正在保存已验证的 worker 配置");
        let saved = self.save_target(SaveTarget {
            id: target.id.clone(),
            name: target.name,
            enabled: target.enabled,
            backend: match target.backend {
                ComputeBackend::Ssh { .. } => ComputeBackend::Ssh { settings },
                _ => ComputeBackend::Slurm { settings },
            },
            data_domains: target.data_domains,
        })?;
        Ok(saved)
    }
}

pub(super) fn worker_profile(executable: &str, sha256: &str) -> serde_json::Value {
    serde_json::json!({
        "worker": {
            "executable": executable,
            "executable_sha256": sha256,
            "expected_source_revision": dag_core::source_revision(),
        },
        "local_kinds": [
            "channel", "dynamic_fanout", "table_from_df", "figure_embed",
            "source_literature", "source_literature_fetch", "literature_fulltext",
            "source_literature_citations", "source_s2_recommendations", "bib_save",
            "evidence_merge", "evidence_export",
            // Local PubMed mirror nodes run in-process (shared DuckDB
            // handle, read-only) — no container/cluster execution.
            "source_pubmed_local", "pubmed_mesh_analysis", "pubmed_author_network",
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};

    #[tokio::test]
    async fn bundled_worker_identity_must_match_coordinator_and_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("autonomics-worker");
        for revision in ["old-source", dag_core::source_revision()] {
            let script = format!(
                r#"#!/bin/sh
hash=$(sha256sum -- "$0"); hash=${{hash%% *}}
printf '{{"artifact":"autonomics-worker","protocol":1,"source_revision":"{}","executable_sha256":"%s","capabilities":["task-worker"]}}\n' "$hash"
"#,
                revision
            );
            fs::write(&path, script).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            let result = validate_worker_package(&path).await;
            if revision == "old-source" {
                assert!(result.unwrap_err().contains("differs from coordinator"));
            } else {
                result.unwrap();
            }
        }
        assert!(
            validate_worker_package(&dir.path().join("missing"))
                .await
                .unwrap_err()
                .contains("matching worker package unavailable")
        );
    }
}
