//! The execution contract implemented by DAG nodes.

use async_trait::async_trait;

use super::input::NodeInput;
use super::layout::NodePorts;
use crate::dag::graph::PortOutputs;
use crate::dag::{DagError, node_event::NodeReporter};

/// A single unit of work in the DAG.
///
/// `execute` receives one [`NodeInput`] per connected input port and returns
/// outputs keyed by output port index. Implementors must be `Send + Sync`
/// because the async scheduler may run nodes on different tasks.
#[async_trait]
pub trait DagNode: Send + Sync {
    /// Return this node's static metadata (declared ports).
    fn ports(&self) -> &NodePorts;

    /// Run the node's computation.
    ///
    /// The [`NodeReporter`] is the node's channel for emitting mid-run
    /// observations. A node with nothing to report can ignore it; the scheduler
    /// emits the authoritative terminal event regardless.
    ///
    /// `ctx` is framework-injected and immutable. A node needing a
    /// `SessionContext` obtains a fresh, isolated one through
    /// [`NodeCtx::session`](crate::registry::NodeCtx::session) rather than
    /// storing mutable catalog state as a field.
    async fn execute(
        &mut self,
        ctx: &crate::registry::NodeCtx,
        inputs: &[NodeInput],
        reporter: &NodeReporter,
    ) -> Result<PortOutputs, DagError>;

    /// Clone this node into a boxed trait object.
    ///
    /// `Clone` is not object-safe, while the DAG stores `Box<dyn DagNode>` and
    /// must duplicate nodes for spawned tasks and validation dry-runs.
    fn clone_box(&self) -> Box<dyn DagNode>;

    /// The kind string identifying this node type, such as `"file_to_dataframe"` or
    /// `"sql"`. It must match the factory kind that builds the node.
    fn kind(&self) -> &'static str;

    /// Downcast helper for concrete-type introspection.
    fn as_any(&self) -> &dyn std::any::Any;

    /// The file path written by a file sink, if any.
    fn sink_path(&self) -> Option<&str> {
        None
    }

    /// The artifact path produced by a node, such as a rendered plot, if any.
    fn artifact_path(&self) -> Option<&str> {
        None
    }

    /// Whether this node is an intentional workflow sink and therefore may not
    /// have outgoing DAG edges. This is a graph-level contract, not a display
    /// hint.
    fn is_terminal(&self) -> bool {
        false
    }
}

impl Clone for Box<dyn DagNode> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}
