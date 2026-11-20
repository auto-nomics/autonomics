//! Manager of per-agent `AgentEngineActor`s (Layer 3).
//!
//! Lazily spawns a dedicated actor (tokio task + channel) per agent, all
//! sharing the same heavy infrastructure from the template engine. This
//! provides **full cross-agent isolation**: a long DAG run in agent-A's
//! actor never blocks agent-B's commands, and a panic in one agent's
//! handler does not affect others. The manager also owns the agent
//! activity tracker and the storage maintenance policy.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use datafusion::common::HashMap;
use tokio::{sync::mpsc, task::JoinHandle};

use crate::data_engine::DataEngine;
use crate::node_registry::NodeRegistry;
use crate::runtime::actor::AgentEngineActor;
use crate::runtime::client::DataEngineClient;
use crate::runtime::storage::{
    AgentActivityTracker, MaintenancePolicy, RunAwareCleanReport, StorageCoordinator,
};
use crate::runtime::types::{DataEngineCmd, EngineMsg};

pub(super) struct SessionHandle {
    tx: mpsc::UnboundedSender<EngineMsg>,
    running: Arc<AtomicBool>,
    /// Mirrors the actor's `closing` flag: once set, `client_for_agent`
    /// stops handing out this generation and spawns a fresh actor instead.
    closing: Arc<AtomicBool>,
    _task: JoinHandle<()>,
}

pub struct DataEngineManager {
    template: DataEngine,
    node_registry: Arc<NodeRegistry>,
    actors: std::sync::Mutex<HashMap<String, SessionHandle>>,
    activity: Arc<AgentActivityTracker>,
    storage: Arc<StorageCoordinator>,
    policy: MaintenancePolicy,
}

impl DataEngineManager {
    /// Create a manager from a template engine. The template's
    /// `NodeRegistry`, `RuntimeEnv`, `DagHistory`, etc. are shared across all
    /// lazily-created actors via `Arc` / `Clone`.
    pub fn new(engine: DataEngine) -> Self {
        Self::with_policy(engine, MaintenancePolicy::from_env())
    }

    pub fn with_policy(engine: DataEngine, policy: MaintenancePolicy) -> Self {
        let work_root = container_runtime::PodmanConfig::from_env().workspace_root;
        let receipt_root = dag_core::dag_tasks_root();
        Self::with_policy_and_roots(engine, policy, work_root, receipt_root)
    }

    /// [`Self::with_policy`] with explicit managed roots — used by tests and
    /// embedders that do not derive storage roots from the environment.
    pub fn with_policy_and_roots(
        engine: DataEngine,
        policy: MaintenancePolicy,
        work_root: impl Into<PathBuf>,
        receipt_root: impl Into<PathBuf>,
    ) -> Self {
        let node_registry = Arc::clone(engine.node_registry());
        Self {
            template: engine,
            node_registry,
            actors: std::sync::Mutex::new(HashMap::new()),
            activity: Arc::new(AgentActivityTracker::default()),
            storage: Arc::new(StorageCoordinator::new(work_root, receipt_root)),
            policy,
        }
    }

    /// Node kinds available to every agent — a snapshot of the shared
    /// registry. Management surfaces (the gateway's plugin/node listing)
    /// use this; per-agent introspection goes through an engine client.
    pub fn list_nodes(&self) -> Vec<crate::node_registry::NodeInfo> {
        self.node_registry.list_nodes()
    }

    /// Replace factories declared by one runtime-ready manifest plugin.
    pub fn reload_plugin(&self, plugin: plugin::factory::Plugin) {
        self.node_registry.reload_plugin(&plugin);
    }

    /// Remove factories declared by one runtime-ready manifest plugin.
    pub fn remove_plugin(&self, plugin_name: &str) {
        self.node_registry.remove_plugin(plugin_name);
    }

