//! Skill library handlers.

use axum::http::StatusCode;
use axum::{
    Json,
    extract::{Path, State},
};
use runtime::SharedInfra;

use super::error::{GatewayError, GatewayResult};
use super::state::GatewayState;
use crate::proto::*;

// ── skill library ─────────────────────────────────────────────────────

/// The unified library view: every installed skill (all tiers) with
/// its usage telemetry. Pure [`SharedInfra`] reads — the manager's
/// always-fresh scans are the source of truth.
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

/// Listing sort order: builtin → global → workspace, then name —
/// grouped so tier boundaries are visible while scanning.
fn tier_rank(tier: &str) -> u8 {
    match tier {
        "builtin" => 0,
        "global" => 1,
        _ => 2,
    }
}

fn skill_library(infra: &SharedInfra) -> Vec<SkillLibraryView> {
    let manager = &infra.skills;
    let usage: std::collections::HashMap<String, skills::UsageRecord> =
        manager.usage_snapshot().into_iter().collect();

    let mut rows = Vec::new();
    for entry in manager.registry().list() {
        let record = usage.get(&entry.meta.name);
        rows.push(SkillLibraryView {
            name: entry.meta.name.clone(),
            tier: entry.tier.as_str().to_string(),
            tags: entry.meta.tags.clone(),
            description: entry.meta.description.clone(),
            installed: true,
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

/// One skill's full detail: metadata, usage telemetry, and the
/// complete SKILL.md body, served from the registry.
#[utoipa::path(
    get,
    path = "/api/v1/skills/library/{name}",
    tag = "skills",
    responses(
        (status = 200, body = SkillLibraryDetail),
        (status = 404, description = "No installed skill with that name")
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
    let usage_fields = |record: Option<skills::UsageRecord>| {
        (
            record.map(|r| r.gets).unwrap_or(0),
            record.map(|r| r.search_hits).unwrap_or(0),
            record.map(|r| r.runs).unwrap_or(0),
            record.map(|r| r.evals).unwrap_or(0),
            record.map(|r| r.last_used).unwrap_or(0),
        )
    };

    let Some(doc) = manager.registry().get(&name).unwrap_or(None) else {
        return Err(GatewayError::Status(
            StatusCode::NOT_FOUND,
            format!("no skill named {name:?}"),
        ));
    };
    let (gets, hits, runs, evals, last_used) = usage_fields(usage);
    Ok(Json(SkillLibraryDetail {
        name: doc.meta.name.clone(),
        tier: doc.tier.as_str().to_string(),
        tags: doc.meta.tags.clone(),
        description: doc.meta.description.clone(),
        installed: true,
        body: doc.body,
        workflows: doc.workflows.clone(),
        evals: doc.evals.clone(),
        usage_gets: gets,
        usage_search_hits: hits,
        usage_runs: runs,
        usage_evals: evals,
        usage_last_used: last_used,
    }))
}
