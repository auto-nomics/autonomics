//! The DAG data structure: a payload store + a structural index.
//!
//! The [`DAG`] struct, its field layout, and the small query/config API live
//! here. Heavier concerns are split into submodules of this module — they all
//! operate on the same struct via `impl DAG` blocks:
//!
//! - [`mutation`] — node/edge add/delete/replace, reset, and the
//!   incremental-execution fingerprint state (`mark_dirty` & co.).
//! - [`topology`] — predecessor/successor/edge queries, topological sort,
//!   cycle-path recovery, dot rendering, TUI snapshots.
//! - [`validation`] — cycle / wiring / schema / path-aliasing validation.
//! - [`scheduler`] — the async readiness scheduler (`DAG::run`).
//! - [`streaming`] — channel item propagation between streaming nodes.
//! - [`fanout`] — dynamic fanout job expansion.
//! - [`reports`] — run-report assembly (`NodeReport`, logical summaries).
//! - [`manifest`] — manifest export and retained-spec queries.
//! - [`memory`] — memory-guard sampling helpers for the scheduler.
//! - [`PortOutputs`] — per-port output values.

mod fanout;
mod manifest;
mod memory;
mod mutation;
mod port_outputs;
mod reports;
mod scheduler;
mod streaming;
mod topology;
mod validation;

#[cfg(test)]
mod tests;

pub use port_outputs::PortOutputs;

use std::sync::Arc;

use datafusion::common::HashMap;
use petgraph::graph::{DiGraph, NodeIndex};

use super::error::DagError;
use super::execution::{TaskExecutor, TaskResources};
use super::logical::LogicalGraph;
use super::physical::PhysicalJobRef;
use super::runtime::{InputBinding, NodeRunDetails, RuntimeStatus};
use super::{DagNode, NodeId};

pub struct DagEdge {
    #[allow(dead_code)]
    pub from_node: NodeId,
    #[allow(dead_code)]
    pub to_node: NodeId,
    #[allow(dead_code)]
    pub from_port: u8,
    #[allow(dead_code)]
    pub to_port: u8,
}

/// Metadata attached to every edge in the graph: which output port of the
/// source feeds which input port of the target. Exactly one [`DataFrame`] flows
/// along each edge.
#[derive(Debug, Clone)]
pub struct EdgeLabel {
    /// Output port name on the `from` node.
    pub from_port: u8,
    /// Input port name on the `to` node.
    pub to_port: u8,
}

/// Builds a registry node after dynamic fanout discovers an item.
pub type DynamicNodeBuilder =
    Arc<dyn Fn(&str, serde_json::Value) -> Result<Box<dyn DagNode>> + Send + Sync>;

#[derive(Clone)]
struct TaskExecutorHandle {
    executor: Arc<dyn TaskExecutor>,
}

impl Default for TaskExecutorHandle {
    fn default() -> Self {
        Self {
            executor: super::execution::local_task_executor(),
        }
    }
}

/// Module-local Result alias — every fallible operation in this module fails
/// with [`DagError`].
pub type Result<T> = std::result::Result<T, DagError>;

/// The workflow graph: payload store + connectivity index.
///
/// Node payloads (`Box<dyn DagNode>`) live in a [`HashMap`] keyed by id; a
/// lightweight [`petgraph`] directed graph mirrors both the connectivity and
/// edge metadata (see [`DependencyKind`]) so we get cycle detection,
/// topological sort, predecessor/successor queries, and edge iteration for
/// free. The two are decoupled on purpose — keeping payloads out of the graph
/// lets the scheduler `clone_box` a node into a spawned task without
/// fighting the graph's borrow.
#[derive(Default)]
pub struct DAG {
    /// Node payloads, keyed by id. Public so external tooling can introspect.
    pub nodes: HashMap<NodeId, Box<dyn DagNode>>,
    /// Connectivity index (edge weight carries the port label).
    graph: DiGraph<NodeId, EdgeLabel>,
    pub id_to_idx: HashMap<NodeId, NodeIndex>,
    /// Per-node runtime status. Populated on [`Self::run`], queryable via [`Self::status`].
    pub statuses: HashMap<NodeId, RuntimeStatus>,
    outputs: HashMap<NodeId, PortOutputs>,
    errors: HashMap<NodeId, DagError>,
    /// Retained `(kind, spec)` per node, populated via [`Self::add_node_with_spec`]
    /// / [`Self::replace_node_with_spec`]. Enables manifest export for
    /// snapshot persistence without modifying the `DagNode` trait.
    specs: HashMap<NodeId, (String, serde_json::Value)>,
    /// Execution fingerprint of each node's last successful execution
    /// (see [`crate::fingerprint::compute_node_fingerprint`]). The reuse key
    /// for incremental runs: at dispatch a node whose candidate fingerprint
    /// — computed over kind, spec, input identities, engine version, and
    /// the build's source revision — matches the recorded one, and whose
    /// cached outputs are still present, is skipped. Mutations drop the
    /// affected node's entry; staleness of cached file outputs drops it at
    /// run start; descendants never need explicit invalidation because
    /// their identities chain through upstream fingerprints.
    fingerprints: HashMap<NodeId, String>,
    /// Upstream bindings captured at dispatch time, for the run report's
    /// audit trail. Cleared at the start of every run so the report expresses
    /// exactly what *this* run injected.
    input_bindings: HashMap<NodeId, Vec<InputBinding>>,
    /// Node-reported execution evidence harvested from terminal results
    /// (see [`NodeReporter::set_run_details`]). Same per-run lifetime as
    /// `input_bindings`.
    node_run_details: HashMap<NodeId, NodeRunDetails>,
    /// Logical provenance for jobs installed from an expanded logical graph.
    pub(crate) physical_jobs: HashMap<NodeId, PhysicalJobRef>,
    /// Logical source graphs whose compiled jobs are installed in this DAG.
    pub(crate) logical_graphs: Vec<LogicalGraph>,
    /// Node factory used to materialize jobs discovered by dynamic fanout.
    dynamic_node_builder: Option<DynamicNodeBuilder>,
    /// Execution backend used for ordinary physical tasks.
    task_executor: TaskExecutorHandle,
    /// Physical-node resource override.
    pub(crate) task_resources: HashMap<NodeId, TaskResources>,
    /// Logical-process resource request inherited by every physical job.
    pub(crate) logical_task_resources: HashMap<String, TaskResources>,
    pub(crate) default_task_resources: TaskResources,
}

