use std::sync::Arc;

use super::job::KnowledgeJob;
use super::stage::{KnowledgeStage, StageContext, StageOutcome};
use super::{grounding, phase1, phase2};
use crate::pipeline::error::Result;

pub struct Phase1Stage;

#[async_trait::async_trait]
impl KnowledgeStage for Phase1Stage {
    fn name(&self) -> &'static str {
        "phase1_extract_agent_sessions"
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
        phase1::run(
            agent_id,
            &context.source,
            &context.system.store(),
            &context.model,
            &config,
        )
        .await?;
        Ok(StageOutcome::completed(vec![
            KnowledgeJob::ConsolidateKnowledge {
                trigger: "phase1_completed".to_string(),
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
        Ok(StageOutcome::completed(vec![
            KnowledgeJob::GroundObservations,
        ]))
    }
}

pub struct GroundingStage;

#[async_trait::async_trait]
impl KnowledgeStage for GroundingStage {
    fn name(&self) -> &'static str {
        "ground_observations"
    }

    fn accepts(&self, job: &KnowledgeJob) -> bool {
        matches!(job, KnowledgeJob::GroundObservations)
    }

    async fn execute(&self, context: StageContext, _job: KnowledgeJob) -> Result<StageOutcome> {
        let Some(grounding) = context.system.grounding() else {
            return Ok(StageOutcome::completed(Vec::new()));
        };
        grounding::run(&context.system, &grounding).await?;
        Ok(StageOutcome::completed(Vec::new()))
    }
}

#[must_use]
pub fn default_stages() -> Vec<Arc<dyn KnowledgeStage>> {
    vec![
        Arc::new(Phase1Stage),
        Arc::new(Phase2Stage),
        Arc::new(GroundingStage),
    ]
}
