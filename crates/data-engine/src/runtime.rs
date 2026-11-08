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

use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};

use datafusion::common::HashMap;
use futures::FutureExt;
use tokio::{sync::mpsc, task::JoinHandle};
use tokio_util::sync::CancellationToken;

use crate::data_engine::DataEngine;
use crate::node_registry::NodeRegistry;
use crate::runtime::error::{ClientError, Result};
use crate::runtime::types::{DataEngineCmd, EngineMsg};

pub mod error;
pub mod storage;
pub mod types;

pub use storage::{
    AgentActivityTracker, ArtifactEntry, CachedFileOutput, MaintenancePolicy, ReferenceEntry,
    ResultCache, RunAwareCleanReport, StorageCoordinator, StorageStatus,
};

// ═══════════════════════════════════════════════════════════════════════
// Per-agent actor (Layer 2 + 3)
// ═══════════════════════════════════════════════════════════════════════

// Single-engine DataEngine actor (one per agent).
///
/// One instance per agent. The engine is behind a `Mutex` so that
/// `RunFlow` can be spawned as a background task while the actor loop continues
/// processing other commands. The `running` flag prevents concurrent DAG runs
/// and allows mutation commands to fast-fail instead of blocking.
struct AgentEngineActor {
    agent_id: String,
    engine: Arc<tokio::sync::Mutex<DataEngine>>,
    live_view: crate::dag::live::LiveDagView,
    live_valid: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    /// Set by the manager when this actor generation is being evicted.
    /// Queued runs are rejected instead of executing on an actor the manager
    /// has already removed from its map.
    closing: Arc<AtomicBool>,
    rx: mpsc::UnboundedReceiver<EngineMsg>,
}

impl AgentEngineActor {
    async fn run(mut self) {
        Self::restore_flow_cache(&self.engine, &self.agent_id).await;
        Self::restore_flow_workspace(&self.engine, &self.agent_id).await;
        Self::restore_flow_report(&self.engine, &self.agent_id).await;
        Self::restore_agent_workspace(&self.engine, &self.agent_id).await;
        while let Some(msg) = self.rx.recv().await {
            if let Err(panic) = AssertUnwindSafe(self.handle(msg)).catch_unwind().await {
                tracing::error!(
                    agent_id = %self.agent_id,
                    "engine actor handler panicked: {:?}",
                    panic
                );
            }
            if self.shutdown.load(Ordering::SeqCst) {
                break;
            }
        }
        tracing::debug!(agent_id = %self.agent_id, "engine actor exited");
    }

