//! Native bridge between durable HPC outboxes and the owning agent actor.
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agentik_core::agent::{ExternalNotificationAck, InternalEvent};
use agentik_core::storage::AgentStorage;
use async_trait::async_trait;
use hpc_tools::monitor::{CompletionDelivery, CompletionSink, MonitorRecord};
use tokio::sync::mpsc::UnboundedSender;
use uuid::Uuid;

pub use hpc_tools::monitor::{MonitorView, Owner};

/// All target revisions remain observed after changing the default target.
pub struct MonitorHub {
    hubs:
        std::sync::RwLock<std::collections::BTreeMap<String, Arc<hpc_tools::monitor::MonitorHub>>>,
    changed: tokio::sync::broadcast::Sender<MonitorView>,
    sink: Mutex<Option<Arc<dyn CompletionSink>>>,
}

impl MonitorHub {
    pub fn empty() -> Arc<Self> {
        let (changed, _) = tokio::sync::broadcast::channel(256);
        Arc::new(Self {
            hubs: Default::default(),
            changed,
            sink: Mutex::new(None),
        })
    }
    pub fn open(client: Arc<hpc_tools::HpcClient>, cadence: Duration) -> Result<Arc<Self>, String> {
        let hub = Self::empty();
        hub.add("legacy-slurm-r1", client, cadence)?;
        Ok(hub)
    }
    pub fn add(
        self: &Arc<Self>,
        key: &str,
        client: Arc<hpc_tools::HpcClient>,
        cadence: Duration,
    ) -> Result<(), String> {
        let mut hubs = self.hubs.write().map_err(|_| "monitor registry poisoned")?;
        if hubs.contains_key(key) {
            return Ok(());
        }
        let hub = hpc_tools::monitor::MonitorHub::open(client, cadence)?;
        let mut receiver = hub.subscribe();
        let weak = Arc::downgrade(self);
        let target_key = key.to_owned();
        tokio::spawn(async move {
            loop {
                match receiver.recv().await {
                    Ok(mut view) => {
                        let Some(parent) = weak.upgrade() else { break };
                        annotate_target(&target_key, &mut view);
                        let _ = parent.changed.send(view);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
        });
        let sink = self
            .sink
            .lock()
            .map_err(|_| "monitor registry poisoned")?
            .clone();
        if let Some(sink) = sink {
            hub.start(target_sink(key, sink));
        }
        hubs.insert(key.into(), hub);
        Ok(())
    }
    pub fn target_hub(&self, key: &str) -> Result<Arc<hpc_tools::monitor::MonitorHub>, String> {
        self.hubs
            .read()
            .map_err(|_| "monitor registry poisoned")?
            .get(key)
            .cloned()
            .ok_or("target monitor unavailable".into())
    }
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<MonitorView> {
        self.changed.subscribe()
    }
    pub fn views(&self, owner: Option<&str>) -> Result<Vec<MonitorView>, String> {
        let hubs = self.hubs.read().map_err(|_| "monitor registry poisoned")?;
        let mut views = Vec::new();
        for (key, hub) in hubs.iter() {
            for mut view in hub.views(owner)? {
                annotate_target(key, &mut view);
                views.push(view);
            }
        }
        Ok(views)
    }
    pub fn start(self: &Arc<Self>, sink: Arc<dyn CompletionSink>) {
        let hubs = self.hubs.read().expect("monitor registry poisoned");
        let mut installed = self.sink.lock().expect("monitor registry poisoned");
        if installed.is_some() {
            return;
        }
        for (key, hub) in hubs.iter() {
            hub.start(target_sink(key, sink.clone()));
        }
        *installed = Some(sink);
    }
    pub async fn watch_existing(
        &self,
        cluster: Option<&str>,
        key: &str,
        owner: Owner,
        plan: serde_json::Value,
    ) -> Result<MonitorRecord, String> {
        self.target_hub("legacy-slurm-r1")?
            .watch_existing(cluster, key, owner, plan)
            .await
    }
}

fn annotate_target(key: &str, view: &mut MonitorView) {
    if let Some((id, revision)) = key.rsplit_once("-r") {
        view.target_id = Some(id.into());
        view.target_revision = revision.parse().ok();
    }
}

struct TargetSink {
    key: String,
    inner: Arc<dyn CompletionSink>,
}
fn target_sink(key: &str, inner: Arc<dyn CompletionSink>) -> Arc<dyn CompletionSink> {
    Arc::new(TargetSink {
        key: key.into(),
        inner,
    })
}
#[async_trait]
impl CompletionSink for TargetSink {
    async fn deliver(
        &self,
        record: &MonitorRecord,
        event_id: &str,
        message: String,
    ) -> Result<CompletionDelivery, String> {
        let mut view = record.view();
        annotate_target(&self.key, &mut view);
        let binding =
            serde_json::json!({"target_id":view.target_id,"target_revision":view.target_revision});
        self.inner
            .deliver(
                record,
                event_id,
                format!("[Original compute target]\n{binding}\n{message}"),
            )
            .await
    }
}

pub(crate) fn owner_has_pending_jobs(
    jobs: &[MonitorView],
    agent_id: Uuid,
    agent_path: &str,
    session_id: Uuid,
) -> bool {
    let agent_id = agent_id.to_string();
    let session_id = session_id.to_string();
    jobs.iter().any(|job| {
        job.agent_id == agent_id
            && job.agent_path == agent_path
            && job.session_id == session_id
            && job.completion_pending()
    })
}

pub struct NotificationRouter {
    storage: Arc<dyn AgentStorage>,
    agents: Mutex<HashMap<Uuid, UnboundedSender<InternalEvent>>>,
    pending: Mutex<HashMap<String, PendingNotification>>,
}

struct PendingNotification {
    owner: Owner,
    received: tokio::sync::oneshot::Receiver<Result<ExternalNotificationAck, String>>,
}
impl NotificationRouter {
    pub fn new(storage: Arc<dyn AgentStorage>) -> Arc<Self> {
        Arc::new(Self {
            storage,
            agents: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
        })
    }
    pub fn register(&self, id: Uuid, tx: UnboundedSender<InternalEvent>) {
        self.agents
            .lock()
            .expect("HPC notification registry poisoned")
            .insert(id, tx);
    }
}
#[async_trait]
impl CompletionSink for NotificationRouter {
    async fn deliver(
        &self,
        record: &MonitorRecord,
        event_id: &str,
        message: String,
    ) -> Result<CompletionDelivery, String> {
        let agent_id =
            Uuid::parse_str(&record.owner.agent_id).map_err(|_| "invalid notification owner")?;
        let session_id = Uuid::parse_str(&record.owner.session_id)
            .map_err(|_| "invalid notification context")?;
        let agent = self
            .storage
            .get_agent(agent_id)
            .await
            .map_err(|_| "notification owner unavailable")?
            .ok_or("notification owner missing")?;
        if agent.name != record.owner.agent_path
            || self
                .storage
                .agent_context_id(agent_id)
                .await
                .map_err(|_| "notification context unavailable")?
                != Some(session_id)
        {
            return Err("notification must remain with its original agent/context".into());
        }
        // Keep the original receipt across polls. Dropping it after a fixed
        // timeout loses a successful late acknowledgement and queues duplicates
        // behind a busy actor. No monitor worker waits for a model turn here.
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| "notification receipts unavailable")?;
        if let Some(delivery) = pending.get_mut(event_id) {
            if delivery.owner != record.owner {
                return Err("notification receipt belongs to another context".into());
            }
            let result = delivery.received.try_recv();
            if matches!(
                result,
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            ) {
                return Ok(CompletionDelivery::Pending);
            }
            pending.remove(event_id);
            return result
                .map_err(|_| "notification acknowledgement lost".to_owned())?
                .map(|ack| match ack {
                    ExternalNotificationAck::Handled => CompletionDelivery::Delivered,
                    ExternalNotificationAck::Busy => CompletionDelivery::Pending,
                });
        }
        let tx = self
            .agents
            .lock()
            .map_err(|_| "notification registry unavailable")?
            .get(&agent_id)
            .cloned()
            .ok_or("original agent not restored yet")?;
        let (acknowledged, received) = tokio::sync::oneshot::channel();
        tx.send(InternalEvent::ExternalJobCompleted {
            session_id,
            event_id: event_id.to_owned(),
            message,
            acknowledged,
        })
        .map_err(|_| "original agent is offline")?;
        pending.insert(
            event_id.to_owned(),
            PendingNotification {
                owner: record.owner.clone(),
                received,
            },
        );
        Ok(CompletionDelivery::Pending)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentik_core::TursoAgentStorage;
    use agentik_core::storage::AgentRecord;
    use serde_json::json;

    async fn fixture() -> (Arc<NotificationRouter>, MonitorRecord) {
        let store = Arc::new(TursoAgentStorage::open_in_memory().await.unwrap());
        let agent = Uuid::new_v4();
        let context = Uuid::new_v4();
        store
            .upsert_agent(AgentRecord {
                id: agent,
                name: "/root/toy".into(),
                config_json: json!({}),
                created_at: 1,
                last_active: 1,
            })
            .await
            .unwrap();
        store.start_session(agent, context).await.unwrap();
        store.bind_agent_context(agent, context).await.unwrap();
        let record = MonitorRecord {
            version: 1,
            id: "a".repeat(64),
            owner: Owner {
                agent_id: agent.to_string(),
                agent_path: "/root/toy".into(),
                session_id: context.to_string(),
            },
            ssh_host: "toy".into(),
            cluster: "default".into(),
            submission_key: "toy".into(),
            nonce: "b".repeat(32),
            request_sha256: "c".repeat(64),
            job_id: Some("123".into()),
            state: "terminal".into(),
            observation: None,
            notification: "pending".into(),
            verification_plan: json!({}),
            next_poll: 0,
            failures: 0,
            last_error: None,
            revision: 1,
            created_at: 1,
            updated_at: 1,
        };
        (NotificationRouter::new(store), record)
    }
    #[tokio::test]
    async fn pending_jobs_are_bound_to_exact_owner_and_context_until_delivery() {
        let (_, record) = fixture().await;
        let agent_id = Uuid::parse_str(&record.owner.agent_id).unwrap();
        let session_id = Uuid::parse_str(&record.owner.session_id).unwrap();
        let path = record.owner.agent_path.as_str();
        let pending =
            |jobs: &[MonitorView]| owner_has_pending_jobs(jobs, agent_id, path, session_id);
        let mut job = record.view();
        assert!(pending(&[job.clone()]));
        assert!(!owner_has_pending_jobs(
            &[job.clone()],
            Uuid::new_v4(),
            path,
            session_id
        ));
        assert!(!owner_has_pending_jobs(
            &[job.clone()],
            agent_id,
            "/root/other",
            session_id
        ));
        assert!(!owner_has_pending_jobs(
            &[job.clone()],
            agent_id,
            path,
            Uuid::new_v4()
        ));
        for state in [
            "awaiting_submission",
            "watching",
            "submission_uncertain",
            "terminal",
        ] {
            job.state = state.into();
            assert!(pending(&[job.clone()]), "{state}");
        }
        let mut delivered = job.clone();
        delivered.notification = "delivered".into();
        assert!(!pending(&[delivered.clone()]));
        assert!(pending(&[delivered, job.clone()]));
        for state in ["stopped", "not_submitted", "followup_only"] {
            job.state = state.into();
            assert!(!pending(&[job.clone()]), "{state}");
        }
    }

    #[tokio::test]
    async fn hpc_notification_router_requires_exact_original_owner_and_context() {
        let (router, r) = fixture().await;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        router.register(Uuid::parse_str(&r.owner.agent_id).unwrap(), tx);
        let mut changed = r.clone();
        changed.owner.session_id = Uuid::new_v4().to_string();
        assert!(
            router
                .deliver(&changed, "event", "verify".into())
                .await
                .is_err()
        );
        changed = r.clone();
        changed.owner.agent_path = "/root/somebody_else".into();
        assert!(
            router
                .deliver(&changed, "event", "verify".into())
                .await
                .is_err()
        );
        assert!(rx.try_recv().is_err());
        let expected = r.clone();
        let ack = tokio::spawn(async move {
            match rx.recv().await.unwrap() {
                InternalEvent::ExternalJobCompleted {
                    session_id,
                    event_id,
                    message,
                    acknowledged,
                } => {
                    assert_eq!(session_id.to_string(), expected.owner.session_id);
                    assert_eq!(event_id, "event");
                    assert_eq!(message, "verify");
                    acknowledged
                        .send(Ok(ExternalNotificationAck::Handled))
                        .unwrap();
                }
                _ => panic!("unexpected event"),
            }
        });
        assert_eq!(
            router.deliver(&r, "event", "verify".into()).await.unwrap(),
            CompletionDelivery::Pending
        );
        ack.await.unwrap();
        assert_eq!(
            router.deliver(&r, "event", "verify".into()).await.unwrap(),
            CompletionDelivery::Delivered
        );
    }
    #[tokio::test]
    async fn hpc_notification_router_retains_offline_or_rejected_deliveries() {
        let (router, r) = fixture().await;
        assert!(router.deliver(&r, "event", "verify".into()).await.is_err());
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        router.register(Uuid::parse_str(&r.owner.agent_id).unwrap(), tx);
        let pending = tokio::spawn(async move {
            if let InternalEvent::ExternalJobCompleted { acknowledged, .. } =
                rx.recv().await.unwrap()
            {
                acknowledged
                    .send(Err("busy original context".into()))
                    .unwrap();
            }
        });
        assert_eq!(
            router.deliver(&r, "event", "verify".into()).await.unwrap(),
            CompletionDelivery::Pending
        );
        pending.await.unwrap();
        assert!(
            router
                .deliver(&r, "event", "verify".into())
                .await
                .unwrap_err()
                .contains("busy")
        );
        assert!(router.deliver(&r, "event", "verify".into()).await.is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn hpc_notification_router_retains_late_receipt_without_duplicate_queueing() {
        let (router, r) = fixture().await;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        router.register(Uuid::parse_str(&r.owner.agent_id).unwrap(), tx);
        assert_eq!(
            router.deliver(&r, "event", "verify".into()).await.unwrap(),
            CompletionDelivery::Pending
        );
        // The original actor is still busy with its implementation handoff.
        tokio::time::advance(Duration::from_secs(90)).await;
        assert_eq!(
            router.deliver(&r, "event", "verify".into()).await.unwrap(),
            CompletionDelivery::Pending
        );
        let InternalEvent::ExternalJobCompleted { acknowledged, .. } = rx.try_recv().unwrap()
        else {
            panic!("unexpected event")
        };
        assert!(rx.try_recv().is_err());
        // Verification itself can also exceed the old 30-second deadline.
        tokio::time::advance(Duration::from_secs(90)).await;
        assert_eq!(
            router.deliver(&r, "event", "verify".into()).await.unwrap(),
            CompletionDelivery::Pending
        );
        assert!(rx.try_recv().is_err());
        acknowledged
            .send(Ok(ExternalNotificationAck::Handled))
            .unwrap();
        assert_eq!(
            router.deliver(&r, "event", "verify".into()).await.unwrap(),
            CompletionDelivery::Delivered
        );
        assert!(router.pending.lock().unwrap().is_empty());
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn hpc_notification_router_keeps_durability_failure_and_retries_same_event() {
        let (router, r) = fixture().await;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        router.register(Uuid::parse_str(&r.owner.agent_id).unwrap(), tx);
        router.deliver(&r, "event", "verify".into()).await.unwrap();
        let InternalEvent::ExternalJobCompleted { acknowledged, .. } = rx.try_recv().unwrap()
        else {
            panic!("unexpected event")
        };
        acknowledged
            .send(Err("checkpoint persistence writer is closed".into()))
            .unwrap();
        assert!(
            router
                .deliver(&r, "event", "verify".into())
                .await
                .unwrap_err()
                .contains("checkpoint")
        );
        assert_eq!(
            router.deliver(&r, "event", "verify".into()).await.unwrap(),
            CompletionDelivery::Pending
        );
        let InternalEvent::ExternalJobCompleted {
            event_id,
            acknowledged,
            ..
        } = rx.try_recv().unwrap()
        else {
            panic!("unexpected event")
        };
        assert_eq!(event_id, "event");
        acknowledged
            .send(Ok(ExternalNotificationAck::Handled))
            .unwrap();
        assert_eq!(
            router.deliver(&r, "event", "verify".into()).await.unwrap(),
            CompletionDelivery::Delivered
        );
    }

    #[tokio::test]
    async fn hpc_notification_router_dropped_receipt_remains_an_error() {
        let (router, r) = fixture().await;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        router.register(Uuid::parse_str(&r.owner.agent_id).unwrap(), tx);
        router.deliver(&r, "event", "verify".into()).await.unwrap();
        drop(rx.try_recv().unwrap());
        assert!(
            router
                .deliver(&r, "event", "verify".into())
                .await
                .unwrap_err()
                .contains("acknowledgement lost")
        );
        assert!(router.pending.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn hpc_notification_router_busy_ack_retries_without_claiming_delivery() {
        let (router, r) = fixture().await;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        router.register(Uuid::parse_str(&r.owner.agent_id).unwrap(), tx);
        router.deliver(&r, "event", "verify".into()).await.unwrap();
        let InternalEvent::ExternalJobCompleted { acknowledged, .. } = rx.try_recv().unwrap()
        else {
            panic!("unexpected event")
        };
        acknowledged
            .send(Ok(ExternalNotificationAck::Busy))
            .unwrap();
        assert_eq!(
            router.deliver(&r, "event", "verify".into()).await.unwrap(),
            CompletionDelivery::Pending
        );
        assert!(rx.try_recv().is_err());
        assert_eq!(
            router.deliver(&r, "event", "verify".into()).await.unwrap(),
            CompletionDelivery::Pending
        );
        let InternalEvent::ExternalJobCompleted { acknowledged, .. } = rx.try_recv().unwrap()
        else {
            panic!("unexpected event")
        };
        acknowledged
            .send(Ok(ExternalNotificationAck::Handled))
            .unwrap();
        assert_eq!(
            router.deliver(&r, "event", "verify".into()).await.unwrap(),
            CompletionDelivery::Delivered
        );
    }

    #[tokio::test]
    async fn hpc_notification_router_cannot_consume_another_owners_receipt() {
        let (router, r) = fixture().await;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        router.register(Uuid::parse_str(&r.owner.agent_id).unwrap(), tx);
        router.deliver(&r, "event", "verify".into()).await.unwrap();
        let InternalEvent::ExternalJobCompleted { acknowledged, .. } = rx.try_recv().unwrap()
        else {
            panic!("unexpected event")
        };
        acknowledged
            .send(Ok(ExternalNotificationAck::Handled))
            .unwrap();
        let mut changed = r.clone();
        changed.owner.session_id = Uuid::new_v4().to_string();
        assert!(
            router
                .deliver(&changed, "event", "verify".into())
                .await
                .is_err()
        );
        assert_eq!(
            router.deliver(&r, "event", "verify".into()).await.unwrap(),
            CompletionDelivery::Delivered
        );
    }
}
