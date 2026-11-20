//! The per-agent `DataEngine` actor (Layers 2 + 3 of the runtime isolation).
//!
//! One instance per agent: a dedicated tokio task driving an mpsc command
//! loop over a private, lazily-created engine. The engine sits behind a
//! `Mutex` so that `RunFlow` can be spawned as a background task while the
//! actor loop keeps processing other commands. The `running` flag prevents
//! concurrent DAG runs and lets mutation commands fast-fail instead of
//! blocking; `closing` marks an evicted generation whose queued runs must be
//! rejected. Startup restores the persisted flow workspace, result cache and
//! last-run report from the shared DAG history.

use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::FutureExt;
use tokio::sync::mpsc;

use crate::data_engine::DataEngine;
use crate::runtime::types::{DataEngineCmd, EngineMsg};

pub(super) struct AgentEngineActor {
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
    pub(super) fn new(
        agent_id: String,
        engine: Arc<tokio::sync::Mutex<DataEngine>>,
        live_view: crate::dag::live::LiveDagView,
        running: Arc<AtomicBool>,
        shutdown: Arc<AtomicBool>,
        closing: Arc<AtomicBool>,
        rx: mpsc::UnboundedReceiver<EngineMsg>,
    ) -> Self {
        Self {
            agent_id,
            engine,
            live_view,
            live_valid: Arc::new(AtomicBool::new(false)),
            running,
            shutdown,
            closing,
            rx,
        }
    }

