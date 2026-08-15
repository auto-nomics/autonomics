//! Ground persistent-memory semantic observations into KMS.

use std::collections::BTreeSet;
use std::sync::Arc;

use agentik_core::memory::{SemanticGrounding, SemanticGroundingOutcome, SemanticObservation};
use async_trait::async_trait;
use kms::storage::repo;
use kms::{
    Diagnostic, Entity, Index, KmsService, Knowledge, KnowledgeType, Language, Nomenclature,
    Severity, TargetType,
};
use uuid::Uuid;

#[derive(Clone)]
pub struct KmsMemoryGrounding {
    service: Arc<KmsService>,
}

impl KmsMemoryGrounding {
    #[must_use]
    pub fn new(service: Arc<KmsService>) -> Self {
        Self { service }
    }

    #[must_use]
    pub fn service(&self) -> Arc<KmsService> {
        Arc::clone(&self.service)
    }
}

struct GroundingDraft {
    indexes: Vec<Index>,
    knowledge: Vec<Knowledge>,
    entities: Vec<Entity>,
}

impl GroundingDraft {
    const fn new() -> Self {
        Self {
            indexes: Vec::new(),
            knowledge: Vec::new(),
            entities: Vec::new(),
        }
    }
}

#[async_trait]
impl SemanticGrounding for KmsMemoryGrounding {
    async fn ground(
        &self,
        observation: &SemanticObservation,
    ) -> Result<SemanticGroundingOutcome, String> {
        let before = self.service.diagnose().await?;
        let mut draft = GroundingDraft::new();

        match apply_observation(&self.service, observation, &mut draft).await {
            Ok(()) => {
                let after = self.service.diagnose().await?;
                let new_errors = new_error_diagnostics(&before, &after);
                if new_errors.is_empty() {
                    Ok(SemanticGroundingOutcome {
                        accepted: true,
                        reason: "observation grounded into KMS".to_string(),
                    })
                } else {
                    cleanup(&self.service, &mut draft).await;
                    Ok(SemanticGroundingOutcome {
                        accepted: false,
                        reason: format!(
                            "KMS diagnostics rejected the observation: {}",
                            new_errors.join("; ")
                        ),
                    })
                }
            }
            Err(error) => {
                cleanup(&self.service, &mut draft).await;
                Ok(SemanticGroundingOutcome {
                    accepted: false,
                    reason: format!("failed to ground observation: {error}"),
                })
            }
        }
    }
}

async fn apply_observation(
    service: &KmsService,
    observation: &SemanticObservation,
    draft: &mut GroundingDraft,
) -> Result<(), String> {
    let subject = clean_name(&observation.subject);
    let object = clean_name(&observation.object);
    if subject.is_empty() || object.is_empty() || observation.predicate.is_empty() {
        return Err("subject, predicate, and object must be non-empty".to_string());
    }

    let (subject_entity, subject_created) = ensure_entity(service, &subject).await?;
    if subject_created {
        draft.entities.push(subject_entity.clone());
    }
    let (object_entity, object_created) = if subject == object {
        (subject_entity.clone(), false)
    } else {
        ensure_entity(service, &object).await?
    };
    if object_created {
        draft.entities.push(object_entity.clone());
    }

    let title = knowledge_title(&subject, &observation.predicate, &object);
    let knowledge = match service.resolve_knowledge(&title).await {
        Ok(id) => service.get_knowledge(id).await?,
        Err(_) => {
            let knowledge = service
                .create_knowledge(
                    &title,
                    KnowledgeType::Relation,
                    vec![subject_entity.id, object_entity.id],
                    Some(knowledge_content(observation, &subject, &object)),
                )
                .await?;
            draft.knowledge.push(knowledge.clone());
            knowledge
        }
    };

    let root = service.find_root().await?;
    let (semantic_root, root_created) = ensure_group(service, root.id, "Semantic Memory").await?;
    if root_created {
        draft.indexes.push(semantic_root.clone());
    }
    let (subject_group, subject_created) =
        ensure_group(service, semantic_root.id, &subject).await?;
    if subject_created {
        draft.indexes.push(subject_group.clone());
    }

    let already_mounted = service
        .get_children(Some(subject_group.id))
        .await?
        .iter()
        .any(|child| {
            child.target_type == TargetType::Knowledge && child.target == Some(knowledge.id)
        });
    if !already_mounted {
        let mount = service
            .create_index(
                subject_group.id,
                Some(title.clone()),
                Some(knowledge.id),
                Some(TargetType::Knowledge),
            )
            .await?;
        draft.indexes.push(mount);
    }

    Ok(())
}

