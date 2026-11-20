//! Runtime of Data Engine based on tokio runtime.
//!
//! # Architecture (three-layer isolation)
//!
//! **Layer 1 — Metadata bypass.** Read-only node-registry queries
//! (`list_node_factories`, `get_node_spec`, `get_node_ports`, `get_node_doc`)
//! are served directly from `Arc<NodeRegistry>` on the client side and never
//! enter the actor channel, so they cannot be blocked by a running DAG.
//!
//! **Layer 2 — RunFlow fire-and-forget.** `RunFlow` is spawned as a background
//! task inside the engine actor; the actor loop returns immediately and keeps
//! processing subsequent commands. A per-actor `AtomicBool` guards against
//! concurrent runs; mutation commands during a run return an immediate error
//! instead of blocking.
//!
//! **Layer 3 — Per-agent actors.** Each agent gets its own tokio task
//! and its own mpsc channel, managed by `DataEngineManager`. A long DAG run in
//! agent-A's actor never blocks agent-B's commands. Heavy infrastructure
//! (`NodeRegistry`, `DagHistory`, `RuntimeEnv`) is shared via `Arc`.
//!
//! # Module layout
//!
//! - [`actor`] — the per-agent command loop (`AgentEngineActor`), including
//!   workspace/cache/report restore and the flow-run task.
//! - [`client`] — `DataEngineClient` and the run-cancel-on-drop reply
//!   wrapper.
//! - [`manager`] — `DataEngineManager`: lazy per-agent actor spawn, idle
//!   eviction, and storage maintenance.
//! - [`storage`], [`types`], [`error`] — storage coordination, command
//!   types, and client error types.

mod actor;
mod client;
mod manager;

pub mod error;
pub mod storage;
pub mod types;