    async fn restore_flow_cache(engine: &tokio::sync::Mutex<DataEngine>, agent_id: &str) {
        let (history, cache) = {
            let engine = engine.lock().await;
            (engine.history.clone(), Arc::clone(&engine.flow_cache))
        };
        let Some(history) = history else {
            return;
        };
        match history.load_flow_cache(agent_id).await {
            Ok(Some(serialized)) => {
                let restored = serde_json::from_str::<serde_json::Value>(&serialized)
                    .ok()
                    .and_then(dag_engine::InstanceCache::from_json);
                if let Some(restored) = restored {
                    *cache.lock().await = restored;
                } else {
                    tracing::warn!(
                        agent_id,
                        "invalid persisted flow cache; starting with an empty cache"
                    );
                }
            }
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(agent_id, %error, "could not load flow cache; starting empty")
            }
        }
    }

    async fn restore_flow_workspace(engine: &tokio::sync::Mutex<DataEngine>, agent_id: &str) {
        let engine = engine.lock().await;
        let Some(history) = &engine.history else {
            return;
        };
        match history.load_flow_workspace(agent_id).await {
            Ok(Some(json)) => match serde_json::from_str::<dag_engine::FlowGraph>(&json) {
                Ok(flow) if dag_engine::dataflow::Dataflow::<()>::new(&flow).is_ok() => {
                    *engine.flow.write().expect("flow lock") = flow;
                }
                _ => tracing::warn!(agent_id, "invalid persisted flow workspace; starting empty"),
            },
            Ok(None) => {}
            Err(error) => tracing::warn!(agent_id, %error, "could not load flow workspace"),
        }
    }

    async fn restore_flow_report(engine: &tokio::sync::Mutex<DataEngine>, agent_id: &str) {
        let engine = engine.lock().await;
        let Some(history) = &engine.history else {
            return;
        };
        let hash = flow_content_hash(&engine.flow.read().expect("flow lock"));
        match history.latest_flow_run(agent_id, &hash).await {
            Ok(Some(run)) => {
                if let Some(json) = run.run_report_json {
                    match serde_json::from_str::<crate::flow::FlowRunReportSummary>(&json) {
                        Ok(summary) => {
                            *engine.flow_report.write().expect("flow report lock") = Some(summary)
                        }
                        Err(error) => {
                            tracing::warn!(agent_id, %error, "invalid persisted flow report")
                        }
                    }
                }
            }
            Ok(None) => {}
            Err(error) => tracing::warn!(agent_id, %error, "could not restore flow report"),
        }
    }

    async fn persist_flow_workspace(
        engine: &DataEngine,
        agent_id: &str,
        flow: &dag_engine::FlowGraph,
    ) -> crate::error::Result<()> {
        if let Some(history) = &engine.history {
            let json = serde_json::to_string(flow)
                .map_err(|e| crate::error::Error::Custom(e.to_string()))?;
            history
                .store_flow_workspace(agent_id, &json)
                .await
                .map_err(|e| crate::error::Error::Custom(e.to_string()))?;
        }
        Ok(())
    }

    /// Materialize the history ref's head snapshot into a freshly created
    /// agent so a restarted agent resumes its previous DAG instead of an
    /// empty workspace. Runs before the actor loop, so it can never race a
    /// client command. Failures are logged and swallowed: an unrestorable
    /// history must not stop the actor from serving commands.
    async fn restore_agent_workspace(engine: &tokio::sync::Mutex<DataEngine>, agent_id: &str) {
        if !agent_workspace_auto_restore_enabled() {
            return;
        }
        let mut engine = engine.lock().await;
        match engine.restore_ref_head().await {
            Ok(true) => tracing::info!(agent_id, "restored DAG from history ref head"),
            Ok(false) => {}
            Err(error) => tracing::warn!(
                agent_id,
                error = %error,
                "could not restore DAG from history ref head; starting with an empty workspace"
            ),
        }
    }

    async fn handle(&self, msg: EngineMsg) {
        let EngineMsg { cmd } = msg;

        // Any idle command may change the engine; invalidate the last run's
        // projection conservatively before serving it. Run-time commands cannot
        // mutate the graph and must not hide the active projection.
        if !self.running.load(Ordering::SeqCst)
            && !matches!(
                &cmd,
                DataEngineCmd::GetDagTuiSnapshot { .. } | DataEngineCmd::ViewDag { .. }
            )
        {
            self.live_valid.store(false, Ordering::SeqCst);
        }

        match cmd {
            // ── Stage-model flow session state ───────────────────────────
            DataEngineCmd::SetFlow { flow, reply } => {
                if self.running.load(Ordering::SeqCst) {
                    let _ = reply.send(Err(crate::error::Error::Custom("a flow is running; wait before editing it".into())));
                    return;
                }
                let engine = self.engine.lock().await;
                let result = match dag_engine::dataflow::Dataflow::<()>::new(&flow) {
                    Err(error) => Err(crate::error::Error::Custom(error)),
                    Ok(_) => match Self::persist_flow_workspace(&engine, &self.agent_id, &flow).await {
                        Ok(()) => {
                            if flow_content_hash(&engine.flow.read().expect("flow lock")) != flow_content_hash(&flow) {
                                *engine.flow_report.write().expect("flow report lock") = None;
                            }
                            *engine.flow.write().expect("flow lock") = flow;
                            Ok(())
                        },
                        Err(error) => Err(error),
                    },
                };
                let _ = reply.send(result);
            }
            DataEngineCmd::GetFlow { reply } => {
                let engine = self.engine.lock().await;
                let flow = engine.flow.read().expect("flow lock").clone();
                let _ = reply.send(Ok(flow));
            }
            DataEngineCmd::GetFlowView { reply } => {
                let engine = self.engine.lock().await;
                let view = crate::flow::FlowView {
                    flow: engine.flow.read().expect("flow lock").clone(),
                    last_run: engine
                        .flow_report
                        .read()
                        .expect("flow report lock")
                        .clone(),
                };
                let _ = reply.send(Ok(view));
            }

            // ── Stage-model flow execution ────────────────────────────────
            //
            // Flow runs never touch engine mutable state beyond reading the
            // session flow, so the engine lock is taken only to resolve the
            // graph and assemble the runner; it is released for the whole
            // execution.
            DataEngineCmd::RunFlow {
                flow,
                options,
                reply,
                cancel_token,
            } => {
                if self.closing.load(Ordering::SeqCst) {
                    let _ = reply.send(Err(crate::error::Error::Custom(
                        "engine actor is shutting down; reconnect to start a new run".to_string(),
                    )));
                    return;
                }
                if self.running.swap(true, Ordering::SeqCst) {
                    let _ = reply.send(Err(crate::error::Error::Custom(
                        "a run is already active for this agent; wait for it to complete".to_string(),
                    )));
                    return;
                }
                let engine = Arc::clone(&self.engine);
                let engine_for_report = Arc::clone(&self.engine);
                let live_view = self.live_view.clone();
                let live_valid = Arc::clone(&self.live_valid);
                let running = Arc::clone(&self.running);
                let agent_id = self.agent_id.clone();
                tokio::spawn(async move {
                    let (runner, flow, history, flow_cache) = {
                        let engine = engine.lock().await;
                        let flow = match flow {
                            Some(flow) => {
                                if let Err(error) = dag_engine::dataflow::Dataflow::<()>::new(&flow) {
                                    let _ = reply.send(Err(crate::error::Error::Custom(
                                        error.to_string(),
                                    )));
                                    running.store(false, Ordering::SeqCst);
                                    return;
                                }
                                if let Err(error) = Self::persist_flow_workspace(&engine, &agent_id, &flow).await {
                                    let _ = reply.send(Err(error));
                                    running.store(false, Ordering::SeqCst);
                                    return;
                                }
                                if flow_content_hash(&engine.flow.read().expect("flow lock")) != flow_content_hash(&flow) {
                                    *engine.flow_report.write().expect("flow report lock") = None;
                                }
                                *engine.flow.write().expect("flow lock") = flow.clone();
                                flow
                            }
                            None => {
                                let stored = engine.flow.read().expect("flow lock").clone();
                                if stored.is_empty() {
                                    let _ = reply.send(Err(crate::error::Error::Custom(
                                        "no flow is set for this session; pass `flow` or call set_flow first".to_string(),
                                    )));
                                    running.store(false, Ordering::SeqCst);
                                    return;
                                }
                                stored
                            }
                        };
                        *engine.flow_report.write().expect("flow report lock") = None;
                        (
                            engine.flow_runner().with_live_view(live_view.clone()),
                            flow,
                            engine.history.clone(),
                            Arc::clone(&engine.flow_cache),
                        )
                    };
                    let snapshot = crate::flow::flow_snapshot(&flow);
                    let live_run_id = snapshot.run_id.clone().expect("flow snapshot has a run id");
                    live_view.begin(snapshot);
                    live_valid.store(true, Ordering::SeqCst);
                    let started_at = chrono::Utc::now().to_rfc3339();
                    let trigger = Some(format!("agent:{agent_id}"));
                    let mut result = AssertUnwindSafe(
                        runner.run(&flow, &options, Some(cancel_token.clone())),
                    )
                    .catch_unwind()
                    .await
                    .unwrap_or_else(|panic| {
                        Err(crate::error::Error::Custom(format!(
                            "flow run panicked: {panic:?}"
                        )))
                    });
                    if let Ok(report) = &result {
                        use dag_core::dag::runtime::RuntimeStatus;
                        for source in flow.sources() { live_view.status(&source.id, if report.cancelled { RuntimeStatus::Cancelled } else { RuntimeStatus::Success }); }
                        for process in flow.processes() {
                            let summary = report.processes.get(&process.id);
                            let status = if report.cancelled { RuntimeStatus::Cancelled }
                                else if summary.is_some_and(|s| s.failed > 0) { RuntimeStatus::Failed }
                                else if summary.is_some_and(|s| s.cancelled > 0) { RuntimeStatus::Cancelled }
                                else if summary.is_none_or(|s| s.total == 0 || (s.skipped > 0 && s.succeeded == 0)) { RuntimeStatus::Skipped }
                                else { RuntimeStatus::Success };
                            live_view.status(&process.id, status);
                        }
                        live_view.end();
                        let engine = engine_for_report.lock().await;
                        *engine.flow_report.write().expect("flow report lock") = Some(crate::flow::FlowRunReportSummary::from(report));
                    }
                    if result.is_err() { live_view.cancel(); }
                    if let Some(history) = &history {
                        let cache_json = {
                            let cache = flow_cache.lock().await;
                            cache.to_json().to_string()
                        };
                        if let Err(error) = history.store_flow_cache(&agent_id, &cache_json).await {
                            tracing::warn!(agent_id, %error, "flow cache could not be persisted");
                        }
                    }
                    // Audit completed runs and top-level execution errors.
                    // The hash identifies the flow but does not reconstruct it.
                    if let Some(history) = &history {
                        let run_id = live_run_id;
                        if let Ok(report) = &mut result {
                            report.run_id = Some(run_id.clone());
                            report.audit_persisted = true;
                        }
                        let report = result.as_ref().ok();
                        let record = dag_core::dag::history::RunRecord {
                            id: run_id,
                            ref_name: "flow".into(),
                            snapshot_id: None,
                            manifest_hash: flow_content_hash(&flow),
                            trigger: trigger.clone(),
                            started_at: started_at.clone(),
                            finished_at: chrono::Utc::now().to_rfc3339(),
                            ok: report.is_some_and(|report| report.ok),
                            cancelled: report.map_or_else(|| cancel_token.is_cancelled(), |report| report.cancelled),
                            error: result.as_ref().err().map(ToString::to_string),
                            message: Some("stage-model flow run".into()),
                            engine_version: dag_core::engine_version().to_string(),
                            source_revision: dag_core::source_revision().to_string(),
                            run_report_json: report.and_then(|report| {
                                let mut json = serde_json::to_value(report).ok()?;
                                json["flow"] = serde_json::to_value(&flow).ok()?;
                                serde_json::to_string(&json).ok()
                            }),
                        };
                        if let Err(error) = history.record_run(&record).await {
                            if let Ok(report) = &mut result { report.audit_persisted = false; }
                            tracing::warn!(%error, "flow run record could not be persisted");
                        }
                    }
                    running.store(false, Ordering::SeqCst);
                    let _ = reply.send(result);
                });
            }

            // ── Commands that need exclusive engine access ───────────────
            //
            // When a DAG is running, the mutex is held by the run task.
            // Instead of blocking, these commands fast-fail so the actor
            // loop stays responsive for other messages (and other agents).
            DataEngineCmd::RunDagShell {
                script,
                dry_run,
                timeout_ms,
                reply,
            } => {
                if self.running.load(Ordering::SeqCst) {
                    let _ = reply.send(Ok(crate::dag_shell::DagShellOutcome {
                        ok: false,
                        applied: false,
                        dry_run,
                        committed: false,
                        result: None,
                        operations: Vec::new(),
                        graph: serde_json::json!({}),
                        error: Some(crate::dag_shell::DagShellError {
                            code: "dag_running".into(),
                            message: "DAG is currently running; wait for it to complete before running dag_shell".into(),
                            operation_index: None,
                            line: None,
                            column: None,
                        }),
                    }));
                    return;
                }
                let mut engine = self
                    .engine
                    .try_lock()
                    .expect("uncontended: running flag is false");
                let old = engine.flow.read().expect("flow lock").clone();
                let outcome = engine.run_flow_shell(&script, dry_run, timeout_ms);
                if outcome.applied {
                    let flow = engine.flow.read().expect("flow lock").clone();
                    if let Err(error) = Self::persist_flow_workspace(&engine, &self.agent_id, &flow).await {
                        *engine.flow.write().expect("flow lock") = old;
                        let _ = reply.send(Err(error));
                        return;
                    }
                    if flow_content_hash(&old) != flow_content_hash(&flow) {
                        *engine.flow_report.write().expect("flow report lock") = None;
                    }
                }
                let _ = reply.send(Ok(outcome));
            }
            DataEngineCmd::AddEdge {
                from,
                from_port,
                to,
                to_port,
                reply,
            } => {
                if self.running.load(Ordering::SeqCst) {
                    let _ = reply.send(Err(crate::error::Error::Custom(
                        "DAG is currently running; wait for it to complete before modifying the graph"
                            .to_string(),
                    )));
                    return;
                }
                let mut engine = self
                    .engine
                    .try_lock()
                    .expect("uncontended: running flag is false");
                let res = match (from_port, to_port) {
                    (Some(fp), Some(tp)) => engine.add_edge(from, to, fp, tp).map(|_| ()),
                    (None, None) => engine.add_edge(from, to, 0, 0).map(|_| ()),
                    _ => Err(crate::error::Error::Custom(
                        "add_edge: from_port and to_port must both be Some or both None"
                            .to_string(),
                    )),
                };
                let _ = reply.send(res);
            }
            DataEngineCmd::DeleteEdge {
                from,
                from_port,
                to,
                to_port,
                reply,
            } => {
                if self.running.load(Ordering::SeqCst) {
                    let _ = reply.send(Err(crate::error::Error::Custom(
                        "DAG is currently running; wait for it to complete before modifying the graph"
                            .to_string(),
                    )));
                    return;
                }
                let mut engine = self
                    .engine
                    .try_lock()
                    .expect("uncontended: running flag is false");
                let _ = reply.send(engine.delete_edge(from, to, from_port, to_port));
            }
            DataEngineCmd::AddNode {
                id,
                kind,
                spec,
                reply,
            } => {
                if self.running.load(Ordering::SeqCst) {
                    let _ = reply.send(Err(crate::error::Error::Custom(
                        "DAG is currently running; wait for it to complete before modifying the graph"
                            .to_string(),
                    )));
                    return;
                }
                let mut engine = self
                    .engine
                    .try_lock()
                    .expect("uncontended: running flag is false");
                let _ = reply.send(engine.add_node_from_registry(id, &kind, spec));
            }
            DataEngineCmd::UpdateNode { id, spec, reply } => {
                if self.running.load(Ordering::SeqCst) {
                    let _ = reply.send(Err(crate::error::Error::Custom(
                        "DAG is currently running; wait for it to complete before modifying the graph"
                            .to_string(),
                    )));
                    return;
                }
                let mut engine = self
                    .engine
                    .try_lock()
                    .expect("uncontended: running flag is false");
                let _ = reply.send(engine.update_node(id, spec));
            }
            DataEngineCmd::RemoveNode { id, reply } => {
                if self.running.load(Ordering::SeqCst) {
                    let _ = reply.send(Err(crate::error::Error::Custom(
                        "DAG is currently running; wait for it to complete before modifying the graph"
                            .to_string(),
                    )));
                    return;
                }
                let mut engine = self
                    .engine
                    .try_lock()
                    .expect("uncontended: running flag is false");
                let _ = reply.send(engine.remove_node(id).map(|_| ()));
            }
            DataEngineCmd::ClearDag { reply } => {
                if self.running.load(Ordering::SeqCst) {
                    let _ = reply.send(Err(crate::error::Error::Custom(
                        "DAG is currently running; wait for it to complete before clearing"
                            .to_string(),
                    )));
                    return;
                }
                let mut engine = self
                    .engine
                    .try_lock()
                    .expect("uncontended: running flag is false");
                let result = engine.clear_dag().await;
                let _ = reply.send(result);
            }

            DataEngineCmd::ReleaseOutputs { reply } => {
                if self.running.load(Ordering::SeqCst) {
                    let _ = reply.send(Err(crate::error::Error::Custom(
                        "DAG is currently running; cannot release outputs".to_string(),
                    )));
                    return;
                }
                let mut engine = self
                    .engine
                    .try_lock()
                    .expect("uncontended: running flag is false");
                let released = engine.release_output_cache();
                let _ = reply.send(Ok(released));
            }
            DataEngineCmd::Shutdown { reply } => {
                if self.running.load(Ordering::SeqCst) {
                    let _ = reply.send(Ok(false));
                    return;
                }
                {
                    let mut engine = self
                        .engine
                        .try_lock()
                        .expect("uncontended: running flag is false");
                    engine.release_output_cache();
                }
                // Shared storage references are released by the manager after
                // the shutdown ack, guarded against a newer actor generation
                // having recreated this agent id in the meantime.
                self.shutdown.store(true, Ordering::SeqCst);
                let _ = reply.send(Ok(true));
            }

            // ── Read-only engine inspection ──────────────────────────────
            DataEngineCmd::GetOutput { id, reply } => {
                if self.running.load(Ordering::SeqCst) {
                    let _ = reply.send(Err(crate::error::Error::Custom(
                        "DAG is currently running; query outputs after it completes".to_string(),
                    )));
                    return;
                }
                let engine = self
                    .engine
                    .try_lock()
                    .expect("uncontended: running flag is false");
                let _ = reply.send(Ok(engine.get_output(id).await));
            }
            DataEngineCmd::ReadFile { path, reply } => {
                if self.running.load(Ordering::SeqCst) {
                    let _ = reply.send(Err(crate::error::Error::Custom(
                        "DAG is currently running; read artifact files after it completes"
                            .to_string(),
                    )));
                    return;
                }
                let engine = self
                    .engine
                    .try_lock()
                    .expect("uncontended: running flag is false");
                let _ = reply.send(engine.read_file(&path).await);
            }
            DataEngineCmd::GetNodeStatus { id, reply } => {
                if self.running.load(Ordering::SeqCst) {
                    let _ = reply.send(Err(crate::error::Error::Custom(
                        "DAG is currently running; use event stream for progress".to_string(),
                    )));
                    return;
                }
                let engine = self
                    .engine
                    .try_lock()
                    .expect("uncontended: running flag is false");
                let _ = reply.send(Ok(engine.node_status(&id)));
            }
            DataEngineCmd::GetNode { id, reply } => {
                if self.running.load(Ordering::SeqCst) {
                    let _ = reply.send(Err(crate::error::Error::Custom(
                        "DAG is currently running; query the node after it completes".to_string(),
                    )));
                    return;
                }
                let engine = self
                    .engine
                    .try_lock()
                    .expect("uncontended: running flag is false");
                let _ = reply.send(Ok(engine.get_node(&id)));
            }
            DataEngineCmd::NodeExists { id, reply } => {
                if self.running.load(Ordering::SeqCst) {
                    let _ = reply.send(Err(crate::error::Error::Custom(
                        "DAG is currently running; query the node after it completes".to_string(),
                    )));
                    return;
                }
                let engine = self
                    .engine
                    .try_lock()
                    .expect("uncontended: running flag is false");
                let _ = reply.send(Ok(engine.node_exists(&id)));
            }
            DataEngineCmd::ViewDag { reply } => {
                if self.live_valid.load(Ordering::SeqCst) {
                    let _ = reply.send(
                        self.live_view
                            .snapshot()
                            .map(|snapshot| snapshot.to_dot())
                            .ok_or_else(|| {
                                crate::error::Error::Custom(
                                    "DAG live snapshot is not available".to_string(),
                                )
                            }),
                    );
                    return;
                }
                let engine = self.engine.lock().await;
                let _ = reply.send(engine.view_dag());
            }
            DataEngineCmd::GetDagTuiSnapshot { reply } => {
                if self.live_valid.load(Ordering::SeqCst) {
                    let _ = reply.send(self.live_view.snapshot().ok_or_else(|| {
                        crate::error::Error::Custom(
                            "DAG live snapshot is not available".to_string(),
                        )
                    }));
                    return;
                }
                let engine = self.engine.lock().await;
                let _ = reply.send(engine.dag_tui_snapshot());
            }
            // ── History / ref management (async, may hold lock across await) ──
            DataEngineCmd::NewDagRef { name, reply } => {
                if self.running.load(Ordering::SeqCst) {
                    let _ = reply.send(Err(crate::error::Error::Custom(
                        "DAG is currently running; wait for completion before managing refs"
                            .to_string(),
                    )));
                    return;
                }
                let mut engine = self
                    .engine
                    .try_lock()
                    .expect("uncontended: running flag is false");
                let _ = reply.send(engine.new_dag_ref(&name).await);
            }
            DataEngineCmd::SwitchDagRef { name, reply } => {
                if self.running.load(Ordering::SeqCst) {
                    let _ = reply.send(Err(crate::error::Error::Custom(
                        "DAG is currently running; wait for completion before switching refs"
                            .to_string(),
                    )));
                    return;
                }
                let mut engine = self
                    .engine
                    .try_lock()
                    .expect("uncontended: running flag is false");
                let _ = reply.send(engine.switch_dag_ref(&name).await);
            }
            DataEngineCmd::ListDagRefs { reply } => {
                if self.running.load(Ordering::SeqCst) {
                    let _ = reply.send(Err(crate::error::Error::Custom(
                        "DAG is currently running; try again after completion".to_string(),
                    )));
                    return;
                }
                let engine = self
                    .engine
                    .try_lock()
                    .expect("uncontended: running flag is false");
                let _ = reply.send(engine.list_dag_refs().await);
            }
            DataEngineCmd::DagLog {
                ref_name,
                limit,
                reply,
            } => {
                if self.running.load(Ordering::SeqCst) {
                    let _ = reply.send(Err(crate::error::Error::Custom(
                        "DAG is currently running; try again after completion".to_string(),
                    )));
                    return;
                }
                let engine = self
                    .engine
                    .try_lock()
                    .expect("uncontended: running flag is false");
                let _ = reply.send(engine.dag_log(ref_name.as_deref(), limit).await);
            }
            DataEngineCmd::DagRunsLog {
                ref_name,
                limit,
                run_id,
                reply,
            } => {
                if self.running.load(Ordering::SeqCst) {
                    let _ = reply.send(Err(crate::error::Error::Custom(
                        "DAG is currently running; try again after completion".to_string(),
                    )));
                    return;
                }
                let engine = self
                    .engine
                    .try_lock()
                    .expect("uncontended: running flag is false");
                let result = if let Some(run_id) = run_id {
                    if run_id.trim().eq_ignore_ascii_case("latest") {
                        engine
                            .list_runs(1, None)
                            .await
                            .map(|mut runs| runs.pop().into_iter().collect())
                    } else {
                        engine
                            .get_run(run_id.trim())
                            .await
                            .map(|run| run.into_iter().collect())
                    }
                } else {
                    engine.list_runs(limit, ref_name.as_deref()).await
                };
                let _ = reply.send(result);
            }
            DataEngineCmd::ExportRun {
                run_id,
                format,
                out_dir,
                reply,
            } => {
                if self.running.load(Ordering::SeqCst) {
                    let _ = reply.send(Err(crate::error::Error::Custom(
                        "DAG is currently running; try again after completion".to_string(),
                    )));
                    return;
                }
                let engine = self
                    .engine
                    .try_lock()
                    .expect("uncontended: running flag is false");
                let result = match crate::dag::ExportFormat::parse(&format) {
                    Ok(format) => engine.export_run(&run_id, format, out_dir).await,
                    Err(error) => Err(crate::error::Error::Custom(error)),
                };
                let _ = reply.send(result);
            }
            DataEngineCmd::CheckoutDag { snapshot_id, reply } => {
                if self.running.load(Ordering::SeqCst) {
                    let _ = reply.send(Err(crate::error::Error::Custom(
                        "DAG is currently running; wait for completion before checkout".to_string(),
                    )));
                    return;
                }
                let mut engine = self
                    .engine
                    .try_lock()
                    .expect("uncontended: running flag is false");
                let _ = reply.send(engine.checkout_dag(&snapshot_id).await);
            }
            DataEngineCmd::BranchFromSnapshot {
                snapshot_id,
                ref_name,
                reply,
            } => {
                if self.running.load(Ordering::SeqCst) {
                    let _ = reply.send(Err(crate::error::Error::Custom(
                        "DAG is currently running; wait for completion before branching"
                            .to_string(),
                    )));
                    return;
                }
                let mut engine = self
                    .engine
                    .try_lock()
                    .expect("uncontended: running flag is false");
                let _ = reply.send(engine.branch_from_snapshot(&snapshot_id, &ref_name).await);
            }
            DataEngineCmd::GetDagRef { reply } => {
                if self.running.load(Ordering::SeqCst) {
                    let _ = reply.send(Err(crate::error::Error::Custom(
                        "DAG is currently running; try again after completion".to_string(),
                    )));
                    return;
                }
                let engine = self
                    .engine
                    .try_lock()
                    .expect("uncontended: running flag is false");
                let _ = reply.send(Ok(engine.history_ref().to_string()));
            }
            DataEngineCmd::GetSnapshot { snapshot_id, reply } => {
                if self.running.load(Ordering::SeqCst) {
                    let _ = reply.send(Err(crate::error::Error::Custom(
                        "DAG is currently running; try again after completion".to_string(),
                    )));
                    return;
                }
                let engine = self
                    .engine
                    .try_lock()
                    .expect("uncontended: running flag is false");
                let _ = reply.send(engine.get_snapshot(&snapshot_id).await);
            }
            DataEngineCmd::DiffSnapshots {
                old_id,
                new_id,
                reply,
            } => {
                if self.running.load(Ordering::SeqCst) {
                    let _ = reply.send(Err(crate::error::Error::Custom(
                        "DAG is currently running; try again after completion".to_string(),
                    )));
                    return;
                }
                let engine = self
                    .engine
                    .try_lock()
                    .expect("uncontended: running flag is false");
                let _ = reply.send(engine.diff_snapshots(&old_id, &new_id).await);
            }
        }
    }
}

