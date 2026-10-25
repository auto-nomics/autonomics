use agentik_sdk::AgentMessageExt;
use agentik_sdk::types::messages::Message;
use uuid::Uuid;

use super::super::error::{Error, Result};
use crate::artifacts::{KnowledgeRepairOutput, hash_source, parse_json_object, redact_secrets};
use crate::knowledge::{Knowledge, KnowledgePath};
use crate::now_ms;
use crate::pipeline::job::KnowledgeJob;
use crate::pipeline::prompting::{build_knowledge_repair_prompt, message_text};
use crate::pipeline::stage::{KnowledgeStage, StageContext, StageOutcome};
use crate::store::KnowledgeDiagnosticFinding;
use crate::store::KnowledgeStore;

pub struct RepairKnowledgeNodeStage;

#[async_trait::async_trait]
impl KnowledgeStage for RepairKnowledgeNodeStage {
    fn name(&self) -> &'static str {
        "repair_knowledge_node"
    }

    fn accepts(&self, job: &KnowledgeJob) -> bool {
        matches!(job, KnowledgeJob::RepairKnowledgeNode { .. })
    }

    async fn execute(&self, context: StageContext, job: KnowledgeJob) -> Result<StageOutcome> {
        let KnowledgeJob::RepairKnowledgeNode { finding_id } = job else {
            return Ok(StageOutcome::failed("job kind mismatch"));
        };
        let store = context.system.store();
        let Some(finding) = store.get_diagnostic_finding(finding_id).await? else {
            return Ok(StageOutcome::completed(Vec::new()));
        };
        if finding.status != "open" {
            return Ok(StageOutcome::completed(Vec::new()));
        }
        let Some(target) = store.get_knowledge_by_path(&finding.target_path).await? else {
            store
                .set_diagnostic_finding_status(finding_id, "stale")
                .await?;
            return Ok(StageOutcome::completed(Vec::new()));
        };
        let children = store.list_child_knowledge(&finding.target_path).await?;
        if family_hash(&target, &children) != finding.family_hash {
            store
                .set_diagnostic_finding_status(finding_id, "stale")
                .await?;
            return Ok(StageOutcome::completed(Vec::new()));
        }
        let mut proposed_path_exists = false;
        if finding.action == "split" {
            for path in &finding.proposed_paths {
                if store.get_knowledge_by_path(path).await?.is_some() {
                    proposed_path_exists = true;
                    break;
                }
            }
        }
        if proposed_path_exists {
            store
                .set_diagnostic_finding_status(finding_id, "stale")
                .await?;
            return Ok(StageOutcome::completed(Vec::new()));
        }

        let Some(model) = context.model.load_full() else {
            return Ok(StageOutcome::retry(60, "no active model"));
        };
        let prompt = build_knowledge_repair_prompt(
            &target,
            &children,
            &finding.action,
            &finding.message,
            &finding.proposed_paths,
        );
        let response = model
            .request(vec![Message::user(prompt)], &[])
            .await
            .map_err(|e| Error::model_request("knowledge repair model request", e))?;
        let output = parse_json_object::<KnowledgeRepairOutput>(&message_text(&response))
            .map_err(|e| Error::model_output("knowledge repair output", e))?;
        let (nodes, archive_paths) = validate_repair(&target, &children, &finding, output)?;

        store.apply_knowledge_repair(nodes, archive_paths).await?;
        store
            .set_diagnostic_finding_status(finding_id, "resolved")
            .await?;
        Ok(StageOutcome::completed(Vec::new()))
    }
}

fn family_hash(target: &Knowledge, children: &[Knowledge]) -> String {
    #[derive(serde::Serialize)]
    struct Family<'a> {
        target: &'a Knowledge,
        children: &'a [Knowledge],
    }
    hash_source(&serde_json::to_string(&Family { target, children }).unwrap_or_default())
}

fn validate_repair(
    target: &Knowledge,
    children: &[Knowledge],
    finding: &KnowledgeDiagnosticFinding,
    output: KnowledgeRepairOutput,
) -> Result<(Vec<Knowledge>, Vec<String>)> {
    let mut nodes = Vec::new();
    for draft in output.nodes {
        let path = KnowledgePath::normalize(&draft.knowledge_path).ok_or_else(|| {
            Error::model_output(
                "knowledge repair validation",
                format!("invalid node path {}", draft.knowledge_path),
            )
        })?;
        let title = draft.title.trim();
        let body = redact_secrets(draft.body_md.trim());
        if title.is_empty() || body.is_empty() {
            return Err(Error::model_output(
                "knowledge repair validation",
                "repair nodes require a title and body",
            ));
        }
        nodes.push(Knowledge::leaf(
            path,
            title.to_string(),
            body,
            draft.confidence.clamp(0.0, 1.0),
            &format!("repair:{}", finding.id),
            now_ms(),
        ));
    }
    nodes.sort_by(|a, b| a.knowledge_path.cmp(&b.knowledge_path));

    let mut node_paths = nodes
        .iter()
        .map(|node| node.knowledge_path.as_str())
        .collect::<Vec<_>>();
    node_paths.sort();
    node_paths.dedup();
    if node_paths.len() != nodes.len() {
        return Err(Error::model_output(
            "knowledge repair validation",
            "repair output contains duplicate node paths",
        ));
    }

    let mut archive_paths = output.archive_paths;
    archive_paths.sort();
    archive_paths.dedup();
    if finding.action == "split" {
        let mut expected = finding.proposed_paths.clone();
        expected.push(finding.target_path.clone());
        expected.sort();
        if node_paths != expected || !archive_paths.is_empty() {
            return Err(Error::model_output(
                "knowledge repair validation",
                "split repair must emit the target and every proposed child without archives",
            ));
        }
        for proposed in &finding.proposed_paths {
            let path = KnowledgePath::normalize(proposed).ok_or_else(|| {
                Error::model_output(
                    "knowledge repair validation",
                    format!("invalid diagnosed path {proposed}"),
                )
            })?;
            if !is_descendant(&path, &target.knowledge_path) || store_path_exists(children, &path) {
                return Err(Error::model_output(
                    "knowledge repair validation",
                    "split paths must be new descendants of the target",
                ));
            }
        }
    } else if finding.action == "merge_children" {
        let mut expected_archives = finding.proposed_paths.clone();
        expected_archives.sort();
        if node_paths != vec![finding.target_path.clone()] || archive_paths != expected_archives {
            return Err(Error::model_output(
                "knowledge repair validation",
                "merge repair must emit the target and archive every proposed child",
            ));
        }
        let child_paths = children
            .iter()
            .map(|child| child.knowledge_path.as_str())
            .collect::<Vec<_>>();
        if archive_paths.iter().any(|path| !child_paths.contains(path)) {
            return Err(Error::model_output(
                "knowledge repair validation",
                "merge repair can only archive existing direct children",
            ));
        }
    } else {
        return Err(Error::model_output(
            "knowledge repair validation",
            format!("unsupported repair action {}", finding.action),
        ));
    }

    Ok((nodes, archive_paths))
}

