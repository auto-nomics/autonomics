use std::collections::BTreeMap;
use std::sync::Arc;

use agentik_sdk::AgentMessageExt;
use agentik_sdk::model::Model;
use agentik_sdk::types::messages::Message;
use arc_swap::ArcSwapOption;
use uuid::Uuid;

use super::error::{Error, Result};
use super::prompting::{
    build_phase2_node_prompt, build_phase2_route_prompt, build_phase2_summary_prompt, message_text,
};
use crate::artifacts::{
    KnowledgeDraft, KnowledgeNodeConsolidation, KnowledgeRoutingPlan,
    KnowledgeSummaryConsolidation, KnowledgeUpdateInput, hash_source, normalize_summary,
    parse_json_object, redact_secrets,
};
use crate::knowledge::{Knowledge, KnowledgePath};
use crate::store::{MemoryNote, MemoryStage1Record, MemoryStore, SemanticObservation};
use crate::{MemoryConfig, now_ms};

pub(crate) mod routing;

use routing::{group_by_target, updates_from_inputs, validate_plan};

/// Run the deterministic Phase 2 workflow.
///
/// The model performs three bounded steps, while Rust owns validation,
/// grouping, preservation of untouched nodes, and persistence:
///
/// 1. Route each new update to an existing node or a controlled new path.
/// 2. Rewrite only the nodes selected by that routing plan.
/// 3. Summarize the resulting active knowledge tree.
pub(super) async fn run(
    memory: &Arc<dyn MemoryStore>,
    model_handle: &Arc<ArcSwapOption<Model>>,
    config: &MemoryConfig,
) -> Result<Vec<super::job::KnowledgeJob>> {
    let mut rows = memory
        .list_stage1_outputs(config.phase2_inputs)
        .await
        .map_err(|e| Error::store("select memory stage 1 outputs", e))?;
    rows.retain(|row| !row.raw_memory.is_empty() || !row.rollout_summary.is_empty());
    rows.sort_by_key(|row| row.session_id);

    let notes = memory
        .list_pending_notes()
        .await
        .map_err(|e| Error::store("list memory notes", e))?;
    let evidence = memory
        .list_pending_evidence(config.phase2_inputs)
        .await
        .map_err(|e| Error::store("list knowledge evidence", e))?;
    let existing_nodes = memory
        .list_knowledge(config.phase2_inputs)
        .await
        .map_err(|e| Error::store("list knowledge nodes", e))?;
    let existing_summary = memory
        .get_summary()
        .await
        .map_err(|e| Error::store("load memory summary", e))?;
    let source_hash = phase2_source_hash(&rows, &notes, &evidence);
    let lease_until = now_ms() + config.lease_seconds * 1000;
    if !memory
        .claim_phase2(&source_hash, lease_until)
        .await
        .map_err(|e| Error::store("claim memory phase 2", e))?
    {
        return Ok(Vec::new());
    }

    let Some(model) = model_handle.load_full() else {
        tracing::debug!("memory phase 2 skipped: no active model");
        return Ok(Vec::new());
    };
    let updates = updates_from_inputs(&rows, &notes, &evidence);
    let workflow = run_workflow(
        &model,
        &updates,
        &existing_nodes,
        existing_summary
            .as_ref()
            .map(|summary| summary.summary_md.as_str())
            .unwrap_or(""),
        &source_hash,
    )
    .await;

    match workflow {
        Ok((nodes, summary, observations, diagnosis_targets)) => {
            let consumed_note_ids = notes.iter().map(|note| note.id).collect::<Vec<_>>();
            memory
                .complete_phase2(
                    &source_hash,
                    nodes,
                    &summary,
                    observations,
                    consumed_note_ids,
                )
                .await
                .map_err(|e| Error::store("complete memory phase 2", e))?;
            let consumed_evidence_ids = evidence
                .iter()
                .map(|item| item.id.clone())
                .collect::<Vec<_>>();
            memory
                .mark_evidence_consumed(consumed_evidence_ids)
                .await
                .map_err(|e| Error::store("consume knowledge evidence", e))?;
            let mut next_jobs = skill_revision_jobs(memory, &evidence).await?;
            next_jobs.extend(diagnostic_jobs(memory, &diagnosis_targets).await?);
            Ok(next_jobs)
        }
        Err(error) => memory
            .fail_phase2(&source_hash, &error.to_string())
            .await
            .map_err(|e| Error::store("mark memory phase 2 failed", e))
            .map(|_| Vec::new()),
    }
}

