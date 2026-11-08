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

    /// Allocate a node-owned local work directory for this execution attempt.
    /// When present, the local executor places its receipt and staging data
    /// underneath it, keeping task evidence together with the node's output.
    fn prepare_local_workdir(
        &mut self,
        _inputs: &[NodeInput],
    ) -> Result<Option<std::path::PathBuf>, DagError> {
        Ok(None)
    }

    /// Minimum executor resources implied by this loaded node's compiled
    /// behavior. A remote adapter may fill absent task directives from this
    /// declaration, but must reject explicit budgets below it. This is not
    /// scientific resource admission and must not replace signed contracts.
    fn task_resources(&self) -> Result<crate::dag::execution::TaskResources, DagError> {
        Ok(crate::dag::execution::TaskResources::default())
    }

    /// This node only prepares a container request; its computation belongs to
    /// the injected container backend. Placement may orchestrate it on the
    /// coordinator when that backend is explicitly bound to remote execution.
    /// This capability comes from the implementation, never a task spec.
    fn delegates_to_container_runtime(&self) -> bool {
        false
    }

    /// A process-local service that has no independent-worker protocol.
    /// Reject before staging/submission instead of running a predictably broken
    /// job. This declaration never authorizes a fallback to local execution.
    fn remote_execution_blocker(&self) -> Option<&'static str> {
        None
    }

    /// Images required by this compiled payload, never inferred from caller metadata.
    fn required_container_images(&self) -> Vec<&str> {
        self.plugin_identity()
            .map(|identity| vec![identity.image_reference.as_str()])
            .unwrap_or_default()
    }

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

    /// Complete local publication directories declared by this node's immutable
    /// spec. The runtime may project these into fresh invocation directories;
    /// undeclared publishers cannot silently participate in that projection.
    fn runtime_output_directories(&self) -> Vec<std::path::PathBuf> {
        Vec::new()
    }

    /// Whether a successful output may replace execution in an incremental
    /// run. Read-only auditors of external state must execute every time,
    /// even when their ordinary port/spec fingerprints are unchanged.
    fn allows_cache_reuse(&self) -> bool {
        true
    }

    /// File paths this node reads from the filesystem by configured path
    /// instead of through an input port (`file_reference`-style path
    /// dependencies).
    ///
    /// Validation intersects these with the write paths other nodes declare
    /// via [`DagNode::sink_path`](DagNode::sink_path) /
    /// [`DagNode::artifact_path`](DagNode::artifact_path): a node that reads
    /// by path a file another node in the same DAG writes has no ordering the
    /// port graph could enforce, so the read would race the write — a
    /// missing-file error, or worse, a silent read of a stale file left by an
    /// earlier run.
    fn referenced_file_paths(&self) -> Vec<String> {
        Vec::new()
    }

    /// JSON pointers to file inputs in the immutable spec that need transport
    /// when this node runs on another host. Only these exact fields may be
    /// rebound to verified worker snapshots.
    /// Omit fallback spec inputs when a connected port takes precedence.
    fn file_input_spec_pointers(&self, _inputs: &[NodeInput]) -> Vec<&'static str> {
        Vec::new()
    }

    /// Resolve declared input qualifiers before fingerprinting or transport.
    /// Implementations must be deterministic and safe to call more than once.
    fn prepare_task_inputs(&self, _inputs: &mut [NodeInput]) -> Result<(), DagError> {
        Ok(())
    }

    /// Output directory spec pointers and their FileSet ports. Remote workers
    /// write these into attempt-owned directories; the coordinator publishes
    /// verified members to the original destination using its VFS principal.
    /// Other spec fields must never be rebound as publication directories.
    fn file_output_directory_spec_pointers(&self) -> Vec<(&'static str, u8)> {
        Vec::new()
    }

    /// Read-only identity of dependencies not represented by ports or spec,
    /// such as the relevant rows of an external library. Called before the
    /// incremental reuse decision. Returning an error blocks this node;
    /// `None` preserves the existing identity of stateless nodes. Do not
    /// return credentials or fetch/update external resources here.
    async fn cache_dependency_state(
        &self,
        _ctx: &crate::registry::NodeCtx,
        _inputs: &[NodeInput],
    ) -> Result<Option<serde_json::Value>, DagError> {
        Ok(None)
    }

    /// The identity of the plugin implementation this node executes, when
    /// the node's behavior is defined by a loaded plugin family (e.g.
    /// container-plugin manifests) rather than purely by its kind + spec.
    ///
    /// Fed into the node's execution fingerprint (WO-R09) so that swapping a
    /// plugin family's manifest, script, or image invalidates cached
    /// incremental outputs. The identity is captured from the *loaded*
    /// plugin definition — not re-read from disk at dispatch — so it always
    /// matches what the engine would actually execute. `None` (the default)
    /// for built-in nodes; their inputs and optional external dependency
    /// state are fingerprinted separately.
    fn plugin_identity(&self) -> Option<&crate::fingerprint::PluginIdentity> {
        None
    }
}

impl Clone for Box<dyn DagNode> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}
