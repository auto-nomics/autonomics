use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Notify, watch};

use crate::now_ms;
use crate::pipeline::{KnowledgeJobKind, KnowledgePipeline};

#[derive(Debug, Clone)]
pub struct KnowledgeWorkerConfig {
    pub batch_limit: usize,
    pub poll_interval: Duration,
}

impl Default for KnowledgeWorkerConfig {
    fn default() -> Self {
        Self {
            batch_limit: 16,
            poll_interval: Duration::from_secs(30),
        }
    }
}

pub struct KnowledgeWorker {
    pipeline: Arc<KnowledgePipeline>,
    kind: KnowledgeJobKind,
    label: &'static str,
    config: KnowledgeWorkerConfig,
    wake: Arc<Notify>,
    shutdown_rx: watch::Receiver<bool>,
    status: Arc<std::sync::Mutex<KnowledgeWorkerStatus>>,
}

impl KnowledgeWorker {
    async fn run(mut self) {
        loop {
            if *self.shutdown_rx.borrow() {
                break;
            }

            self.set_running(true);
            let result = self
                .pipeline
                .run_due_for_kind(self.kind, self.config.batch_limit)
                .await;
            self.set_running(false);

            let mut continue_draining = false;
            match result {
                Ok(summary) => {
                    let mut status = self.lock_status();
                    status.total_claimed += u64::try_from(summary.claimed).unwrap_or(u64::MAX);
                    status.total_executed += u64::try_from(summary.executed).unwrap_or(u64::MAX);
                    status.total_released += u64::try_from(summary.released).unwrap_or(u64::MAX);
                    status.last_run_at = Some(now_ms());
                    status.last_error = None;
                    continue_draining = summary.executed > 0;
                }
                Err(error) => {
                    let mut status = self.lock_status();
                    status.last_run_at = Some(now_ms());
                    status.last_error = Some(error.to_string());
                }
            }

            if continue_draining {
                continue;
            }
            if !*self.shutdown_rx.borrow() {
                tokio::select! {
                    _ = self.wake.notified() => {}
                    _ = tokio::time::sleep(self.config.poll_interval) => {}
                    changed = self.shutdown_rx.changed() => {
                        if changed.is_ok() && *self.shutdown_rx.borrow() {
                            break;
                        }
                    }
                }
            }
        }

        self.set_running(false);
        tracing::debug!(worker = self.label, "knowledge worker stopped");
    }

    fn lock_status(&self) -> std::sync::MutexGuard<'_, KnowledgeWorkerStatus> {
        self.status
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    fn set_running(&self, running: bool) {
        self.lock_status().running = running;
    }
}

#[derive(Clone)]
pub struct KnowledgeWorkerHandle {
    wake: Arc<Notify>,
    shutdown_tx: watch::Sender<bool>,
    status: Arc<std::sync::Mutex<KnowledgeWorkerStatus>>,
}

impl KnowledgeWorkerHandle {
    pub fn wake(&self) {
        self.wake.notify_one();
    }

    pub fn request_shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
        self.wake.notify_one();
    }

    #[must_use]
    pub fn status(&self) -> KnowledgeWorkerStatus {
        self.status
            .lock()
            .map(|status| status.clone())
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, Default)]
pub struct KnowledgeWorkerStatus {
    pub running: bool,
    pub last_run_at: Option<i64>,
    pub last_error: Option<String>,
    pub total_claimed: u64,
    pub total_executed: u64,
    pub total_released: u64,
}

pub fn start_knowledge_worker(
    pipeline: Arc<KnowledgePipeline>,
    kind: KnowledgeJobKind,
    label: &'static str,
    config: KnowledgeWorkerConfig,
) -> KnowledgeWorkerHandle {
    let wake = Arc::new(Notify::new());
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let status = Arc::new(std::sync::Mutex::new(KnowledgeWorkerStatus::default()));
    let worker = KnowledgeWorker {
        pipeline,
        kind,
        label,
        config,
        wake: Arc::clone(&wake),
        shutdown_rx,
        status: Arc::clone(&status),
    };
    tokio::spawn(worker.run());
    KnowledgeWorkerHandle {
        wake,
        shutdown_tx,
        status,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use agentik_sdk::AgentMessageExt;
    use agentik_sdk::model::{Model, ModelInfo};
    use agentik_sdk::provider::client::MockApiClient;
    use agentik_sdk::types::messages::Message;
    use arc_swap::ArcSwapOption;
    use async_trait::async_trait;
    use uuid::Uuid;

    use super::*;
    use crate::KmsSystem;
    use crate::now_ms;
    use crate::source::{MemorySessionRecord, MemorySource};
    use crate::store::{KnowledgeStore, MemoryStore};
    use crate::{Error, KnowledgeEvent, TursoKmsStorage};

    struct OneSessionSource {
        agent_id: Uuid,
        session_id: Uuid,
    }

    #[async_trait]
    impl MemorySource for OneSessionSource {
        async fn list_session_records(
            &self,
            agent_id: Uuid,
        ) -> Result<Vec<MemorySessionRecord>, Error> {
            if agent_id != self.agent_id {
                return Ok(Vec::new());
            }
            Ok(vec![MemorySessionRecord {
                session_id: self.session_id,
                title: Some("session extract worker".to_string()),
                started_at: now_ms() - 2_000,
                ended_at: Some(now_ms() - 1_000),
            }])
        }

        async fn get_messages_since_for_session(
            &self,
            session_id: Uuid,
            _sequence: i64,
        ) -> Result<Vec<Message>, Error> {
            if session_id != self.session_id {
                return Ok(Vec::new());
            }
            Ok(vec![AgentMessageExt::user("always run tests")])
        }
    }

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
    async fn session_extract_and_consolidation_workers_complete_the_pipeline() {
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
                    r#"{"raw_memory":"Prefer targeted tests.","rollout_summary":"Implemented worker.","rollout_slug":"session-extract-worker"}"#,
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
            model_info("resident-knowledge-workers"),
            mock,
        ))));

        let store = Arc::new(TursoKmsStorage::open_in_memory().await.unwrap());
        let mut config = crate::MemoryConfig::new();
        config.min_idle_hours = 0;
        let agent_id = Uuid::new_v4();
        let source = Arc::new(OneSessionSource {
            agent_id,
            session_id: Uuid::new_v4(),
        });
        let system = KmsSystem::new(config, Arc::clone(&store), source).await;
        system.model_slot().store(model.load_full());

        system
            .submit_event(KnowledgeEvent::AgentStarted { agent_id })
            .await
            .unwrap();

        let session_extract = system.session_extract_worker().expect("worker mounted");
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while system
            .store()
            .list_stage1_outputs(10)
            .await
            .unwrap()
            .is_empty()
        {
            assert!(
                std::time::Instant::now() < deadline,
                "session extract worker never extracted the session"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        while system
            .store()
            .list_knowledge(10)
            .await
            .unwrap()
            .iter()
            .all(|node| node.structural)
        {
            assert!(
                std::time::Instant::now() < deadline,
                "consolidation worker never produced knowledge"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let consolidation = system.consolidation_worker().expect("worker mounted");
        system.shutdown();
        while session_extract.status().running || consolidation.status().running {
            assert!(std::time::Instant::now() < deadline, "workers did not stop");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}
