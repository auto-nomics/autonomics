//! Manifest export and retained-spec queries: serialize the current DAG
//! topology + node specs for snapshot persistence.

use petgraph::visit::EdgeRef;

use super::super::history::{
    DagManifest, EdgeEntry, LOGICAL_COMPILER_VERSION, LogicalManifest, MANIFEST_SCHEMA_VERSION,
    NodeEntry,
};
use super::super::logical::LogicalGraph;
use super::DAG;

impl DAG {
    /// Export the current DAG topology + node specs as a serializable manifest
    /// for snapshot persistence. Nodes without a retained spec are omitted
    /// (they were added via the raw [`Self::add_node`] path, not through the
    /// registry).
    pub fn to_manifest(&self) -> DagManifest {
        let mut nodes = Vec::new();
        for (id, (kind, spec)) in &self.specs {
            nodes.push(NodeEntry {
                id: id.clone(),
                kind: kind.clone(),
                spec: spec.clone(),
            });
        }
        nodes.sort_by(|left, right| left.id.cmp(&right.id));

        let mut edges = Vec::new();
        for edge in self.graph.edge_references() {
            let from = &self.graph[edge.source()];
            let to = &self.graph[edge.target()];
            let label = edge.weight();
            edges.push(EdgeEntry {
                from: from.clone(),
                from_port: label.from_port,
                to: to.clone(),
                to_port: label.to_port,
            });
        }

        DagManifest {
            schema_version: MANIFEST_SCHEMA_VERSION,
            logical: LogicalManifest {
                compiler_version: LOGICAL_COMPILER_VERSION,
                graphs: self.logical_graphs.clone(),
                default_task_resources: self.default_task_resources.clone(),
                logical_task_resources: self
                    .logical_task_resources
                    .iter()
                    .map(|(id, resources)| (id.clone(), resources.clone()))
                    .collect(),
                physical_task_resources: self
                    .task_resources
                    .iter()
                    .map(|(id, resources)| (id.clone(), resources.clone()))
                    .collect(),
            },
            nodes,
            edges,
            physical_jobs: self
                .physical_jobs
                .iter()
                .map(|(id, job)| (id.clone(), job.clone()))
                .collect(),
        }
    }

    /// Return the retained logical source graphs.
    pub fn logical_graphs(&self) -> &[LogicalGraph] {
        &self.logical_graphs
    }

    /// Whether a spec has been retained for `id` (i.e. the node was added via
    /// [`Self::add_node_with_spec`] or [`Self::replace_node_with_spec`]).
    pub fn has_spec(&self, id: &str) -> bool {
        self.specs.contains_key(id)
    }

    /// Get the retained `(kind, spec)` for a node id.
    ///
    /// Returns `None` if the node does not exist or was added via the raw
    /// [`Self::add_node`] path (no retained spec). Returned values are cloned
    /// so the caller owns them without borrowing `self`.
    pub fn node_spec(&self, id: &str) -> Option<(String, serde_json::Value)> {
        self.specs.get(id).cloned()
    }
}
