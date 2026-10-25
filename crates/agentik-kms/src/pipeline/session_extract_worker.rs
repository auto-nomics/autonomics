use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Notify, watch};

use super::{KnowledgeJobKind, KnowledgePipeline};
use crate::now_ms;

const SESSION_EXTRACT_JOB_KIND: KnowledgeJobKind = KnowledgeJobKind::SessionExtract;

/// Tune the resident session-extraction consumer without changing its protocol.
#[derive(Debug, Clone)]
pub struct SessionExtractWorkerConfig {
    pub batch_limit: usize,
    pub poll_interval: Duration,
}

impl Default for SessionExtractWorkerConfig {
    fn default() -> Self {
        Self {
            batch_limit: 16,
            poll_interval: Duration::from_secs(30),
        }
    }
}

/// A resident consumer for Phase 1 extraction jobs only.
///
/// The worker claims due work, executes `ExtractAgentSessions`, and immediately
/// releases any other job kind. Completion may enqueue `ConsolidateKnowledge`;
/// that job remains pending for a later Phase 2 consumer.
pub struct SessionExtractWorker {
    pipeline: Arc<KnowledgePipeline>,
    config: SessionExtractWorkerConfig,
    wake: Arc<Notify>,
    shutdown_rx: watch::Receiver<bool>,
    status: Arc<std::sync::Mutex<SessionExtractWorkerStatus>>,
}

impl SessionExtractWorker {
    #[must_use]
    pub fn new(
        pipeline: Arc<KnowledgePipeline>,
        config: SessionExtractWorkerConfig,
        wake: Arc<Notify>,
        shutdown_rx: watch::Receiver<bool>,
        status: Arc<std::sync::Mutex<SessionExtractWorkerStatus>>,
    ) -> Self {
        Self {
            pipeline,
            config,
            wake,
            shutdown_rx,
            status,
        }
    }

    async fn run(mut self) {
        loop {
            if *self.shutdown_rx.borrow() {
                break;
            }

            self.set_running(true);
            let result = self
                .pipeline
                .run_due_for_kind(SESSION_EXTRACT_JOB_KIND, self.config.batch_limit)
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

            // A full batch may have more extraction work behind it. When only
            // foreign jobs were claimed, waiting avoids a busy release loop.
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
        tracing::debug!("session extract worker stopped");
    }

    fn lock_status(&self) -> std::sync::MutexGuard<'_, SessionExtractWorkerStatus> {
        self.status
            .lock()
            .expect("session extract worker status mutex poisoned")
    }

    fn set_running(&self, running: bool) {
        self.lock_status().running = running;
    }
}

/// Cheap cloneable control surface for the resident task.
#[derive(Clone)]
pub struct SessionExtractWorkerHandle {
    wake: Arc<Notify>,
    shutdown_tx: watch::Sender<bool>,
    status: Arc<std::sync::Mutex<SessionExtractWorkerStatus>>,
}

impl SessionExtractWorkerHandle {
    pub fn wake(&self) {
        self.wake.notify_one();
    }

    pub fn request_shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
        self.wake.notify_one();
    }

    #[must_use]
    pub fn status(&self) -> SessionExtractWorkerStatus {
        self.status
            .lock()
            .map(|status| status.clone())
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, Default)]
pub struct SessionExtractWorkerStatus {
    pub running: bool,
    pub last_run_at: Option<i64>,
    pub last_error: Option<String>,
    pub total_claimed: u64,
    pub total_executed: u64,
    pub total_released: u64,
}

#[must_use]
pub fn start_session_extract_worker(
    pipeline: Arc<KnowledgePipeline>,
    config: SessionExtractWorkerConfig,
) -> SessionExtractWorkerHandle {
    let wake = Arc::new(Notify::new());
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let status = Arc::new(std::sync::Mutex::new(SessionExtractWorkerStatus::default()));
    let worker = SessionExtractWorker::new(
        pipeline,
        config,
        Arc::clone(&wake),
        shutdown_rx,
        Arc::clone(&status),
    );
    tokio::spawn(worker.run());
    SessionExtractWorkerHandle {
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
            model_info("session-extract-worker"),
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

        let handle = system.session_extract_worker().expect("worker mounted");

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
                "consolidation worker never produced knowledge: {:?}",
                system
                    .consolidation_worker()
                    .expect("worker mounted")
                    .status()
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let consolidation_handle = system.consolidation_worker().expect("worker mounted");
        system.shutdown();
        while handle.status().running || consolidation_handle.status().running {
            assert!(std::time::Instant::now() < deadline, "workers did not stop");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}
