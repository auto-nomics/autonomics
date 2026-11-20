//! Client face of the DataEngine runtime (Layer 1).
//!
//! Channel-based access to a per-agent actor. Metadata queries
//! (`list_node_factories`, `get_node_spec`, `get_node_ports`, `get_node_doc`)
//! are served directly from the shared `Arc<NodeRegistry>` and never enter
//! the actor channel, so they cannot be blocked by a running DAG.

use std::sync::Arc;
use std::task::{Context, Poll};

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::node_registry::NodeRegistry;
use crate::runtime::AgentActivityTracker;
use crate::runtime::error::{ClientError, Result};
use crate::runtime::types::{DataEngineCmd, EngineMsg};

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
    pub(super) fn new(
        tx: mpsc::UnboundedSender<EngineMsg>,
        node_registry: Arc<NodeRegistry>,
        agent_id: String,
        activity: Arc<AgentActivityTracker>,
    ) -> Self {
        Self {
            tx,
            node_registry,
            agent_id,
            activity,
        }
    }

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
        if kind == "channel" {
            return crate::data_engine::channel_schema().map_err(ClientError::Engine);
        }
        self.node_registry
            .get_node_spec(kind)
            .map_err(|e| ClientError::Engine(crate::error::Error::from(e)))
    }

    /// Get the input/output port layout of a node kind. Synchronous, never blocks.
    pub fn get_node_ports(&self, kind: &str) -> Result<crate::nodes::meta::NodePorts> {
        if kind == "channel" {
            return crate::data_engine::channel_ports(None).map_err(ClientError::Engine);
        }
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
        if kind == "channel" {
            return crate::data_engine::channel_ports(Some(&spec)).map_err(ClientError::Engine);
        }
        self.node_registry
            .get_node_ports_for_spec(kind, spec)
            .map_err(|e| ClientError::Engine(crate::error::Error::from(e)))
    }

    /// Get the documentation string for a node kind. Synchronous, never blocks.
    pub fn get_node_doc(&self, kind: &str) -> Result<String> {
        if kind == "channel" {
            return Ok(crate::data_engine::CHANNEL_DOC.into());
        }
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

    /// Add one process to the session flow via its default input wiring.
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
        flow: Option<crate::workflow::FlowGraph>,
        options: crate::flow::FlowOptions,
    ) -> Result<crate::workflow::FlowRunReport> {
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
    pub async fn set_flow(&self, flow: crate::workflow::FlowGraph) -> Result<()> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(
            DataEngineCmd::SetFlow {
                flow,
                reply: reply_tx,
            },
            reply_rx,
        )
        .await
    }

    /// Read the session's current authored flow.
    pub async fn get_flow(&self) -> Result<crate::workflow::FlowGraph> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(DataEngineCmd::GetFlow { reply: reply_tx }, reply_rx)
            .await
    }

    /// Read the flow view: the authored flow plus the last run's grouped
    /// report, for TUI / GUI / gateway surfaces.
    pub async fn get_flow_view(&self) -> Result<crate::flow::FlowView> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(DataEngineCmd::GetFlowView { reply: reply_tx }, reply_rx)
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

    /// Return the structured snapshot consumed by interactive DAG rendering
    /// (the session flow's live projection when a run exists, the authored
    /// topology otherwise).
    pub async fn dag_tui_snapshot(&self) -> Result<crate::dag::DagTuiSnapshot> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.request(
            DataEngineCmd::GetDagTuiSnapshot { reply: reply_tx },
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

    /// Update one process's spec (validated against the registry schema).
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
