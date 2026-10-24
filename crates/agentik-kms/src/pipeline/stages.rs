use std::sync::Arc;

use super::job::KnowledgeJob;
use super::stage::{KnowledgeStage, StageContext, StageOutcome};
use super::{phase2, session_extract};
use crate::pipeline::error::Result;

pub struct SessionExtractStage;

#[async_trait::async_trait]
impl KnowledgeStage for SessionExtractStage {
    fn name(&self) -> &'static str {
        "session_extract"
    }

    fn accepts(&self, job: &KnowledgeJob) -> bool {
        matches!(job, KnowledgeJob::ExtractAgentSessions { .. })
    }

    async fn execute(&self, context: StageContext, job: KnowledgeJob) -> Result<StageOutcome> {
        let KnowledgeJob::ExtractAgentSessions { agent_id } = job else {
            return Ok(StageOutcome::failed("job kind mismatch"));
        };
        if context.model.load_full().is_none() {
            return Ok(StageOutcome::retry(60, "no active model"));
        }
        let config = context.system.effective_config();
        session_extract::run(
            agent_id,
            &context.source,
            &context.system.store(),
            &context.model,
            &config,
        )
        .await?;
        Ok(StageOutcome::completed(vec![
            KnowledgeJob::ConsolidateKnowledge {
                trigger: "session_extract_completed".to_string(),
            },
        ]))
    }
}

pub struct Phase2Stage;

#[async_trait::async_trait]
impl KnowledgeStage for Phase2Stage {
    fn name(&self) -> &'static str {
        "phase2_consolidate_knowledge"
    }

    fn accepts(&self, job: &KnowledgeJob) -> bool {
        matches!(job, KnowledgeJob::ConsolidateKnowledge { .. })
    }

    async fn execute(&self, context: StageContext, job: KnowledgeJob) -> Result<StageOutcome> {
        let KnowledgeJob::ConsolidateKnowledge { trigger } = job else {
            return Ok(StageOutcome::failed("job kind mismatch"));
        };
        if context.model.load_full().is_none() {
            return Ok(StageOutcome::retry(60, "no active model"));
        }
        let _ = trigger;
        let config = context.system.effective_config();
        phase2::run(&context.system.store(), &context.model, &config).await?;
        Ok(StageOutcome::completed(Vec::new()))
    }
}

#[must_use]
pub fn default_stages() -> Vec<Arc<dyn KnowledgeStage>> {
    vec![Arc::new(SessionExtractStage), Arc::new(Phase2Stage)]
}
