use agentik_sdk::AgentMessageExt;
use agentik_sdk::types::messages::Message;
use uuid::Uuid;

use super::super::error::{Error, Result};
use crate::KnowledgeDiagnosticFinding;
use crate::artifacts::{KnowledgeNodeDiagnosisOutput, hash_source, parse_json_object};
use crate::knowledge::KnowledgePath;
use crate::now_ms;
use crate::pipeline::job::KnowledgeJob;
use crate::pipeline::prompting::{build_knowledge_diagnosis_prompt, message_text};
use crate::pipeline::stage::{KnowledgeStage, StageContext, StageOutcome};
use crate::store::KnowledgeStore;

const OVERSIZED_BODY_BYTES: usize = 6_000;
const OVERFRAGMENTED_CHILDREN: usize = 8;
const SMALL_CHILD_BODY_BYTES: usize = 500;
const SMALL_CHILD_RATIO: f64 = 0.6;

pub struct DiagnoseKnowledgeNodeStage;

#[async_trait::async_trait]
impl KnowledgeStage for DiagnoseKnowledgeNodeStage {
    fn name(&self) -> &'static str {
        "diagnose_knowledge_node"
    }

    fn accepts(&self, job: &KnowledgeJob) -> bool {
        matches!(job, KnowledgeJob::DiagnoseKnowledgeNode { .. })
    }

    async fn execute(&self, context: StageContext, job: KnowledgeJob) -> Result<StageOutcome> {
        let KnowledgeJob::DiagnoseKnowledgeNode {
            target_path,
            family_hash,
            trigger: _,
        } = job
        else {
            return Ok(StageOutcome::failed("job kind mismatch"));
        };
        let Some(path) = KnowledgePath::normalize(&target_path) else {
            return Ok(StageOutcome::failed(format!(
                "invalid diagnosis target path {target_path}"
            )));
        };
        let path = path.as_str();
        let store = context.system.store();
        let Some(target) = store.get_knowledge_by_path(&path).await? else {
            return Ok(StageOutcome::completed(Vec::new()));
        };
        let children = store.list_child_knowledge(&path).await?;
        let current_family_hash = family_hash_of(&target, &children);
        if family_hash
            .as_deref()
            .is_some_and(|expected| expected != current_family_hash)
        {
            return Ok(StageOutcome::completed(Vec::new()));
        }

        let suspected_oversized =
            !target.structural && target.body_md.len() >= OVERSIZED_BODY_BYTES;
        let suspected_overfragmented = suspected_overfragmented(&children);
        if !suspected_oversized && !suspected_overfragmented {
            return Ok(StageOutcome::completed(Vec::new()));
        }

        let Some(model) = context.model.load_full() else {
            return Ok(StageOutcome::retry(60, "no active model"));
        };
        let prompt = build_knowledge_diagnosis_prompt(
            &target,
            &children,
            suspected_oversized,
            suspected_overfragmented,
        );
        let response = model
            .request(vec![Message::user(prompt)], &[])
            .await
            .map_err(|e| Error::model_request("knowledge diagnosis model request", e))?;
        let output = parse_json_object::<KnowledgeNodeDiagnosisOutput>(&message_text(&response))
            .map_err(|e| Error::model_output("knowledge diagnosis output", e))?;
        let finding = validate_diagnosis(&target, &children, output, &current_family_hash)?;
        if let Some(finding) = finding {
            let finding = store.insert_diagnostic_finding(finding).await?;
            return Ok(StageOutcome::completed(vec![
                KnowledgeJob::RepairKnowledgeNode {
                    finding_id: finding.id,
                },
            ]));
        }
        Ok(StageOutcome::completed(Vec::new()))
    }
}

fn family_hash_of(
    target: &crate::knowledge::Knowledge,
    children: &[crate::knowledge::Knowledge],
) -> String {
    #[derive(serde::Serialize)]
    struct Family<'a> {
        target: &'a crate::knowledge::Knowledge,
        children: &'a [crate::knowledge::Knowledge],
    }
    hash_source(&serde_json::to_string(&Family { target, children }).unwrap_or_default())
}

fn suspected_overfragmented(children: &[crate::knowledge::Knowledge]) -> bool {
    children.len() >= OVERFRAGMENTED_CHILDREN
        && children
            .iter()
            .filter(|child| !child.structural && child.body_md.len() < SMALL_CHILD_BODY_BYTES)
            .count() as f64
            / children.len() as f64
            >= SMALL_CHILD_RATIO
}

