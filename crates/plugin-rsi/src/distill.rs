//! Background publication for locally active plugin forks and updates.
//!
//! The foreground development loop stops at local activation. This distiller
//! later converts durable local snapshots into reviewed GitHub sources without
//! making agents wait for repository publication or PR merge.

use container_plugin::manifest::{PluginInstallationStatus, PluginManifest};

use crate::{PluginStatus, Result, RsiInfra, ValidationOutcome};

/// One locally active plugin waiting to become a durable remote source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DistillationCandidate {
    pub plugin_name: String,
    pub status: PluginStatus,
    pub source_plugin: Option<String>,
    pub request_ids: Vec<String>,
}

/// Summary of one non-blocking background publication pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginDistillReport {
    pub considered: usize,
    pub completed: usize,
    pub failed: usize,
    pub failures: Vec<DistillFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DistillFailure {
    pub plugin_name: String,
    pub stage: String,
    pub message: String,
}

/// Callable one-shot distiller; a runtime worker may schedule this periodically.
#[derive(Clone)]
pub struct PluginDistiller {
    infra: RsiInfra,
}

impl PluginDistiller {
    pub fn new(infra: RsiInfra) -> Self {
        Self { infra }
    }

    /// List local-active work that still needs background distillation.
    pub fn pending(&self) -> Result<Vec<DistillationCandidate>> {
        Ok(self
            .infra
            .store()
            .list()?
            .into_iter()
            .filter(is_distillable)
            .map(|manifest| DistillationCandidate {
                plugin_name: manifest.plugin_name,
                status: manifest.status,
                source_plugin: manifest.lifecycle.source_plugin,
                request_ids: manifest.lifecycle.request_ids,
            })
            .collect())
    }

    /// Advance every current candidate as far as trusted publishing allows.
    ///
    /// A failure does not stop other candidates and does not invalidate their
    /// already-active local runtime snapshots.
    pub fn run_once(&self) -> PluginDistillReport {
        let candidates = match self.pending() {
            Ok(candidates) => candidates,
            Err(error) => {
                return PluginDistillReport {
                    considered: 0,
                    completed: 0,
                    failed: 1,
                    failures: vec![DistillFailure {
                        plugin_name: "<queue>".into(),
                        stage: "read_queue".into(),
                        message: error.to_string(),
                    }],
                };
            }
        };
        let mut report = PluginDistillReport {
            considered: candidates.len(),
            ..Default::default()
        };
        for candidate in candidates {
            match self.distill_plugin(&candidate.plugin_name) {
                Ok(()) => report.completed += 1,
                Err((stage, error)) => {
                    report.failed += 1;
                    report.failures.push(DistillFailure {
                        plugin_name: candidate.plugin_name,
                        stage,
                        message: error,
                    });
                }
            }
        }
        report
    }

    fn distill_plugin(&self, plugin_name: &str) -> std::result::Result<(), (String, String)> {
        // A normal pass needs at most review, publish/open-PR, merge, and
        // install. A publish retry first returns through validation and review.
        for _ in 0..6 {
            let store = self.infra.store();
            let mut operator = store
                .develop(plugin_name)
                .map_err(|error| ("open_workspace".into(), error.to_string()))?
                .ok_or_else(|| {
                    (
                        "open_workspace".into(),
                        format!("unknown plugin `{plugin_name}`"),
                    )
                })?;
            let status = operator.status();
            match status {
                PluginStatus::Draft | PluginStatus::Updating => {
                    if !operator.manifest().lifecycle.publication_pending {
                        return Err((
                            "not_queued".into(),
                            "local-active plugin is not pending publication".into(),
                        ));
                    }
                    if !operator
                        .repository()
                        .is_clean()
                        .map_err(stage("check_clean"))?
                    {
                        return Err((
                            "dirty_workspace".into(),
                            "plugin changed again before background publication".into(),
                        ));
                    }
                    operator
                        .transition_snapshot(
                            PluginStatus::PendingReview,
                            "distiller: submit local snapshot for review",
                        )
                        .map_err(stage("submit_review"))?;
                }
                PluginStatus::PendingReview => {
                    self.infra
                        .review(plugin_name, true)
                        .map_err(stage("review"))?;
                }
                PluginStatus::Approved => {
                    let is_update = operator.manifest().lifecycle.base_remote.is_some()
                        && operator.manifest().lifecycle.base_commit.is_some();
                    if is_update {
                        self.infra
                            .open_update_pull_request(plugin_name)
                            .map_err(stage("open_pull_request"))?;
                    } else {
                        self.infra
                            .publish_reviewed(plugin_name)
                            .map_err(stage("publish"))?;
                    }
                }
                PluginStatus::Published => {
                    self.infra.install(plugin_name).map_err(stage("install"))?;
                }
                PluginStatus::PullRequestOpen => {
                    self.infra
                        .merge_update_pull_request(plugin_name)
                        .map_err(stage("merge_pull_request"))?;
                }
                PluginStatus::PullRequestMerged => {
                    self.infra.install(plugin_name).map_err(stage("install"))?;
                }
                PluginStatus::PublishFailed => {
                    operator
                        .transition_snapshot(
                            PluginStatus::Updating,
                            "distiller: retry local activation",
                        )
                        .map_err(stage("retry"))?;
                    match self
                        .infra
                        .validate_and_submit(plugin_name)
                        .map_err(stage("revalidate"))?
                    {
                        ValidationOutcome::Submitted(_) => {}
                        ValidationOutcome::Passed(_) => {}
                        ValidationOutcome::NeedsFix(report) => {
                            return Err((
                                "revalidate".into(),
                                format!("validation failed: {:?}", report.gates),
                            ));
                        }
                    }
                }
                PluginStatus::Installed => return Ok(()),
                _ => {
                    return Err((
                        "unsupported_status".into(),
                        format!("local-active plugin is in unsupported status {status:?}"),
                    ));
                }
            }
        }
        Err((
            "too_many_steps".into(),
            "distiller did not reach an installed source".into(),
        ))
    }
}

fn is_distillable(manifest: &PluginManifest) -> bool {
    manifest.installation.is_runtime_active()
        && manifest.lifecycle.publication_pending
        && matches!(
            manifest.status,
            PluginStatus::Draft
                | PluginStatus::Updating
                | PluginStatus::PendingReview
                | PluginStatus::Approved
                | PluginStatus::PublishFailed
                | PluginStatus::Published
                | PluginStatus::PullRequestOpen
                | PluginStatus::PullRequestMerged
        )
        && manifest.installation.status == Some(PluginInstallationStatus::LocalActive)
}

fn stage(name: &str) -> impl Fn(crate::Error) -> (String, String) {
    let name = name.to_string();
    move |error| (name.clone(), error.to_string())
}
