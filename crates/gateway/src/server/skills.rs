//! Skill evolution and proposal handlers.

use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use runtime::SharedInfra;

use super::error::{GatewayError, GatewayResult};
use super::state::GatewayState;
use crate::proto::*;

// ── skill evolution ───────────────────────────────────────────────────

/// All pure [`SharedInfra`] operations: no host lock, no agent state —
/// the manager's always-fresh scans are the source of truth.
#[utoipa::path(
    get,
    path = "/api/v1/skills/evolution",
    tag = "skills",
    responses((status = 200, body = SkillEvolutionStatus))
)]
pub(crate) async fn get_skill_evolution_status(
    State(state): State<GatewayState>,
) -> Json<SkillEvolutionStatus> {
    Json(skill_evolution_status(&state.infra))
}

fn skill_evolution_status(infra: &SharedInfra) -> SkillEvolutionStatus {
    let manager = &infra.skills;
    let (auto_approve, max_approvals_per_cycle, sweep_interval_secs) = match &infra.skill_evolution
    {
        Some(handle) => (
            handle.policy().auto_approve,
            handle.policy().max_approvals_per_cycle,
            handle.timer().map(|d| d.as_secs()),
        ),
        // Service off: the loop still runs on demand (CLI,
        // skill_evolve tool); display the daemon's configured
        // gate from the manager's defaults.
        None => (false, 5, None),
    };
    let mut skills_workspace = 0;
    let mut skills_global = 0;
    let mut skills_builtin = 0;
    let mut skills_auto = 0;
    for entry in manager.registry().list() {
        match entry.tier {
            skills::SkillTier::Workspace => skills_workspace += 1,
            skills::SkillTier::Global => skills_global += 1,
            skills::SkillTier::Builtin => skills_builtin += 1,
        }
        if entry.meta.tags.iter().any(|t| t == "auto") {
            skills_auto += 1;
        }
    }
    let mut proposals_pending = 0;
    let mut proposals_approved = 0;
    let mut proposals_rejected = 0;
    for proposal in manager.proposals().list() {
        match proposal.status {
            skills::ProposalStatus::Pending => proposals_pending += 1,
            skills::ProposalStatus::Approved => proposals_approved += 1,
            skills::ProposalStatus::Rejected => proposals_rejected += 1,
        }
    }
    SkillEvolutionStatus {
        service_enabled: infra.skill_evolution.is_some(),
        generation: manager.generation(),
        auto_approve,
        max_approvals_per_cycle,
        sweep_interval_secs,
        observations: manager.observations().list().len(),
        skills_workspace,
        skills_global,
        skills_builtin,
        skills_auto,
        proposals_pending,
        proposals_approved,
        proposals_rejected,
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/skills/evolution/trigger",
    tag = "skills",
    request_body = TriggerEvolutionRequest,
    responses(
        (status = 200, body = SkillEvolutionReport),
        (status = 503, description = "Skill evolution service is disabled")
    )
)]
pub(crate) async fn trigger_skill_evolution(
    State(state): State<GatewayState>,
    Json(request): Json<TriggerEvolutionRequest>,
) -> GatewayResult<Json<SkillEvolutionReport>> {
    // Channel-only path: the worker is the sole executor. We
    // send a `RunCycle` command with the requested policy override
    // and await the oneshot reply (10 s timeout for the whole
    // distillation pass; the worker runs the cycle synchronously
    // on its task so this is generous headroom, not a stall
    // budget).
    let handle = state.infra.skill_evolution.as_ref().ok_or_else(|| {
        GatewayError::Status(
            StatusCode::SERVICE_UNAVAILABLE,
            "skill evolution service is disabled".to_string(),
        )
    })?;
    let mut policy = handle.policy().clone();
    if let Some(auto) = request.auto_approve {
        policy.auto_approve = auto;
    }
    let outcome = tokio::time::timeout(Duration::from_secs(10), handle.call_run_cycle(policy))
        .await
        .map_err(|_| {
            GatewayError::Message("skill evolution cycle timed out after 10 s".to_string())
        })?;
    let report = match outcome {
        skills::SkillCommandOutcome::Ok(report) => report,
        skills::SkillCommandOutcome::Err(e) => {
            return Err(GatewayError::Message(format!(
                "evolution cycle failed: {e}"
            )));
        }
        skills::SkillCommandOutcome::WorkerGone => {
            return Err(GatewayError::Status(
                StatusCode::SERVICE_UNAVAILABLE,
                "skill evolution worker is gone".to_string(),
            ));
        }
    };
    Ok(Json(SkillEvolutionReport {
        triggers: report.triggers.iter().map(|t| t.to_string()).collect(),
        clusters_considered: report.clusters_considered,
        proposals_written: report.proposals_written,
        updated_existing: report.updated_existing,
        auto_approved: report.auto_approved,
        left_pending: report.left_pending,
        skipped: report
            .skipped
            .into_iter()
            .map(|(cluster_hash, reason)| SkippedCluster {
                cluster_hash,
                reason,
            })
            .collect(),
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/skills/evolution/proposals",
    tag = "skills",
    responses((status = 200, body = [SkillProposalView]))
)]
pub(crate) async fn list_skill_proposals(
    State(state): State<GatewayState>,
) -> Json<Vec<SkillProposalView>> {
    let manager = &state.infra.skills;
    let mut rows: Vec<SkillProposalView> = manager
        .proposals()
        .list()
        .into_iter()
        .map(|p| SkillProposalView {
            name: p.name.clone(),
            status: p.status.as_str().to_string(),
            authored_by: p.authored_by.clone(),
            update: p.update,
            rationale: p.rationale.clone(),
            cluster_hash: p.cluster_hash.clone(),
            observation_count: p.source_observation_ids.len(),
            created_at: p.created_at,
        })
        .collect();
    // Pending first (actionable), then newest first.
    rows.sort_by(|a, b| {
        let pending = |s: &str| if s == "pending" { 0 } else { 1 };
        pending(&a.status)
            .cmp(&pending(&b.status))
            .then(b.created_at.cmp(&a.created_at))
            .then(a.name.cmp(&b.name))
    });
    Json(rows)
}

#[utoipa::path(
    post,
    path = "/api/v1/skills/evolution/proposals/{name}/approve",
    tag = "skills",
    responses(
        (status = 200, body = SkillApproveOutcome),
        (status = 404, description = "No pending proposal with that name"),
        (status = 400, description = "Validation refused the promotion"),
        (status = 503, description = "Skill evolution service is disabled")
    )
)]
pub(crate) async fn approve_skill_proposal(
    State(state): State<GatewayState>,
    Path(name): Path<String>,
) -> GatewayResult<Json<SkillApproveOutcome>> {
    // Channel-only path: approve flows through the worker so an
    // in-flight cycle (coalesced batch) cannot strand the HTTP
    // caller. The worker drains inline approve commands BEFORE the
    // batch runs.
    let handle = state.infra.skill_evolution.as_ref().ok_or_else(|| {
        GatewayError::Status(
            StatusCode::SERVICE_UNAVAILABLE,
            "skill evolution service is disabled".to_string(),
        )
    })?;
    let outcome = tokio::time::timeout(Duration::from_secs(5), handle.call_approve(name))
        .await
        .map_err(|_| GatewayError::Message("approve timed out after 5 s".to_string()))?;
    let outcome = match outcome {
        skills::SkillCommandOutcome::Ok(o) => o,
        skills::SkillCommandOutcome::Err(e) => return Err(GatewayError::Message(e.to_string())),
        skills::SkillCommandOutcome::WorkerGone => {
            return Err(GatewayError::Status(
                StatusCode::SERVICE_UNAVAILABLE,
                "skill evolution worker is gone".to_string(),
            ));
        }
    };
    Ok(Json(SkillApproveOutcome {
        name: outcome.name,
        destination: outcome.destination.display().to_string(),
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/skills/evolution/proposals/{name}/reject",
    tag = "skills",
    responses(
        (status = 200, description = "Rejected; cluster consumed"),
        (status = 404, description = "No pending proposal with that name"),
        (status = 503, description = "Skill evolution service is disabled")
    )
)]
pub(crate) async fn reject_skill_proposal(
    State(state): State<GatewayState>,
    Path(name): Path<String>,
) -> GatewayResult<Json<SkillProposalView>> {
    let handle = state.infra.skill_evolution.as_ref().ok_or_else(|| {
        GatewayError::Status(
            StatusCode::SERVICE_UNAVAILABLE,
            "skill evolution service is disabled".to_string(),
        )
    })?;
    let outcome = tokio::time::timeout(Duration::from_secs(5), handle.call_reject(name))
        .await
        .map_err(|_| GatewayError::Message("reject timed out after 5 s".to_string()))?;
    let proposal = match outcome {
        skills::SkillCommandOutcome::Ok(p) => p,
        skills::SkillCommandOutcome::Err(e) => return Err(GatewayError::Message(e.to_string())),
        skills::SkillCommandOutcome::WorkerGone => {
            return Err(GatewayError::Status(
                StatusCode::SERVICE_UNAVAILABLE,
                "skill evolution worker is gone".to_string(),
            ));
        }
    };
    Ok(Json(SkillProposalView {
        name: proposal.name,
        status: proposal.status.as_str().to_string(),
        authored_by: proposal.authored_by,
        update: proposal.update,
        rationale: proposal.rationale,
        cluster_hash: proposal.cluster_hash,
        observation_count: proposal.source_observation_ids.len(),
        created_at: proposal.updated_at,
    }))
}
