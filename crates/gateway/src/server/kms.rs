//! Frontend observability for the resident knowledge workers.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use uuid::Uuid;

use super::error::{GatewayError, GatewayResult};
use super::state::GatewayState;
use crate::proto::{
    KmsEvidenceView, KmsKnowledgeNodeView, KmsSkillArtifactView, KmsSkillDetailView,
    KmsSkillLibraryView, KmsSkillProposalDecision, KmsSkillProposalView,
    KmsSkillVersionSummaryView, KmsWorkerStatusView,
};

#[utoipa::path(
    get,
    path = "/api/v1/kms/workers",
    tag = "kms",
    responses((status = 200, body = [KmsWorkerStatusView]))
)]
pub(crate) async fn get_worker_statuses(
    State(state): State<GatewayState>,
) -> Json<Vec<KmsWorkerStatusView>> {
    Json(
        state
            .infra
            .kms
            .worker_statuses()
            .into_iter()
            .map(Into::into)
            .collect(),
    )
}

#[utoipa::path(
    get,
    path = "/api/v1/kms/evidence",
    tag = "kms",
    responses((status = 200, body = [KmsEvidenceView]))
)]
pub(crate) async fn list_evidence(
    State(state): State<GatewayState>,
) -> GatewayResult<Json<Vec<KmsEvidenceView>>> {
    let evidence = state
        .infra
        .kms
        .store()
        .list_knowledge_evidence(500)
        .await
        .map_err(|e| GatewayError::Status(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(
        evidence
            .into_iter()
            .map(|item| KmsEvidenceView {
                id: item.id,
                source_kind: item.source_kind,
                source_id: item.source_id,
                evidence_kind: item.evidence_kind,
                summary: item.summary,
                body: item.body,
                anchor_path: item.anchor_path,
                error: item.error,
                status: item.status,
                occurrence_count: item.occurrence_count,
                created_at: item.created_at,
                last_seen_at: item.last_seen_at,
            })
            .collect(),
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/kms/knowledge",
    tag = "kms",
    responses((status = 200, body = [KmsKnowledgeNodeView]))
)]
pub(crate) async fn list_knowledge(
    State(state): State<GatewayState>,
) -> GatewayResult<Json<Vec<KmsKnowledgeNodeView>>> {
    use agentik_core::memory::KnowledgeStore;

    let nodes = state
        .infra
        .kms
        .store()
        .list_knowledge(10_000)
        .await
        .map_err(|e| GatewayError::Status(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(nodes.into_iter().map(knowledge_view).collect()))
}

#[derive(serde::Deserialize)]
struct KmsSkillMetadata {
    #[serde(default)]
    description: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    tier: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/kms/skills",
    tag = "kms",
    responses((status = 200, body = [KmsSkillLibraryView]))
)]
pub(crate) async fn list_skills(
    State(state): State<GatewayState>,
) -> GatewayResult<Json<Vec<KmsSkillLibraryView>>> {
    let store = state.infra.kms.store();
    let versions = store
        .list_skill_versions(10_000)
        .await
        .map_err(|e| GatewayError::Status(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let mut rows = Vec::new();
    for version in versions
        .into_iter()
        .filter(|version| version.status == "active")
    {
        let metadata = skill_metadata(&version.metadata_json)?;
        let artifacts = store
            .list_skill_artifacts(version.id)
            .await
            .map_err(|e| GatewayError::Status(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        rows.push(KmsSkillLibraryView {
            id: version.id,
            skill_name: version.skill_name,
            node_path: version.node_path,
            version: version.version,
            title: version.title,
            description: metadata.description,
            tags: metadata.tags,
            tier: metadata.tier.unwrap_or_else(|| version.source_tier.clone()),
            source_tier: version.source_tier,
            created_at: version.created_at,
            artifact_paths: artifacts
                .into_iter()
                .map(|artifact| artifact.relative_path)
                .collect(),
        });
    }
    rows.sort_by(|a, b| a.skill_name.cmp(&b.skill_name));
    Ok(Json(rows))
}

#[utoipa::path(
    get,
    path = "/api/v1/kms/skills/{name}",
    tag = "kms",
    responses(
        (status = 200, body = KmsSkillDetailView),
        (status = 404, description = "No active KMS skill with that name")
    )
)]
pub(crate) async fn get_skill_detail(
    State(state): State<GatewayState>,
    Path(name): Path<String>,
) -> GatewayResult<Json<KmsSkillDetailView>> {
    let store = state.infra.kms.store();
    let mut versions = store
        .list_skill_versions(10_000)
        .await
        .map_err(|e| GatewayError::Status(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    versions.retain(|version| version.skill_name == name);
    versions.sort_by(|a, b| b.version.cmp(&a.version));
    let Some(active) = versions.iter().find(|version| version.status == "active") else {
        return Err(GatewayError::Status(
            StatusCode::NOT_FOUND,
            format!("no active KMS skill named {name:?}"),
        ));
    };
    let metadata = skill_metadata(&active.metadata_json)?;
    let artifacts = store
        .list_skill_artifacts(active.id)
        .await
        .map_err(|e| GatewayError::Status(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .into_iter()
        .map(|artifact| KmsSkillArtifactView {
            relative_path: artifact.relative_path,
            content: artifact.content,
        })
        .collect::<Vec<_>>();
    let history = versions
        .iter()
        .map(|version| KmsSkillVersionSummaryView {
            id: version.id,
            version: version.version,
            status: version.status.clone(),
            title: version.title.clone(),
            content_hash: version.content_hash.clone(),
            created_at: version.created_at,
        })
        .collect();

    Ok(Json(KmsSkillDetailView {
        id: active.id,
        skill_name: active.skill_name.clone(),
        node_path: active.node_path.clone(),
        version: active.version,
        title: active.title.clone(),
        description: metadata.description,
        tags: metadata.tags,
        tier: metadata.tier.unwrap_or_else(|| active.source_tier.clone()),
        source_tier: active.source_tier.clone(),
        body_md: active.body_md.clone(),
        created_at: active.created_at,
        artifacts,
        history,
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/kms/skill-proposals",
    tag = "kms",
    responses((status = 200, body = [KmsSkillProposalView]))
)]
pub(crate) async fn list_skill_proposals(
    State(state): State<GatewayState>,
) -> GatewayResult<Json<Vec<KmsSkillProposalView>>> {
    let proposals = state
        .infra
        .kms
        .store()
        .list_skill_proposals(500)
        .await
        .map_err(|e| GatewayError::Status(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(proposals.into_iter().map(proposal_view).collect()))
}

#[utoipa::path(
    post,
    path = "/api/v1/kms/skill-proposals/{id}/approve",
    tag = "kms",
    responses(
        (status = 200, body = KmsSkillProposalDecision),
        (status = 404, description = "Proposal not found"),
        (status = 409, description = "Proposal is not pending")
    )
)]
pub(crate) async fn approve_skill_proposal(
    State(state): State<GatewayState>,
    Path(id): Path<Uuid>,
) -> GatewayResult<Json<KmsSkillProposalDecision>> {
    let proposal = load_proposal(&state, id).await?;
    if proposal.status != "pending" {
        return Err(GatewayError::Status(
            StatusCode::CONFLICT,
            format!(
                "skill proposal {} is {}, not pending",
                proposal.id, proposal.status
            ),
        ));
    }
    let Some(version) = state
        .infra
        .kms
        .store()
        .approve_skill_proposal(id)
        .await
        .map_err(|e| GatewayError::Status(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    else {
        return Err(GatewayError::Status(
            StatusCode::CONFLICT,
            format!("skill proposal {id} is no longer pending"),
        ));
    };
    Ok(Json(KmsSkillProposalDecision {
        id,
        skill_name: version.skill_name,
        status: "approved".to_string(),
        skill_version_id: Some(version.id),
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/kms/skill-proposals/{id}/reject",
    tag = "kms",
    responses(
        (status = 200, body = KmsSkillProposalDecision),
        (status = 404, description = "Proposal not found"),
        (status = 409, description = "Proposal is not pending")
    )
)]
pub(crate) async fn reject_skill_proposal(
    State(state): State<GatewayState>,
    Path(id): Path<Uuid>,
) -> GatewayResult<Json<KmsSkillProposalDecision>> {
    let proposal = load_proposal(&state, id).await?;
    if proposal.status != "pending" {
        return Err(GatewayError::Status(
            StatusCode::CONFLICT,
            format!(
                "skill proposal {} is {}, not pending",
                proposal.id, proposal.status
            ),
        ));
    }
    let rejected = state
        .infra
        .kms
        .store()
        .reject_skill_proposal(id)
        .await
        .map_err(|e| GatewayError::Status(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if !rejected {
        return Err(GatewayError::Status(
            StatusCode::CONFLICT,
            format!("skill proposal {id} is no longer pending"),
        ));
    }
    Ok(Json(KmsSkillProposalDecision {
        id,
        skill_name: proposal.skill_name,
        status: "rejected".to_string(),
        skill_version_id: None,
    }))
}

async fn load_proposal(
    state: &GatewayState,
    id: Uuid,
) -> GatewayResult<agentik_core::memory::SkillProposal> {
    state
        .infra
        .kms
        .store()
        .list_skill_proposals(500)
        .await
        .map_err(|e| GatewayError::Status(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .into_iter()
        .find(|proposal| proposal.id == id)
        .ok_or_else(|| {
            GatewayError::Status(StatusCode::NOT_FOUND, format!("no skill proposal {id}"))
        })
}

fn skill_metadata(raw: &str) -> GatewayResult<KmsSkillMetadata> {
    serde_json::from_str(raw).map_err(|e| {
        GatewayError::Status(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("parse KMS skill metadata: {e}"),
        )
    })
}

fn knowledge_view(node: agentik_core::memory::Knowledge) -> KmsKnowledgeNodeView {
    KmsKnowledgeNodeView {
        id: node.id,
        parent_id: node.parent_id,
        path: node.knowledge_path.as_str(),
        title: node.title,
        body_md: node.body_md,
        status: node.status,
        confidence: node.confidence,
        source_hash: node.source_hash,
        structural: node.structural,
        created_at: node.created_at,
        updated_at: node.updated_at,
    }
}

fn proposal_view(proposal: agentik_core::memory::SkillProposal) -> KmsSkillProposalView {
    KmsSkillProposalView {
        id: proposal.id,
        skill_name: proposal.skill_name,
        node_path: proposal.node_path,
        status: proposal.status,
        title: proposal.title,
        body_md: proposal.body_md,
        rationale: proposal.rationale,
        source_evidence_ids: proposal.source_evidence_ids,
        artifacts: proposal
            .artifacts
            .into_iter()
            .map(|artifact| KmsSkillArtifactView {
                relative_path: artifact.relative_path,
                content: artifact.content,
            })
            .collect(),
        content_hash: proposal.content_hash,
        approved_skill_version_id: proposal.approved_skill_version_id,
        created_at: proposal.created_at,
        updated_at: proposal.updated_at,
    }
}
