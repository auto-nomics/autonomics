use std::sync::Arc;

use agentik_sdk::AgentMessageExt;
use agentik_sdk::model::Model;
use agentik_sdk::types::messages::Message;
use arc_swap::ArcSwapOption;
use uuid::Uuid;

use super::MEMORY_SCOPE_ID;
use super::error::{Error, Result};
use super::prompting::{build_phase2_prompt, message_text};
use crate::artifacts::{
    KnowledgeDraft, MemoryConsolidation, hash_source, normalize_summary, parse_json_object,
    redact_secrets,
};
use crate::knowledge::{Knowledge, KnowledgePath};
use crate::store::{MemoryNote, MemoryStage1Record, MemoryStore, SemanticObservation};
use crate::{MemoryConfig, now_ms};

pub(super) async fn run(
    memory: &Arc<dyn MemoryStore>,
    model_handle: &Arc<ArcSwapOption<Model>>,
    config: &MemoryConfig,
) -> Result<()> {
    let mut rows = memory
        .list_stage1_outputs(MEMORY_SCOPE_ID, config.phase2_inputs)
        .await
        .map_err(|e| Error::store("select memory stage 1 outputs", e))?;
    rows.retain(|row| !row.raw_memory.is_empty() || !row.rollout_summary.is_empty());
    rows.sort_by_key(|row| row.session_id);

    let notes = memory
        .list_pending_notes(MEMORY_SCOPE_ID)
        .await
        .map_err(|e| Error::store("list memory notes", e))?;
    let existing_nodes = memory
        .list_knowledge(config.phase2_inputs)
        .await
        .map_err(|e| Error::store("list knowledge nodes", e))?;
    let existing_summary = memory
        .get_summary(MEMORY_SCOPE_ID)
        .await
        .map_err(|e| Error::store("load memory summary", e))?;
    let source_hash = phase2_source_hash(&rows, &notes);
    let lease_until = now_ms() + config.lease_seconds * 1000;
    if !memory
        .claim_phase2(MEMORY_SCOPE_ID, &source_hash, lease_until)
        .await
        .map_err(|e| Error::store("claim memory phase 2", e))?
    {
        return Ok(());
    }

    let Some(model) = model_handle.load_full() else {
        tracing::debug!("memory phase 2 skipped: no active model");
        return Ok(());
    };
    let prompt = build_phase2_prompt(
        &rows,
        &notes,
        &existing_nodes,
        existing_summary
            .as_ref()
            .map(|summary| summary.summary_md.as_str())
            .unwrap_or(""),
    );
    let consolidation = model
        .request(vec![Message::user(prompt)], &[])
        .await
        .map_err(|e| Error::model_request("memory consolidation model request", e))
        .and_then(|response| {
            parse_json_object::<MemoryConsolidation>(&message_text(&response))
                .map_err(|e| Error::model_output("memory consolidation output", e))
        });

    match consolidation {
        Ok(output) => {
            let now = now_ms();
            let nodes = normalized_nodes(output.nodes, &source_hash, now);
            let summary = if output.summary.trim().is_empty() {
                normalize_summary("No durable memories yet.")
            } else {
                normalize_summary(&output.summary)
            };
            let observations = build_observations(output.semantic_updates, &source_hash, now);
            let consumed_note_ids = notes.iter().map(|note| note.id).collect::<Vec<_>>();
            memory
                .complete_phase2(
                    MEMORY_SCOPE_ID,
                    &source_hash,
                    nodes,
                    &summary,
                    observations,
                    consumed_note_ids,
                )
                .await
                .map_err(|e| Error::store("complete memory phase 2", e))
        }
        Err(error) => memory
            .fail_phase2(MEMORY_SCOPE_ID, &source_hash, &error.to_string())
            .await
            .map_err(|e| Error::store("mark memory phase 2 failed", e)),
    }
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
            scope_id: MEMORY_SCOPE_ID,
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

fn phase2_source_hash(rows: &[MemoryStage1Record], notes: &[MemoryNote]) -> String {
    #[derive(serde::Serialize)]
    struct Input<'a> {
        rows: &'a [MemoryStage1Record],
        notes: &'a [MemoryNote],
    }
    let input = Input { rows, notes };
    hash_source(&serde_json::to_string(&input).unwrap_or_default())
}
