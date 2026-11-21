//! Manager of per-agent `AgentEngineActor`s (Layer 3).
//!
//! Lazily spawns a dedicated actor (tokio task + channel) per agent, all
//! sharing the same heavy infrastructure from the template engine. This
//! provides **full cross-agent isolation**: a long DAG run in agent-A's
//! actor never blocks agent-B's commands, and a panic in one agent's
//! handler does not affect others. The manager also owns the agent
//! activity tracker.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use datafusion::common::HashMap;
use tokio::{sync::mpsc, task::JoinHandle};

use crate::data_engine::DataEngine;
use crate::node_registry::NodeRegistry;
use crate::runtime::actor::AgentEngineActor;
use crate::runtime::client::DataEngineClient;
use crate::runtime::storage::{AgentActivityTracker, RunAwareCleanReport, StorageCoordinator};
use crate::runtime::types::{DataEngineCmd, EngineMsg};

pub(super) struct SessionHandle {
    tx: mpsc::UnboundedSender<EngineMsg>,
    running: Arc<AtomicBool>,
    /// Mirrors the actor's `closing` flag: once set, `client_for_agent`
    /// stops handing out this generation and spawns a fresh actor instead.
    closing: Arc<AtomicBool>,
    _task: JoinHandle<()>,
}

/// Lifecycle manager of per-agent DAG engine actors.
///
/// The manager owns the shared template engine, the node registry, and the
/// activity tracker. It lazily spawns a dedicated tokio task per agent, and
/// provides a channel-based client to send commands to that actor. Each
/// agent's actor derives its own isolated engine from the shared template,
/// so that each agent's DAG runs and shell commands are fully isolated from
/// other agents.
///
/// - Global singleton: the manager is a singleton in the gateway process, and all agents share the same template engine and node registry.
/// - Per-agent isolation: each agent's actor has its own engine derived from the template, so that DAG runs and shell commands are fully isolated from other agents.
pub struct DataEngineManager {
    /// Shared template every actor derives its isolated engine from. Shared
    /// by `Arc` so an actor can build its own engine without the manager
    /// giving up the template.
    template: Arc<DataEngine>,
    node_registry: Arc<NodeRegistry>,
    actors: std::sync::Mutex<HashMap<String, SessionHandle>>,
    activity: Arc<AgentActivityTracker>,
    storage: Arc<StorageCoordinator>,
}

impl DataEngineManager {
    /// Create a manager from a template engine. The template's
    /// `NodeRegistry`, `RuntimeEnv`, `DagHistory`, etc. are shared across all
    /// lazily-created actors via `Arc` / `Clone`.
    pub fn new(engine: DataEngine) -> Self {
        let work_root = container_runtime::PodmanConfig::from_env().workspace_root;
        let receipt_root = dag_core::dag_tasks_root();
        Self::with_policy_and_roots(engine, work_root, receipt_root)
    }