/// Stable content hash of an authored flow, used as the run record's
/// manifest identity (canonical JSON of the serialized graph).
fn flow_content_hash(flow: &dag_engine::FlowGraph) -> String {
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(flow).unwrap_or_default();
    format!("{:x}", Sha256::digest(bytes))
}

// ═══════════════════════════════════════════════════════════════════════
// Session workspace auto-restore
// ═══════════════════════════════════════════════════════════════════════

/// Env gate for the actor-start workspace auto-restore (default on). Set
/// `AUTONOMICS_SESSION_AUTO_RESTORE=0|false|off|disabled` to start every
/// agent with an empty DAG regardless of snapshot history.
const SESSION_AUTO_RESTORE_ENV: &str = "AUTONOMICS_SESSION_AUTO_RESTORE";

fn agent_workspace_auto_restore_enabled() -> bool {
    agent_workspace_auto_restore_from(std::env::var_os(SESSION_AUTO_RESTORE_ENV).as_deref())
}

fn agent_workspace_auto_restore_from(value: Option<&std::ffi::OsStr>) -> bool {
    !value.is_some_and(|value| {
        matches!(
            value.to_string_lossy().trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "off" | "disabled"
        )
    })
}

// ═══════════════════════════════════════════════════════════════════════
// Client (Layer 1: metadata bypass)
// ═══════════════════════════════════════════════════════════════════════