async fn ensure_entity(service: &KmsService, name: &str) -> Result<(Entity, bool), String> {
    service
        .create_entity(
            vec![Nomenclature {
                id: Uuid::new_v4(),
                lang: Language::ZH,
                full: name.to_string(),
                abbr: None,
            }],
            &format!("Semantic entity grounded from persistent memory: {name}"),
        )
        .await
}

async fn ensure_group(
    service: &KmsService,
    parent_id: Uuid,
    title: &str,
) -> Result<(Index, bool), String> {
    if let Some(id) = service.find_child_by_title(parent_id, title).await? {
        return Ok((service.get_index(id).await?, false));
    }
    let index = service
        .create_index(parent_id, Some(title.to_string()), None, None)
        .await?;
    Ok((index, true))
}

fn clean_name(raw: &str) -> String {
    let mut result = String::new();
    let mut last_space = false;
    for ch in raw.trim().chars() {
        if ch.is_whitespace() {
            if !last_space && !result.is_empty() {
                result.push(' ');
                last_space = true;
            }
        } else {
            result.push(ch);
            last_space = false;
        }
    }
    result.chars().take(160).collect()
}

fn knowledge_title(subject: &str, predicate: &str, object: &str) -> String {
    format!("{subject} · {predicate}: {object}")
}

fn knowledge_content(observation: &SemanticObservation, subject: &str, object: &str) -> String {
    format!(
        "[[{subject}]] --{}--> [[{object}]]\n\n{}\n\nconfidence: {:.2}\nsource: codex-memory observation {}\nsource_hash: {}\n",
        observation.predicate,
        observation.content.trim(),
        observation.confidence,
        observation.id,
        observation.source_hash
    )
}

fn diagnostic_fingerprints(diagnostics: &[Diagnostic]) -> BTreeSet<String> {
    diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .map(|diagnostic| {
            format!(
                "{}\u{1f}{}\u{1f}{}",
                diagnostic.code, diagnostic.location, diagnostic.message
            )
        })
        .collect()
}

fn new_error_diagnostics(before: &[Diagnostic], after: &[Diagnostic]) -> Vec<String> {
    let before = diagnostic_fingerprints(before);
    after
        .iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .map(|diagnostic| {
            format!(
                "{}\u{1f}{}\u{1f}{}",
                diagnostic.code, diagnostic.location, diagnostic.message
            )
        })
        .filter(|fingerprint| !before.contains(fingerprint))
        .collect()
}

