//! Runtime of Data Engine based on tokio runtime
//!
//! The server maintains a **pool of per-session `DataEngine` instances**.
//! Each session (agent) gets its own DAG graph, but all sessions share the
//! same heavy infrastructure via `Arc` (NodeRegistry, RuntimeEnv, DagHistory).
//!
//! New sessions are created lazily via `DataEngine::new_session()` on the
//! first command for a given `session_id`.

use std::panic::AssertUnwindSafe;

use datafusion::common::HashMap;
use futures::FutureExt;
use tokio::{sync::mpsc, task::JoinHandle};

use crate::data_engine::DataEngine;
use crate::runtime::error::Result;
use crate::runtime::types::{DataEngineCmd, EngineMsg};

pub mod error;
pub mod types;

/// Multi-session DataEngine actor.
///
/// Holds a template engine (created at startup with all heavy infra) and
/// a lazily-populated map of per-session engines. Each session engine
/// shares the template's `Arc<NodeRegistry>`, `NodeCtx`, `DagHistory`, etc.
/// but has its own independent `DAG` and `history_ref`.
pub struct DataEngineServer {
    /// Template engine — used to spawn new sessions via `new_session()`.
    template: DataEngine,
    /// Per-session engines, keyed by session_id.
    sessions: HashMap<String, DataEngine>,
    rx: mpsc::UnboundedReceiver<EngineMsg>,
}

impl DataEngineServer {
    /// Main event loop. Exits when all senders are dropped.
    pub async fn run(mut self) {
        while let Some(msg) = self.rx.recv().await {
            if let Err(panic) = AssertUnwindSafe(self.handle(msg)).catch_unwind().await {
                tracing::error!("data engine handler panicked: {:?}", panic);
            }
        }
    }

    /// Get (or lazily create) the session engine for `session_id`.
    fn session(&mut self, session_id: &str) -> &mut DataEngine {
        if !self.sessions.contains_key(session_id) {
            let new_engine = self.template.new_session();
            tracing::info!(session_id, "created new DAG session");
            self.sessions.insert(session_id.to_string(), new_engine);
        }
        self.sessions.get_mut(session_id).expect("just inserted")
    }