async fn run_workflow(
    model: &Model,
    updates: &[KnowledgeUpdateInput],
    existing_nodes: &[Knowledge],
    existing_summary: &str,
    source_hash: &str,
) -> Result<(
    Vec<Knowledge>,
    String,
    Vec<SemanticObservation>,
    Vec<KnowledgePath>,
)> {
    // Step 1: classify where each update belongs in the current tree.
    let route_prompt = build_phase2_route_prompt(updates, existing_nodes);
    let route_plan = request_json::<KnowledgeRoutingPlan>(
        model,
        "knowledge routing model request",
        "knowledge routing output",
        route_prompt,
    )
    .await?;
    let routed = validate_plan(route_plan.routes, updates, existing_nodes)
        .map_err(|e| Error::model_output("knowledge routing validation", e))?;

    // Step 2: deterministically group routed updates, then rewrite each target.
    let groups = group_by_target(routed);
    let node_prompt = build_phase2_node_prompt(&groups, existing_nodes);
    let consolidation = request_json::<KnowledgeNodeConsolidation>(
        model,
        "knowledge node consolidation model request",
        "knowledge node consolidation output",
        node_prompt,
    )
    .await?;
    let now = now_ms();
    let target_paths = groups.keys().cloned().collect::<Vec<_>>();
    let mut nodes = normalized_nodes(consolidation.nodes, source_hash, now);
    let node_paths = nodes
        .iter()
        .map(|node| node.knowledge_path.as_str())
        .collect::<Vec<_>>();
    if node_paths != target_paths {
        return Err(Error::model_output(
            "knowledge node consolidation validation",
            format!(
                "model must emit exactly the routed targets; expected {target_paths:?}, got {node_paths:?}"
            ),
        ));
    }
    let diagnosis_targets = target_paths
        .iter()
        .filter_map(|path| KnowledgePath::normalize(path))
        .collect::<Vec<_>>();

    // complete_phase2 supersedes active nodes before upserting the result, so
    // untouched existing nodes are carried forward explicitly.
    let touched_paths = groups.keys().cloned().collect::<Vec<String>>();
    for node in existing_nodes {
        if !touched_paths.contains(&node.knowledge_path.as_str()) {
            nodes.push(node.clone());
        }
    }
    nodes.sort_by(|a, b| a.knowledge_path.cmp(&b.knowledge_path));
    let observations = build_observations(consolidation.semantic_updates, source_hash, now);

    // Step 3: summarize the complete active tree after node updates.
    let summary_prompt = build_phase2_summary_prompt(&nodes, existing_summary);
    let summary_output = request_json::<KnowledgeSummaryConsolidation>(
        model,
        "knowledge summary model request",
        "knowledge summary output",
        summary_prompt,
    )
    .await?;
    let summary = if summary_output.summary.trim().is_empty() {
        normalize_summary("No durable memories yet.")
    } else {
        normalize_summary(&summary_output.summary)
    };
    Ok((nodes, summary, observations, diagnosis_targets))
}

