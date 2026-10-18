//! Runtime of Data Engine based on tokio runtime.
//!
//! # Architecture (three-layer isolation)
//!
//! **Layer 1 — Metadata bypass.** Read-only node-registry queries
//! (`list_node_factories`, `get_node_spec`, `get_node_ports`, `get_node_doc`)
//! are served directly from `Arc<NodeRegistry>` on the client side and never
//! enter the actor channel, so they cannot be blocked by a running DAG.
//!
//! **Layer 2 — RunDag fire-and-forget.** `RunDag` is spawned as a background
//! task inside the session actor; the actor loop returns immediately and keeps
//! processing subsequent commands. A per-session `AtomicBool` guards against
//! concurrent runs; mutation commands during a run return an immediate error
//! instead of blocking.
//!
//! **Layer 3 — Per-agent actors.** Each agent session gets its own tokio task
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
use crate::runtime::types::{DataEngineCmd, EngineMsg, RunDagOptions};

pub mod error;
pub mod storage;
pub mod types;

pub use storage::{
    ArtifactEntry, CachedFileOutput, MaintenancePolicy, ReferenceEntry, ResultCache,
    RunAwareCleanReport, SessionTracker, StorageCoordinator, StorageStatus,
};

// ═══════════════════════════════════════════════════════════════════════
// Per-session actor (Layer 2 + 3)
// ═══════════════════════════════════════════════════════════════════════

/// Single-session DataEngine actor.
///
/// One instance per agent session. The engine is behind a `Mutex` so that
/// `RunDag` can be spawned as a background task while the actor loop continues
/// processing other commands. The `running` flag prevents concurrent DAG runs
/// and allows mutation commands to fast-fail instead of blocking.
struct SessionServer {
    session_id: String,
    engine: Arc<tokio::sync::Mutex<DataEngine>>,
    running: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    /// Set by the manager when this session generation is being evicted.
    /// Queued runs are rejected instead of executing on a session the manager
    /// has already removed from its map.
    closing: Arc<AtomicBool>,
    result_cache: Arc<ResultCache>,
    storage: Arc<StorageCoordinator>,
    rx: mpsc::UnboundedReceiver<EngineMsg>,
}

impl SessionServer {
    async fn run(mut self) {
        Self::restore_session_workspace(&self.engine, &self.session_id).await;
        while let Some(msg) = self.rx.recv().await {
            if let Err(panic) = AssertUnwindSafe(self.handle(msg)).catch_unwind().await {
                tracing::error!(
                    session_id = %self.session_id,
                    "session actor handler panicked: {:?}",
                    panic
                );
            }
            if self.shutdown.load(Ordering::SeqCst) {
                break;
            }
        }
        tracing::debug!(session_id = %self.session_id, "session actor exited");
    }

    /// Materialize the history ref's head snapshot into a freshly created
    /// session so a restarted agent resumes its previous DAG instead of an
    /// empty workspace. Runs before the actor loop, so it can never race a
    /// client command. Failures are logged and swallowed: an unrestorable
    /// history must not stop the session from serving commands.
    async fn restore_session_workspace(engine: &tokio::sync::Mutex<DataEngine>, session_id: &str) {
        if !session_auto_restore_enabled() {
            return;
        }
        let mut engine = engine.lock().await;
        match engine.restore_ref_head().await {
            Ok(true) => tracing::info!(session_id, "restored DAG from history ref head"),
            Ok(false) => {}
            Err(error) => tracing::warn!(
                session_id,
                error = %error,
                "could not restore DAG from history ref head; starting with an empty workspace"
            ),
        }
    }