    /// Get (or lazily create) a [`DataEngineClient`] for `agent_id`.
    ///
    /// The first call for a given agent spawns a dedicated tokio task.
    /// Subsequent calls return a cheap clone of the same channel sender.
    ///
    /// This method is synchronous because the internal actors map uses a
    /// `std::sync::Mutex` — it is never held across an `.await` point, so
    /// there is no risk of blocking the executor.
    pub fn client_for_agent(&self, agent_id: &str) -> DataEngineClient {
        self.client_for_agent_with_ref(agent_id, agent_id)
    }

    /// Like [`client_for_agent`](Self::client_for_agent) but lets the
    /// caller choose the default DAG history ref for the new agent.
    ///
    /// When the actor already exists, `history_ref` is ignored (the
    /// existing actor's ref is kept).  When creating a new actor, the
    /// engine's `history_ref` is set to `history_ref` instead of the
    /// default `"main"` — this is what gives each agent its own isolated
    /// snapshot lineage in the shared history database.
    pub fn client_for_agent_with_ref(&self, agent_id: &str, history_ref: &str) -> DataEngineClient {
        let mut actors = self.actors.lock().expect("actors mutex poisoned");

        // Fast path: actor already exists and is not being evicted. A
        // closing generation is treated as absent so callers transparently
        // get a fresh actor (new generation) with the same id.
        if let Some(handle) = actors.get(agent_id)
            && !handle.closing.load(Ordering::SeqCst)
        {
            self.activity.touch(agent_id);
            return DataEngineClient::new(
                handle.tx.clone(),
                Arc::clone(&self.node_registry),
                agent_id.to_string(),
                Arc::clone(&self.activity),
            );
        }

        // Slow path: create new actor.
        // Use the caller-provided history_ref so each agent's run records
        // default to its own audit lineage.
        let new_engine = self
            .template
            .new_isolated_engine()
            .with_history_ref(history_ref);
        let live_view = crate::dag::live::LiveDagView::default();
        let (tx, rx) = mpsc::unbounded_channel::<EngineMsg>();
        let running = Arc::new(AtomicBool::new(false));
        let shutdown = Arc::new(AtomicBool::new(false));
        let closing = Arc::new(AtomicBool::new(false));
        let server = AgentEngineActor::new(
            agent_id.to_string(),
            Arc::new(tokio::sync::Mutex::new(new_engine)),
            live_view,
            Arc::clone(&running),
            Arc::clone(&shutdown),
            Arc::clone(&closing),
            rx,
        );
        let task = tokio::task::spawn(server.run());

        tracing::info!(agent_id, history_ref, "created new DAG engine actor");
        actors.insert(
            agent_id.to_string(),
            SessionHandle {
                tx: tx.clone(),
                running,
                closing,
                _task: task,
            },
        );
        drop(actors);

        DataEngineClient::new(
            tx,
            Arc::clone(&self.node_registry),
            agent_id.to_string(),
            Arc::clone(&self.activity),
        )
    }

    pub fn agent_count(&self) -> usize {
        self.actors.lock().expect("actors mutex poisoned").len()
    }