impl Clone for DAG {
    fn clone(&self) -> Self {
        Self {
            nodes: self.nodes.clone(),
            graph: self.graph.clone(),
            id_to_idx: self.id_to_idx.clone(),
            statuses: self.statuses.clone(),
            outputs: self.outputs.clone(),
            // DataFusion errors are display-only and not cloneable. They are
            // run-scoped, so a transaction candidate starts with no stale errors.
            errors: HashMap::new(),
            specs: self.specs.clone(),
            fingerprints: self.fingerprints.clone(),
            input_bindings: self.input_bindings.clone(),
            node_run_details: self.node_run_details.clone(),
            physical_jobs: self.physical_jobs.clone(),
            logical_graphs: self.logical_graphs.clone(),
            dynamic_node_builder: self.dynamic_node_builder.clone(),
            task_executor: self.task_executor.clone(),
            task_resources: self.task_resources.clone(),
            logical_task_resources: self.logical_task_resources.clone(),
            default_task_resources: self.default_task_resources.clone(),
        }
    }
}

impl DAG {
    /// Query the runtime status of a node. Returns `None` if the DAG has never
    /// been run.
    pub fn status(&self, id: &str) -> Option<RuntimeStatus> {
        self.statuses.get(id).copied()
    }

    pub fn output(&self, id: &str) -> Option<PortOutputs> {
        self.outputs.get(id).cloned()
    }

    /// The execution fingerprint recorded at `id`'s last successful
    /// execution, if any (see [`crate::fingerprint::compute_node_fingerprint`]).
    /// Read-only introspection for tests, audits, and tooling.
    pub fn recorded_fingerprint(&self, id: &str) -> Option<&str> {
        self.fingerprints.get(id).map(String::as_str)
    }

    /// Install the factory used to materialize runtime-discovered jobs.
    pub fn set_dynamic_node_builder(&mut self, builder: DynamicNodeBuilder) {
        self.dynamic_node_builder = Some(builder);
    }

    /// Replace the backend used to execute dispatched physical tasks.
    pub fn set_task_executor(&mut self, executor: std::sync::Arc<dyn TaskExecutor>) {
        self.task_executor = TaskExecutorHandle { executor };
    }

    /// Set resources for one concrete physical node.
    pub fn set_task_resources(
        &mut self,
        id: impl Into<NodeId>,
        resources: TaskResources,
    ) -> Result<()> {
        let id = id.into();
        if !self.nodes.contains_key(&id) {
            return Err(DagError::UnknownNode(id));
        }
        resources.validate()?;
        self.task_resources.insert(id, resources);
        Ok(())
    }

    /// Set resources for every physical job generated by a logical process.
    pub fn set_logical_task_resources(
        &mut self,
        logical_node: impl Into<String>,
        resources: TaskResources,
    ) -> Result<()> {
        let logical_node = logical_node.into();
        if logical_node.trim().is_empty() {
            return Err(DagError::Schedule(
                "logical task resource id cannot be empty".into(),
            ));
        }
        resources.validate()?;
        self.logical_task_resources.insert(logical_node, resources);
        Ok(())
    }

    /// Set resources inherited by tasks without a physical or logical override.
    pub fn set_default_task_resources(&mut self, resources: TaskResources) -> Result<()> {
        resources.validate()?;
        self.default_task_resources = resources;
        Ok(())
    }
}
