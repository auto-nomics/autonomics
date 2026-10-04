#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn run_evolution_cycle(
    manager: &SkillManager,
    policy: &EvolutionPolicy,
) -> Result<EvolutionReport, SkillError> {
    run_evolution_cycle_labeled(manager, policy, Vec::new())
}

/// [`run_evolution_cycle`] with the trigger labels that caused it,
/// used by the worker so reports say why they ran.
pub(crate) fn run_evolution_cycle_labeled(
    manager: &SkillManager,
    policy: &EvolutionPolicy,
    triggers: Vec<&'static str>,
) -> Result<EvolutionReport, SkillError> {
    let distill_report = manager.distill()?;
    let mut report = EvolutionReport {
        triggers,
        clusters_considered: distill_report.candidates_considered,
        proposals_written: distill_report.proposals_written,
        updated_existing: distill_report.updated_existing,
        skipped: distill_report.skipped,
        ..Default::default()
    };

    let all_pending = manager.proposals().list();
    let pending: Vec<String> = all_pending
        .iter()
        .filter(|p| p.status == ProposalStatus::Pending)
        .map(|p| p.name.clone())
        .collect();
    // Agent-authored proposals are human-review-only: the model may
    // draft, never promote its own draft.
    let agent_authored_pending = all_pending
        .iter()
        .filter(|p| p.status == ProposalStatus::Pending && p.authored_by == "agent")
        .count();

    if policy.auto_approve {
        // Filter before the cap so agent entries consume no approval
        // slots.
        let eligible: Vec<String> = all_pending
            .iter()
            .filter(|p| p.status == ProposalStatus::Pending && p.authored_by != "agent")
            .map(|p| p.name.clone())
            .collect();
        for name in eligible.iter().take(policy.max_approvals_per_cycle) {
            match manager.approve_proposal(name) {
                Ok(outcome) => {
                    tracing::info!(
                        skill = %outcome.name,
                        destination = %outcome.destination.display(),
                        "evolution cycle auto-approved a proposal"
                    );
                    report.auto_approved.push(name.clone());
                }
                Err(e) => {
                    // Validation refused it — that is the gate doing
                    // its job; surface and continue with the rest.
                    tracing::warn!(proposal = name, error = %e, "auto-approve refused");
                }
            }
        }
    }
    report.left_pending = pending.len() - report.auto_approved.len();
    report.human_review_only = agent_authored_pending;
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
