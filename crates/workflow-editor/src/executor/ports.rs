//! Runtime I/O ports + per-node execution context.
//!
//! `NodeExecutor` is the trait every node kind implements (via the
//! [`NodeFactory::build`](crate::registry::NodeFactory::build) call). The
//! scheduler builds a `NodeExecutor` once per node, then calls
//! [`NodeExecutor::execute`] once per node run.

use crate::error::Result;
use crate::executor::sop_context::SopContext;
use async_trait::async_trait;
use serde_json::Map;
use std::collections::HashMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Inputs to one node execution: `port_id -> value`.
pub type PortInputs = HashMap<String, serde_json::Value>;

/// Outputs from one node execution: `port_id -> value`.
pub type PortOutputs = HashMap<String, serde_json::Value>;

/// Execution context shared across the DAG run.
///
/// The executor owns this; each [`NodeExecutor::execute`] receives a
/// `&mut NodeCtx` so it can read / write to the SOP stack and the
/// scratch map.
pub struct NodeCtx {
    /// Workflow id (for node's own records).
    pub workflow_id: Uuid,
    /// The id of the node currently executing.
    pub node_id: Uuid,
    /// SOP stack — nodes read this on entry to compose their prompt.
    pub sop: SopContext,
    /// User-provided scratch state. Nodes may keep arbitrary data here for
    /// the duration of a single run.
    pub scratch: Map<String, serde_json::Value>,
    /// Cooperative cancellation token tied to the parent run.
    pub cancel: CancellationToken,
}

impl NodeCtx {
    /// Reject a tool call if the SOP context forbids it.
    pub fn require_tool(&self, tool: &str) -> Result<()> {
        use crate::error::ExecutorError;
        match self.sop.check_tool(tool) {
            Ok(()) => Ok(()),
            Err(info) => Err(ExecutorError::ToolDenied {
                tool: info.tool.to_string(),
                allowed: info.allowed.into_iter().map(|s| s.to_string()).collect(),
            }
            .into()),
        }
    }
}

/// Things a node wants to know about other nodes it might invoke.
#[derive(Default, Clone)]
pub struct NodeReporter {
    _private: Arc<()>,
}

/// A node instance — built once, executed once per run.
///
/// Phase 1 ships this as a minimal trait; real nodes will implement
/// `execute` via the `NodeFactory::build` factory.
#[async_trait]
pub trait NodeExecutor: Send + Sync {
    /// Stable kind identifier (e.g. `"http_request"`, `"echo"`, `"skill"`).
    fn kind(&self) -> &'static str;

    /// Execute one node.
    async fn execute(
        &self,
        ctx: &mut NodeCtx,
        inputs: &PortInputs,
        reporter: &NodeReporter,
    ) -> Result<PortOutputs>;
}
