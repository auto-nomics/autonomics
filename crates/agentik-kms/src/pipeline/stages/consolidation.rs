use crate::pipeline::error::Result;
use crate::pipeline::job::KnowledgeJob;
use crate::pipeline::phase2;
use crate::pipeline::stage::{KnowledgeStage, StageContext, StageOutcome};

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
