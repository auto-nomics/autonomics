//! Compatibility handlers for the filesystem skill library.
//!
//! Durable skill evolution and governance are exposed under `/kms`.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use runtime::SharedInfra;

use super::error::{GatewayError, GatewayResult};
use super::state::GatewayState;
use crate::proto::*;

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

    let installed: std::collections::HashSet<String> =
        rows.iter().map(|r| r.name.clone()).collect();
    for proposal in &proposals {
        if installed.contains(&proposal.name) || proposal.status != skills::ProposalStatus::Pending
        {
            continue;
        }
        let meta = manager.proposals().meta(&proposal.name);
        let (tags, description) = match meta {
            Some(meta) => (meta.tags, meta.description),
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
        .map(|(_, record)| record);
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