async fn skill_revision_jobs(
    memory: &Arc<dyn MemoryStore>,
    evidence: &[crate::store::KnowledgeEvidence],
) -> Result<Vec<super::job::KnowledgeJob>> {
    const REVISION_EVIDENCE_OCCURRENCES: u64 = 3;
    let mut groups: BTreeMap<String, Vec<&crate::store::KnowledgeEvidence>> = BTreeMap::new();
    for item in evidence {
        if item.source_kind == "skill_failure"
            && item
                .anchor_path
                .as_deref()
                .is_some_and(|path| path.starts_with("/procedures/"))
        {
            let skill = item
                .anchor_path
                .as_deref()
                .and_then(|path| path.rsplit('/').next())
                .unwrap_or_default()
                .to_string();
            if !skill.is_empty() {
                groups.entry(skill).or_default().push(item);
            }
        }
    }
    if groups.is_empty() {
        return Ok(Vec::new());
    }

    let versions = memory
        .list_skill_versions(10_000)
        .await
        .map_err(|e| Error::store("list skill versions for revision proposals", e))?;
    let mut jobs = Vec::new();
    for (skill, failures) in groups {
        let occurrences = failures
            .iter()
            .map(|item| item.occurrence_count)
            .sum::<u64>();
        if occurrences < REVISION_EVIDENCE_OCCURRENCES {
            continue;
        }
        let Some(version) = versions
            .iter()
            .find(|version| version.status == "active" && version.skill_name == skill)
        else {
            tracing::debug!(skill = %skill, "skill failure has no active KMS version");
            continue;
        };
        let artifacts = memory
            .list_skill_artifacts(version.id)
            .await
            .map_err(|e| Error::store("list skill artifacts for revision proposal", e))?;
        let metadata_json = serde_json::from_str::<serde_json::Value>(&version.metadata_json)
            .map_err(Error::serialization)?;

        let mut body_md = format!(
            "{}\n\n## Proposed revision evidence\n\nThis pending revision is based on \
             {} recorded failure occurrence(s). A human must review and approve it before \
             it becomes active.\n\n",
            version.body_md.trim_end(),
            occurrences
        );
        for failure in &failures {
            body_md.push_str(&format!(
                "### {} ({} occurrence(s))\n{}\n",
                failure.summary, failure.occurrence_count, failure.body
            ));
            if let Some(error) = failure.error.as_deref() {
                body_md.push_str(&format!("Error: {error}\n"));
            }
            body_md.push('\n');
        }

        jobs.push(super::job::KnowledgeJob::ProposeSkillRevision {
            input: crate::artifacts::KnowledgeSkillProposalInput {
                skill_name: version.skill_name.clone(),
                title: version.title.clone(),
                body_md,
                metadata_json,
                node_path: version.node_path.clone(),
                rationale: format!(
                    "{occurrences} recorded skill failure occurrence(s) require revision review"
                ),
                source_evidence_ids: failures
                    .iter()
                    .map(|item| item.id.clone())
                    .collect::<Vec<_>>(),
                artifacts: artifacts
                    .into_iter()
                    .map(|artifact| crate::artifacts::KnowledgeSkillArtifactInput {
                        relative_path: artifact.relative_path,
                        content: artifact.content,
                    })
                    .collect(),
            },
        });
    }
    Ok(jobs)
}

async fn diagnostic_jobs(
    memory: &Arc<dyn MemoryStore>,
    targets: &[KnowledgePath],
) -> Result<Vec<super::job::KnowledgeJob>> {
    let mut paths = Vec::new();
    for target in targets {
        if !paths.contains(target) {
            paths.push(target.clone());
        }
        if let Some(parent) = target.parent()
            && !paths.contains(&parent)
        {
            paths.push(parent);
        }
    }

    let mut jobs = Vec::new();
    for path in paths {
        let Some(node) = memory.get_knowledge_by_path(&path.as_str()).await? else {
            continue;
        };
        let children = memory.list_child_knowledge(&path.as_str()).await?;
        jobs.push(super::job::KnowledgeJob::DiagnoseKnowledgeNode {
            target_path: path.as_str(),
            family_hash: Some(family_hash(&node, &children)),
            trigger: "consolidation_completed".to_string(),
        });
    }
    Ok(jobs)
}

fn family_hash(target: &Knowledge, children: &[Knowledge]) -> String {
    #[derive(serde::Serialize)]
    struct Family<'a> {
        target: &'a Knowledge,
        children: &'a [Knowledge],
    }
    hash_source(&serde_json::to_string(&Family { target, children }).unwrap_or_default())
}

async fn request_json<T: for<'de> serde::Deserialize<'de>>(
    model: &Model,
    request_operation: &str,
    output_operation: &str,
    prompt: String,
) -> Result<T> {
    model
        .request(vec![Message::user(prompt)], &[])
        .await
        .map_err(|e| Error::model_request(request_operation, e))
        .and_then(|response| {
            parse_json_object::<T>(&message_text(&response))
                .map_err(|e| Error::model_output(output_operation, e))
        })
}

