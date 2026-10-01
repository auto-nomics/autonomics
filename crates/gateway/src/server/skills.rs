//! Skill evolution and proposal handlers.

use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use runtime::SharedInfra;

use super::error::{GatewayError, GatewayResult};
use super::state::GatewayState;
use crate::proto::*;

// ── skill library ─────────────────────────────────────────────────────

#[utoipa::path(
    get,
    path = "/api/v1/skills/evolution/observations",
    tag = "skills",
    responses((status = 200, body = [SkillObservationView]))
)]
pub(crate) async fn list_skill_observations(
    State(state): State<GatewayState>,
) -> Json<Vec<SkillObservationView>> {
    let mut observations = state.infra.skills.observations().list();
    observations.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(a.id.cmp(&b.id)));
    Json(
        observations
            .into_iter()
            .map(|observation| SkillObservationView {
                kind: observation.kind_label().to_string(),
                source: match observation.source {
                    skills::ObservationSource::Agent => "agent",
                    skills::ObservationSource::Eval => "eval",
                    skills::ObservationSource::WorkflowRun => "workflow_run",
                    skills::ObservationSource::Cli => "cli",
                }
                .to_string(),
                id: observation.id,
                created_at: observation.created_at,
                summary: observation.summary,
                body: observation.body,
                node_kind: observation.node_kind,
                error: observation.error,
            })
            .collect(),
    )
}

/// The unified library view: every installed skill (all tiers) plus
/// every proposed-but-not-installed name, with the proposal pipeline
/// as one attribute among others. Pure [`SharedInfra`] reads — the
/// manager's always-fresh scans are the source of truth.
#[utoipa::path(
    get,
    path = "/api/v1/skills/library",
    tag = "skills",
    responses((status = 200, body = [SkillLibraryView]))
)]
pub(crate) async fn list_skill_library(
    State(state): State<GatewayState>,
) -> Json<Vec<SkillLibraryView>> {
    Json(skill_library(&state.infra))
}

/// Listing sort order: builtin → global → workspace → proposed, then
/// name — grouped so tier boundaries are visible while scanning.
fn tier_rank(tier: &str) -> u8 {
    match tier {
        "builtin" => 0,
        "global" => 1,
        "workspace" => 2,
        _ => 3,
    }
}

fn skill_library(infra: &SharedInfra) -> Vec<SkillLibraryView> {
    let manager = &infra.skills;
    let usage: std::collections::HashMap<String, skills::UsageRecord> =
        manager.usage_snapshot().into_iter().collect();
    let proposals = manager.proposals().list();

    let mut rows = Vec::new();
    for entry in manager.registry().list() {
        let name = &entry.meta.name;
        let proposal = proposals.iter().find(|p| &p.name == name);
        let record = usage.get(name);
        rows.push(SkillLibraryView {
            name: name.clone(),
            tier: entry.tier.as_str().to_string(),
            tags: entry.meta.tags.clone(),
            description: entry.meta.description.clone(),
            installed: true,
            proposal_status: proposal.map(|p| p.status.as_str().to_string()),
            proposal_update: proposal.is_some_and(|p| p.update),
            usage_gets: record.map(|r| r.gets).unwrap_or(0),
            usage_search_hits: record.map(|r| r.search_hits).unwrap_or(0),
            usage_runs: record.map(|r| r.runs).unwrap_or(0),
            usage_evals: record.map(|r| r.evals).unwrap_or(0),
            usage_last_used: record.map(|r| r.last_used).unwrap_or(0),
        });
    }
    // Proposed-but-not-installed names surface as their own rows — a
    // pending create has no installed skill to attach to yet.
    let installed: std::collections::HashSet<String> =
        rows.iter().map(|r| r.name.clone()).collect();
    for proposal in &proposals {
        if installed.contains(&proposal.name) || proposal.status != skills::ProposalStatus::Pending
        {
            continue;
        }
        // Metadata comes from the proposal's own SKILL.md draft; a
        // missing/unparsable draft falls back to the manifest.
        let meta = manager.proposals().meta(&proposal.name);
        let (tags, description) = match meta {
            Some(m) => (m.tags, m.description),
            None => (Vec::new(), proposal.rationale.clone()),
        };
        let record = usage.get(&proposal.name);
        rows.push(SkillLibraryView {
            name: proposal.name.clone(),
            tier: "proposed".to_string(),
            tags,
            description,
            installed: false,
            proposal_status: Some(proposal.status.as_str().to_string()),
            proposal_update: proposal.update,
            usage_gets: record.map(|r| r.gets).unwrap_or(0),
            usage_search_hits: record.map(|r| r.search_hits).unwrap_or(0),
            usage_runs: record.map(|r| r.runs).unwrap_or(0),
            usage_evals: record.map(|r| r.evals).unwrap_or(0),
            usage_last_used: record.map(|r| r.last_used).unwrap_or(0),
        });
    }
    rows.sort_by(|a, b| {
        tier_rank(&a.tier)
            .cmp(&tier_rank(&b.tier))
            .then(a.name.cmp(&b.name))
    });
    rows
}