    /// [`Self::new`] with explicit managed roots — used by tests and
    /// embedders that do not derive storage roots from the environment.
    pub fn with_policy_and_roots(
        engine: DataEngine,
        work_root: impl Into<PathBuf>,
        receipt_root: impl Into<PathBuf>,
    ) -> Self {
        let node_registry = Arc::clone(engine.node_registry());
        Self {
            template: Arc::new(engine),
            node_registry,
            actors: std::sync::Mutex::new(HashMap::new()),
            activity: Arc::new(AgentActivityTracker::default()),
            storage: Arc::new(StorageCoordinator::new(work_root, receipt_root)),
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
        // The actor derives its own isolated engine from the shared template
        // and applies the caller-provided history_ref, so each agent's run
        // records default to its own audit lineage.
        let live_view = crate::dag::live::LiveDagView::default();
        let (tx, rx) = mpsc::unbounded_channel::<EngineMsg>();
        let running = Arc::new(AtomicBool::new(false));
        let closing = Arc::new(AtomicBool::new(false));
        let server = AgentEngineActor::new(
            agent_id.to_string(),
            &self.template,
            history_ref,
            live_view,
            Arc::clone(&running),
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

    /// Stop idle engine actors. Active runs are never evicted. An actor is
    /// idle when its last command is older than `idle` (`Duration::ZERO`
    /// evicts every non-running actor — what tests use to force a rebuild).
    ///
    /// Only the current generation is evicted: targets are marked `closing`
    /// under the actors lock, so any client asking for the same agent id
    /// afterwards receives a freshly created actor whose storage references
    /// are never released by the old actor's shutdown.
    pub async fn evict_idle_actors(&self, idle: std::time::Duration) -> usize {
        let targets = {
            let mut actors = self.actors.lock().expect("actors mutex poisoned");
            let mut targets = Vec::new();
            for (id, handle) in actors.iter_mut() {
                if !handle.running.load(Ordering::SeqCst) && self.activity.is_idle(id, idle) {
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

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    use crate::data_engine::DataEngine;

    /// Barrier executor: two concurrent runs must both reach the executor
    /// before either completes — the cross-agent isolation contract.
    struct RendezvousExecutor {
        barrier: Arc<tokio::sync::Barrier>,
        calls: Arc<std::sync::atomic::AtomicUsize>,
        inner: Arc<dyn dag_core::TaskExecutor>,
    }
    #[async_trait::async_trait]
    impl dag_core::TaskExecutor for RendezvousExecutor {
        fn name(&self) -> &'static str {
            "rendezvous-test"
        }
        async fn run(
            &self,
            execution: dag_core::TaskExecution,
        ) -> dag_core::dag::node_event::JobResult {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.barrier.wait().await;
            self.inner.run(execution).await
        }
    }

    #[tokio::test]
    async fn agents_run_concurrently_through_the_shared_configured_executor() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("input.txt");
        std::fs::write(&path, "payload").unwrap();
        let graph = crate::workflow::FlowGraph::builder()
            .process(crate::workflow::Process::new(
                "file",
                "file_reference",
                serde_json::json!({"path":path.to_string_lossy()}),
            ))
            .build()
            .unwrap();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut template = DataEngine::builder().build();
        template.set_task_executor(Arc::new(RendezvousExecutor {
            barrier: Arc::new(tokio::sync::Barrier::new(2)),
            calls: Arc::clone(&calls),
            inner: dag_core::local_task_executor(),
        }));
        let manager = DataEngineManager::with_policy_and_roots(
            template,
            directory.path().join("work"),
            directory.path().join("receipts"),
        );
        let first = manager.client_for_agent("flow-concurrency-a");
        let second = manager.client_for_agent("flow-concurrency-b");
        let options = crate::flow::FlowOptions::default();
        let (first_report, second_report) =
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                tokio::join!(
                    first.run_flow(Some(graph.clone()), options.clone()),
                    second.run_flow(Some(graph), options)
                )
            })
            .await
            .expect("agent runs must reach the executor concurrently");
        assert!(first_report.unwrap().ok);
        assert!(second_report.unwrap().ok);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn flow_resume_cache_survives_new_manager_and_is_isolated_by_agent() {
        let directory = tempfile::tempdir().unwrap();
        let db = directory.path().join("history.db");
        let file = directory.path().join("input.txt");
        std::fs::write(&file, "input").unwrap();
        let graph = crate::workflow::FlowGraph::builder()
            .source(crate::workflow::Source::items(
                "files",
                vec![serde_json::json!(file.to_string_lossy())],
            ))
            .process(crate::workflow::Process::new(
                "file",
                "file_reference",
                serde_json::json!({"path":"{{item}}"}),
            ))
            .wire(crate::workflow::Wire::direct("files", 0, "file", 0))
            .build()
            .unwrap();
        let history = dag_core::dag::history::DagHistory::open(&db).await.unwrap();
        let manager = DataEngineManager::with_policy_and_roots(
            DataEngine::builder().build().with_history(history),
            directory.path().join("work-a"),
            directory.path().join("receipts-a"),
        );
        let first = manager.client_for_agent("resume-agent-a");
        let opts = crate::flow::FlowOptions {
            resume: Some(true),
            ..Default::default()
        };
        let initial = first
            .run_flow(Some(graph.clone()), opts.clone())
            .await
            .unwrap();
        assert_eq!(initial.processes["file"].reused, 0);
        assert!(initial.audit_persisted);
        assert!(initial.run_id.is_some());
        for length in [8, 12] {
            let prefix = &initial.run_id.as_deref().unwrap()[..length];
            let found = first
                .dag_runs_log(None, 20, Some(prefix.into()))
                .await
                .unwrap();
            assert_eq!(found[0].id, *initial.run_id.as_ref().unwrap());
        }
        let audit = dag_core::dag::history::DagHistory::open(&db).await.unwrap();
        let record = audit
            .get_run(initial.run_id.as_deref().unwrap())
            .await
            .unwrap()
            .unwrap();
        let persisted: serde_json::Value =
            serde_json::from_str(record.run_report_json.as_deref().unwrap()).unwrap();
        assert_eq!(persisted["nodes"][0]["logical_node"], "file");
        assert_eq!(persisted["flow"], serde_json::to_value(&graph).unwrap());
        assert_eq!(
            persisted["nodes"][0]["output_files"][0]["path"],
            file.to_string_lossy().as_ref()
        );
        assert!(persisted["nodes"][0]["execution"]["workspace"].is_string());
        assert_eq!(
            manager.evict_idle_actors(std::time::Duration::ZERO).await,
            1
        );
        let recreated = manager.client_for_agent("resume-agent-a");
        let restored_view = recreated.get_flow_view().await.unwrap();
        assert_eq!(
            restored_view.last_run.unwrap().processes["file"].succeeded,
            1
        );
        let after_eviction = recreated.run_flow(None, opts.clone()).await.unwrap();
        assert_eq!(after_eviction.processes["file"].reused, 1);
        let reopened = dag_core::dag::history::DagHistory::open(&db).await.unwrap();
        let restarted = DataEngineManager::with_policy_and_roots(
            DataEngine::builder().build().with_history(reopened),
            directory.path().join("work-b"),
            directory.path().join("receipts-b"),
        );
        let same = restarted.client_for_agent("resume-agent-a");
        let other = restarted.client_for_agent("resume-agent-b");
        assert_eq!(
            same.get_flow_view()
                .await
                .unwrap()
                .last_run
                .unwrap()
                .processes["file"]
                .succeeded,
            1
        );
        assert!(other.get_flow_view().await.unwrap().last_run.is_none());
        let restored = same.run_flow(None, opts.clone()).await.unwrap();
        assert_eq!(restored.processes["file"].reused, 1);
        std::fs::write(&file, "changed input bytes").unwrap();
        let changed = same
            .run_flow(Some(graph.clone()), opts.clone())
            .await
            .unwrap();
        assert_eq!(changed.processes["file"].reused, 0);
        let isolated = other.run_flow(Some(graph), opts).await.unwrap();
        assert_eq!(isolated.processes["file"].reused, 0);
    }

    #[tokio::test]
    async fn flow_shell_commits_survive_eviction_and_dry_run_does_not() {
        let directory = tempfile::tempdir().unwrap();
        let history =
            dag_core::dag::history::DagHistory::open(&directory.path().join("history.db"))
                .await
                .unwrap();
        let manager = DataEngineManager::with_policy_and_roots(
            DataEngine::builder().build().with_history(history),
            directory.path().join("work"),
            directory.path().join("receipts"),
        );
        let client = manager.client_for_agent("shell-agent");
        let outcome = client
            .run_dag_shell("from_list('reads', ['a'])\ncommit()".into(), false, None)
            .await
            .unwrap();
        assert!(outcome.applied);
        let dry = client
            .run_dag_shell("from_list('preview', ['b'])\ncommit()".into(), true, None)
            .await
            .unwrap();
        assert!(dry.ok && !dry.applied);
        assert_eq!(
            manager.evict_idle_actors(std::time::Duration::ZERO).await,
            1
        );
        let reopened = manager.client_for_agent("shell-agent");
        let restored = reopened.get_flow().await.unwrap();
        assert!(restored.source("reads").is_some());
        assert!(restored.source("preview").is_none());
        let changed = reopened
            .run_dag_shell("from_list('extra', ['c'])\ncommit()".into(), false, None)
            .await
            .unwrap();
        assert!(changed.applied);
        assert_eq!(
            manager.evict_idle_actors(std::time::Duration::ZERO).await,
            1
        );
        let again = manager
            .client_for_agent("shell-agent")
            .get_flow()
            .await
            .unwrap();
        assert!(again.source("reads").is_some() && again.source("extra").is_some());
        let other = manager.client_for_agent("other-agent");
        assert!(other.get_flow().await.unwrap().is_empty());
        other.set_flow(again.clone()).await.unwrap();
        assert_eq!(
            manager.evict_idle_actors(std::time::Duration::ZERO).await,
            2
        );
        let restored_other = manager
            .client_for_agent("other-agent")
            .get_flow()
            .await
            .unwrap();
        assert!(
            restored_other.source("reads").is_some() && restored_other.source("extra").is_some()
        );
    }
}
