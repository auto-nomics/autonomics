use std::sync::Arc;

use async_trait::async_trait;
use uuid::Uuid;

use crate::storage::{AgentStorage, StorageError};
use agentik_kms::{Error, MemorySessionRecord, MemorySource, TursoKmsStorage};

pub(crate) fn storage_error(error: StorageError) -> Error {
    Error::Source(error.to_string())
}

impl From<StorageError> for Error {
    fn from(error: StorageError) -> Self {
        storage_error(error)
    }
}

/// Core-owned bridge between the generic memory pipeline and persisted rollouts.
pub struct AgentStorageMemorySource(pub Arc<dyn AgentStorage>);

#[async_trait]
impl MemorySource for AgentStorageMemorySource {
    async fn list_session_records(
        &self,
        agent_id: Uuid,
    ) -> Result<Vec<MemorySessionRecord>, Error> {
        self.0
            .list_session_records(agent_id)
            .await
            .map(|records| {
                records
                    .into_iter()
                    .map(|record| MemorySessionRecord {
                        session_id: record.session_id,
                        title: record.title,
                        started_at: record.started_at,
                        ended_at: record.ended_at,
                    })
                    .collect()
            })
            .map_err(storage_error)
    }

    async fn get_messages_since_for_session(
        &self,
        session_id: Uuid,
        sequence: i64,
    ) -> Result<Vec<agentik_sdk::types::messages::Message>, Error> {
        self.0
            .get_messages_since_for_session(session_id, sequence)
            .await
            .map_err(storage_error)
    }
}

/// Wrap the core storage object into the source expected by `agentik-kms`.
pub fn agent_storage_memory_source(storage: Arc<dyn AgentStorage>) -> Arc<dyn MemorySource> {
    Arc::new(AgentStorageMemorySource(storage))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use agentik_kms::{KmsAgentHandle, KnowledgeEvent, MemoryConfig, MemoryStore, TursoKmsStorage};
    use agentik_sdk::AgentMessageExt;
    use agentik_sdk::model::Model;
    use agentik_sdk::provider::client::MockApiClient;
    use arc_swap::ArcSwapOption;
    use uuid::Uuid;

    use crate::memory::agent_storage_memory_source;
    use crate::testing::dummy_model_info;

    #[tokio::test]
    async fn pipeline_dispatches_extraction_and_skips_unchanged_sources() {
        let mut mock = MockApiClient::new();
        mock.expect_request()
            .times(1)
            .withf(|messages, _, _| {
                messages
                    .iter()
                    .any(|message| message.log_summary().contains("Convert this"))
            })
            .returning(|_, _, _| {
                Ok(AgentMessageExt::assistant_text(
                    r#"{"raw_memory":"User prefers verified tests.","rollout_summary":"Implemented request.","rollout_slug":"test preference"}"#,
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
                Ok(AgentMessageExt::assistant_text(
                    r##"{"nodes":[{"knowledge_path":"/engineering/testing/verification","title":"Verification workflow","body_md":"Run targeted tests before cargo check.","confidence":0.92}],"summary":"v1\n\nRun targeted tests.","semantic_updates":[]}"##,
                ))
            });
        let model = Arc::new(ArcSwapOption::from_pointee(Some(Model::with_client(
            dummy_model_info("memory-test"),
            mock,
        ))));
        let storage: Arc<dyn crate::storage::AgentStorage> =
            Arc::new(crate::TursoAgentStorage::open_in_memory().await.unwrap());
        let source = agent_storage_memory_source(storage.clone());
        let memory_store = Arc::new(TursoKmsStorage::open_in_memory().await.unwrap());
        let mut config = MemoryConfig::new();
        config.min_idle_hours = 0;
        let memory = Arc::new(KmsAgentHandle::new(config, memory_store, source).await);
        memory.system().model_slot().store(model.load_full());

        let agent_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();
        storage.start_session(agent_id, session_id).await.unwrap();
        storage
            .append_message(session_id, &AgentMessageExt::user("always run tests"))
            .await
            .unwrap();
        storage.end_session(session_id).await.unwrap();

        memory
            .submit_knowledge_event(KnowledgeEvent::AgentStarted { agent_id })
            .await
            .unwrap();
        memory
            .submit_knowledge_event(KnowledgeEvent::AgentStarted { agent_id })
            .await
            .unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut rows = memory.store().list_stage1_outputs(10).await.unwrap();
        while rows.is_empty() {
            assert!(
                std::time::Instant::now() < deadline,
                "session extract worker never ran"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            rows = memory.store().list_stage1_outputs(10).await.unwrap();
        }
        assert_eq!(rows.len(), 1);
        assert!(rows[0].raw_memory.contains("verified tests"));
        while memory
            .store()
            .list_knowledge(10)
            .await
            .unwrap()
            .iter()
            .all(|node| node.structural)
        {
            assert!(
                std::time::Instant::now() < deadline,
                "consolidation worker never ran"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        memory.system().shutdown();
    }
}