/// One skill's full detail: metadata, usage telemetry, the pipeline
/// record (with evidence), and the complete SKILL.md body. Serves
/// installed skills from the registry and proposed ones from the
/// proposal area.
#[utoipa::path(
    get,
    path = "/api/v1/skills/library/{name}",
    tag = "skills",
    responses(
        (status = 200, body = SkillLibraryDetail),
        (status = 404, description = "No installed skill or proposal with that name")
    )
)]
pub(crate) async fn get_skill_library_detail(
    State(state): State<GatewayState>,
    Path(name): Path<String>,
) -> GatewayResult<Json<SkillLibraryDetail>> {
    let manager = &state.infra.skills;
    let usage = manager
        .usage_snapshot()
        .into_iter()
        .find(|(n, _)| n == &name)
        .map(|(_, r)| r);
    let proposal = manager.proposals().find(&name);
    let proposal_view = proposal.as_ref().map(|p| SkillProposalView {
        name: p.name.clone(),
        status: p.status.as_str().to_string(),
        authored_by: p.authored_by.clone(),
        update: p.update,
        rationale: p.rationale.clone(),
        cluster_hash: p.cluster_hash.clone(),
        observation_count: p.source_observation_ids.len(),
        created_at: p.created_at,
    });
    let evidence_count = proposal
        .map(|p| p.source_observation_ids.len())
        .unwrap_or(0);
    let usage_fields = |record: Option<skills::UsageRecord>| {
        (
            record.map(|r| r.gets).unwrap_or(0),
            record.map(|r| r.search_hits).unwrap_or(0),
            record.map(|r| r.runs).unwrap_or(0),
            record.map(|r| r.evals).unwrap_or(0),
            record.map(|r| r.last_used).unwrap_or(0),
        )
    };

    // Installed skills come from the registry (shadowing-resolved).
    if let Ok(Some(doc)) = manager.registry().get(&name) {
        let (gets, hits, runs, evals, last_used) = usage_fields(usage);
        return Ok(Json(SkillLibraryDetail {
            name: doc.meta.name.clone(),
            tier: doc.tier.as_str().to_string(),
            tags: doc.meta.tags.clone(),
            description: doc.meta.description.clone(),
            installed: true,
            body: doc.body,
            workflows: doc.workflows.clone(),
            evals: doc.evals.clone(),
            proposal: proposal_view,
            evidence_count,
            usage_gets: gets,
            usage_search_hits: hits,
            usage_runs: runs,
            usage_evals: evals,
            usage_last_used: last_used,
        }));
    }
    // Proposed-but-not-installed: read the draft from the proposal
    // area (the document approval would promote).
    if let Some((meta, body)) = manager.proposals().read_skill_md(&name) {
        let (gets, hits, runs, evals, last_used) = usage_fields(usage);
        return Ok(Json(SkillLibraryDetail {
            name: meta.name.clone(),
            tier: "proposed".to_string(),
            tags: meta.tags.clone(),
            description: meta.description.clone(),
            installed: false,
            body,
            workflows: Vec::new(),
            evals: Vec::new(),
            proposal: proposal_view,
            evidence_count,
            usage_gets: gets,
            usage_search_hits: hits,
            usage_runs: runs,
            usage_evals: evals,
            usage_last_used: last_used,
        }));
    }
    Err(GatewayError::Status(
        StatusCode::NOT_FOUND,
        format!("no skill or proposal named {name:?}"),
    ))
}

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
