use crate::artifacts::redact_secrets;
use crate::pipeline::error::Result;
use crate::pipeline::job::KnowledgeJob;
use crate::pipeline::stage::{KnowledgeStage, StageContext, StageOutcome};
use crate::store::MemoryStore;

/// Distinct evidence items needed before an LLM consolidation is worth running.
const CONSOLIDATION_EVIDENCE_ITEMS: usize = 3;

pub struct IngestObservationStage;

#[async_trait::async_trait]
impl KnowledgeStage for IngestObservationStage {
    fn name(&self) -> &'static str {
        "ingest_observation"
    }

    fn accepts(&self, job: &KnowledgeJob) -> bool {
        matches!(job, KnowledgeJob::IngestObservation { .. })
    }

    async fn execute(&self, context: StageContext, job: KnowledgeJob) -> Result<StageOutcome> {
        let KnowledgeJob::IngestObservation { mut input } = job else {
            return Ok(StageOutcome::failed("job kind mismatch"));
        };
        if input.source_kind.trim().is_empty()
            || input.source_id.trim().is_empty()
            || input.evidence_kind.trim().is_empty()
            || input.summary.trim().is_empty()
            || input.body.trim().is_empty()
        {
            return Ok(StageOutcome::failed(
                "evidence source, kind, summary, and body must not be empty",
            ));
        }

        input.source_kind = input.source_kind.trim().to_lowercase();
        input.source_id = input.source_id.trim().to_string();
        input.evidence_kind = input.evidence_kind.trim().to_lowercase();
        input.summary = redact_secrets(input.summary.trim());
        input.body = redact_secrets(input.body.trim());
        input.error = input
            .error
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(redact_secrets);

        context.system.store().upsert_evidence(input).await?;
        let batch_limit = context.system.effective_config().phase2_inputs;
        let pending = context
            .system
            .store()
            .list_pending_evidence(batch_limit)
            .await?;
        if pending.len() >= CONSOLIDATION_EVIDENCE_ITEMS {
            return Ok(StageOutcome::completed(vec![
                KnowledgeJob::ConsolidateKnowledge {
                    trigger: "evidence_threshold_reached".to_string(),
                },
            ]));
        }
        Ok(StageOutcome::completed(Vec::new()))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use uuid::Uuid;

    use super::*;
    use crate::{EmptyMemorySource, KmsSystem, KnowledgeEvent, TursoKmsStorage};

    #[tokio::test]
    async fn observation_worker_deduplicates_and_redacts_evidence() {
        let store = Arc::new(TursoKmsStorage::open_in_memory().await.unwrap());
        let system = KmsSystem::new(
            crate::MemoryConfig::new(),
            Arc::clone(&store),
            Arc::new(EmptyMemorySource),
        )
        .await;
        let input = crate::artifacts::KnowledgeEvidenceInput {
            source_kind: "legacy_observation".to_string(),
            source_id: Uuid::new_v4().to_string(),
            evidence_kind: "recipe".to_string(),
            summary: "token=secret Use deterministic tests".to_string(),
            body: "Run cargo test before cargo check.".to_string(),
            anchor_path: None,
            error: None,
        };

        for expected in 1..=3_u64 {
            system
                .submit_event(KnowledgeEvent::ObservationRecorded {
                    input: input.clone(),
                })
                .await
                .unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            let mut evidence = store.list_pending_evidence(10).await.unwrap();
            while evidence.len() != 1 || evidence[0].occurrence_count < expected {
                assert!(
                    std::time::Instant::now() < deadline,
                    "observation worker never reached occurrence {expected}"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
                evidence = store.list_pending_evidence(10).await.unwrap();
            }
        }
        let evidence = store.list_pending_evidence(10).await.unwrap();
        assert_eq!(evidence[0].status, "pending");
        assert!(evidence[0].summary.contains("REDACTED"));
        system.shutdown();
    }
}