fn store_path_exists(children: &[Knowledge], path: &KnowledgePath) -> bool {
    children
        .iter()
        .any(|child| child.knowledge_path.as_str() == path.as_str())
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

    use super::*;
    use crate::knowledge::KnowledgePath;
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
    async fn repair_worker_merges_fragmented_children() {
        let mut mock = MockApiClient::new();
        mock.expect_request()
            .times(1)
            .withf(|messages, _, _| {
                messages
                    .iter()
                    .any(|message| message.log_summary().contains("Repair one knowledge-node"))
            })
            .returning(|_, _, _| {
                Ok(Message::user(
                    r#"{"nodes":[{"knowledge_path":"/engineering/testing/merge","title":"Merged testing","body_md":"Use focused unit tests and integration checks together.","confidence":0.93}],"archive_paths":["/engineering/testing/merge/unit","/engineering/testing/merge/integration"]}"#,
                ))
            });
        let model = Arc::new(ArcSwapOption::from_pointee(Some(Model::with_client(
            model_info("repair-knowledge"),
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

        let target_path = KnowledgePath::normalize("/engineering/testing/merge").unwrap();
        let unit_path = KnowledgePath::normalize("/engineering/testing/merge/unit").unwrap();
        let integration_path =
            KnowledgePath::normalize("/engineering/testing/merge/integration").unwrap();
        store
            .create_knowledge(Knowledge::leaf(
                target_path.clone(),
                "Testing".into(),
                "Testing parent guidance.".into(),
                0.9,
                "seed",
                now_ms(),
            ))
            .await
            .unwrap();
        let unit = store
            .create_knowledge(Knowledge::leaf(
                unit_path.clone(),
                "Unit".into(),
                "Run focused unit tests.".into(),
                0.9,
                "seed",
                now_ms(),
            ))
            .await
            .unwrap();
        let integration = store
            .create_knowledge(Knowledge::leaf(
                integration_path.clone(),
                "Integration".into(),
                "Run component integration checks.".into(),
                0.9,
                "seed",
                now_ms(),
            ))
            .await
            .unwrap();
        let target = store
            .get_knowledge_by_path("/engineering/testing/merge")
            .await
            .unwrap()
            .unwrap();
        let mut children = vec![unit, integration];
        children.sort_by(|a, b| a.knowledge_path.cmp(&b.knowledge_path));
        let finding_id = Uuid::new_v4();
        store
            .insert_diagnostic_finding(KnowledgeDiagnosticFinding {
                id: finding_id,
                target_path: target_path.as_str(),
                action: "merge_children".to_string(),
                severity: "warning".to_string(),
                status: "open".to_string(),
                message: "Children cover one testing workflow.".to_string(),
                recommendation: "Merge the children.".to_string(),
                proposed_paths: vec![unit_path.as_str(), integration_path.as_str()],
                confidence: 0.9,
                family_hash: family_hash(&target, &children),
                created_at: now_ms(),
            })
            .await
            .unwrap();

        system
            .submit_event(KnowledgeEvent::KnowledgeRepairRequested { finding_id })
            .await
            .unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut finding = store
            .get_diagnostic_finding(finding_id)
            .await
            .unwrap()
            .unwrap();
        while finding.status != "resolved" {
            assert!(
                std::time::Instant::now() < deadline,
                "repair worker never resolved the merge finding: {:?}",
                (
                    finding.status.clone(),
                    system.repair_worker().map(|worker| worker.status())
                )
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
            finding = store
                .get_diagnostic_finding(finding_id)
                .await
                .unwrap()
                .unwrap();
        }

        let repaired = store
            .get_knowledge_by_path("/engineering/testing/merge")
            .await
            .unwrap()
            .unwrap();
        assert!(repaired.body_md.contains("unit tests"));
        assert!(repaired.body_md.contains("integration checks"));
        assert!(
            store
                .get_knowledge_by_path("/engineering/testing/merge/unit")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .get_knowledge_by_path("/engineering/testing/merge/integration")
                .await
                .unwrap()
                .is_none()
        );
        system.shutdown();
    }
}
