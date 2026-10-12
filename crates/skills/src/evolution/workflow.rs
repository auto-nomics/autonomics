#[cfg_attr(not(test), allow(dead_code))]
pub(crate) async fn run_evolution_cycle(
    manager: &SkillManager,
    policy: &EvolutionPolicy,
) -> Result<EvolutionReport, SkillError> {
    run_evolution_cycle_labeled(manager, policy, Vec::new()).await
}

/// [`run_evolution_cycle`] with the trigger labels that caused it,
/// used by the worker so reports say why they ran.
pub(crate) async fn run_evolution_cycle_labeled(
    manager: &SkillManager,
    policy: &EvolutionPolicy,
    triggers: Vec<&'static str>,
) -> Result<EvolutionReport, SkillError> {
    let distill_report = manager.distill().await?;
    let mut report = EvolutionReport {
        triggers,
        clusters_considered: distill_report.candidates_considered,
        proposals_written: distill_report.proposals_written,
        updated_existing: distill_report.updated_existing,
        skipped: distill_report.skipped,
        ..Default::default()
    };

    let all_pending = manager.proposals().list();
    // Legacy manifests from the removed `skill_propose` channel are
    // consumed, never installed — the model never wrote the library,
    // and it does not start now.
    let mut installable = Vec::new();
    for proposal in &all_pending {
        if proposal.status != ProposalStatus::Pending {
            continue;
        }
        if proposal.authored_by != "distiller" {
            tracing::info!(
                proposal = %proposal.name,
                "consuming legacy agent-authored proposal without installing it"
            );
            if let Err(e) = manager.proposals().mark_processed(&proposal.cluster_hash) {
                tracing::warn!(proposal = %proposal.name, error = %e, "cannot consume legacy proposal");
            }
            continue;
        }
        installable.push(proposal.name.clone());
    }
    let deferred = installable
        .len()
        .saturating_sub(policy.max_installs_per_cycle);
    for name in installable.iter().take(policy.max_installs_per_cycle) {
        match manager.apply_proposal(name) {
            Ok(outcome) => {
                tracing::info!(
                    skill = %outcome.name,
                    destination = %outcome.destination.display(),
                    "evolution cycle installed a proposal"
                );
                report.installed.push(name.clone());
            }
            Err(e) => {
                // Validation refused it — the strict contract doing
                // its job; surface and continue with the rest.
                tracing::warn!(proposal = name, error = %e, "install refused");
            }
        }
    }
    report.deferred = deferred;
    // Persist the fitness signal as part of every cycle — the
    // natural checkpoint cadence, and the restart boundary for
    // usage data.
    if let Err(e) = manager.persist_usage() {
        tracing::warn!(error = %e, "cannot persist usage table");
    }
    Ok(report)
}

/// The reply payload a worker sends back to a request that needs one.
///
/// Three-variant sum type (not `Result<T, SkillError>`): `WorkerGone`
/// is a distinct fault class — the worker task exited before
/// replying, which surfaces as HTTP 503 at the gateway boundary.
/// Folding it into `Result` would force every handler to sniff error
use crate::error::SkillError;
use crate::manager::SkillManager;
use crate::proposals::ProposalStatus;

use super::model::{EvolutionPolicy, EvolutionReport};
