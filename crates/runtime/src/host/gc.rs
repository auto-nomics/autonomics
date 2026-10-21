//! Background maintenance loops started once per process by
//! [`SharedInfra::open`](super::SharedInfra::open): container workspace GC,
//! dag-task receipt GC, and the plugin/environment distillers.

use std::sync::Arc;

use container_runtime::{ContainerExecutionInfra, WorkspaceGcPolicy, sweep_workspace};

use crate::config::PluginRsiConfig;

/// Background garbage collector for the container data plane: one sweep when
/// the host opens, then one per configured interval
/// (`AUTONOMICS_WORKSPACE_GC_INTERVAL_SECS`, `0` disables the loop).
///
/// The sweeper never removes a directory a live run holds a lock on, a
/// scratch owned by a live process younger than the GC age, or user-declared
/// workdirs. The panel data cache is not managed here.
pub(super) fn spawn_container_gc(infra: &Arc<ContainerExecutionInfra>) {
    let config = infra.config.clone();
    let workspace_policy = WorkspaceGcPolicy {
        min_age: container_runtime::workspace_gc_age(),
        dry_run: false,
    };
    // Purely opt-in: work dirs are the durable store of container outputs,
    // so the sweep only runs when an age was set explicitly.
    let work_dir_policy =
        container_runtime::work_dir_gc_age().map(|min_age| container_runtime::WorkDirGcPolicy {
            min_age,
            dry_run: false,
        });
    let interval = container_runtime::workspace_gc_interval();
    tokio::spawn(async move {
        loop {
            let workspace = sweep_workspace(&config.workspace_root, &workspace_policy).await;
            if !workspace.errors.is_empty() {
                tracing::warn!(errors = ?workspace.errors, "container workspace GC errors");
            }
            tracing::info!(
                removed = workspace.removed,
                bytes_freed = workspace.bytes_freed,
                in_use = workspace.retained_in_use,
                recent = workspace.retained_recent,
                foreign = workspace.foreign_entries,
                pending_cleared = workspace.pending_cleared,
                "container workspace GC"
            );
            if let Some(policy) = &work_dir_policy {
                let work_dirs =
                    container_runtime::sweep_work_dirs(&config.workspace_root, policy).await;
                if !work_dirs.errors.is_empty() {
                    tracing::warn!(errors = ?work_dirs.errors, "container work dir GC errors");
                }
                tracing::info!(
                    removed = work_dirs.removed,
                    bytes_freed = work_dirs.bytes_freed,
                    in_use = work_dirs.retained_in_use,
                    recent = work_dirs.retained_recent,
                    "container work dir GC"
                );
            }
            let Some(interval) = interval else {
                break;
            };
            tokio::time::sleep(interval).await;
        }
    });
}