fn validate_diagnosis(
    target: &crate::knowledge::Knowledge,
    children: &[crate::knowledge::Knowledge],
    output: KnowledgeNodeDiagnosisOutput,
    family_hash: &str,
) -> Result<Option<KnowledgeDiagnosticFinding>> {
    let action = output.action.trim();
    if action == "no_action" {
        return Ok(None);
    }
    if action != "split" && action != "merge_children" {
        return Err(Error::model_output(
            "knowledge diagnosis validation",
            format!("unknown diagnosis action {action}"),
        ));
    }

    let mut proposed_paths = Vec::new();
    for raw_path in output.proposed_paths {
        let path = KnowledgePath::normalize(&raw_path).ok_or_else(|| {
            Error::model_output(
                "knowledge diagnosis validation",
                format!("invalid proposed path {raw_path}"),
            )
        })?;
        proposed_paths.push(path);
    }
    proposed_paths.sort();
    proposed_paths.dedup();

    if action == "split" {
        if proposed_paths.len() < 2
            || proposed_paths
                .iter()
                .any(|path| !is_descendant(path, &target.knowledge_path))
            || children
                .iter()
                .any(|child| proposed_paths.contains(&child.knowledge_path))
        {
            return Err(Error::model_output(
                "knowledge diagnosis validation",
                "split must propose at least two descendants of the target",
            ));
        }
    } else {
        let child_paths = children
            .iter()
            .map(|child| child.knowledge_path.clone())
            .collect::<Vec<_>>();
        if proposed_paths.len() < 2
            || proposed_paths
                .iter()
                .any(|path| !child_paths.contains(path))
        {
            return Err(Error::model_output(
                "knowledge diagnosis validation",
                "merge_children must propose at least two existing direct children",
            ));
        }
    }

    let reason = output.reason.trim();
    if reason.is_empty() {
        return Err(Error::model_output(
            "knowledge diagnosis validation",
            "diagnosis reason must not be empty",
        ));
    }

    Ok(Some(KnowledgeDiagnosticFinding {
        id: Uuid::new_v4(),
        target_path: target.knowledge_path.as_str(),
        action: action.to_string(),
        severity: "warning".to_string(),
        status: "open".to_string(),
        message: reason.to_string(),
        recommendation: match action {
            "split" => "Split the target into the proposed child nodes.".to_string(),
            _ => "Merge the proposed child nodes.".to_string(),
        },
        proposed_paths: proposed_paths
            .into_iter()
            .map(|path| path.as_str())
            .collect(),
        confidence: output.confidence.clamp(0.0, 1.0),
        family_hash: family_hash.to_string(),
        created_at: now_ms(),
    }))
}

fn is_descendant(path: &KnowledgePath, ancestor: &KnowledgePath) -> bool {
    path.segments().len() > ancestor.segments().len()
        && path.segments()[..ancestor.segments().len()] == *ancestor.segments()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use agentik_sdk::model::{Model, ModelInfo};
    use agentik_sdk::provider::client::MockApiClient;
    use arc_swap::ArcSwapOption;
    use uuid::Uuid;

    use super::*;
    use crate::knowledge::Knowledge;
    use crate::{EmptyMemorySource, KmsSystem, KnowledgeEvent, TursoKmsStorage};

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

    #[tokio::test]
    async fn diagnoses_incremental_oversized_node() {
        let mut mock = MockApiClient::new();
        mock.expect_request()
            .times(1)
            .withf(|messages, _, _| {
                messages
                    .iter()
                    .any(|message| message.log_summary().contains("Diagnose one local"))
            })
            .returning(|_, _, _| {
                Ok(Message::user(
                    r#"{"action":"split","reason":"The node mixes unit and integration testing.","proposed_paths":["/engineering/testing/oversized/unit","/engineering/testing/oversized/integration"],"confidence":0.88}"#,
                ))
            });
        mock.expect_request()
            .times(1)
            .withf(|messages, _, _| {
                messages
                    .iter()
                    .any(|message| message.log_summary().contains("Repair one knowledge-node"))
            })
            .returning(|_, _, _| {
                Ok(Message::user(
                    r#"{"nodes":[{"knowledge_path":"/engineering/testing/oversized","title":"Oversized testing","body_md":"Testing overview.","confidence":0.9},{"knowledge_path":"/engineering/testing/oversized/unit","title":"Unit testing","body_md":"Use unit tests for focused behavior.","confidence":0.9},{"knowledge_path":"/engineering/testing/oversized/integration","title":"Integration testing","body_md":"Use integration tests for component boundaries.","confidence":0.9}],"archive_paths":[]}"#,
                ))
            });
        let model = Arc::new(ArcSwapOption::from_pointee(Some(Model::with_client(
            model_info("diagnose-knowledge"),
            mock,
        ))));
        let store = Arc::new(TursoKmsStorage::open_in_memory().await.unwrap());
        let system = KmsSystem::new(
            crate::MemoryConfig::new(),
            Arc::clone(&store),
            Arc::new(EmptyMemorySource),
        )
        .await;
        system.model_slot().store(model.load_full());
        let path = KnowledgePath::normalize("/engineering/testing/oversized").unwrap();
        store
            .create_knowledge(Knowledge::leaf(
                path,
                "Oversized testing".into(),
                "targeted testing guidance ".repeat(400),
                0.9,
                "test",
                now_ms(),
            ))
            .await
            .unwrap();

        system
            .submit_event(KnowledgeEvent::KnowledgeDiagnosisRequested {
                target_path: "/engineering/testing/oversized".into(),
                reason: "test".into(),
            })
            .await
            .unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut findings = store
            .list_diagnostic_findings("/engineering/testing/oversized", 10)
            .await
            .unwrap();
        while findings.is_empty() {
            assert!(
                std::time::Instant::now() < deadline,
                "diagnosis worker never persisted a finding"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
            findings = store
                .list_diagnostic_findings("/engineering/testing/oversized", 10)
                .await
                .unwrap();
        }

        assert_eq!(findings[0].action, "split");
        assert_eq!(findings[0].proposed_paths.len(), 2);
        let finding_id = findings[0].id;
        let mut finding = store
            .get_diagnostic_finding(finding_id)
            .await
            .unwrap()
            .unwrap();
        while finding.status != "resolved" {
            assert!(
                std::time::Instant::now() < deadline,
                "repair worker never resolved the finding"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
            finding = store
                .get_diagnostic_finding(finding_id)
                .await
                .unwrap()
                .unwrap();
        }
        assert!(
            store
                .get_knowledge_by_path("/engineering/testing/oversized/unit")
                .await
                .unwrap()
                .is_some()
        );
        system.shutdown();
    }
}