pub use client::{CancelOnDropReceiver, DataEngineClient};
pub use manager::{DataEngineManager, spawn_with_engine};
pub use storage::{
    AgentActivityTracker, ArtifactEntry, MaintenancePolicy, RunAwareCleanReport, StorageCoordinator,
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    use crate::data_engine::DataEngine;

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
            zero_idle_policy(),
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
            zero_idle_policy(),
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
        assert_eq!(manager.evict_idle_actors().await, 1);
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
            zero_idle_policy(),
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
    async fn channel_metadata_describes_runtime_operators() {
        let (client, _handle) = spawn_with_engine(DataEngine::builder().build());
        assert!(
            client
                .list_node_factories()
                .unwrap()
                .iter()
                .any(|node| node.kind == "channel")
        );
        let schema = serde_json::to_value(client.get_node_spec("channel").unwrap()).unwrap();
        let variants = schema["oneOf"].as_array().unwrap();
        for operator in [
            "collect",
            "transpose",
            "buffer",
            "if_empty",
            "concat",
            "to_sorted_list",
        ] {
            assert!(
                variants
                    .iter()
                    .any(|v| v["properties"]["operator"]["const"] == operator),
                "{operator}"
            );
        }
        let ports = client
            .get_node_ports_for_spec("channel", serde_json::json!({"operator":"join"}))
            .unwrap();
        assert_eq!(ports.input_ports().len(), 2);
        assert_eq!(ports.output_ports().len(), 1);
        let branch = client
            .get_node_ports_for_spec(
                "channel",
                serde_json::json!({"operator":"branch", "branches":[{"name":"a"}, {"name":"b"}]}),
            )
            .unwrap();
        assert_eq!(branch.output_ports().len(), 2);
        assert!(
            client
                .get_node_ports_for_spec("channel", serde_json::json!({"operator":"missing"}))
                .is_err()
        );
    }

    #[tokio::test]
    async fn latest_failed_run_does_not_restore_an_older_report() {
        let directory = tempfile::tempdir().unwrap();
        let history =
            dag_core::dag::history::DagHistory::open(&directory.path().join("history.db"))
                .await
                .unwrap();
        let manager = DataEngineManager::with_policy_and_roots(
            DataEngine::builder().build().with_history(history.clone()),
            zero_idle_policy(),
            directory.path().join("work"),
            directory.path().join("receipts"),
        );
        let client = manager.client_for_agent("a");
        let graph = crate::workflow::FlowGraph::builder()
            .process(crate::workflow::Process::new(
                "hello",
                "echo",
                serde_json::json!({}),
            ))
            .build()
            .unwrap();
        assert!(
            client
                .run_flow(Some(graph), crate::flow::FlowOptions::default())
                .await
                .unwrap()
                .ok
        );
        let mut failed = history
            .list_runs(1, Some("flow"))
            .await
            .unwrap()
            .pop()
            .unwrap();
        failed.id = uuid::Uuid::new_v4().to_string();
        failed.started_at = "9999-01-01T00:00:00Z".into();
        failed.ok = false;
        failed.error = Some("execution failed".into());
        failed.run_report_json = None;
        history.record_run(&failed).await.unwrap();
        assert_eq!(manager.evict_idle_actors().await, 1);
        assert!(
            manager
                .client_for_agent("a")
                .get_flow_view()
                .await
                .unwrap()
                .last_run
                .is_none()
        );
    }

    #[tokio::test]
    async fn failed_flow_run_is_recorded_in_history() {
        let history = dag_core::dag::history::DagHistory::open_in_memory()
            .await
            .unwrap();
        let (client, _handle) =
            spawn_with_engine(DataEngine::builder().build().with_history(history.clone()));
        let graph = crate::workflow::FlowGraph::builder()
            .process(crate::workflow::Process::new(
                "broken",
                "missing_kind",
                serde_json::json!({}),
            ))
            .build()
            .unwrap();
        let error = client
            .run_flow(Some(graph), crate::flow::FlowOptions::default())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("missing_kind"));
        let runs = history.list_runs(10, Some("flow")).await.unwrap();
        assert_eq!(runs.len(), 1);
        assert!(!runs[0].ok && !runs[0].cancelled);
        assert!(runs[0].error.as_deref().unwrap().contains("missing_kind"));
        assert!(runs[0].run_report_json.is_none());
    }

    #[tokio::test]
    async fn flow_view_tracks_last_run_and_clears_after_edit() {
        let (client, _handle) = spawn_with_engine(DataEngine::builder().build());
        let graph = crate::workflow::FlowGraph::builder()
            .process(crate::workflow::Process::new(
                "hello",
                "echo",
                serde_json::json!({}),
            ))
            .build()
            .unwrap();
        client.set_flow(graph.clone()).await.unwrap();
        assert!(client.get_flow_view().await.unwrap().last_run.is_none());
        let report = client
            .run_flow(None, crate::flow::FlowOptions::default())
            .await
            .unwrap();
        assert!(report.ok);
        let view = client.get_flow_view().await.unwrap();
        let summary = view.last_run.unwrap();
        assert!(summary.ok);
        assert_eq!(summary.processes["hello"].succeeded, 1);
        let preview = client
            .run_dag_shell("node('more', 'echo', {})\ncommit()".into(), true, None)
            .await
            .unwrap();
        assert!(preview.ok && !preview.applied);
        assert!(client.get_flow_view().await.unwrap().last_run.is_some());
        client.set_flow(graph).await.unwrap();
        assert!(client.get_flow_view().await.unwrap().last_run.is_some());
        let edit = client
            .run_dag_shell("node('more', 'echo', {})\ncommit()".into(), false, None)
            .await
            .unwrap();
        assert!(edit.applied);
        assert!(client.get_flow_view().await.unwrap().last_run.is_none());
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
            zero_idle_policy(),
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
        assert_eq!(manager.evict_idle_actors().await, 1);
        let reopened = manager.client_for_agent("shell-agent");
        let restored = reopened.get_flow().await.unwrap();
        assert!(restored.source("reads").is_some());
        assert!(restored.source("preview").is_none());
        let changed = reopened
            .run_dag_shell("from_list('extra', ['c'])\ncommit()".into(), false, None)
            .await
            .unwrap();
        assert!(changed.applied);
        assert_eq!(manager.evict_idle_actors().await, 1);
        let again = manager
            .client_for_agent("shell-agent")
            .get_flow()
            .await
            .unwrap();
        assert!(again.source("reads").is_some() && again.source("extra").is_some());
        let other = manager.client_for_agent("other-agent");
        assert!(other.get_flow().await.unwrap().is_empty());
        other.set_flow(again.clone()).await.unwrap();
        assert_eq!(manager.evict_idle_actors().await, 2);
        let restored_other = manager
            .client_for_agent("other-agent")
            .get_flow()
            .await
            .unwrap();
        assert!(
            restored_other.source("reads").is_some() && restored_other.source("extra").is_some()
        );
    }

    #[tokio::test]
    async fn snapshot_and_dot_remain_available_during_flow_execution() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("input.txt");
        std::fs::write(&path, "payload").unwrap();
        let flow = crate::workflow::FlowGraph::builder()
            .process(crate::workflow::Process::new(
                "slow",
                "file_reference",
                serde_json::json!({"path":path.to_string_lossy()}),
            ))
            .build()
            .unwrap();
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let mut template = DataEngine::builder().build();
        template.set_task_executor(Arc::new(RendezvousExecutor {
            barrier: barrier.clone(),
            calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            inner: dag_core::local_task_executor(),
        }));
        let manager = DataEngineManager::with_policy_and_roots(
            template,
            zero_idle_policy(),
            directory.path().join("work"),
            directory.path().join("receipts"),
        );
        let client = manager.client_for_agent("live-test");
        let running_client = client.clone();
        let task = tokio::spawn(async move {
            running_client
                .run_flow(Some(flow), crate::flow::FlowOptions::default())
                .await
        });
        let snapshot = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let snapshot = client.dag_tui_snapshot().await.unwrap();
                if snapshot.running
                    && snapshot
                        .node("slow")
                        .is_some_and(|n| n.status == crate::dag::runtime::RuntimeStatus::Running)
                {
                    break snapshot;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("live flow snapshot should be readable while the task runs");
        assert!(snapshot.run_id.is_some());
        assert!(
            client
                .dag_tui_snapshot()
                .await
                .unwrap()
                .to_dot()
                .contains("slow")
        );
        barrier.wait().await;
        assert!(task.await.unwrap().unwrap().ok);
        let finished = client.dag_tui_snapshot().await.unwrap();
        assert!(!finished.running);
        assert_eq!(
            finished.node("slow").unwrap().status,
            crate::dag::runtime::RuntimeStatus::Success
        );
    }

    #[tokio::test]
    async fn legacy_edit_commands_use_the_session_flow() {
        let (client, _handle) = spawn_with_engine(DataEngine::builder().build());
        client
            .add_node("first".into(), "echo".into(), serde_json::json!({}))
            .await
            .unwrap();
        client
            .add_node("second".into(), "echo".into(), serde_json::json!({}))
            .await
            .unwrap();
        client
            .add_edge("first".into(), "second".into())
            .await
            .unwrap();
        let flow = client.get_flow().await.unwrap();
        assert!(flow.process("first").is_some());
        assert_eq!(flow.wires().len(), 1);
        assert!(client.node_exists("first".into()).await.unwrap());
        assert_eq!(
            client.get_node("first".into()).await.unwrap().unwrap().0,
            "echo"
        );
        assert!(
            client
                .dag_tui_snapshot()
                .await
                .unwrap()
                .to_dot()
                .contains("second")
        );
        assert!(
            client
                .add_edge("second".into(), "first".into())
                .await
                .is_err()
        );
        assert_eq!(client.get_flow().await.unwrap().wires().len(), 1);
        client.remove_node("first".into()).await.unwrap();
        let flow = client.get_flow().await.unwrap();
        assert!(flow.process("first").is_none());
        assert!(flow.wires().is_empty());
    }

    #[tokio::test]
    async fn add_node_client_rejects_generic_container_kind() {
        let engine = DataEngine::builder().build();
        let (client, _handle) = spawn_with_engine(engine);

        let error = client
            .add_node(
                "generic_container".to_string(),
                "container_command".to_string(),
                serde_json::json!({}),
            )
            .await
            .expect_err("container_command creation must be rejected");

        assert!(
            error.to_string().contains(
                "node kind 'container_command' is disabled; use a registered dedicated node instead"
            ),
            "unexpected error: {error}"
        );
    }

    #[cfg(target_os = "linux")]
    fn zero_idle_policy() -> MaintenancePolicy {
        MaintenancePolicy {
            agent_idle: std::time::Duration::ZERO,
            maintenance_interval: None,
            clean_keep_runs: 2,
            storage_max_bytes: 0,
        }
    }
}