fn build_observations(
    drafts: Vec<crate::artifacts::SemanticObservationDraft>,
    source_hash: &str,
    now: i64,
) -> Vec<SemanticObservation> {
    drafts
        .into_iter()
        .map(|observation| SemanticObservation {
            id: Uuid::new_v4(),
            subject: redact_secrets(observation.subject.trim()),
            predicate: normalize_predicate(&observation.predicate),
            object: redact_secrets(observation.object.trim()),
            content: redact_secrets(observation.content.trim()),
            status: "candidate".to_string(),
            confidence: observation.confidence.clamp(0.0, 1.0),
            source_hash: source_hash.to_string(),
            created_at: now,
            last_error: None,
        })
        .filter(|observation| {
            !observation.subject.is_empty()
                && !observation.predicate.is_empty()
                && !observation.object.is_empty()
        })
        .collect()
}

fn normalized_nodes(drafts: Vec<KnowledgeDraft>, source_hash: &str, now: i64) -> Vec<Knowledge> {
    let mut by_path = std::collections::BTreeMap::new();
    for draft in drafts {
        let title = draft.title.trim().to_string();
        let body = redact_secrets(draft.body_md.trim());
        let Some(path) = KnowledgePath::normalize(&draft.knowledge_path) else {
            continue;
        };
        if title.is_empty() || body.is_empty() {
            continue;
        }
        let node = Knowledge::leaf(
            path.clone(),
            title,
            body,
            draft.confidence.clamp(0.0, 1.0),
            source_hash,
            now,
        );
        by_path.insert(path, node);
    }
    by_path.into_values().collect()
}

fn normalize_predicate(raw: &str) -> String {
    let mut result = String::new();
    for ch in raw.trim().chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            result.push(ch.to_ascii_lowercase());
        } else if !result.ends_with('_') && !result.is_empty() {
            result.push('_');
        }
    }
    result.trim_matches('_').to_string()
}

