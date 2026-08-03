//! Runtime of Data Engine based on tokio runtime
//!

use std::panic::AssertUnwindSafe;

use futures::FutureExt;
use tokio::{sync::mpsc, task::JoinHandle};

use crate::data_engine::DataEngine;
use crate::runtime::error::Result;
use crate::runtime::types::DataEngineCmd;

pub mod error;
pub mod types;

/// DataEngineServer -> DataEngine -> graph
pub struct DataEngineServer {
    engine: DataEngine,
    rx: mpsc::UnboundedReceiver<DataEngineCmd>,
}

impl DataEngineServer {
    /// Main event loop. Exits when all senders are dropped.
    pub async fn run(mut self) {
        while let Some(cmd) = self.rx.recv().await {
            if let Err(panic) = AssertUnwindSafe(self.handle(cmd)).catch_unwind().await {
                tracing::error!("data engine handler panicked: {:?}", panic);
            }
        }
    }

    async fn handle(&mut self, cmd: DataEngineCmd) {
        match cmd {
            DataEngineCmd::AddEdge {
                from,
                from_port,
                to,
                to_port,
                reply,
            } => {
                let res = match (from_port, to_port) {
                    (Some(fp), Some(tp)) => self.engine.add_edge(from, to, fp, tp).map(|_| ()),
                    (None, None) => self.engine.add_edge(from, to, 0, 0).map(|_| ()),
                    _ => Err(crate::error::Error::Custom(
                        "add_edge: from_port and to_port must both be Some or both None"
                            .to_string(),
                    )),
                };
                let _ = reply.send(res);
            }
            DataEngineCmd::RunDag { event_tx, reply } => {
                let res = match event_tx {
                    Some(sink) => self.engine.run_with_events(sink).await,
                    None => self.engine.run().await,
                };
                let _ = reply.send(res);
            }
            DataEngineCmd::GetOutput { id, reply } => {
                let _ = reply.send(Ok(self.engine.get_output(id).await));
            }
            DataEngineCmd::RemoveNode { id, reply } => {
                let _ = reply.send(self.engine.remove_node(id).map(|_| ()));
            }
            DataEngineCmd::ViewDag { reply } => {
                let _ = reply.send(self.engine.view_dag());
            }
            DataEngineCmd::ClearDag { reply } => {
                let _ = reply.send(self.engine.clear_dag().map(|_| ()));
            }
            DataEngineCmd::NewDagRef { name, reply } => {
                let res = self.engine.new_dag_ref(&name).await;
                let _ = reply.send(res);
            }
            DataEngineCmd::SwitchDagRef { name, reply } => {
                let _ = reply.send(self.engine.switch_dag_ref(&name));
            }
            DataEngineCmd::ListDagRefs { reply } => {
                let _ = reply.send(self.engine.list_dag_refs().await);
            }
            DataEngineCmd::DagLog {
                ref_name,
                limit,
                reply,
            } => {
                let _ = reply.send(self.engine.dag_log(ref_name.as_deref(), limit).await);
            }
            DataEngineCmd::CheckoutDag { snapshot_id, reply } => {
                let _ = reply.send(self.engine.checkout_dag(&snapshot_id).await);
            }
            DataEngineCmd::BranchFromSnapshot {
                snapshot_id,
                ref_name,
                reply,
            } => {
                let _ = reply.send(
                    self.engine
                        .branch_from_snapshot(&snapshot_id, &ref_name)
                        .await,
                );
            }
            DataEngineCmd::GetDagRef { reply } => {
                let _ = reply.send(Ok(self.engine.history_ref().to_string()));
            }
            DataEngineCmd::GetNodeSpec { kind, reply } => {
                let _ = reply.send(self.engine.get_node_spec(&kind));
            }
            DataEngineCmd::ListNodeFactories { reply } => {
                let _ = reply.send(Ok(self.engine.list_nodes()));
            }
            DataEngineCmd::AddNode {
                id,
                kind,
                spec,
                reply,
            } => {
                let _ = reply.send(self.engine.add_node_from_registry(id, &kind, spec));
            }
            DataEngineCmd::UpdateNode { id, spec, reply } => {
                let _ = reply.send(self.engine.update_node(id, spec));
            }
            DataEngineCmd::GetNodePorts { kind, reply } => {
                let _ = reply.send(self.engine.get_node_ports(&kind));
            }
            DataEngineCmd::GetNodeDoc { kind, reply } => {
                let _ = reply.send(self.engine.get_node_doc(&kind));
            }
        }
    }
}

