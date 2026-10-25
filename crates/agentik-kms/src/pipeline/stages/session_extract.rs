use crate::pipeline::error::Result;
use crate::pipeline::job::KnowledgeJob;
use crate::pipeline::session_extract;
use crate::pipeline::stage::{KnowledgeStage, StageContext, StageOutcome};

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