fn phase2_source_hash(
    rows: &[MemoryStage1Record],
    notes: &[MemoryNote],
    evidence: &[crate::store::KnowledgeEvidence],
) -> String {
    #[derive(serde::Serialize)]
    struct Input<'a> {
        rows: &'a [MemoryStage1Record],
        notes: &'a [MemoryNote],
        evidence: &'a [crate::store::KnowledgeEvidence],
    }
    let input = Input {
        rows,
        notes,
        evidence,
    };
    hash_source(&serde_json::to_string(&input).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{KnowledgeJob, TursoKmsStorage};
    use agentik_sdk::provider::client::MockApiClient;

    fn model_info(name: &str) -> agentik_sdk::model::ModelInfo {
        agentik_sdk::model::ModelInfo {
            model_name: name.into(),
            provider_id: Uuid::nil(),
            context_length: 4096,
            max_output_tokens: 1024,
            vision_ability: true,
            supports_function_calling: true,
            supports_streaming: true,
            supports_thinking: false,
            thinking_enabled: false,
            max_reasoning_effort: None,
            reasoning_effort: None,
            thinking_required: false,
            thinking_budget: None,
            input_token_price: 1.0,
            output_token_price: 2.0,
            custom: false,
        }
    }

    #[tokio::test]
    async fn workflow_rewrites_routed_node_and_preserves_untouched_nodes() {
        let mut mock = MockApiClient::new();
        mock.expect_request()
            .times(1)
            .withf(|messages, _, _| {
                messages
                    .iter()
                    .any(|message| message.log_summary().contains("Route each knowledge update"))
            })
            .returning(|_, _, _| {
                Ok(Message::user(
                    r#"{"routes":[{"update_id":"note:1","action":"rewrite_existing","anchor_path":"/engineering/testing","target_path":"/engineering/testing","reason":"testing guidance","confidence":0.95}]}"#,
                ))
            });
        mock.expect_request()
            .times(1)
            .withf(|messages, _, _| {
                messages
                    .iter()
                    .any(|message| message.log_summary().contains("Rewrite only the target nodes"))
            })
            .returning(|_, _, _| {
                Ok(Message::user(
                    r#"{"nodes":[{"knowledge_path":"/engineering/testing","title":"Testing","body_md":"Old guidance plus targeted tests.","confidence":0.95}],"semantic_updates":[]}"#,
                ))
            });
        mock.expect_request()
            .times(1)
            .withf(|messages, _, _| {
                messages.iter().any(|message| {
                    message
                        .log_summary()
                        .contains("Summarize the active knowledge tree")
                })
            })
            .returning(|_, _, _| Ok(Message::user(r#"{"summary":"v1\n\nTesting guidance."}"#)));
        let model = Model::with_client(model_info("phase2-workflow"), mock);
        let updates = vec![KnowledgeUpdateInput {
            id: "note:1".to_string(),
            kind: "memory_note".to_string(),
            content: "Prefer targeted tests.".to_string(),
        }];
        let testing_path = KnowledgePath::normalize("/engineering/testing").unwrap();
        let preferences_path = KnowledgePath::normalize("/user/preferences").unwrap();
        let existing_nodes = vec![
            Knowledge::leaf(
                testing_path,
                "Testing".into(),
                "Old guidance.".into(),
                0.8,
                "old",
                1,
            ),
            Knowledge::leaf(
                preferences_path,
                "Preferences".into(),
                "Keep concise answers.".into(),
                0.8,
                "old",
                1,
            ),
        ];

        let (nodes, summary, observations, _diagnosis_targets) =
            run_workflow(&model, &updates, &existing_nodes, "v1\n\nOld.", "new")
                .await
                .unwrap();

        assert_eq!(nodes.len(), 2);
        assert!(nodes.iter().any(
            |node| node.knowledge_path.as_str() == "/engineering/testing"
                && node.body_md.contains("targeted tests")
        ));
        assert!(
            nodes
                .iter()
                .any(|node| node.knowledge_path.as_str() == "/user/preferences"
                    && node.body_md.contains("concise answers"))
        );
        assert!(summary.starts_with("v1\n"));
        assert!(observations.is_empty());
    }

    #[tokio::test]
    async fn repeated_skill_failures_draft_a_pending_revision_job() {
        let storage = Arc::new(TursoKmsStorage::open_in_memory().await.unwrap());
        let memory: Arc<dyn MemoryStore> = storage;
        memory
            .import_skill_version(crate::artifacts::KnowledgeSkillVersionInput {
                skill_name: "echo-flow".to_string(),
                title: "Echo flow".to_string(),
                body_md: "Run the echo workflow.".to_string(),
                metadata_json: serde_json::json!({
                    "description": "Run an echo workflow.",
                    "tags": ["test"]
                }),
                node_path: "/procedures/echo-flow".to_string(),
                source_tier: "workspace".to_string(),
                source_path: None,
                artifacts: vec![crate::artifacts::KnowledgeSkillArtifactInput {
                    relative_path: "workflow/main.toml".to_string(),
                    content: "description = 'Echo'\n".to_string(),
                }],
            })
            .await
            .unwrap();
        let evidence = [crate::store::KnowledgeEvidence {
            id: "evidence-1".to_string(),
            source_kind: "skill_failure".to_string(),
            source_id: "failure-1".to_string(),
            evidence_kind: "eval_failure".to_string(),
            summary: "eval case smoke fails".to_string(),
            body: "The smoke eval failed after workflow execution.".to_string(),
            anchor_path: Some("/procedures/echo-flow".to_string()),
            error: Some("output rows did not match".to_string()),
            content_hash: "failure-hash".to_string(),
            status: "pending".to_string(),
            occurrence_count: 3,
            created_at: 1,
            last_seen_at: 3,
        }];

        let jobs = skill_revision_jobs(&memory, &evidence).await.unwrap();
        assert_eq!(jobs.len(), 1);
        let KnowledgeJob::ProposeSkillRevision { input } = &jobs[0] else {
            panic!("expected a skill revision proposal job");
        };
        assert_eq!(input.skill_name, "echo-flow");
        assert!(input.body_md.contains("Run the echo workflow."));
        assert!(input.body_md.contains("3 recorded failure occurrence(s)"));
        assert_eq!(input.source_evidence_ids, vec!["evidence-1".to_string()]);
        assert_eq!(input.artifacts.len(), 1);
    }
}