async fn cleanup(service: &KmsService, draft: &mut GroundingDraft) {
    let conn = &*service.lock_conn().await;
    for index in draft.indexes.iter().rev() {
        if let Err(error) = repo::index_delete(conn, index.id).await {
            tracing::warn!(
                index_id = %index.id,
                error = %error,
                "failed to roll back KMS index"
            );
        }
        if let Some(parent) = index.parent_id {
            let _ = repo::index_reindex_positions(conn, Some(parent)).await;
        }
    }
    for knowledge in draft.knowledge.iter().rev() {
        if let Err(error) = repo::knowledge_delete(conn, knowledge.id).await {
            tracing::warn!(
                knowledge_id = %knowledge.id,
                error = %error,
                "failed to roll back KMS knowledge"
            );
        }
    }
    for entity in draft.entities.iter().rev() {
        if let Err(error) = repo::entity_delete(conn, entity.id).await {
            tracing::warn!(
                entity_id = %entity.id,
                error = %error,
                "failed to roll back KMS entity"
            );
        }
    }
    draft.indexes.clear();
    draft.knowledge.clear();
    draft.entities.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentik_core::memory::MEMORY_SCOPE_ID;
    use agentik_core::memory::MemoryBackend;
    use agentik_core::memory::MemoryConfig;
    use agentik_core::memory::run_memory_pipeline;
    use agentik_core::message_ext::AgentMessageExt;
    use agentik_core::storage::AgentStorage;
    use agentik_core::testing::dummy_model_info;
    use agentik_core::{TursoAgentStorage, memory::MemoryStore};
    use agentik_sdk::model::Model;
    use agentik_sdk::provider::client::MockApiClient;
    use agentik_sdk::types::messages::Message;
    use arc_swap::ArcSwapOption;

    fn observation() -> SemanticObservation {
        SemanticObservation {
            id: Uuid::new_v4(),
            scope_id: MEMORY_SCOPE_ID,
            subject: "Agent Runtime".to_string(),
            predicate: "uses".to_string(),
            object: "Turso Memory".to_string(),
            content: "Persistent memory uses Turso as its authority store.".to_string(),
            status: "candidate".to_string(),
            confidence: 0.92,
            source_hash: "source-hash".to_string(),
            created_at: 1_000,
            last_error: None,
        }
    }

    #[tokio::test]
    async fn grounds_observation_into_diagnostic_tree() {
        let service = Arc::new(
            KmsService::from_storage(kms::Storage::open_in_memory().await.unwrap())
                .await
                .unwrap(),
        );
        let grounding = KmsMemoryGrounding::new(Arc::clone(&service));
        let outcome = grounding.ground(&observation()).await.unwrap();

        assert!(outcome.accepted, "{reason}", reason = outcome.reason);
        let knowledge = service
            .resolve_knowledge("Agent Runtime · uses: Turso Memory")
            .await
            .unwrap();
        assert!(service.get_knowledge(knowledge).await.is_ok());
        assert!(
            service
                .diagnose()
                .await
                .unwrap()
                .iter()
                .all(|diagnostic| diagnostic.severity != Severity::Error)
        );
    }

    #[tokio::test]
    async fn duplicate_grounding_is_idempotent() {
        let service = Arc::new(
            KmsService::from_storage(kms::Storage::open_in_memory().await.unwrap())
                .await
                .unwrap(),
        );
        let grounding = KmsMemoryGrounding::new(Arc::clone(&service));
        let observation = observation();
        assert!(grounding.ground(&observation).await.unwrap().accepted);
        assert!(grounding.ground(&observation).await.unwrap().accepted);

        let root = service.find_root().await.unwrap();
        let children = service.get_children(Some(root.id)).await.unwrap();
        assert_eq!(children.len(), 1);
    }

    #[tokio::test]
    async fn memory_pipeline_promotes_observations_into_kms() {
        let mut mock = MockApiClient::new();
        mock.expect_request()
            .times(1)
            .withf(|messages, _, _| {
                messages
                    .iter()
                    .any(|message| message.log_summary().contains("Convert this"))
            })
            .returning(|_, _, _| {
                Ok(Message::assistant_text(
                    r#"{"raw_memory":"memory","rollout_summary":"summary","rollout_slug":"memory-kms"}"#,
                ))
            });
        mock.expect_request()
            .times(1)
            .withf(|messages, _, _| {
                messages
                    .iter()
                    .any(|message| message.log_summary().contains("Consolidate raw session"))
            })
            .returning(|_, _, _| {
                Ok(Message::assistant_text(
                    r##"{"entries":[],"summary":"v1\n\nmemory","semantic_updates":[{"subject":"Agent Runtime","predicate":"uses","object":"Turso Memory","content":"Memory is grounded through KMS diagnostics.","confidence":0.95}]}"##,
                ))
            });
        let model = Arc::new(ArcSwapOption::from_pointee(Some(Model::with_client(
            dummy_model_info("memory-kms-test"),
            mock,
        ))));

        let turso_store = Arc::new(TursoAgentStorage::open_in_memory().await.unwrap());
        let agent_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();
        turso_store
            .start_session(agent_id, session_id)
            .await
            .unwrap();
        turso_store
            .append_message(session_id, &Message::user("ground this session"))
            .await
            .unwrap();
        turso_store.end_session(session_id).await.unwrap();

        let kms = KmsService::from_storage(kms::Storage::open_in_memory().await.unwrap())
            .await
            .unwrap();
        let grounding = KmsMemoryGrounding::new(Arc::new(kms));
        let mut config = MemoryConfig::new();
        config.min_idle_hours = 0;
        let memory_store: Arc<dyn MemoryStore> = turso_store.clone();
        let semantic_grounding: Arc<dyn agentik_core::memory::SemanticGrounding> =
            Arc::new(grounding.clone());
        let memory = Arc::new(MemoryBackend::new(
            config,
            memory_store,
            Some(semantic_grounding),
        ));

        run_memory_pipeline(agent_id, turso_store, Arc::clone(&memory), model).await;

        let observations = memory
            .store
            .list_observations(MEMORY_SCOPE_ID, "accepted", 10)
            .await
            .unwrap();
        assert_eq!(observations.len(), 1);
        assert!(observations[0].last_error.is_none());
        assert!(
            memory
                .store
                .list_observations(MEMORY_SCOPE_ID, "candidate", 10)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            grounding
                .service()
                .resolve_knowledge("Agent Runtime · uses: Turso Memory")
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn kms_tables_are_initialized_in_shared_agent_database() {
        let agent_storage = TursoAgentStorage::open_in_memory().await.unwrap();
        let storage = kms::Storage::from_shared_connection(agent_storage.shared_connection())
            .await
            .unwrap();
        let service = KmsService::from_storage(storage).await.unwrap();
        service
            .create_entity(
                vec![Nomenclature {
                    id: Uuid::new_v4(),
                    lang: Language::ZH,
                    full: "Shared DB Entity".to_string(),
                    abbr: None,
                }],
                "Entity stored in agent.db",
            )
            .await
            .unwrap();

        let conn = agent_storage.shared_connection();
        let conn = conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN
                    ('entities', 'nomenclatures', 'knowledges', 'indexes')",
                turso::params_from_iter([] as [turso::Value; 0]),
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        let count = match row.get_value(0).unwrap() {
            turso::Value::Integer(count) => count,
            other => panic!("expected integer table count, got {other:?}"),
        };
        assert_eq!(count, 4);
    }

    #[tokio::test]
    async fn operation_failure_rolls_back_created_knowledge() {
        let service = Arc::new(
            KmsService::from_storage(kms::Storage::open_in_memory().await.unwrap())
                .await
                .unwrap(),
        );
        let root = service.find_root().await.unwrap();
        let semantic = service
            .create_index(root.id, Some("Semantic Memory".into()), None, None)
            .await
            .unwrap();
        let subject = service
            .create_index(semantic.id, Some("Agent Runtime".into()), None, None)
            .await
            .unwrap();
        service
            .create_index(
                subject.id,
                Some("Agent Runtime · uses: Turso Memory".into()),
                None,
                None,
            )
            .await
            .unwrap();

        let grounding = KmsMemoryGrounding::new(Arc::clone(&service));
        let outcome = grounding.ground(&observation()).await.unwrap();
        assert!(!outcome.accepted);
        assert!(
            service
                .resolve_knowledge("Agent Runtime · uses: Turso Memory")
                .await
                .is_err()
        );
    }
}
