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

    use agentik_kms::{
        KmsAgentHandle, MEMORY_SCOPE_ID, MemoryConfig, MemoryStore, TursoKmsStorage,
        run_memory_pipeline,
    };
    use agentik_sdk::AgentMessageExt;
    use agentik_sdk::model::Model;
    use agentik_sdk::provider::client::MockApiClient;
    use arc_swap::ArcSwapOption;
    use uuid::Uuid;

    use crate::memory::agent_storage_memory_source;
    use crate::testing::dummy_model_info;

    #[tokio::test]
    async fn pipeline_extracts_consolidates_and_skips_unchanged_sources() {
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
                    r##"{"nodes":[{"knowledge_path":"/engineering/testing/verification","title":"Verification workflow","body_md":"Run targeted tests before cargo check.","confidence":0.92}],"summary":"v1\n\nRun tests for workflow changes.","semantic_updates":[{"subject":"agent runtime","predicate":"uses","object":"Turso memory","content":"Persistent memory is stored in Turso.","confidence":0.9}]}"##,
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
        let memory = Arc::new(KmsAgentHandle::new(config, memory_store));

        let agent_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();
        storage.start_session(agent_id, session_id).await.unwrap();
        storage
            .append_message(session_id, &AgentMessageExt::user("always run tests"))
            .await
            .unwrap();
        storage.end_session(session_id).await.unwrap();

        run_memory_pipeline(agent_id, source.clone(), memory.clone(), model.clone()).await;
        run_memory_pipeline(agent_id, source, memory.clone(), model).await;

        let rows = memory
            .store()
            .list_stage1_outputs(MEMORY_SCOPE_ID, 10)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        let nodes = memory.store().list_knowledge(10).await.unwrap();
        assert_eq!(nodes.iter().filter(|node| !node.structural).count(), 1);
        assert!(
            nodes
                .iter()
                .any(|node| node.body_md.contains("targeted tests"))
        );
        let summary = memory
            .store()
            .get_summary(MEMORY_SCOPE_ID)
            .await
            .unwrap()
            .unwrap();
        assert!(summary.summary_md.starts_with("v1\n"));
        let observations = memory
            .store()
            .list_observations(MEMORY_SCOPE_ID, "candidate", 10)
            .await
            .unwrap();
        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0].predicate, "uses");
    }
}