    pub(super) async fn run(mut self) {
        Self::restore_flow_cache(&self.engine, &self.agent_id).await;
        Self::restore_flow_workspace(&self.engine, &self.agent_id).await;
        Self::restore_flow_report(&self.engine, &self.agent_id).await;
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
                    .and_then(crate::workflow::InstanceCache::from_json);
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
            Ok(Some(json)) => match serde_json::from_str::<crate::workflow::FlowGraph>(&json) {
                Ok(flow) if crate::workflow::dataflow::Dataflow::<()>::new(&flow).is_ok() => {
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
        flow: &crate::workflow::FlowGraph,
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

    async fn apply_flow_edit(
        engine: &DataEngine,
        agent_id: &str,
        edit: crate::dag_shell::GraphEditOp,
    ) -> crate::error::Result<()> {
        let candidate = engine.preview_flow_edits(&[edit])?;
        Self::persist_flow_workspace(engine, agent_id, &candidate).await?;
        *engine.flow.write().expect("flow lock") = candidate;
        *engine.flow_report.write().expect("flow report lock") = None;
        Ok(())
    }

    async fn handle(&self, msg: EngineMsg) {
        let EngineMsg { cmd } = msg;

        // Any idle command may change the engine; invalidate the last run's
        // projection conservatively before serving it. Run-time commands cannot
        // mutate the graph and must not hide the active projection.
        if !self.running.load(Ordering::SeqCst)
            && !matches!(
                &cmd,
                DataEngineCmd::GetDagTuiSnapshot { .. }
                    | DataEngineCmd::GetNodeStatus { .. }
                    | DataEngineCmd::GetNode { .. }
                    | DataEngineCmd::NodeExists { .. }
            )
        {
            self.live_valid.store(false, Ordering::SeqCst);
        }

        match cmd {
            // ── Stage-model flow session state ───────────────────────────
            DataEngineCmd::SetFlow { flow, reply } => {
                if self.running.load(Ordering::SeqCst) {
                    let _ = reply.send(Err(crate::error::Error::Custom(
                        "a flow is running; wait before editing it".into(),
                    )));
                    return;
                }
                let engine = self.engine.lock().await;
                let result = match crate::workflow::dataflow::Dataflow::<()>::new(&flow) {
                    Err(error) => Err(crate::error::Error::Custom(error)),
                    Ok(_) => {
                        match Self::persist_flow_workspace(&engine, &self.agent_id, &flow).await {
                            Ok(()) => {
                                if flow_content_hash(&engine.flow.read().expect("flow lock"))
                                    != flow_content_hash(&flow)
                                {
                                    *engine.flow_report.write().expect("flow report lock") = None;
                                }
                                *engine.flow.write().expect("flow lock") = flow;
                                Ok(())
                            }
                            Err(error) => Err(error),
                        }
                    }
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
                    last_run: engine.flow_report.read().expect("flow report lock").clone(),
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
                        "a run is already active for this agent; wait for it to complete"
                            .to_string(),
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
                                if let Err(error) =
                                    crate::workflow::dataflow::Dataflow::<()>::new(&flow)
                                {
                                    let _ = reply
                                        .send(Err(crate::error::Error::Custom(error.to_string())));
                                    running.store(false, Ordering::SeqCst);
                                    return;
                                }
                                if let Err(error) =
                                    Self::persist_flow_workspace(&engine, &agent_id, &flow).await
                                {
                                    let _ = reply.send(Err(error));
                                    running.store(false, Ordering::SeqCst);
                                    return;
                                }
                                if flow_content_hash(&engine.flow.read().expect("flow lock"))
                                    != flow_content_hash(&flow)
                                {
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
                    let mut result =
                        AssertUnwindSafe(runner.run(&flow, &options, Some(cancel_token.clone())))
                            .catch_unwind()
                            .await
                            .unwrap_or_else(|panic| {
                                Err(crate::error::Error::Custom(format!(
                                    "flow run panicked: {panic:?}"
                                )))
                            });
                    if let Ok(report) = &result {
                        use dag_core::dag::runtime::RuntimeStatus;
                        for source in flow.sources() {
                            live_view.status(
                                &source.id,
                                if report.cancelled {
                                    RuntimeStatus::Cancelled
                                } else {
                                    RuntimeStatus::Success
                                },
                            );
                        }
                        for process in flow.processes() {
                            let summary = report.processes.get(&process.id);
                            let status = if report.cancelled {
                                RuntimeStatus::Cancelled
                            } else if summary.is_some_and(|s| s.failed > 0) {
                                RuntimeStatus::Failed
                            } else if summary.is_some_and(|s| s.cancelled > 0) {
                                RuntimeStatus::Cancelled
                            } else if summary
                                .is_none_or(|s| s.total == 0 || (s.skipped > 0 && s.succeeded == 0))
                            {
                                RuntimeStatus::Skipped
                            } else {
                                RuntimeStatus::Success
                            };
                            live_view.status(&process.id, status);
                        }
                        live_view.end();
                        let engine = engine_for_report.lock().await;
                        *engine.flow_report.write().expect("flow report lock") =
                            Some(crate::flow::FlowRunReportSummary::from(report));
                    }
                    if result.is_err() {
                        live_view.cancel();
                    }
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
                            cancelled: report.map_or_else(
                                || cancel_token.is_cancelled(),
                                |report| report.cancelled,
                            ),
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
                            if let Ok(report) = &mut result {
                                report.audit_persisted = false;
                            }
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
                    if let Err(error) =
                        Self::persist_flow_workspace(&engine, &self.agent_id, &flow).await
                    {
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
                let engine = self.engine.lock().await;
                let ports = match (from_port, to_port) {
                    (Some(fp), Some(tp)) => Some((fp, tp)),
                    (None, None) => Some((0, 0)),
                    _ => None,
                };
                let res = if let Some((from_port, to_port)) = ports {
                    Self::apply_flow_edit(
                        &engine,
                        &self.agent_id,
                        crate::dag_shell::GraphEditOp::AddEdge {
                            from,
                            from_port,
                            to,
                            to_port,
                        },
                    )
                    .await
                } else {
                    Err(crate::error::Error::Custom(
                        "add_edge: from_port and to_port must both be Some or both None".into(),
                    ))
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
                let engine = self
                    .engine
                    .try_lock()
                    .expect("uncontended: running flag is false");
                let _ = reply.send(
                    Self::apply_flow_edit(
                        &engine,
                        &self.agent_id,
                        crate::dag_shell::GraphEditOp::RemoveEdge {
                            from,
                            from_port,
                            to,
                            to_port,
                        },
                    )
                    .await,
                );
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
                let engine = self
                    .engine
                    .try_lock()
                    .expect("uncontended: running flag is false");
                let result = DataEngine::ensure_node_kind_allowed(&kind).and_then(|()| {
                    engine
                        .get_node_ports_for_spec(&kind, spec.clone())
                        .map(|_| ())
                });
                let result = match result {
                    Ok(()) => {
                        Self::apply_flow_edit(
                            &engine,
                            &self.agent_id,
                            crate::dag_shell::GraphEditOp::AddNode { id, kind, spec },
                        )
                        .await
                    }
                    Err(error) => Err(error),
                };
                let _ = reply.send(result);
            }
            DataEngineCmd::UpdateNode { id, spec, reply } => {
                if self.running.load(Ordering::SeqCst) {
                    let _ = reply.send(Err(crate::error::Error::Custom(
                        "DAG is currently running; wait for it to complete before modifying the graph"
                            .to_string(),
                    )));
                    return;
                }
                let engine = self
                    .engine
                    .try_lock()
                    .expect("uncontended: running flag is false");
                let kind = engine
                    .flow
                    .read()
                    .expect("flow lock")
                    .process(&id)
                    .map(|node| node.kind.clone());
                let result = match kind {
                    Some(kind) => engine
                        .get_node_ports_for_spec(&kind, spec.clone())
                        .map(|_| ()),
                    None => Err(crate::error::Error::Custom(format!(
                        "unknown process `{id}`"
                    ))),
                };
                let result = match result {
                    Ok(()) => {
                        Self::apply_flow_edit(
                            &engine,
                            &self.agent_id,
                            crate::dag_shell::GraphEditOp::UpdateNode { id, spec },
                        )
                        .await
                    }
                    Err(error) => Err(error),
                };
                let _ = reply.send(result);
            }
            DataEngineCmd::RemoveNode { id, reply } => {
                if self.running.load(Ordering::SeqCst) {
                    let _ = reply.send(Err(crate::error::Error::Custom(
                        "DAG is currently running; wait for it to complete before modifying the graph"
                            .to_string(),
                    )));
                    return;
                }
                let engine = self
                    .engine
                    .try_lock()
                    .expect("uncontended: running flag is false");
                let _ = reply.send(
                    Self::apply_flow_edit(
                        &engine,
                        &self.agent_id,
                        crate::dag_shell::GraphEditOp::RemoveNode { id, cascade: true },
                    )
                    .await,
                );
            }
            DataEngineCmd::Shutdown { reply } => {
                if self.running.load(Ordering::SeqCst) {
                    let _ = reply.send(Ok(false));
                    return;
                }
                // Shared storage references are released by the manager after
                // the shutdown ack, guarded against a newer actor generation
                // having recreated this agent id in the meantime.
                self.shutdown.store(true, Ordering::SeqCst);
                let _ = reply.send(Ok(true));
            }

            // ── Read-only engine inspection ──────────────────────────────
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
                let status = self
                    .live_valid
                    .load(Ordering::SeqCst)
                    .then(|| self.live_view.snapshot())
                    .flatten()
                    .and_then(|snapshot| snapshot.node(&id).map(|node| node.status));
                let _ = reply.send(Ok(status));
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
                let graph = engine.flow.read().expect("flow lock");
                let node = graph
                    .process(&id)
                    .map(|node| (node.kind.clone(), node.spec.clone()))
                    .or_else(|| {
                        graph.source(&id).and_then(|source| {
                            serde_json::to_value(&source.spec)
                                .ok()
                                .map(|spec| ("__channel_source".into(), spec))
                        })
                    });
                let _ = reply.send(Ok(node));
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
                let graph = engine.flow.read().expect("flow lock");
                let _ = reply.send(Ok(
                    graph.process(&id).is_some() || graph.source(&id).is_some()
                ));
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
                let graph = engine.flow.read().expect("flow lock");
                let _ = reply.send(Ok(crate::flow::flow_snapshot(&graph)));
            }
            // ── Execution audit trail (async, may hold lock across await) ──
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
        }
    }
}

/// Stable content hash of an authored flow, used as the run record's
/// manifest identity (canonical JSON of the serialized graph).
fn flow_content_hash(flow: &crate::workflow::FlowGraph) -> String {
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(flow).unwrap_or_default();
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    use crate::data_engine::DataEngine;

    /// Barrier executor used to hold a flow run open while the test inspects
    /// live actor state through the client.
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

    #[cfg(target_os = "linux")]
    fn zero_idle_policy() -> crate::runtime::MaintenancePolicy {
        crate::runtime::MaintenancePolicy {
            agent_idle: std::time::Duration::ZERO,
            maintenance_interval: None,
            clean_keep_runs: 2,
            storage_max_bytes: 0,
        }
    }

    #[tokio::test]
    async fn latest_failed_run_does_not_restore_an_older_report() {
        let directory = tempfile::tempdir().unwrap();
        let history =
            dag_core::dag::history::DagHistory::open(&directory.path().join("history.db"))
                .await
                .unwrap();
        let manager = crate::runtime::DataEngineManager::with_policy_and_roots(
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
        let (client, _handle) = crate::runtime::spawn_with_engine(
            DataEngine::builder().build().with_history(history.clone()),
        );
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
        let (client, _handle) = crate::runtime::spawn_with_engine(DataEngine::builder().build());
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
        let manager = crate::runtime::DataEngineManager::with_policy_and_roots(
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
        let (client, _handle) = crate::runtime::spawn_with_engine(DataEngine::builder().build());
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
        let (client, _handle) = crate::runtime::spawn_with_engine(engine);

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
}