/// Periodically reclaim expired `LocalTaskExecutor` receipt directories from
/// the persistent dag-tasks tree (`AUTONOMICS_DAG_TASKS_GC_AGE_SECS`,
/// default 7 days; `AUTONOMICS_DAG_TASKS_GC_INTERVAL_SECS`, default 6h,
/// `0` sweeps once at startup and stops).
///
/// Receipt directories hold run evidence only — manifests, statuses, logs,
/// and materialized DataFrame artifacts that are never read back as data —
/// so age-based deletion is always safe. Directories owned by a live
/// process are retained regardless of age by the age sweep.
///
/// Each cycle then enforces a total-size ceiling
/// (`AUTONOMICS_DAG_TASKS_GC_MAX_BYTES`, default 32 GiB, `0` disables):
/// when the tree grows past it, receipts are deleted oldest-first —
/// dead-owner first, then live-owner ones past a 6h grace, so a long-lived
/// daemon's history cannot grow without bound either — back down to 75% of
/// the ceiling.
pub(super) fn spawn_dag_task_gc() {
    let root = dag_core::dag_tasks_root();
    let policy = dag_core::DagTaskGcPolicy {
        min_age: dag_core::dag_task_gc_age(),
        dry_run: false,
    };
    let interval = dag_core::dag_task_gc_interval();
    let max_bytes = dag_core::dag_task_gc_max_bytes();
    tokio::spawn(async move {
        loop {
            let report = dag_core::sweep_dag_tasks(&root, &policy).await;
            if !report.errors.is_empty() {
                tracing::warn!(errors = ?report.errors, "dag task GC errors");
            }
            tracing::info!(
                removed = report.removed,
                bytes_freed = report.bytes_freed,
                live_owner = report.retained_live_owner,
                recent = report.retained_recent,
                foreign = report.foreign_entries,
                "dag task GC"
            );
            let watermark = dag_core::sweep_dag_tasks_watermark(&root, max_bytes).await;
            if !watermark.errors.is_empty() {
                tracing::warn!(errors = ?watermark.errors, "dag task watermark GC errors");
            }
            if watermark.removed > 0 {
                tracing::warn!(
                    removed = watermark.removed,
                    bytes_freed = watermark.total_bytes_before - watermark.total_bytes_after,
                    before = watermark.total_bytes_before,
                    after = watermark.total_bytes_after,
                    ceiling = max_bytes,
                    "dag task GC watermark exceeded, oldest receipts reclaimed"
                );
            }
            let Some(interval) = interval else {
                break;
            };
            tokio::time::sleep(interval).await;
        }
    });
}

/// Periodically convert locally active plugin work into remote sources.
///
/// The first interval is skipped intentionally: startup should not turn an
/// already usable local snapshot into a GitHub operation before the host has
/// finished opening its registries.
pub(super) fn spawn_plugin_distiller(rsi: &Arc<plugin_rsi::PluginInfra>, config: &PluginRsiConfig) {
    if !config.distillation_enabled {
        tracing::info!("plugin distillation disabled");
        return;
    }
    let interval_secs = config.effective_distillation_interval_secs();
    let distiller = plugin_rsi::PluginDistiller::new((**rsi).clone());
    tokio::spawn(async move {
        let mut timer = tokio::time::interval(std::time::Duration::from_secs(interval_secs));
        timer.tick().await;
        loop {
            timer.tick().await;
            let worker = distiller.clone();
            let report = match tokio::task::spawn_blocking(move || worker.run_once()).await {
                Ok(report) => report,
                Err(error) => {
                    tracing::warn!(error = %error, "plugin distillation task failed");
                    continue;
                }
            };
            if report.failed == 0 {
                tracing::info!(
                    considered = report.considered,
                    completed = report.completed,
                    "plugin distillation pass"
                );
            } else {
                tracing::warn!(
                    considered = report.considered,
                    completed = report.completed,
                    failed = report.failed,
                    failures = ?report.failures,
                    "plugin distillation pass"
                );
            }
        }
    });
}

/// Periodically convert locally activated environment images into published
/// registry references.
///
/// The first interval is skipped intentionally, matching the plugin
/// distiller: startup must not turn an already usable local activation into
/// a push before the host has finished opening its registries.
pub(super) fn spawn_environment_distiller(
    environment_dev: &Arc<plugin_rsi::EnvironmentDevInfra>,
    config: &crate::config::EnvironmentDevConfig,
) {
    if !config.distillation_enabled {
        tracing::info!("environment distillation disabled");
        return;
    }
    let interval_secs = config.effective_distillation_interval_secs();
    let distiller = plugin_rsi::EnvironmentDistiller::new((**environment_dev).clone());
    tokio::spawn(async move {
        let mut timer = tokio::time::interval(std::time::Duration::from_secs(interval_secs));
        timer.tick().await;
        loop {
            timer.tick().await;
            let worker = distiller.clone();
            let report = worker.run_once().await;
            if report.failed == 0 {
                tracing::info!(
                    considered = report.considered,
                    completed = report.completed,
                    "environment distillation pass"
                );
            } else {
                tracing::warn!(
                    considered = report.considered,
                    completed = report.completed,
                    failed = report.failed,
                    failures = ?report.failures,
                    "environment distillation pass"
                );
            }
        }
    });
}
