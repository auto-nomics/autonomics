//! Background publication for locally activated environments.
//!
//! The foreground development loop stops at local catalog activation. This
//! distiller later converts those local references into reviewed, pushed
//! registry references without making agents wait for review or push.
//!
//! Unlike the plugin distiller there is no pull-request stage: an image
//! publication is push + catalog re-pin, and a failed push returns the
//! workspace to `Updating` for revalidation on the next pass.

use crate::{
    EnvironmentDevInfra, EnvironmentStatus, EnvironmentValidationOutcome, Result,
    env_manifest::EnvironmentManifest,
};

/// One locally active environment waiting to become a published reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentDistillationCandidate {
    pub environment_id: String,
    pub status: EnvironmentStatus,
    pub source_environment: Option<String>,
    pub request_ids: Vec<String>,
}

/// Summary of one non-blocking background publication pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvironmentDistillReport {
    pub considered: usize,
    pub completed: usize,
    pub failed: usize,
    pub failures: Vec<EnvironmentDistillFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentDistillFailure {
    pub environment_id: String,
    pub stage: String,
    pub message: String,
}

/// Callable one-shot distiller; a runtime worker may schedule this periodically.
#[derive(Clone)]
pub struct EnvironmentDistiller {
    infra: EnvironmentDevInfra,
}

impl EnvironmentDistiller {
    pub fn new(infra: EnvironmentDevInfra) -> Self {
        Self { infra }
    }

    /// List locally active work that still needs background publication.
    pub fn pending(&self) -> Result<Vec<EnvironmentDistillationCandidate>> {
        let registry = self.infra.environment_registry();
        let mut candidates = Vec::new();
        for manifest in self.infra.store().list()? {
            if !is_distillable(&manifest) {
                continue;
            }
            // Local activation means the catalog resolves the environment id.
            if registry.get(&manifest.environment_id)?.is_none() {
                continue;
            }
            candidates.push(EnvironmentDistillationCandidate {
                environment_id: manifest.environment_id,
                status: manifest.status,
                source_environment: manifest.lifecycle.source_environment,
                request_ids: manifest.lifecycle.request_ids,
            });
        }
        Ok(candidates)
    }

    /// Advance every current candidate as far as trusted publishing allows.
    ///
    /// A failure does not stop other candidates and does not invalidate their
    /// already-active local catalog references.
    pub async fn run_once(&self) -> EnvironmentDistillReport {
        let candidates = match self.pending() {
            Ok(candidates) => candidates,
            Err(error) => {
                return EnvironmentDistillReport {
                    considered: 0,
                    completed: 0,
                    failed: 1,
                    failures: vec![EnvironmentDistillFailure {
                        environment_id: "<queue>".into(),
                        stage: "read_queue".into(),
                        message: error.to_string(),
                    }],
                };
            }
        };
        let mut report = EnvironmentDistillReport {
            considered: candidates.len(),
            ..Default::default()
        };
        for candidate in candidates {
            match self.distill_environment(&candidate.environment_id).await {
                Ok(()) => report.completed += 1,
                Err((stage, message)) => {
                    report.failed += 1;
                    report.failures.push(EnvironmentDistillFailure {
                        environment_id: candidate.environment_id,
                        stage,
                        message,
                    });
                }
            }
        }
        report
    }

    async fn distill_environment(
        &self,
        environment_id: &str,
    ) -> std::result::Result<(), (String, String)> {
        // A normal pass needs at most submit, review, publish, and finalize.
        // A publish retry first returns through revalidation and review.
        for _ in 0..6 {
            let store = self.infra.store();
            let Ok(Some(mut operator)) = store.develop(environment_id) else {
                return Err((
                    "open_workspace".into(),
                    format!("unknown environment `{environment_id}`"),
                ));
            };
            match operator.status() {
                EnvironmentStatus::Draft | EnvironmentStatus::Updating => {
                    if !operator.manifest().lifecycle.publication_pending {
                        return Err((
                            "not_queued".into(),
                            "locally active environment is not pending publication".into(),
                        ));
                    }
                    if !operator
                        .repository()
                        .is_clean()
                        .map_err(stage("check_clean"))?
                    {
                        return Err((
                            "dirty_workspace".into(),
                            "environment changed again before background publication".into(),
                        ));
                    }
                    operator
                        .transition_snapshot(
                            EnvironmentStatus::PendingReview,
                            "distiller: submit local activation for review",
                        )
                        .map_err(stage("submit_review"))?;
                }
                EnvironmentStatus::PendingReview => {
                    self.infra
                        .review_environment(environment_id, true)
                        .map_err(stage("review"))?;
                }
                EnvironmentStatus::Approved => {
                    self.infra
                        .publish_reviewed_environment(environment_id)
                        .await
                        .map_err(stage("publish"))?;
                }
                EnvironmentStatus::PublishFailed => {
                    operator
                        .transition_snapshot(
                            EnvironmentStatus::Updating,
                            "distiller: retry local activation",
                        )
                        .map_err(stage("retry"))?;
                    match self
                        .infra
                        .build_environment(environment_id)
                        .await
                        .map_err(stage("revalidate"))?
                    {
                        EnvironmentValidationOutcome::Passed(_) => {}
                        EnvironmentValidationOutcome::NeedsFix(report) => {
                            return Err((
                                "revalidate".into(),
                                format!("validation failed: {:?}", report.gates),
                            ));
                        }
                    }
                }
                EnvironmentStatus::Installed => return Ok(()),
                status => {
                    return Err((
                        "unsupported_status".into(),
                        format!("locally active environment is in unsupported status {status:?}"),
                    ));
                }
            }
        }
        Err((
            "too_many_steps".into(),
            "distiller did not reach an installed environment".into(),
        ))
    }
}

fn is_distillable(manifest: &EnvironmentManifest) -> bool {
    manifest.lifecycle.publication_pending
        && matches!(
            manifest.status,
            EnvironmentStatus::Draft
                | EnvironmentStatus::Updating
                | EnvironmentStatus::PendingReview
                | EnvironmentStatus::Approved
                | EnvironmentStatus::PublishFailed
        )
}

fn stage(name: &str) -> impl Fn(crate::Error) -> (String, String) {
    let name = name.to_string();
    move |error| (name.clone(), error.to_string())
}