    async fn handle(&self, msg: EngineMsg) {
        let EngineMsg { cmd, .. } = msg;

        match cmd {
            // ── DAG execution (fire-and-forget) ──────────────────────────
            DataEngineCmd::RunDag {
                event_tx,
                commit_message,
                trigger,
                options,
                reply,
                cancel_token,
            } => {
                if self.closing.load(Ordering::SeqCst) {
                    let _ = reply.send(Err(crate::error::Error::Custom(
                        "session is shutting down; reconnect to start a new run".to_string(),
                    )));
                    return;
                }
                if self.running.swap(true, Ordering::SeqCst) {
                    let _ = reply.send(Err(crate::error::Error::Custom(
                        "DAG is already running for this session; wait for the current run to complete"
                            .to_string(),
                    )));
                    return;
                }
                // Seed matching file outputs from the manager-wide fingerprint
                // cache before the run task takes the engine lock. DataFrame
                // results intentionally remain session-local.
                {
                    let entries = self.result_cache.snapshot();
                    if !entries.is_empty() {
                        let mut engine = self.engine.lock().await;
                        let restored = engine.restore_cached_file_outputs(&entries).await;
                        if restored > 0 {
                            tracing::debug!(
                                session_id = %self.session_id,
                                restored,
                                "restored cross-session file outputs"
                            );
                        }
                    }
                }
                let engine = Arc::clone(&self.engine);
                let running = Arc::clone(&self.running);
                let session_id = self.session_id.clone();
                let result_cache = Arc::clone(&self.result_cache);
                let storage = Arc::clone(&self.storage);
                tokio::spawn(async move {
                    // Race the DAG run against cancellation. When the caller
                    // drops the reply receiver (e.g. the agent task is
                    // cancelled), `cancel_token` fires and we abort early —
                    // releasing the engine mutex and resetting `running`
                    // instead of leaving the session stuck.
                    let run_fut = AssertUnwindSafe(async {
                        let mut engine = engine.lock().await;
                        engine.set_commit_message(commit_message);
                        engine.set_run_trigger(trigger);
                        if let Some(resume) = options.resume {
                            engine.set_incremental(resume);
                        }
                        if let Some(input_hashing) = options.input_hashing {
                            engine.set_input_hashing(input_hashing);
                        }
                        if let Some(wave_size) = options.wave_size {
                            engine.set_wave_size(wave_size)?;
                        }
                        match event_tx {
                            Some(sink) => {
                                engine
                                    .run_with_events_and_cancel(sink, Some(cancel_token.clone()))
                                    .await
                            }
                            None => {
                                let (sink, _dropped_sink) = mpsc::channel(1);
                                engine
                                    .run_with_events_and_cancel(sink, Some(cancel_token.clone()))
                                    .await
                            }
                        }
                    });

                    let result = tokio::select! {
                        r = run_fut.catch_unwind() => r,
                        _ = cancel_token.cancelled() => {
                            tracing::info!(
                                "DAG run cancelled by caller; releasing engine lock"
                            );
                            running.store(false, Ordering::SeqCst);
                            // reply receiver is already gone — send fails silently.
                            let _ = reply.send(Err(crate::error::Error::Custom(
                                "DAG run cancelled".to_string(),
                            )));
                            return;
                        }
                    };

                    running.store(false, Ordering::SeqCst);

                    match result {
                        Ok(res) => {
                            if let Ok(report) = &res {
                                if report.ok {
                                    let (cached, displaced) =
                                        result_cache.record_report_collected(&session_id, report);
                                    if !cached.is_empty() {
                                        storage.retain_cache_owner(&cached);
                                    }
                                    if !displaced.is_empty() {
                                        storage.release_cache_owner(&displaced);
                                    }
                                }
                                storage.retain_report(&session_id, report);
                            }
                            let _ = reply.send(res);
                        }
                        Err(panic) => {
                            let _ = reply.send(Err(crate::error::Error::Custom(format!(
                                "DAG run panicked: {:?}",
                                panic
                            ))));
                        }
                    }
                });
            }

            // ── Commands that need exclusive engine access ───────────────
            //
            // When a DAG is running, the mutex is held by the run task.
            // Instead of blocking, these commands fast-fail so the actor
            // loop stays responsive for other messages (and other sessions).
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
                let _ = reply.send(Ok(engine.run_dag_shell(&script, dry_run, timeout_ms)));
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
            DataEngineCmd::AddLogicalGraph { graph, reply } => {
                if self.running.load(Ordering::SeqCst) {
                    let _ = reply.send(Err(crate::error::Error::Custom(
                        "DAG is currently running; wait for it to complete before installing a logical graph"
                            .to_string(),
                    )));
                    return;
                }
                let mut engine = self
                    .engine
                    .try_lock()
                    .expect("uncontended: running flag is false");
                let _ = reply.send(engine.add_logical_graph(graph));
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
                // the shutdown ack, guarded against a newer session generation
                // having recreated this session id in the meantime.
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
                if self.running.load(Ordering::SeqCst) {
                    let _ = reply.send(Err(crate::error::Error::Custom(
                        "DAG is currently running; view it after completion".to_string(),
                    )));
                    return;
                }
                let engine = self
                    .engine
                    .try_lock()
                    .expect("uncontended: running flag is false");
                let _ = reply.send(engine.view_dag());
            }
            DataEngineCmd::GetDagTuiSnapshot { reply } => {
                if self.running.load(Ordering::SeqCst) {
                    let _ = reply.send(Err(crate::error::Error::Custom(
                        "DAG is currently running; view it after completion".to_string(),
                    )));
                    return;
                }
                let engine = self
                    .engine
                    .try_lock()
                    .expect("uncontended: running flag is false");
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

// ═══════════════════════════════════════════════════════════════════════
// Session workspace auto-restore
// ═══════════════════════════════════════════════════════════════════════

/// Env gate for the session-start auto-restore (default on). Set
/// `AUTONOMICS_SESSION_AUTO_RESTORE=0|false|off|disabled` to start every
/// session with an empty DAG regardless of snapshot history.
const SESSION_AUTO_RESTORE_ENV: &str = "AUTONOMICS_SESSION_AUTO_RESTORE";

fn session_auto_restore_enabled() -> bool {
    session_auto_restore_from(std::env::var_os(SESSION_AUTO_RESTORE_ENV).as_deref())
}

fn session_auto_restore_from(value: Option<&std::ffi::OsStr>) -> bool {
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
/// DAG run task in `SessionServer` listens on via `select!`. Without this
/// guard the DAG task would continue running in the background, holding the
/// engine mutex and leaving `running = true` forever.
pub struct CancelOnDropReceiver {
    rx: tokio::sync::oneshot::Receiver<
        std::result::Result<crate::dag::RunReport, crate::error::Error>,
    >,
    cancel: CancellationToken,
}

impl std::future::Future for CancelOnDropReceiver {
    type Output = std::result::Result<
        std::result::Result<crate::dag::RunReport, crate::error::Error>,
        tokio::sync::oneshot::error::RecvError,
    >;

    fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        std::pin::Pin::new(&mut self.rx).poll(cx)
    }
}

impl Drop for CancelOnDropReceiver {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// Channel-based client for the DataEngine actor.
///
/// Each client carries a `session_id` that routes to the right per-agent
/// `SessionServer`. Metadata queries (`list_node_factories`, `get_node_spec`,
/// `get_node_ports`, `get_node_doc`) bypass the actor entirely — they read
/// from the shared `Arc<NodeRegistry>` synchronously and therefore never
/// block on a running DAG.
#[derive(Clone)]
pub struct DataEngineClient {
    tx: mpsc::UnboundedSender<EngineMsg>,
    /// Shared, immutable node registry. Cloned once at session creation;
    /// subsequent metadata queries are zero-cost reads with no channel hop.
    node_registry: Arc<NodeRegistry>,
    session_id: String,
    activity: Arc<SessionTracker>,
}

impl DataEngineClient {
    /// Returns the session ID this client routes to.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    // ── Metadata queries (Layer 1: synchronous, bypass actor) ────────────

    /// List metadata of every registered node kind (kind + JSON Schema).
    ///
    /// Reads directly from the shared `NodeRegistry` — never blocks on a
    /// running DAG.
    pub fn list_node_factories(&self) -> Result<Vec<crate::node_registry::NodeInfo>> {
        Ok(self.node_registry.list_nodes())
    }

    /// Get the JSON Schema for a specific node kind. Synchronous, never blocks.
    pub fn get_node_spec(&self, kind: &str) -> Result<schemars::Schema> {
        self.node_registry
            .get_node_spec(kind)
            .map_err(|e| ClientError::Engine(crate::error::Error::from(e)))
    }

    /// Get the input/output port layout of a node kind. Synchronous, never blocks.
    pub fn get_node_ports(&self, kind: &str) -> Result<crate::nodes::meta::NodePorts> {
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
        self.node_registry
            .get_node_ports_for_spec(kind, spec)
            .map_err(|e| ClientError::Engine(crate::error::Error::from(e)))
    }

    /// Get the documentation string for a node kind. Synchronous, never blocks.
    pub fn get_node_doc(&self, kind: &str) -> Result<String> {
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
        self.activity.touch(&self.session_id);
        self.tx
            .send(EngineMsg {
                session_id: self.session_id.clone(),
                cmd,
            })
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

    /// Remove the matching edge. Both endpoints and ports must identify an
    /// existing edge.
    pub async fn delete_edge(
        &self,
        from: String,
        from_port: u8,
        to: String,
        to_port: u8,
    ) -> Result<()> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(
            DataEngineCmd::DeleteEdge {
                from,
                from_port,
                to,
                to_port,
                reply: reply_tx,
            },
            reply_rx,
        )
        .await
    }

    pub async fn run_dag(&self, trigger: Option<String>) -> Result<crate::dag::RunReport> {
        self.run_dag_with_options(trigger, RunDagOptions::default())
            .await
    }

    pub async fn run_dag_with_options(
        &self,
        trigger: Option<String>,
        options: RunDagOptions,
    ) -> Result<crate::dag::RunReport> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let cancel = CancellationToken::new();
        let wrapped = CancelOnDropReceiver {
            rx: reply_rx,
            cancel: cancel.clone(),
        };
        self.request(
            DataEngineCmd::RunDag {
                event_tx: None,
                commit_message: None,
                trigger,
                options,
                reply: reply_tx,
                cancel_token: cancel,
            },
            wrapped,
        )
        .await
    }

    /// Kick off a DAG run with live per-node event streaming.
    ///
    /// Returns `(event_rx, reply_rx)`: the caller drains `event_rx`
    /// concurrently with awaiting `reply_rx` (the final [`RunReport`]).
    /// Events are lightweight observations (status/progress/log/finished) and
    /// may be dropped on a full channel — they never affect the run's outcome.
    ///
    /// `reply_rx` is a [`CancelOnDropReceiver`]: dropping it (e.g. when the
    /// agent task is cancelled) cancels the DAG run and releases the engine
    /// lock promptly.
    pub fn run_dag_stream(
        &self,
        commit_message: Option<String>,
        trigger: Option<String>,
    ) -> (
        mpsc::Receiver<crate::dag::node_event::NodeEvent>,
        CancelOnDropReceiver,
    ) {
        self.run_dag_stream_with_options(commit_message, trigger, RunDagOptions::default())
    }

    pub fn run_dag_stream_with_options(
        &self,
        commit_message: Option<String>,
        trigger: Option<String>,
        options: RunDagOptions,
    ) -> (
        mpsc::Receiver<crate::dag::node_event::NodeEvent>,
        CancelOnDropReceiver,
    ) {
        let (event_tx, event_rx) = mpsc::channel::<crate::dag::node_event::NodeEvent>(128);
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let cancel = CancellationToken::new();
        let _ = self.tx.send(EngineMsg {
            session_id: self.session_id.clone(),
            cmd: DataEngineCmd::RunDag {
                event_tx: Some(event_tx),
                commit_message,
                trigger,
                options,
                reply: reply_tx,
                cancel_token: cancel.clone(),
            },
        });
        (
            event_rx,
            CancelOnDropReceiver {
                rx: reply_rx,
                cancel,
            },
        )
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

    pub async fn branch_from_snapshot(&self, snapshot_id: String, ref_name: String) -> Result<()> {
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

    pub async fn add_logical_graph(
        &self,
        graph: crate::dag::LogicalGraph,
    ) -> Result<crate::data_engine::LogicalInstallReport> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(
            DataEngineCmd::AddLogicalGraph {
                graph,
                reply: reply_tx,
            },
            reply_rx,
        )
        .await
    }

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

/// Lazily spawns a dedicated `SessionServer` (tokio task + channel) per agent
/// session, all sharing the same heavy infrastructure from the template engine.
///
/// This provides **full cross-agent isolation**: a long DAG run in agent-A's
/// actor never blocks agent-B's commands, and a panic in one session's handler
/// does not affect others.
pub struct DataEngineManager {
    template: DataEngine,
    node_registry: Arc<NodeRegistry>,
    sessions: std::sync::Mutex<HashMap<String, SessionHandle>>,
    activity: Arc<SessionTracker>,
    result_cache: Arc<ResultCache>,
    storage: Arc<StorageCoordinator>,
    policy: MaintenancePolicy,
}

struct SessionHandle {
    tx: mpsc::UnboundedSender<EngineMsg>,
    running: Arc<AtomicBool>,
    /// Mirrors the actor's `closing` flag: once set, `client_for_session`
    /// stops handing out this generation and creates a fresh session instead.
    closing: Arc<AtomicBool>,
    _task: JoinHandle<()>,
}

impl DataEngineManager {
    /// Create a manager from a template engine. The template's
    /// `NodeRegistry`, `RuntimeEnv`, `DagHistory`, etc. are shared across all
    /// lazily-created sessions via `Arc` / `Clone`.
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
            sessions: std::sync::Mutex::new(HashMap::new()),
            activity: Arc::new(SessionTracker::default()),
            result_cache: Arc::new(ResultCache::new(
                policy.result_cache_max_entries,
                Some(policy.result_cache_ttl),
            )),
            storage: Arc::new(StorageCoordinator::new(work_root, receipt_root)),
            policy,
        }
    }

    /// Node kinds available to every session — a snapshot of the shared
    /// registry. Management surfaces (the gateway's plugin/node listing)
    /// use this; per-session introspection goes through a session client.
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

    /// Get (or lazily create) a [`DataEngineClient`] for `session_id`.
    ///
    /// The first call for a given session spawns a dedicated tokio task.
    /// Subsequent calls return a cheap clone of the same channel sender.
    ///
    /// This method is synchronous because the internal sessions map uses a
    /// `std::sync::Mutex` — it is never held across an `.await` point, so
    /// there is no risk of blocking the executor.
    pub fn client_for_session(&self, session_id: &str) -> DataEngineClient {
        self.client_for_session_with_ref(session_id, session_id)
    }

    /// Like [`client_for_session`](Self::client_for_session) but lets the
    /// caller choose the default DAG history ref for the new session.
    ///
    /// When the session already exists, `history_ref` is ignored (the
    /// existing session's ref is kept).  When creating a new session, the
    /// engine's `history_ref` is set to `history_ref` instead of the
    /// default `"main"` — this is what gives each agent its own isolated
    /// snapshot lineage in the shared history database.
    pub fn client_for_session_with_ref(
        &self,
        session_id: &str,
        history_ref: &str,
    ) -> DataEngineClient {
        let mut sessions = self.sessions.lock().expect("sessions mutex poisoned");

        // Fast path: session already exists and is not being evicted. A
        // closing generation is treated as absent so callers transparently
        // get a fresh session (new generation) with the same id.
        if let Some(handle) = sessions.get(session_id)
            && !handle.closing.load(Ordering::SeqCst)
        {
            self.activity.touch(session_id);
            return DataEngineClient {
                tx: handle.tx.clone(),
                node_registry: Arc::clone(&self.node_registry),
                session_id: session_id.to_string(),
                activity: Arc::clone(&self.activity),
            };
        }

        // Slow path: create new session.
        // Use the caller-provided history_ref so each agent gets its own
        // snapshot lineage instead of all committing to "main".
        let new_engine = self.template.new_session().with_history_ref(history_ref);
        let (tx, rx) = mpsc::unbounded_channel::<EngineMsg>();
        let running = Arc::new(AtomicBool::new(false));
        let shutdown = Arc::new(AtomicBool::new(false));
        let closing = Arc::new(AtomicBool::new(false));
        let server = SessionServer {
            session_id: session_id.to_string(),
            engine: Arc::new(tokio::sync::Mutex::new(new_engine)),
            running: Arc::clone(&running),
            shutdown: Arc::clone(&shutdown),
            closing: Arc::clone(&closing),
            result_cache: Arc::clone(&self.result_cache),
            storage: Arc::clone(&self.storage),
            rx,
        };
        let task = tokio::task::spawn(server.run());

        tracing::info!(session_id, history_ref, "created new DAG session");
        sessions.insert(
            session_id.to_string(),
            SessionHandle {
                tx: tx.clone(),
                running,
                closing,
                _task: task,
            },
        );
        drop(sessions);

        DataEngineClient {
            tx,
            node_registry: Arc::clone(&self.node_registry),
            session_id: session_id.to_string(),
            activity: Arc::clone(&self.activity),
        }
    }

    pub fn session_count(&self) -> usize {
        self.sessions.lock().expect("sessions mutex poisoned").len()
    }

    /// Release retained outputs for idle sessions without stopping them.
    pub async fn release_idle_outputs(&self) -> usize {
        let targets = {
            let sessions = self.sessions.lock().expect("sessions mutex poisoned");
            sessions
                .iter()
                .filter(|(id, handle)| {
                    !handle.running.load(Ordering::SeqCst)
                        && self.activity.is_idle(id, self.policy.output_cache_idle)
                })
                .map(|(id, handle)| (id.clone(), handle.tx.clone()))
                .collect::<Vec<_>>()
        };
        let mut released = 0;
        for (session_id, tx) in targets {
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            if tx
                .send(EngineMsg {
                    session_id: session_id.clone(),
                    cmd: DataEngineCmd::ReleaseOutputs { reply: reply_tx },
                })
                .is_err()
            {
                continue;
            }
            if matches!(reply_rx.await, Ok(Ok(true))) {
                tracing::info!(session_id, "released idle DAG output cache");
                released += 1;
            }
        }
        released
    }

    /// Stop idle session actors. Active runs are never evicted.
    ///
    /// Only the current generation is evicted: targets are marked `closing`
    /// under the sessions lock, so any client asking for the same session id
    /// afterwards receives a freshly created session whose storage references
    /// are never released by the old actor's shutdown.
    pub async fn evict_idle_sessions(&self) -> usize {
        let targets = {
            let mut sessions = self.sessions.lock().expect("sessions mutex poisoned");
            let mut targets = Vec::new();
            for (id, handle) in sessions.iter_mut() {
                if !handle.running.load(Ordering::SeqCst)
                    && self.activity.is_idle(id, self.policy.session_idle)
                {
                    // Mark closing inside the lock: a client racing this
                    // eviction will create a new generation instead of
                    // queueing work on the doomed actor.
                    handle.closing.store(true, Ordering::SeqCst);
                    targets.push((id.clone(), handle.tx.clone()));
                }
            }
            for (id, _) in &targets {
                sessions.remove(id);
            }
            targets
        };
        let mut evicted = 0;
        for (session_id, tx) in targets {
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            if tx
                .send(EngineMsg {
                    session_id: session_id.clone(),
                    cmd: DataEngineCmd::Shutdown { reply: reply_tx },
                })
                .is_err()
            {
                continue;
            }
            if matches!(reply_rx.await, Ok(Ok(true))) {
                evicted += 1;
                tracing::info!(session_id, "evicted idle DAG session");
            }
            // A client may have recreated the session while the old actor was
            // acknowledging shutdown. Only clear shared refs if it did not.
            let recreated = self
                .sessions
                .lock()
                .expect("sessions mutex poisoned")
                .contains_key(&session_id);
            if !recreated {
                self.activity.forget(&session_id);
                self.storage.release_session(&session_id);
            }
        }
        evicted
    }

    /// Run one lifecycle pass and enforce the central local-storage budget.
    ///
    /// Order is fixed: release idle output caches → evict idle sessions →
    /// prune the result cache (dropping its storage references) → enforce the
    /// local budget with run-aware cleanup. VFS artifacts are only ever
    /// reported by that cleanup, never deleted.
    pub async fn maintain_once(
        &self,
    ) -> std::result::Result<(usize, usize, Option<RunAwareCleanReport>), String> {
        let released = self.release_idle_outputs().await;
        let evicted = self.evict_idle_sessions().await;
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

    /// Point-in-time storage view: sessions, result cache, local managed
    /// bytes, tracked VFS artifacts and the reference index snapshot.
    pub fn storage_status(&self) -> StorageStatus {
        StorageStatus {
            session_count: self.session_count(),
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
                            sessions_evicted = evicted,
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

/// Spawn a single-session DataEngine actor and return a client for the
/// `"default"` session.
///
/// This is a convenience wrapper around [`DataEngineManager`] for code that
/// does not need multi-session support (e.g. tests). New code should use
/// `DataEngineManager::new` + `client_for_session` directly.
pub fn spawn_with_engine(engine: DataEngine) -> (DataEngineClient, JoinHandle<()>) {
    let manager = Box::leak(Box::new(DataEngineManager::new(engine)));
    let client = manager.client_for_session("default");

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

    /// A node that sleeps for 2s — long enough to be interrupted by cancellation.
    #[derive(Clone)]
    struct LongSleepNode(NodePorts);
    #[async_trait::async_trait]
    impl crate::nodes::DagNode for LongSleepNode {
        fn ports(&self) -> &NodePorts {
            &self.0
        }
        fn clone_box(&self) -> Box<dyn crate::nodes::DagNode> {
            Box::new((*self).clone())
        }
        fn kind(&self) -> &'static str {
            "long_sleep"
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        async fn execute(
            &mut self,
            _ctx: &crate::node_registry::registry::NodeCtx,
            _inputs: &[crate::nodes::NodeInput],
            _reporter: &crate::dag::node_event::NodeReporter,
        ) -> std::result::Result<PortOutputs, crate::dag::DagError> {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            Ok(PortOutputs::new())
        }
    }

    #[test]
    fn auto_restore_env_defaults_on_and_parses_off_values() {
        use std::ffi::OsStr;
        assert!(session_auto_restore_from(None));
        assert!(session_auto_restore_from(Some(OsStr::new("1"))));
        assert!(session_auto_restore_from(Some(OsStr::new(" on "))));
        for off in ["0", "false", "OFF", "disabled", " off "] {
            assert!(
                !session_auto_restore_from(Some(OsStr::new(off))),
                "`{off}` must disable auto-restore"
            );
        }
    }

    /// A brand-new session must already see the ref's head snapshot in its
    /// first command: `SessionServer` restores the workspace before entering
    /// its recv loop, so nothing can race ahead of it.
    #[tokio::test]
    async fn session_start_restores_history_ref_head() {
        use crate::dag::DagHistory;

        let directory = tempfile::tempdir().unwrap();
        let history = DagHistory::open(&directory.path().join("history.db"))
            .await
            .unwrap();
        let mut engine = DataEngine::builder().build().with_history(history);
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let csv_path = std::path::Path::new(&manifest_dir)
            .join("test_datasets/Iris.csv")
            .to_string_lossy()
            .into_owned();
        engine
            .add_node_from_registry(
                "src",
                "file_to_dataframe",
                serde_json::json!({ "path": csv_path }),
            )
            .unwrap();
        let report = engine.run().await.unwrap();
        assert!(report.snapshot_id.is_some(), "run must commit a snapshot");

        let manager = DataEngineManager::new(engine);
        let client = manager.client_for_session_with_ref("agent-restore", "main");

        let dot = client.view_dag().await.unwrap();
        assert!(
            dot.contains("src"),
            "restored DAG should contain `src`: {dot}"
        );
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

    #[tokio::test]
    async fn dag_shell_reports_structured_running_error() {
        let mut engine = DataEngine::builder().build();
        let meta = NodePorts::new().add_output_port(None);
        engine
            .add_node("slow".to_string(), LongSleepNode(meta))
            .unwrap();
        let (client, _handle) = spawn_with_engine(engine);

        let (_event_rx, reply_rx) = client.run_dag_stream(None, None);
        let mut outcome = None;
        for _ in 0..20 {
            outcome = Some(
                client
                    .run_dag_shell("let observed = graph_summary();".into(), false, Some(100))
                    .await
                    .unwrap(),
            );
            if !outcome.as_ref().unwrap().ok {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        let outcome = outcome.unwrap();
        println!("dag_shell outcome: {outcome:?}");
        assert!(!outcome.ok);
        assert!(!outcome.applied);
        assert_eq!(outcome.error.as_ref().unwrap().code, "dag_running");

        drop(_event_rx);
        drop(reply_rx);
    }

    /// Dropping the `CancelOnDropReceiver` must:
    /// 1. Fire the `CancellationToken`
    /// 2. Abort the spawned DAG run task
    /// 3. Reset `running` to `false`
    /// 4. Release the engine mutex so the next `run_dag` succeeds
    #[tokio::test]
    async fn dropping_receiver_cancels_dag_and_frees_session() {
        let mut engine = DataEngine::builder().build();
        let meta = NodePorts::new().add_output_port(None);
        engine
            .add_node("slow".to_string(), LongSleepNode(meta))
            .unwrap();

        let (client, _handle) = spawn_with_engine(engine);

        // Start a streaming DAG run, then immediately drop the receiver —
        // simulating agent task cancellation.
        let (_event_rx, reply_rx) = client.run_dag_stream(None, None);
        drop(reply_rx);

        // Give the actor + spawned task a moment to process the cancellation.
        // The DAG run task selects on the cancel token, so this should be
        // nearly instant once the token fires.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // The session must be usable again — the `running` flag is reset and
        // the mutex is released. A second run should not get "already running".
        let result =
            tokio::time::timeout(std::time::Duration::from_secs(5), client.run_dag(None)).await;

        assert!(
            result.is_ok(),
            "second run_dag timed out — session was not freed after cancellation"
        );
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn memory_guard_returns_report_to_client() {
        let engine = DataEngine::builder()
            .build()
            .with_config(crate::dag::SchedulerConfig {
                memory_guard: dag_core::resource::MemoryGuardConfig::new(
                    f64::MIN_POSITIVE,
                    std::time::Duration::from_millis(1),
                ),
                ..crate::dag::SchedulerConfig::default()
            });
        let (client, _handle) = spawn_with_engine(engine);
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let csv_path = std::path::Path::new(&manifest_dir)
            .join("test_datasets/Iris.csv")
            .to_string_lossy()
            .into_owned();
        client
            .add_node(
                "source".to_string(),
                "file_to_dataframe".to_string(),
                serde_json::json!({ "path": csv_path }),
            )
            .await
            .unwrap();

        let report = client.run_dag(None).await.unwrap();

        assert!(!report.ok, "{report:?}");
        assert_eq!(
            report.status("source"),
            Some(crate::dag::runtime::RuntimeStatus::Cancelled)
        );
        assert!(report.resource.memory.trigger.is_some());
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.contains("memory guard"))
        );
    }
    fn zero_idle_policy() -> MaintenancePolicy {
        MaintenancePolicy {
            session_idle: std::time::Duration::ZERO,
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

    #[tokio::test]
    async fn idle_output_cache_releases_and_session_evicts() {
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("result.txt");
        let engine = DataEngine::builder().build();
        let manager = DataEngineManager::with_policy_and_roots(
            engine,
            zero_idle_policy(),
            directory.path().join("work"),
            directory.path().join("receipts"),
        );
        let client = manager.client_for_session("idle");
        install_file_pipeline(&client, &output).await;
        let report = client.run_dag(None).await.unwrap();
        assert!(report.ok, "{report:?}");
        assert!(
            client
                .get_output("src".to_string())
                .await
                .unwrap()
                .is_some()
        );

        let (released, evicted, clean) = manager.maintain_once().await.unwrap();
        assert_eq!(released, 1, "idle output cache must be released once");
        assert_eq!(evicted, 1);
        assert!(clean.is_none(), "budget disabled by default");
        assert_eq!(manager.session_count(), 0);
        assert!(client.get_output("src".to_string()).await.is_err());
    }

    #[tokio::test]
    async fn cross_session_file_results_reuse_by_fingerprint() {
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("shared.csv");
        let engine = DataEngine::builder().build();
        let manager = DataEngineManager::with_policy_and_roots(
            engine,
            MaintenancePolicy {
                output_cache_idle: std::time::Duration::from_secs(3600),
                ..zero_idle_policy()
            },
            directory.path().join("work"),
            directory.path().join("receipts"),
        );

        let first = manager.client_for_session("first");
        install_file_pipeline(&first, &output).await;
        let first_report = first.run_dag(None).await.unwrap();
        assert!(first_report.ok, "{first_report:?}");
        assert!(!node_report(&first_report, "sink").reused);

        let second = manager.client_for_session("second");
        install_file_pipeline(&second, &output).await;
        let second_report = second
            .run_dag_with_options(
                None,
                RunDagOptions {
                    resume: Some(true),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(second_report.ok, "{second_report:?}");
        assert!(
            node_report(&second_report, "sink").reused,
            "second session should consume the central file-result cache"
        );
    }

    /// A session recreated while its previous generation is shutting down must
    /// not have the new generation's storage references released by the old
    /// actor.
    #[tokio::test]
    async fn eviction_recreate_same_session_keeps_new_generation_refs() {
        let directory = tempfile::tempdir().unwrap();
        let work = directory.path().join("work");
        let output = work.join("aa").join("b".repeat(62)).join("out.csv");
        std::fs::create_dir_all(output.parent().unwrap()).unwrap();
        let engine = DataEngine::builder().build();
        let manager = DataEngineManager::with_policy_and_roots(
            engine,
            zero_idle_policy(),
            &work,
            directory.path().join("receipts"),
        );

        let first = manager.client_for_session("dup");
        install_file_pipeline(&first, &output).await;
        assert!(first.run_dag(None).await.unwrap().ok);

        let (_, evicted, _) = manager.maintain_once().await.unwrap();
        assert_eq!(evicted, 1);
        assert_eq!(manager.session_count(), 0);

        // Recreate under the same id: a brand-new generation whose storage
        // references must survive the old actor's shutdown bookkeeping.
        let second = manager.client_for_session("dup");
        assert_eq!(manager.session_count(), 1);
        install_file_pipeline(&second, &output).await;
        assert!(second.run_dag(None).await.unwrap().ok);

        let status = manager.storage_status();
        assert!(
            status
                .references
                .iter()
                .any(|entry| entry.sessions == 1 && entry.path == output.parent().unwrap()),
            "the recreated generation must own its storage references: {:?}",
            status.references
        );
    }

    /// Once a generation is marked closing, queued runs are rejected instead
    /// of executing on a session the manager has already removed.
    #[tokio::test]
    async fn closing_generation_rejects_queued_runs() {
        let directory = tempfile::tempdir().unwrap();
        let engine = DataEngine::builder().build();
        let manager = DataEngineManager::with_policy_and_roots(
            engine,
            MaintenancePolicy {
                session_idle: std::time::Duration::from_secs(3600),
                output_cache_idle: std::time::Duration::from_secs(3600),
                ..zero_idle_policy()
            },
            directory.path().join("work"),
            directory.path().join("receipts"),
        );
        let client = manager.client_for_session("closing");
        install_file_pipeline(&client, &directory.path().join("out.csv")).await;

        // Simulate the eviction pass marking this generation closing.
        {
            let sessions = manager.sessions.lock().expect("sessions mutex poisoned");
            sessions
                .get("closing")
                .expect("session must exist")
                .closing
                .store(true, Ordering::SeqCst);
        }

        let error = client.run_dag(None).await.unwrap_err();
        assert!(
            error.to_string().contains("session is shutting down"),
            "unexpected error: {error}"
        );

        // The next client for the same id transparently gets a new generation.
        let fresh = manager.client_for_session("closing");
        assert!(fresh.run_dag(None).await.unwrap().ok);
    }

    /// Result-cache TTL expiry must also drop the cache-owner storage
    /// references so run-aware cleanup can reclaim the local files later.
    #[tokio::test]
    async fn expired_result_cache_frees_local_references() {
        let directory = tempfile::tempdir().unwrap();
        let engine = DataEngine::builder().build();
        let manager = DataEngineManager::with_policy_and_roots(
            engine,
            MaintenancePolicy {
                session_idle: std::time::Duration::from_secs(3600),
                output_cache_idle: std::time::Duration::from_secs(3600),
                result_cache_ttl: std::time::Duration::ZERO,
                ..zero_idle_policy()
            },
            directory.path().join("work"),
            directory.path().join("receipts"),
        );
        let client = manager.client_for_session("ttl");
        install_file_pipeline(&client, &directory.path().join("out.csv")).await;
        assert!(client.run_dag(None).await.unwrap().ok);
        // TTL=0 expires entries the moment they are recorded, and the same
        // pass must release the cache-owner storage reference that the entry
        // would otherwise hold.
        assert_eq!(
            manager.result_cache.len(),
            0,
            "TTL=0 expires entries immediately"
        );
        let status = manager.storage_status();
        assert!(
            status
                .references
                .iter()
                .all(|entry| entry.cache_owners == 0),
            "expired cache entries must release their storage references: {:?}",
            status.references
        );
    }

    /// Cross-session restore must skip entries whose local files disappeared
    /// or changed on disk — the node re-executes instead of reusing stale
    /// bytes.
    #[tokio::test]
    async fn result_cache_restore_skips_missing_or_changed_files() {
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("reuse.csv");
        let engine = DataEngine::builder().build();
        let manager = DataEngineManager::with_policy_and_roots(
            engine,
            MaintenancePolicy {
                session_idle: std::time::Duration::from_secs(3600),
                output_cache_idle: std::time::Duration::from_secs(3600),
                ..zero_idle_policy()
            },
            directory.path().join("work"),
            directory.path().join("receipts"),
        );

        let first = manager.client_for_session("origin");
        install_file_pipeline(&first, &output).await;
        assert!(first.run_dag(None).await.unwrap().ok);
        assert_eq!(manager.result_cache.len(), 1);

        // Missing file: restore must skip, the node re-executes (and
        // recreates the file).
        std::fs::remove_file(&output).unwrap();
        let second = manager.client_for_session("missing");
        install_file_pipeline(&second, &output).await;
        let report = second
            .run_dag_with_options(
                None,
                RunDagOptions {
                    resume: Some(true),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(report.ok, "{report:?}");
        assert!(
            !node_report(&report, "sink").reused,
            "a deleted output file must not be reused across sessions"
        );

        // Changed file: a different on-disk fingerprint is also skipped.
        std::fs::write(&output, "tampered with much longer content").unwrap();
        let third = manager.client_for_session("changed");
        install_file_pipeline(&third, &output).await;
        let report = third
            .run_dag_with_options(
                None,
                RunDagOptions {
                    resume: Some(true),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(report.ok, "{report:?}");
        assert!(
            !node_report(&report, "sink").reused,
            "a fingerprint-mismatched output file must not be reused across sessions"
        );
    }
}