/// Wraps a `oneshot::Receiver` for a DAG run reply so that **dropping the
/// receiver cancels the in-progress DAG run**.
///
/// When the agent task is cancelled (e.g. user interrupts), the tool's
/// future — and therefore this receiver — is dropped. The `Drop`
/// implementation fires the shared `CancellationToken`, which the spawned
/// DAG run task in `AgentEngineActor` listens on via `select!`. Without this
/// guard the DAG task would continue running in the background, holding the
/// engine mutex and leaving `running = true` forever.
pub struct CancelOnDropReceiver<T> {
    rx: tokio::sync::oneshot::Receiver<std::result::Result<T, crate::error::Error>>,
    cancel: CancellationToken,
}

impl<T> std::future::Future for CancelOnDropReceiver<T> {
    type Output = std::result::Result<
        std::result::Result<T, crate::error::Error>,
        tokio::sync::oneshot::error::RecvError,
    >;

    fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        std::pin::Pin::new(&mut self.rx).poll(cx)
    }
}

impl<T> Drop for CancelOnDropReceiver<T> {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// Channel-based client for the DataEngine actor.
///
/// Each client carries an `agent_id` that routes to the right per-agent
/// `AgentEngineActor`. Metadata queries (`list_node_factories`, `get_node_spec`,
/// `get_node_ports`, `get_node_doc`) bypass the actor entirely — they read
/// from the shared `Arc<NodeRegistry>` synchronously and therefore never
/// block on a running DAG.
#[derive(Clone)]
pub struct DataEngineClient {
    tx: mpsc::UnboundedSender<EngineMsg>,
    /// Shared, immutable node registry. Cloned once at actor creation;
    /// subsequent metadata queries are zero-cost reads with no channel hop.
    node_registry: Arc<NodeRegistry>,
    agent_id: String,
    activity: Arc<AgentActivityTracker>,
}

impl DataEngineClient {
    /// Returns the agent ID this client routes to.
    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }

    // ── Metadata queries (Layer 1: synchronous, bypass actor) ────────────

    /// List metadata of every registered node kind (kind + JSON Schema).
    ///
    /// Reads directly from the shared `NodeRegistry` — never blocks on a
    /// running DAG.
    pub fn list_node_factories(&self) -> Result<Vec<crate::node_registry::NodeInfo>> {
        let mut nodes = self.node_registry.list_nodes();
        nodes.push(crate::data_engine::channel_info());
        Ok(nodes)
    }

    /// Get the JSON Schema for a specific node kind. Synchronous, never blocks.
    pub fn get_node_spec(&self, kind: &str) -> Result<schemars::Schema> {
        if kind == "channel" { return crate::data_engine::channel_schema().map_err(ClientError::Engine); }
        self.node_registry
            .get_node_spec(kind)
            .map_err(|e| ClientError::Engine(crate::error::Error::from(e)))
    }

    /// Get the input/output port layout of a node kind. Synchronous, never blocks.
    pub fn get_node_ports(&self, kind: &str) -> Result<crate::nodes::meta::NodePorts> {
        if kind == "channel" { return crate::data_engine::channel_ports(None).map_err(ClientError::Engine); }
        self.node_registry
            .get_node_ports(kind)
            .map_err(|e| ClientError::Engine(crate::error::Error::from(e)))
    }

    /// Get the concrete port layout for a node kind and spec. Pass a spec for
    /// dynamic-port kinds such as `container_command`.
    pub fn get_node_ports_for_spec(
        &self,
        kind: &str,
        spec: serde_json::Value,
    ) -> Result<crate::nodes::meta::NodePorts> {
        if kind == "channel" { return crate::data_engine::channel_ports(Some(&spec)).map_err(ClientError::Engine); }
        self.node_registry
            .get_node_ports_for_spec(kind, spec)
            .map_err(|e| ClientError::Engine(crate::error::Error::from(e)))
    }

    /// Get the documentation string for a node kind. Synchronous, never blocks.
    pub fn get_node_doc(&self, kind: &str) -> Result<String> {
        if kind == "channel" { return Ok(crate::data_engine::CHANNEL_DOC.into()); }
        self.node_registry
            .get_node_doc(kind)
            .map_err(|e| ClientError::Engine(crate::error::Error::from(e)))
    }

    // ── Actor-routed commands ─────────────────────────────────────────────

    async fn request<T, Rx>(&self, cmd: DataEngineCmd, reply_rx: Rx) -> Result<T>
    where
        Rx: std::future::Future<
                Output = std::result::Result<
                    std::result::Result<T, crate::error::Error>,
                    tokio::sync::oneshot::error::RecvError,
                >,
            >,
    {
        self.activity.touch(&self.agent_id);
        self.tx
            .send(EngineMsg { cmd })
            .map_err(|_| ClientError::ServerClosed)?;
        reply_rx
            .await
            .map_err(|_| ClientError::ServerClosed)?
            .map_err(Into::into)
    }

