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
pub mod types;

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
    rx: mpsc::UnboundedReceiver<EngineMsg>,
}

impl SessionServer {
    async fn run(mut self) {
        while let Some(msg) = self.rx.recv().await {
            if let Err(panic) = AssertUnwindSafe(self.handle(msg)).catch_unwind().await {
                tracing::error!(
                    session_id = %self.session_id,
                    "session actor handler panicked: {:?}",
                    panic
                );
            }
        }
        tracing::debug!(session_id = %self.session_id, "session actor exited");
    }

    async fn handle(&self, msg: EngineMsg) {
        let EngineMsg { cmd, .. } = msg;

        match cmd {
            // ── DAG execution (fire-and-forget) ──────────────────────────
            DataEngineCmd::RunDag {
                event_tx,
                commit_message,
                reply,
                cancel_token,
            } => {
                if self.running.swap(true, Ordering::SeqCst) {
                    let _ = reply.send(Err(crate::error::Error::Custom(
                        "DAG is already running for this session; wait for the current run to complete"
                            .to_string(),
                    )));
                    return;
                }
                let engine = Arc::clone(&self.engine);
                let running = Arc::clone(&self.running);
                tokio::spawn(async move {
                    // Race the DAG run against cancellation. When the caller
                    // drops the reply receiver (e.g. the agent task is
                    // cancelled), `cancel_token` fires and we abort early —
                    // releasing the engine mutex and resetting `running`
                    // instead of leaving the session stuck.
                    let run_fut = AssertUnwindSafe(async {
                        let mut engine = engine.lock().await;
                        engine.set_commit_message(commit_message);
                        match event_tx {
                            Some(sink) => engine.run_with_events(sink).await,
                            None => engine.run().await,
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
                let _ = reply.send(engine.clear_dag().map(|_| ()));
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
            DataEngineCmd::CompileDag { target, reply } => {
                if self.running.load(Ordering::SeqCst) {
                    let _ = reply.send(Err(crate::error::Error::Custom(
                        "DAG is currently running; compile it after completion".to_string(),
                    )));
                    return;
                }
                let engine = self
                    .engine
                    .try_lock()
                    .expect("uncontended: running flag is false");
                let _ = reply.send(engine.compile_dag(target));
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
    /// dynamic-port kinds such as `run_command`.
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

    pub async fn run_dag(&self) -> Result<crate::dag::RunReport> {
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

    pub async fn clear_dag(&self) -> Result<()> {
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

    pub async fn compile_dag(
        &self,
        target: crate::codegen::CodegenTarget,
    ) -> Result<crate::codegen::CompiledScript> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(
            DataEngineCmd::CompileDag {
                target,
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
}

struct SessionHandle {
    tx: mpsc::UnboundedSender<EngineMsg>,
    _task: JoinHandle<()>,
}

impl DataEngineManager {
    /// Create a manager from a template engine. The template's
    /// `NodeRegistry`, `RuntimeEnv`, `DagHistory`, etc. are shared across all
    /// lazily-created sessions via `Arc` / `Clone`.
    pub fn new(engine: DataEngine) -> Self {
        let node_registry = Arc::clone(engine.node_registry());
        Self {
            template: engine,
            node_registry,
            sessions: std::sync::Mutex::new(HashMap::new()),
        }
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

        // Fast path: session already exists.
        if let Some(handle) = sessions.get(session_id) {
            return DataEngineClient {
                tx: handle.tx.clone(),
                node_registry: Arc::clone(&self.node_registry),
                session_id: session_id.to_string(),
            };
        }

        // Slow path: create new session.
        // Use the caller-provided history_ref so each agent gets its own
        // snapshot lineage instead of all committing to "main".
        let new_engine = self.template.new_session().with_history_ref(history_ref);
        let (tx, rx) = mpsc::unbounded_channel::<EngineMsg>();
        let server = SessionServer {
            session_id: session_id.to_string(),
            engine: Arc::new(tokio::sync::Mutex::new(new_engine)),
            running: Arc::new(AtomicBool::new(false)),
            rx,
        };
        let task = tokio::task::spawn(server.run());

        tracing::info!(session_id, history_ref, "created new DAG session");
        sessions.insert(
            session_id.to_string(),
            SessionHandle {
                tx: tx.clone(),
                _task: task,
            },
        );
        drop(sessions);

        DataEngineClient {
            tx,
            node_registry: Arc::clone(&self.node_registry),
            session_id: session_id.to_string(),
        }
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
        let (_event_rx, reply_rx) = client.run_dag_stream(None);
        drop(reply_rx);

        // Give the actor + spawned task a moment to process the cancellation.
        // The DAG run task selects on the cancel token, so this should be
        // nearly instant once the token fires.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // The session must be usable again — the `running` flag is reset and
        // the mutex is released. A second run should not get "already running".
        let result =
            tokio::time::timeout(std::time::Duration::from_secs(5), client.run_dag()).await;

        assert!(
            result.is_ok(),
            "second run_dag timed out — session was not freed after cancellation"
        );
    }
}
