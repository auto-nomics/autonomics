use std::sync::Arc;

use agentik_sdk::AgentMessageExt;
use agentik_sdk::model::Model;
use agentik_sdk::types::messages::Message;
use arc_swap::ArcSwapOption;
use chrono::Utc;
use uuid::Uuid;

use super::MEMORY_SCOPE_ID;
use super::error::{Error, Result};
use super::prompting::{build_phase1_prompt, parse_extraction};
use crate::artifacts::{hash_source, normalize_slug, redact_secrets, render_transcript};
use crate::store::{MemoryStage1Record, MemoryStore};
use crate::{MemoryConfig, MemorySource, now_ms};

pub(super) async fn run(
    agent_id: Uuid,
    storage: &Arc<dyn MemorySource>,
    memory: &Arc<dyn MemoryStore>,
    model_handle: &Arc<ArcSwapOption<Model>>,
    config: &MemoryConfig,
) -> Result<()> {
    let records = storage
        .list_session_records(agent_id)
        .await
        .map_err(|e| Error::source("list sessions for memory phase 1", e))?;
    let now = Utc::now().timestamp_millis();
    let cutoff = now.saturating_sub(config.max_age_days * 24 * 60 * 60 * 1000);
    let mut eligible: Vec<_> = records
        .into_iter()
        .filter(|record| {
            let idle_at = record.ended_at.unwrap_or(record.started_at);
            let idle_for = now.saturating_sub(idle_at);
            idle_at >= cutoff && idle_for >= config.min_idle_hours * 60 * 60 * 1000
        })
        .collect();
    eligible.sort_by(|a, b| b.ended_at.cmp(&a.ended_at));
    eligible.truncate(config.max_source_sessions);

    for record in eligible {
        let messages = storage
            .get_messages_since_for_session(record.session_id, 0)
            .await
            .map_err(|e| Error::source(format!("load session {}", record.session_id), e))?;
        if messages.is_empty() {
            continue;
        }
        let serialized = serde_json::to_string(&messages).map_err(Error::serialization)?;
        let source_hash = hash_source(&serialized);
        let previous = memory
            .get_stage1_output(MEMORY_SCOPE_ID, record.session_id)
            .await
            .map_err(|e| Error::store("load memory stage 1", e))?;
        if previous.is_some_and(|row| {
            row.source_hash == source_hash
                && (row.status == "succeeded"
                    || (row.status == "running" && row.lease_until > now_ms()))
        }) {
            continue;
        }
        let lease_until = now_ms() + config.lease_seconds * 1000;
        if !memory
            .claim_stage1(
                MEMORY_SCOPE_ID,
                record.session_id,
                &source_hash,
                lease_until,
            )
            .await
            .map_err(|e| Error::store("claim memory stage 1", e))?
        {
            continue;
        }

        let Some(model) = model_handle.load_full() else {
            tracing::debug!("memory phase 1 skipped: no active model");
            return Ok(());
        };
        let transcript = render_transcript(&messages, transcript_limit(model.as_ref()));
        let prompt = build_phase1_prompt(record.session_id, record.title.as_deref(), &transcript);
        let extraction = match model.request(vec![Message::user(prompt)], &[]).await {
            Ok(response) => parse_extraction(&response),
            Err(error) => Err(Error::model_request("model request", error)),
        };
        match extraction {
            Ok(extraction) => {
                let output = MemoryStage1Record {
                    session_id: record.session_id,
                    source_hash,
                    raw_memory: redact_secrets(extraction.raw_memory.trim()),
                    rollout_summary: redact_secrets(extraction.rollout_summary.trim()),
                    rollout_slug: extraction
                        .rollout_slug
                        .as_deref()
                        .map(|slug| normalize_slug(Some(slug), record.session_id)),
                    status: "succeeded".to_string(),
                    generated_at: now_ms(),
                    lease_until: 0,
                };
                memory
                    .complete_stage1(MEMORY_SCOPE_ID, output)
                    .await
                    .map_err(|e| Error::store("save memory stage 1", e))?;
            }
            Err(error) => {
                memory
                    .fail_stage1(
                        MEMORY_SCOPE_ID,
                        record.session_id,
                        &source_hash,
                        &error.to_string(),
                    )
                    .await
                    .map_err(|e| Error::store("mark memory stage 1 failed", e))?;
            }
        }
    }
    Ok(())
}

fn transcript_limit(model: &Model) -> usize {
    let configured = model.model_info.context_length as usize;
    let window = if configured == 0 {
        150_000
    } else {
        configured * 4 * 7 / 10
    };
    window.min(150_000).max(20_000)
}