    /// Connect two nodes using their default (single) ports.
    pub async fn add_edge(&self, from: String, to: String) -> Result<()> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(
            DataEngineCmd::AddEdge {
                from,
                from_port: None,
                to,
                to_port: None,
                reply: reply_tx,
            },
            reply_rx,
        )
        .await
    }

    /// Connect `from`'s `from_port` output to `to`'s `to_port` input.
    pub async fn add_edge_port(
        &self,
        from: String,
        from_port: u8,
        to: String,
        to_port: u8,
    ) -> Result<()> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(
            DataEngineCmd::AddEdge {
                from,
                from_port: Some(from_port),
                to,
                to_port: Some(to_port),
                reply: reply_tx,
            },
            reply_rx,
        )
        .await
    }



    /// Export one recorded run as provenance evidence (PROV-JSON / RO-Crate).
    /// `format` is `prov` or `crate`; `out_dir` must be absolute.
    pub async fn export_run(
        &self,
        run_id: String,
        format: String,
        out_dir: std::path::PathBuf,
    ) -> Result<crate::dag::ExportSummary> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(
            DataEngineCmd::ExportRun {
                run_id,
                format,
                out_dir,
                reply: reply_tx,
            },
            reply_rx,
        )
        .await
    }

    pub async fn checkout_dag(&self, snapshot_id: String) -> Result<()> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(
            DataEngineCmd::CheckoutDag {
                snapshot_id,
                reply: reply_tx,
            },
            reply_rx,
        )
        .await
    }

    pub async fn get_dag_ref(&self) -> Result<String> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(DataEngineCmd::GetDagRef { reply: reply_tx }, reply_rx)
            .await
    }

    pub async fn get_snapshot(&self, snapshot_id: String) -> Result<Option<crate::dag::Snapshot>> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(
            DataEngineCmd::GetSnapshot {
                snapshot_id,
                reply: reply_tx,
            },
            reply_rx,
        )
        .await
    }

    pub async fn diff_snapshots(&self, old_id: String, new_id: String) -> Result<String> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(
            DataEngineCmd::DiffSnapshots {
                old_id,
                new_id,
                reply: reply_tx,
            },
            reply_rx,
        )
        .await
    }

    pub async fn add_node(&self, id: String, kind: String, spec: serde_json::Value) -> Result<()> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(
            DataEngineCmd::AddNode {
                id,
                kind,
                spec,
                reply: reply_tx,
            },
            reply_rx,
        )
        .await
    }

    pub async fn run_dag_shell(
        &self,
        script: String,
        dry_run: bool,
        timeout_ms: Option<u64>,
    ) -> Result<crate::dag_shell::DagShellOutcome> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(
            DataEngineCmd::RunDagShell {
                script,
                dry_run,
                timeout_ms,
                reply: reply_tx,
            },
            reply_rx,
        )
        .await
    }

    /// Execute a stage-model flow: resolve sources through the VFS, build
    /// the instance plan, and run it through the executor stack. `flow` of
    /// `None` runs the session's stored flow. The reply receiver cancels the
    /// run when dropped, aborting instances promptly.
    pub async fn run_flow(
        &self,
        flow: Option<dag_engine::FlowGraph>,
        options: crate::flow::FlowOptions,
    ) -> Result<dag_engine::FlowRunReport> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let cancel = CancellationToken::new();
        let wrapped = CancelOnDropReceiver {
            rx: reply_rx,
            cancel: cancel.clone(),
        };
        self.request(
            DataEngineCmd::RunFlow {
                flow,
                options,
                reply: reply_tx,
                cancel_token: cancel,
            },
            wrapped,
        )
        .await
    }

    /// Replace the session's authored flow. Validated atomically: an
    /// invalid graph is rejected and leaves the previous flow intact.
    pub async fn set_flow(&self, flow: dag_engine::FlowGraph) -> Result<()> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(
            DataEngineCmd::SetFlow { flow, reply: reply_tx },
            reply_rx,
        )
        .await
    }

    /// Read the session's current authored flow.
    /// Create a ref pointing at an existing snapshot.
    pub async fn branch_from_snapshot(
        &self,
        snapshot_id: String,
        ref_name: String,
    ) -> Result<()> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(
            DataEngineCmd::BranchFromSnapshot {
                snapshot_id,
                ref_name,
                reply: reply_tx,
            },
            reply_rx,
        )
        .await
    }

    pub async fn get_flow(&self) -> Result<dag_engine::FlowGraph> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(
            DataEngineCmd::GetFlow { reply: reply_tx },
            reply_rx,
        )
        .await
    }

    /// Read the flow view: the authored flow plus the last run's grouped
    /// report, for TUI / GUI / gateway surfaces.
    pub async fn get_flow_view(&self) -> Result<crate::flow::FlowView> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(
            DataEngineCmd::GetFlowView { reply: reply_tx },
            reply_rx,
        )
        .await
    }


    pub async fn get_output(&self, id: String) -> Result<Option<crate::dag::graph::PortOutputs>> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(
            DataEngineCmd::GetOutput {
                id,
                reply: reply_tx,
            },
            reply_rx,
        )
        .await
    }

    /// Read artifact bytes for one output path (`vfs://` URI or absolute
    /// host path). See [`DataEngineCmd::ReadFile`].
    pub async fn read_file(&self, path: String) -> Result<Vec<u8>> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(
            DataEngineCmd::ReadFile {
                path,
                reply: reply_tx,
            },
            reply_rx,
        )
        .await
    }

    pub async fn get_node_status(
        &self,
        id: String,
    ) -> Result<Option<crate::dag::runtime::RuntimeStatus>> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(
            DataEngineCmd::GetNodeStatus {
                id,
                reply: reply_tx,
            },
            reply_rx,
        )
        .await
    }

    /// Alias for [`get_node_status`](Self::get_node_status).
    pub async fn node_status(
        &self,
        id: String,
    ) -> Result<Option<crate::dag::runtime::RuntimeStatus>> {
        self.get_node_status(id).await
    }

    /// Get the retained `(kind, spec)` of an existing node instance by id.
    ///
    /// Returns `None` if the node does not exist or has no retained spec.
    pub async fn get_node(&self, id: String) -> Result<Option<(String, serde_json::Value)>> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(
            DataEngineCmd::GetNode {
                id,
                reply: reply_tx,
            },
            reply_rx,
        )
        .await
    }

    /// Whether a node with `id` exists in the DAG (regardless of whether it
    /// has a retained spec). Routed through the actor so it reflects the live
    /// DAG state.
    pub async fn node_exists(&self, id: String) -> Result<bool> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(
            DataEngineCmd::NodeExists {
                id,
                reply: reply_tx,
            },
            reply_rx,
        )
        .await
    }

    pub async fn remove_node(&self, id: String) -> Result<()> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(
            DataEngineCmd::RemoveNode {
                id,
                reply: reply_tx,
            },
            reply_rx,
        )
        .await
    }

    pub async fn view_dag(&self) -> Result<String> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(DataEngineCmd::ViewDag { reply: reply_tx }, reply_rx)
            .await
    }

    /// Return the structured snapshot consumed by interactive DAG rendering.
    pub async fn dag_tui_snapshot(&self) -> Result<crate::dag::DagTuiSnapshot> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(
            DataEngineCmd::GetDagTuiSnapshot { reply: reply_tx },
            reply_rx,
        )
        .await
    }

    pub async fn clear_dag(&self) -> Result<crate::data_engine::ClearDagOutcome> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(DataEngineCmd::ClearDag { reply: reply_tx }, reply_rx)
            .await
    }

    pub async fn new_dag_ref(&self, name: String) -> Result<()> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(
            DataEngineCmd::NewDagRef {
                name,
                reply: reply_tx,
            },
            reply_rx,
        )
        .await
    }

    pub async fn switch_dag_ref(&self, name: String) -> Result<()> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(
            DataEngineCmd::SwitchDagRef {
                name,
                reply: reply_tx,
            },
            reply_rx,
        )
        .await
    }

    pub async fn list_dag_refs(&self) -> Result<Vec<(String, String, bool)>> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(DataEngineCmd::ListDagRefs { reply: reply_tx }, reply_rx)
            .await
    }

    pub async fn dag_log(
        &self,
        ref_name: Option<String>,
        limit: usize,
    ) -> Result<Vec<crate::dag::Snapshot>> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(
            DataEngineCmd::DagLog {
                ref_name,
                limit,
                reply: reply_tx,
            },
            reply_rx,
        )
        .await
    }

    /// Query the execution audit trail. Without `run_id`: recent runs
    /// (newest-first, optionally filtered by `ref_name`). With `run_id`: that
    /// single run, including its full run report.
    pub async fn dag_runs_log(
        &self,
        ref_name: Option<String>,
        limit: usize,
        run_id: Option<String>,
    ) -> Result<Vec<crate::dag::RunRecord>> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(
            DataEngineCmd::DagRunsLog {
                ref_name,
                limit,
                run_id,
                reply: reply_tx,
            },
            reply_rx,
        )
        .await
    }

    /// Export one recorded run as provenance evidence (PROV-JSON / RO-Crate).
    pub async fn update_node(&self, id: String, spec: serde_json::Value) -> Result<()> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(
            DataEngineCmd::UpdateNode {
                id,
                spec,
                reply: reply_tx,
            },
            reply_rx,
        )
        .await
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Manager (Layer 3: per-agent actors)
// ═══════════════════════════════════════════════════════════════════════