    async fn handle(&mut self, msg: EngineMsg) {
        let EngineMsg { session_id, cmd } = msg;

        match cmd {
            DataEngineCmd::AddEdge {
                from,
                from_port,
                to,
                to_port,
                reply,
            } => {
                let engine = self.session(&session_id);
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
            DataEngineCmd::RunDag {
                event_tx,
                commit_message,
                reply,
            } => {
                let engine = self.session(&session_id);
                engine.set_commit_message(commit_message);
                let res = match event_tx {
                    Some(sink) => engine.run_with_events(sink).await,
                    None => engine.run().await,
                };
                let _ = reply.send(res);
            }
            DataEngineCmd::GetOutput { id, reply } => {
                let engine = self.session(&session_id);
                let _ = reply.send(Ok(engine.get_output(id).await));
            }
            DataEngineCmd::GetNodeStatus { id, reply } => {
                let engine = self.session(&session_id);
                let _ = reply.send(Ok(engine.node_status(&id)));
            }
            DataEngineCmd::RemoveNode { id, reply } => {
                let engine = self.session(&session_id);
                let _ = reply.send(engine.remove_node(id).map(|_| ()));
            }
            DataEngineCmd::ViewDag { reply } => {
                let engine = self.session(&session_id);
                let _ = reply.send(engine.view_dag());
            }
            DataEngineCmd::ClearDag { reply } => {
                let engine = self.session(&session_id);
                let _ = reply.send(engine.clear_dag().map(|_| ()));
            }
            DataEngineCmd::NewDagRef { name, reply } => {
                let engine = self.session(&session_id);
                let res = engine.new_dag_ref(&name).await;
                let _ = reply.send(res);
            }
            DataEngineCmd::SwitchDagRef { name, reply } => {
                let engine = self.session(&session_id);
                let _ = reply.send(engine.switch_dag_ref(&name).await);
            }
            DataEngineCmd::ListDagRefs { reply } => {
                let engine = self.session(&session_id);
                let _ = reply.send(engine.list_dag_refs().await);
            }
            DataEngineCmd::DagLog {
                ref_name,
                limit,
                reply,
            } => {
                let engine = self.session(&session_id);
                let _ = reply.send(engine.dag_log(ref_name.as_deref(), limit).await);
            }
            DataEngineCmd::CheckoutDag { snapshot_id, reply } => {
                let engine = self.session(&session_id);
                let _ = reply.send(engine.checkout_dag(&snapshot_id).await);
            }
            DataEngineCmd::BranchFromSnapshot {
                snapshot_id,
                ref_name,
                reply,
            } => {
                let engine = self.session(&session_id);
                let _ = reply.send(
                    engine
                        .branch_from_snapshot(&snapshot_id, &ref_name)
                        .await,
                );
            }
            DataEngineCmd::GetDagRef { reply } => {
                let engine = self.session(&session_id);
                let _ = reply.send(Ok(engine.history_ref().to_string()));
            }
            DataEngineCmd::GetSnapshot { snapshot_id, reply } => {
                let engine = self.session(&session_id);
                let _ = reply.send(engine.get_snapshot(&snapshot_id).await);
            }
            DataEngineCmd::DiffSnapshots {
                old_id,
                new_id,
                reply,
            } => {
                let engine = self.session(&session_id);
                let _ = reply.send(engine.diff_snapshots(&old_id, &new_id).await);
            }
            DataEngineCmd::GetNodeSpec { kind, reply } => {
                let engine = self.session(&session_id);
                let _ = reply.send(engine.get_node_spec(&kind));
            }
            DataEngineCmd::ListNodeFactories { reply } => {
                let engine = self.session(&session_id);
                let _ = reply.send(Ok(engine.list_nodes()));
            }
            DataEngineCmd::AddNode {
                id,
                kind,
                spec,
                reply,
            } => {
                let engine = self.session(&session_id);
                let _ = reply.send(engine.add_node_from_registry(id, &kind, spec));
            }
            DataEngineCmd::UpdateNode { id, spec, reply } => {
                let engine = self.session(&session_id);
                let _ = reply.send(engine.update_node(id, spec));
            }
            DataEngineCmd::GetNodePorts { kind, reply } => {
                let engine = self.session(&session_id);
                let _ = reply.send(engine.get_node_ports(&kind));
            }
            DataEngineCmd::GetNodeDoc { kind, reply } => {
                let engine = self.session(&session_id);
                let _ = reply.send(engine.get_node_doc(&kind));
            }
            DataEngineCmd::CompileDag { target, reply } => {
                let engine = self.session(&session_id);
                let _ = reply.send(engine.compile_dag(target));
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Client
// ═══════════════════════════════════════════════════════════════════════

/// Channel-based client for the multi-session DataEngine actor.
///
/// Each client carries a `session_id` that routes commands to the right
/// per-agent DAG. Use [`DataEngineClient::with_session`] to create a new
/// client for a different agent — both clients share the same underlying
/// channel (and thus the same actor / infrastructure).
#[derive(Clone)]
pub struct DataEngineClient {
    tx: mpsc::UnboundedSender<EngineMsg>,
    session_id: String,
}

impl DataEngineClient {
    /// Create a new client for a **different session** sharing the same
    /// actor connection. The new session's DAG is created lazily on first
    /// command.
    pub fn with_session(&self, session_id: impl Into<String>) -> Self {
        Self {
            tx: self.tx.clone(),
            session_id: session_id.into(),
        }
    }

    /// Returns the session ID this client routes to.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    async fn request<T>(
        &self,
        cmd: DataEngineCmd,
        reply_rx: tokio::sync::oneshot::Receiver<std::result::Result<T, crate::error::Error>>,
    ) -> Result<T> {
        use crate::runtime::error::ClientError;

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

    pub async fn run_dag(&self) -> Result<crate::dag::RunReport> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(
            DataEngineCmd::RunDag {
                event_tx: None,
                commit_message: None,
                reply: reply_tx,
            },
            reply_rx,
        )
        .await
    }

    /// Kick off a DAG run with live per-node event streaming.
    ///
    /// Returns `(event_rx, reply_rx)`: the caller drains `event_rx`
    /// concurrently with awaiting `reply_rx` (the final [`RunReport`]).
    /// Events are lightweight observations (status/progress/log/finished) and
    /// may be dropped on a full channel — they never affect the run's outcome.
    pub fn run_dag_stream(
        &self,
        commit_message: Option<String>,
    ) -> (
        mpsc::Receiver<crate::dag::node_event::NodeEvent>,
        tokio::sync::oneshot::Receiver<crate::error::Result<crate::dag::RunReport>>,
    ) {
        let (event_tx, event_rx) = mpsc::channel::<crate::dag::node_event::NodeEvent>(128);
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let _ = self.tx.send(EngineMsg {
            session_id: self.session_id.clone(),
            cmd: DataEngineCmd::RunDag {
                event_tx: Some(event_tx),
                commit_message,
                reply: reply_tx,
            },
        });
        (event_rx, reply_rx)
    }

    pub async fn get_output(&self, id: String) -> Result<Option<crate::dag::graph::PortOutputs>> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(
            DataEngineCmd::GetOutput { id, reply: reply_tx },
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
            DataEngineCmd::GetNodeStatus { id, reply: reply_tx },
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

    pub async fn remove_node(&self, id: String) -> Result<()> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(
            DataEngineCmd::RemoveNode { id, reply: reply_tx },
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
        self.request(
            DataEngineCmd::ClearDag { reply: reply_tx },
            reply_rx,
        )
        .await
    }

    pub async fn new_dag_ref(&self, name: String) -> Result<()> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(
            DataEngineCmd::NewDagRef { name, reply: reply_tx },
            reply_rx,
        )
        .await
    }

    pub async fn switch_dag_ref(&self, name: String) -> Result<()> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(
            DataEngineCmd::SwitchDagRef { name, reply: reply_tx },
            reply_rx,
        )
        .await
    }

    pub async fn list_dag_refs(&self) -> Result<Vec<(String, String, bool)>> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(
            DataEngineCmd::ListDagRefs { reply: reply_tx },
            reply_rx,
        )
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

    pub async fn get_dag_ref(&self) -> Result<String> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(
            DataEngineCmd::GetDagRef { reply: reply_tx },
            reply_rx,
        )
        .await
    }

    pub async fn get_snapshot(
        &self,
        snapshot_id: String,
    ) -> Result<Option<crate::dag::Snapshot>> {
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

    pub async fn get_node_spec(&self, kind: String) -> Result<schemars::Schema> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(
            DataEngineCmd::GetNodeSpec { kind, reply: reply_tx },
            reply_rx,
        )
        .await
    }

    pub async fn list_node_factories(&self) -> Result<Vec<crate::node_registry::NodeInfo>> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(
            DataEngineCmd::ListNodeFactories { reply: reply_tx },
            reply_rx,
        )
        .await
    }

    pub async fn add_node(
        &self,
        id: String,
        kind: String,
        spec: serde_json::Value,
    ) -> Result<()> {
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

    pub async fn get_node_ports(&self, kind: String) -> Result<crate::nodes::meta::NodePorts> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(
            DataEngineCmd::GetNodePorts { kind, reply: reply_tx },
            reply_rx,
        )
        .await
    }

    pub async fn get_node_doc(&self, kind: String) -> Result<String> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(
            DataEngineCmd::GetNodeDoc { kind, reply: reply_tx },
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
            DataEngineCmd::CompileDag { target, reply: reply_tx },
            reply_rx,
        )
        .await
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Spawn
// ═══════════════════════════════════════════════════════════════════════

/// Spawn the multi-session DataEngine actor.
///
/// Returns a `DataEngineClient` whose default session is `"default"`. Use
/// `client.with_session("agent-foo")` to create a client that routes to a
/// different (lazily-created) DAG session.
pub fn spawn_with_engine(engine: DataEngine) -> (DataEngineClient, JoinHandle<()>) {
    let (tx, rx) = mpsc::unbounded_channel::<EngineMsg>();
    let server = DataEngineServer {
        template: engine,
        sessions: HashMap::new(),
        rx,
    };
    let client = DataEngineClient {
        tx,
        session_id: "default".to_string(),
    };
    let handle = tokio::task::spawn(server.run());
    (client, handle)
}
