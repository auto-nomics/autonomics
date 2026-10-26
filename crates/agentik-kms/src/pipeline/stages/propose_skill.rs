use crate::pipeline::error::Result;
use crate::pipeline::job::KnowledgeJob;
use crate::pipeline::stage::{KnowledgeStage, StageContext, StageOutcome};

pub struct ProposeSkillRevisionStage;

#[async_trait::async_trait]
impl KnowledgeStage for ProposeSkillRevisionStage {
    fn name(&self) -> &'static str {
        "propose_skill_revision"
    }

    fn accepts(&self, job: &KnowledgeJob) -> bool {
        matches!(job, KnowledgeJob::ProposeSkillRevision { .. })
    }

    async fn execute(&self, context: StageContext, job: KnowledgeJob) -> Result<StageOutcome> {
        let KnowledgeJob::ProposeSkillRevision { input } = job else {
            return Ok(StageOutcome::failed("job kind mismatch"));
        };
        let proposal = context.system.store().create_skill_proposal(input).await?;
        if proposal.is_none() {
            tracing::debug!("skill revision proposal already exists");
        }
        Ok(StageOutcome::completed(Vec::new()))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use agentik_sdk::AgentMessageExt;
    use agentik_sdk::model::{Model, ModelInfo};
    use agentik_sdk::provider::client::MockApiClient;
    use arc_swap::ArcSwapOption;
    use uuid::Uuid;

    use super::*;
    use crate::{
        EmptyMemorySource, KmsSystem, KnowledgeEvent, MemoryConfig, MemoryStore, TursoKmsStorage,
    };

    #[tokio::test]
    async fn skill_proposal_worker_persists_pending_revision() {
        let storage = Arc::new(TursoKmsStorage::open_in_memory().await.unwrap());
        let system =
            KmsSystem::new(MemoryConfig::new(), storage, Arc::new(EmptyMemorySource)).await;
        let input = crate::artifacts::KnowledgeSkillProposalInput {
            skill_name: "targeted-testing".to_string(),
            title: "Targeted testing".to_string(),
            body_md: "Run focused tests before broad checks.".to_string(),
            metadata_json: serde_json::json!({
                "description": "Run focused tests first.",
                "tags": ["testing"]
            }),
            node_path: "/procedures/targeted-testing".to_string(),
            rationale: "Repeated eval failures shared the same cause.".to_string(),
            source_evidence_ids: vec!["evidence-1".to_string()],
            artifacts: Vec::new(),
        };

        system
            .submit_event(KnowledgeEvent::SkillRevisionProposed {
                input: input.clone(),
            })
            .await
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let proposals = system.store().list_skill_proposals(10).await.unwrap();
            if let Some(proposal) = proposals.first() {
                assert_eq!(proposal.skill_name, "targeted-testing");
                assert_eq!(proposal.status, "pending");
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "skill proposal worker never persisted the revision"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        system.shutdown();
    }

    #[tokio::test]
    async fn phase2_repeated_failures_flow_into_skill_proposal_worker() {
        fn model_info(name: &str) -> ModelInfo {
            ModelInfo {
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

        let storage = Arc::new(TursoKmsStorage::open_in_memory().await.unwrap());
        storage
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
                artifacts: Vec::new(),
            })
            .await
            .unwrap();
        let evidence_input = crate::artifacts::KnowledgeEvidenceInput {
            source_kind: "skill_failure".to_string(),
            source_id: "failure-1".to_string(),
            evidence_kind: "eval_failure".to_string(),
            summary: "eval case smoke fails".to_string(),
            body: "The smoke eval failed after workflow execution.".to_string(),
            anchor_path: Some("/procedures/echo-flow".to_string()),
            error: Some("output rows did not match".to_string()),
        };
        let mut evidence = storage
            .upsert_evidence(evidence_input.clone())
            .await
            .unwrap();
        for _ in 1..3 {
            evidence = storage
                .upsert_evidence(evidence_input.clone())
                .await
                .unwrap();
        }
        assert_eq!(evidence.occurrence_count, 3);

        let evidence_id = evidence.id.clone();
        let mut mock = MockApiClient::new();
        mock.expect_request()
            .times(1)
            .withf(|messages, _, _| {
                messages
                    .iter()
                    .any(|message| message.log_summary().contains("Route each knowledge update"))
            })
            .returning(move |_, _, _| {
                Ok(AgentMessageExt::assistant_text(format!(
                    r#"{{"routes":[{{"update_id":"evidence:{evidence_id}","action":"rewrite_existing","anchor_path":"/procedures/echo-flow","target_path":"/procedures/echo-flow","reason":"preserve procedure knowledge","confidence":0.9}}]}}"#
                )))
            });
        mock.expect_request()
            .times(1)
            .withf(|messages, _, _| {
                messages
                    .iter()
                    .any(|message| message.log_summary().contains("Rewrite only the target nodes"))
            })
            .returning(|_, _, _| {
                Ok(AgentMessageExt::assistant_text(
                    r#"{"nodes":[{"knowledge_path":"/procedures/echo-flow","title":"Echo flow","body_md":"Run the echo workflow.","confidence":0.95}],"semantic_updates":[]}"#,
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
            .returning(|_, _, _| {
                Ok(AgentMessageExt::assistant_text(
                    r#"{"summary":"v1\n\nEcho procedure preserved."}"#,
                ))
            });
        let model = Arc::new(ArcSwapOption::from_pointee(Some(Model::with_client(
            model_info("skill-revision-pipeline"),
            mock,
        ))));

        let system =
            KmsSystem::new(MemoryConfig::new(), storage, Arc::new(EmptyMemorySource)).await;
        system.model_slot().store(model.load_full());
        system
            .submit_event(KnowledgeEvent::ConsolidationRequested {
                reason: "test skill failure evidence".to_string(),
            })
            .await
            .unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let proposals = system.store().list_skill_proposals(10).await.unwrap();
            if let Some(proposal) = proposals.first() {
                assert_eq!(proposal.status, "pending");
                assert_eq!(proposal.skill_name, "echo-flow");
                assert!(
                    proposal
                        .body_md
                        .contains("3 recorded failure occurrence(s)")
                );
                assert!(
                    system
                        .store()
                        .list_pending_evidence(10)
                        .await
                        .unwrap()
                        .is_empty()
                );
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "full skill-revision pipeline never produced a proposal"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        system.shutdown();
    }
}