#[derive(Clone)]
pub struct DataEngineClient {
    tx: mpsc::UnboundedSender<DataEngineCmd>,
}

impl DataEngineClient {
    async fn request<T>(
        &self,
        cmd: DataEngineCmd,
        reply_rx: tokio::sync::oneshot::Receiver<std::result::Result<T, crate::error::Error>>,
    ) -> Result<T> {
        use crate::runtime::error::ClientError;

        self.tx.send(cmd).map_err(|_| ClientError::ServerClosed)?;
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
    ) -> (
        mpsc::Receiver<crate::dag::node_event::NodeEvent>,
        tokio::sync::oneshot::Receiver<crate::error::Result<crate::dag::RunReport>>,
    ) {
        let (event_tx, event_rx) = mpsc::channel::<crate::dag::node_event::NodeEvent>(128);
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let _ = self.tx.send(DataEngineCmd::RunDag {
            event_tx: Some(event_tx),
            reply: reply_tx,
        });
        (event_rx, reply_rx)
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

    pub async fn list_node_factories(&self) -> Result<Vec<crate::node_registry::NodeInfo>> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(
            DataEngineCmd::ListNodeFactories { reply: reply_tx },
            reply_rx,
        )
        .await
    }

    pub async fn get_node_spec(&self, kind: String) -> Result<schemars::Schema> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(
            DataEngineCmd::GetNodeSpec {
                kind,
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

    /// Update an existing node's spec in-place. The kind is discovered from
    /// the node's current `node_type()`, so only the new spec is needed.
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

    pub async fn clear_dag(&self) -> Result<()> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(DataEngineCmd::ClearDag { reply: reply_tx }, reply_rx)
            .await
    }

    /// Clear the in-memory DAG and switch to a new history ref. Replaces the
    /// old `clear_dag` — instead of wiping state without trace, it starts a
    /// new independent snapshot lineage.
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

    /// Switch the engine's history ref to an existing ref.
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

    /// List all history refs.
    pub async fn list_dag_refs(&self) -> Result<Vec<(String, String, bool)>> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(
            DataEngineCmd::ListDagRefs { reply: reply_tx },
            reply_rx,
        )
        .await
    }

    /// Show snapshot lineage for a ref (None = current ref).
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

    /// Load a snapshot's DAG into memory without moving the ref.
    /// Short-hash prefixes are accepted.
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

    /// Create a new ref from a snapshot, switch to it, and load its DAG.
    /// Short-hash prefixes are accepted.
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

    /// Query the current history ref name.
    pub async fn get_dag_ref(&self) -> Result<String> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(
            DataEngineCmd::GetDagRef { reply: reply_tx },
            reply_rx,
        )
        .await
    }

    pub async fn get_node_ports(&self, kind: String) -> Result<crate::nodes::meta::NodePorts> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(
            DataEngineCmd::GetNodePorts {
                kind,
                reply: reply_tx,
            },
            reply_rx,
        )
        .await
    }

    pub async fn get_node_doc(&self, kind: String) -> Result<String> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(
            DataEngineCmd::GetNodeDoc {
                kind,
                reply: reply_tx,
            },
            reply_rx,
        )
        .await
    }
}

pub fn spawn_engine() {}

/// Spawn server through dependency injection, good for test purpose.
pub fn spawn_with_engine(engine: DataEngine) -> (DataEngineClient, JoinHandle<()>) {
    let (tx, rx) = mpsc::unbounded_channel::<DataEngineCmd>();
    let server = DataEngineServer { engine, rx };
    let client = DataEngineClient { tx };
    let handle = tokio::task::spawn(server.run());

    (client, handle)
}