/// Lazily spawns a dedicated `AgentEngineActor` (tokio task + channel) per agent
/// agent, all sharing the same heavy infrastructure from the template engine.
///
/// This provides **full cross-agent isolation**: a long DAG run in agent-A's
/// actor never blocks agent-B's commands, and a panic in one agent's handler
/// does not affect others.
pub struct DataEngineManager {
    template: DataEngine,
    node_registry: Arc<NodeRegistry>,
    actors: std::sync::Mutex<HashMap<String, SessionHandle>>,
    activity: Arc<AgentActivityTracker>,
    result_cache: Arc<ResultCache>,
    storage: Arc<StorageCoordinator>,
    policy: MaintenancePolicy,
}

struct SessionHandle {
    tx: mpsc::UnboundedSender<EngineMsg>,
    running: Arc<AtomicBool>,
    /// Mirrors the actor's `closing` flag: once set, `client_for_agent`
    /// stops handing out this generation and spawns a fresh actor instead.
    closing: Arc<AtomicBool>,
    _task: JoinHandle<()>,
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
            result_cache: Arc::new(ResultCache::new(
                policy.result_cache_max_entries,
                Some(policy.result_cache_ttl),
            )),
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
    pub fn reload_plugin(&self, plugin: container_plugin::factory::Plugin) {
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
            return DataEngineClient {
                tx: handle.tx.clone(),
                node_registry: Arc::clone(&self.node_registry),
                agent_id: agent_id.to_string(),
                activity: Arc::clone(&self.activity),
            };
        }

        // Slow path: create new actor.
        // Use the caller-provided history_ref so each agent gets its own
        // snapshot lineage instead of all committing to "main".
        let new_engine = self
            .template
            .new_isolated_engine()
            .with_history_ref(history_ref);
        let live_view = new_engine.dag().live_view.clone();
        let (tx, rx) = mpsc::unbounded_channel::<EngineMsg>();
        let running = Arc::new(AtomicBool::new(false));
        let shutdown = Arc::new(AtomicBool::new(false));
        let closing = Arc::new(AtomicBool::new(false));
        let server = AgentEngineActor {
            agent_id: agent_id.to_string(),
            engine: Arc::new(tokio::sync::Mutex::new(new_engine)),
            live_view,
            live_valid: Arc::new(AtomicBool::new(false)),
            running: Arc::clone(&running),
            shutdown: Arc::clone(&shutdown),
            closing: Arc::clone(&closing),
            rx,
        };
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