    /// Stop idle engine actors. Active runs are never evicted.
    ///
    /// Only the current generation is evicted: targets are marked `closing`
    /// under the actors lock, so any client asking for the same agent id
    /// afterwards receives a freshly created actor whose storage references
    /// are never released by the old actor's shutdown.
    pub async fn evict_idle_actors(&self) -> usize {
        let targets = {
            let mut actors = self.actors.lock().expect("actors mutex poisoned");
            let mut targets = Vec::new();
            for (id, handle) in actors.iter_mut() {
                if !handle.running.load(Ordering::SeqCst)
                    && self.activity.is_idle(id, self.policy.agent_idle)
                {
                    // Mark closing inside the lock: a client racing this
                    // eviction will create a new generation instead of
                    // queueing work on the doomed actor.
                    handle.closing.store(true, Ordering::SeqCst);
                    targets.push((id.clone(), handle.tx.clone()));
                }
            }
            for (id, _) in &targets {
                actors.remove(id);
            }
            targets
        };
        let mut evicted = 0;
        for (agent_id, tx) in targets {
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            if tx
                .send(EngineMsg {
                    cmd: DataEngineCmd::Shutdown { reply: reply_tx },
                })
                .is_err()
            {
                continue;
            }
            if matches!(reply_rx.await, Ok(Ok(true))) {
                evicted += 1;
                tracing::info!(agent_id, "evicted idle DAG engine actor");
            }
            // A client may have recreated the actor while the old one was
            // acknowledging shutdown; dropping activity only matters when it
            // did not (a stale timestamp would re-evict the fresh actor).
            let recreated = self
                .actors
                .lock()
                .expect("actors mutex poisoned")
                .contains_key(&agent_id);
            if !recreated {
                self.activity.forget(&agent_id);
            }
        }
        evicted
    }

    /// Run one lifecycle pass and enforce the central local-storage budget.
    ///
    /// Order is fixed: evict idle actors → enforce the local budget with
    /// run-aware cleanup. VFS artifacts are only ever reported by that
    /// cleanup, never deleted.
    pub async fn maintain_once(
        &self,
    ) -> std::result::Result<(usize, usize, Option<RunAwareCleanReport>), String> {
        let evicted = self.evict_idle_actors().await;
        let clean = if self.policy.storage_max_bytes > 0
            && let Some(history) = self.template.history()
        {
            let history = history.clone();
            Some(
                self.storage
                    .run_aware_clean_once(
                        self.policy.clean_keep_runs,
                        self.policy.storage_max_bytes,
                        false,
                        &history,
                    )
                    .await?,
            )
        } else {
            None
        };
        Ok((0, evicted, clean))
    }

    /// Explicit run-aware clean. `keep_runs` protects the newest runs; a live
    /// `dry_run` reports candidates and protected directories without
    /// touching the disk. VFS artifacts are never deleted.
    pub async fn clean_storage(
        &self,
        keep_runs: usize,
        dry_run: bool,
    ) -> std::result::Result<RunAwareCleanReport, String> {
        let history = self
            .template
            .history()
            .cloned()
            .ok_or_else(|| "no history store attached to the engine".to_string())?;
        self.storage
            .clean_history(&history, keep_runs, dry_run)
            .await
    }

    /// Start the daemon maintenance loop. Interval `None` performs no work.
    pub fn spawn_maintenance(self: &Arc<Self>) -> JoinHandle<()> {
        let manager = Arc::clone(self);
        let interval = manager.policy.maintenance_interval;
        tokio::spawn(async move {
            let Some(interval) = interval else {
                return;
            };
            loop {
                match manager.maintain_once().await {
                    Ok((_, evicted, clean)) => {
                        tracing::info!(
                            actors_evicted = evicted,
                            storage_clean = ?clean,
                            "DAG lifecycle maintenance"
                        );
                    }
                    Err(error) => {
                        tracing::warn!(error = %error, "DAG lifecycle maintenance failed");
                    }
                }
                tokio::time::sleep(interval).await;
            }
        })
    }
}

/// Spawn a single-agent DataEngine actor and return a client for the
/// `"default"` agent.
///
/// This is a convenience wrapper around [`DataEngineManager`] for code that
/// does not need multi-session support (e.g. tests). New code should use
/// `DataEngineManager::new` + `client_for_agent` directly.
pub fn spawn_with_engine(engine: DataEngine) -> (DataEngineClient, JoinHandle<()>) {
    let manager = Box::leak(Box::new(DataEngineManager::new(engine)));
    let client = manager.client_for_agent("default");

    // Provide a dummy JoinHandle for API compatibility — the session task is
    // already spawned inside the manager.
    let handle = tokio::task::spawn(async {
        std::future::pending::<()>().await;
    });
    (client, handle)
}