        DataEngineClient {
            tx,
            node_registry: Arc::clone(&self.node_registry),
            agent_id: agent_id.to_string(),
            activity: Arc::clone(&self.activity),
        }
    }

    pub fn agent_count(&self) -> usize {
        self.actors.lock().expect("actors mutex poisoned").len()
    }

    /// Release retained outputs for idle actors without stopping them.
    pub async fn release_idle_outputs(&self) -> usize {
        let targets = {
            let actors = self.actors.lock().expect("actors mutex poisoned");
            actors
                .iter()
                .filter(|(id, handle)| {
                    !handle.running.load(Ordering::SeqCst)
                        && self.activity.is_idle(id, self.policy.output_cache_idle)
                })
                .map(|(id, handle)| (id.clone(), handle.tx.clone()))
                .collect::<Vec<_>>()
        };
        let mut released = 0;
        for (agent_id, tx) in targets {
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            if tx
                .send(EngineMsg {
                    cmd: DataEngineCmd::ReleaseOutputs { reply: reply_tx },
                })
                .is_err()
            {
                continue;
            }
            if matches!(reply_rx.await, Ok(Ok(true))) {
                tracing::info!(agent_id, "released idle DAG output cache");
                released += 1;
            }
        }
        released
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
            // acknowledging shutdown. Only clear shared refs if it did not.
            let recreated = self
                .actors
                .lock()
                .expect("actors mutex poisoned")
                .contains_key(&agent_id);
            if !recreated {
                self.activity.forget(&agent_id);
                self.storage.release_agent(&agent_id);
            }
        }
        evicted
    }

    /// Run one lifecycle pass and enforce the central local-storage budget.
    ///
    /// Order is fixed: release idle output caches → evict idle actors →
    /// prune the result cache (dropping its storage references) → enforce the
    /// local budget with run-aware cleanup. VFS artifacts are only ever
    /// reported by that cleanup, never deleted.
    pub async fn maintain_once(
        &self,
    ) -> std::result::Result<(usize, usize, Option<RunAwareCleanReport>), String> {
        let released = self.release_idle_outputs().await;
        let evicted = self.evict_idle_actors().await;
        let retired = self.result_cache.prune_retired();
        if !retired.is_empty() {
            self.storage.release_cache_owner(&retired);
        }
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
        Ok((released, evicted, clean))
    }

    /// Point-in-time storage view: actors, result cache, local managed
    /// bytes, tracked VFS artifacts and the reference index snapshot.
    pub fn storage_status(&self) -> StorageStatus {
        StorageStatus {
            agent_count: self.agent_count(),
            result_cache_entries: self.result_cache.len(),
            local_bytes: self.storage.managed_bytes(),
            artifact_count: self.storage.artifact_count(),
            artifact_bytes: self.storage.artifact_bytes(),
            references: self.storage.reference_snapshot(),
            artifacts: self.storage.artifact_snapshot(),
        }
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
                    Ok((released, evicted, clean)) => {
                        tracing::info!(
                            output_caches_released = released,
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

// ═══════════════════════════════════════════════════════════════════════
// Legacy spawn helper (backward compatibility for tests)
// ═══════════════════════════════════════════════════════════════════════

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
    use crate::dag::graph::PortOutputs;
    use crate::data_engine::DataEngine;
    use crate::nodes::meta::NodePorts;

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
        let graph = dag_engine::FlowGraph::builder()
            .process(dag_engine::Process::new(
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
        let (first_report, second_report) = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            async {
                tokio::join!(
                    first.run_flow(Some(graph.clone()), options.clone()),
                    second.run_flow(Some(graph), options)
                )
            },
        )
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
        let graph = dag_engine::FlowGraph::builder()
            .source(dag_engine::Source::items(
                "files",
                vec![serde_json::json!(file.to_string_lossy())],
            ))
            .process(dag_engine::Process::new(
                "file",
                "file_reference",
                serde_json::json!({"path":"{{item}}"}),
            ))
            .wire(dag_engine::Wire::direct("files", 0, "file", 0))
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
            let found = first.dag_runs_log(None, 20, Some(prefix.into())).await.unwrap();
            assert_eq!(found[0].id, *initial.run_id.as_ref().unwrap());
        }
        let audit = dag_core::dag::history::DagHistory::open(&db).await.unwrap();
        let record = audit.get_run(initial.run_id.as_deref().unwrap()).await.unwrap().unwrap();
        let persisted: serde_json::Value = serde_json::from_str(record.run_report_json.as_deref().unwrap()).unwrap();
        assert_eq!(persisted["nodes"][0]["logical_node"], "file");
        assert_eq!(persisted["flow"], serde_json::to_value(&graph).unwrap());
        assert_eq!(persisted["nodes"][0]["output_files"][0]["path"], file.to_string_lossy().as_ref());
        assert!(persisted["nodes"][0]["execution"]["workspace"].is_string());
        assert_eq!(manager.evict_idle_actors().await, 1);
        let recreated = manager.client_for_agent("resume-agent-a");
        let restored_view = recreated.get_flow_view().await.unwrap();
        assert_eq!(restored_view.last_run.unwrap().processes["file"].succeeded, 1);
        let after_eviction = recreated
            .run_flow(None, opts.clone())
            .await
            .unwrap();
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
        assert_eq!(same.get_flow_view().await.unwrap().last_run.unwrap().processes["file"].succeeded, 1);
        assert!(other.get_flow_view().await.unwrap().last_run.is_none());
        let restored = same
            .run_flow(None, opts.clone())
            .await
            .unwrap();
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
        let graph = dag_engine::FlowGraph::builder()
            .process(dag_engine::Process::new(
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
        let history = dag_core::dag::history::DagHistory::open_in_memory().await.unwrap();
        let (client, _handle) = spawn_with_engine(DataEngine::builder().build().with_history(history.clone()));
        let graph = dag_engine::FlowGraph::builder()
            .process(dag_engine::Process::new("broken", "missing_kind", serde_json::json!({})))
            .build().unwrap();
        let error = client.run_flow(Some(graph), crate::flow::FlowOptions::default()).await.unwrap_err();
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
        let graph = dag_engine::FlowGraph::builder()
            .process(dag_engine::Process::new(
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
        let flow = dag_engine::FlowGraph::builder()
            .process(dag_engine::Process::new("slow", "file_reference", serde_json::json!({"path":path.to_string_lossy()})))
            .build().unwrap();
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let mut template = DataEngine::builder().build();
        template.set_task_executor(Arc::new(RendezvousExecutor {
            barrier: barrier.clone(),
            calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            inner: dag_core::local_task_executor(),
        }));
        let manager = DataEngineManager::with_policy_and_roots(template, zero_idle_policy(), directory.path().join("work"), directory.path().join("receipts"));
        let client = manager.client_for_agent("live-test");
        let running_client = client.clone();
        let task = tokio::spawn(async move { running_client.run_flow(Some(flow), crate::flow::FlowOptions::default()).await });
        let snapshot = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let snapshot = client.dag_tui_snapshot().await.unwrap();
                if snapshot.running && snapshot.node("slow").is_some_and(|n| n.status == crate::dag::runtime::RuntimeStatus::Running) {
                    break snapshot;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }).await.expect("live flow snapshot should be readable while the task runs");
        assert!(snapshot.run_id.is_some());
        assert!(client.view_dag().await.unwrap().contains("slow"));
        barrier.wait().await;
        assert!(task.await.unwrap().unwrap().ok);
        let finished = client.dag_tui_snapshot().await.unwrap();
        assert!(!finished.running);
        assert_eq!(finished.node("slow").unwrap().status, crate::dag::runtime::RuntimeStatus::Success);
    }

    #[test]
    fn auto_restore_env_defaults_on_and_parses_off_values() {
        use std::ffi::OsStr;
        assert!(agent_workspace_auto_restore_from(None));
        assert!(agent_workspace_auto_restore_from(Some(OsStr::new("1"))));
        assert!(agent_workspace_auto_restore_from(Some(OsStr::new(" on "))));
        for off in ["0", "false", "OFF", "disabled", " off "] {
            assert!(
                !agent_workspace_auto_restore_from(Some(OsStr::new(off))),
                "`{off}` must disable auto-restore"
            );
        }
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
            output_cache_idle: std::time::Duration::ZERO,
            result_cache_ttl: std::time::Duration::from_secs(60),
            result_cache_max_entries: 16,
            maintenance_interval: None,
            clean_keep_runs: 2,
            storage_max_bytes: 0,
        }
    }

    fn iris_csv_path() -> String {
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        std::path::Path::new(&manifest_dir)
            .join("test_datasets/Iris.csv")
            .to_string_lossy()
            .into_owned()
    }

    /// Install a `file_to_dataframe → dataframe_to_file` pipeline in `client`
    /// writing to `out`. Both node kinds come from the shared registry, so
    /// every session sees the same fingerprints for identical specs.
    async fn install_file_pipeline(client: &DataEngineClient, out: &std::path::Path) {
        client
            .add_node(
                "src".to_string(),
                "file_to_dataframe".to_string(),
                serde_json::json!({ "path": iris_csv_path() }),
            )
            .await
            .unwrap();
        client
            .add_node(
                "sink".to_string(),
                "dataframe_to_file".to_string(),
                serde_json::json!({ "path": out.display().to_string(), "format": "csv" }),
            )
            .await
            .unwrap();
        client
            .add_edge("src".to_string(), "sink".to_string())
            .await
            .unwrap();
    }

    fn node_report<'a>(
        report: &'a crate::dag::RunReport,
        id: &str,
    ) -> &'a crate::dag::runtime::NodeReport {
        report
            .nodes
            .iter()
            .find(|node| node.id == id)
            .unwrap_or_else(|| panic!("run report must contain `{id}`: {report:?}"))
    }






}
