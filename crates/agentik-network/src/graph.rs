//! Petgraph-backed mutable topology graph.
//!
//! Wraps [`petgraph::graph::DiGraph`] with a name→index map for O(1)
//! node look-up and O(out-degree) edge queries. Unlike the previous
//! read-only design, this graph supports **runtime topology mutation** —
//! nodes and edges can be added/removed dynamically.

use std::collections::HashMap;

use petgraph::algo::{has_path_connecting, is_cyclic_directed, kosaraju_scc};
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;
use petgraph::Direction;

use crate::spec::{EdgeSpec, EdgeTrigger, NodeSpec};

/// A mutable petgraph-backed topology graph.
///
/// Node weights are [`NodeSpec`]; edge weights are [`EdgeSpec`].
/// A `name → NodeIndex` map provides O(1) node look-up by name.
pub struct NetworkGraph {
    graph: DiGraph<NodeSpec, EdgeSpec>,
    name_to_idx: HashMap<String, NodeIndex>,
}

impl NetworkGraph {
    /// Create an empty graph.
    pub fn new() -> Self {
        Self {
            graph: DiGraph::new(),
            name_to_idx: HashMap::new(),
        }
    }

    /// Build a graph from a spec's nodes + edges (for batch construction).
    pub fn from_nodes_edges(
        nodes: &[NodeSpec],
        edges: &[EdgeSpec],
    ) -> Result<Self, String> {
        let mut g = Self::new();
        for node in nodes {
            g.add_node(node.clone())?;
        }
        for edge in edges {
            g.add_edge(edge.clone())?;
        }
        Ok(g)
    }

    // ── Topology mutation ──────────────────────────────────

    /// Add a node. Returns error if the name already exists.
    pub fn add_node(&mut self, node: NodeSpec) -> Result<(), String> {
        if self.name_to_idx.contains_key(&node.name) {
            return Err(format!("node already exists: {}", node.name));
        }
        let idx = self.graph.add_node(node);
        // Re-borrow after add_node to get the name from the graph.
        let name = self.graph[idx].name.clone();
        self.name_to_idx.insert(name, idx);
        Ok(())
    }

    /// Remove a node and all its edges.
    pub fn remove_node(&mut self, name: &str) -> Option<NodeSpec> {
        let idx = self.name_to_idx.remove(name)?;
        let node = self.graph.remove_node(idx);
        // petgraph::Graph uses swap_remove — indices may shift.
        // Rebuild the name→index map to stay consistent.
        if node.is_some() {
            self.rebuild_index();
        }
        node
    }

    /// Add a directed edge. Returns error if endpoints don't exist.
    pub fn add_edge(&mut self, edge: EdgeSpec) -> Result<(), String> {
        let from = self
            .node_index(&edge.from)
            .ok_or_else(|| format!("source node not found: {}", edge.from))?;
        let to = self
            .node_index(&edge.to)
            .ok_or_else(|| format!("target node not found: {}", edge.to))?;
        self.graph.add_edge(from, to, edge);
        Ok(())
    }

    /// Remove all edges from `from` to `to`.
    pub fn remove_edges(&mut self, from: &str, to: &str) -> usize {
        let (Some(from_idx), Some(to_idx)) =
            (self.node_index(from), self.node_index(to))
        else {
            return 0;
        };
        let edge_ids: Vec<_> = self
            .graph
            .edges_connecting(from_idx, to_idx)
            .map(|e| e.id())
            .collect();
        let count = edge_ids.len();
        for id in edge_ids {
            self.graph.remove_edge(id);
        }
        count
    }

    /// Remove all edges matching a predicate.
    pub fn remove_edges_where<F>(&mut self, mut pred: F)
    where
        F: FnMut(&EdgeSpec) -> bool,
    {
        let edge_ids: Vec<_> = self
            .graph
            .edge_indices()
            .filter(|i| pred(&self.graph[*i]))
            .collect();
        for id in edge_ids {
            self.graph.remove_edge(id);
        }
    }

    /// Rebuild the name→index map after structural operations that
    /// invalidate indices (e.g. `remove_node` with swap-remove).
    fn rebuild_index(&mut self) {
        self.name_to_idx.clear();
        for idx in self.graph.node_indices() {
            let name = self.graph[idx].name.clone();
            self.name_to_idx.insert(name, idx);
        }
    }

    // ── Node look-up ───────────────────────────────────────

    /// Resolve a node name to its petgraph index.
    pub fn node_index(&self, name: &str) -> Option<NodeIndex> {
        self.name_to_idx.get(name).copied()
    }

    /// Get a node spec by name.
    pub fn node(&self, name: &str) -> Option<&NodeSpec> {
        self.node_index(name).map(|i| &self.graph[i])
    }

    /// Get a mutable node spec by name.
    pub fn node_mut(&mut self, name: &str) -> Option<&mut NodeSpec> {
        let idx = self.node_index(name)?;
        self.graph.node_weight_mut(idx)
    }

    /// All node names.
    pub fn node_names(&self) -> Vec<&str> {
        self.graph
            .node_weights()
            .map(|n| n.name.as_str())
            .collect()
    }

    // ── Edge queries ───────────────────────────────────────

    /// All outgoing edges from a node — the core routing primitive.
    /// O(out-degree).
    pub fn out_edges(&self, name: &str) -> Vec<&EdgeSpec> {
        let Some(idx) = self.node_index(name) else {
            return vec![];
        };
        self.graph
            .edges_directed(idx, Direction::Outgoing)
            .map(|e| e.weight())
            .collect()
    }

    /// All incoming edges to a node.
    pub fn in_edges(&self, name: &str) -> Vec<&EdgeSpec> {
        let Some(idx) = self.node_index(name) else {
            return vec![];
        };
        self.graph
            .edges_directed(idx, Direction::Incoming)
            .map(|e| e.weight())
            .collect()
    }

    /// Direct successors (by name).
    pub fn successors(&self, name: &str) -> Vec<&str> {
        let Some(idx) = self.node_index(name) else {
            return vec![];
        };
        self.graph
            .neighbors_directed(idx, Direction::Outgoing)
            .map(|i| self.graph[i].name.as_str())
            .collect()
    }

    /// Direct predecessors (by name).
    pub fn predecessors(&self, name: &str) -> Vec<&str> {
        let Some(idx) = self.node_index(name) else {
            return vec![];
        };
        self.graph
            .neighbors_directed(idx, Direction::Incoming)
            .map(|i| self.graph[i].name.as_str())
            .collect()
    }

    // ── Graph algorithms ───────────────────────────────────

    /// Whether the topology contains any directed cycle.
    pub fn is_cyclic(&self) -> bool {
        is_cyclic_directed(&self.graph)
    }

    /// Strongly connected components.
    pub fn sccs(&self) -> Vec<Vec<String>> {
        kosaraju_scc(&self.graph)
            .into_iter()
            .map(|scc| {
                scc.into_iter()
                    .map(|i| self.graph[i].name.clone())
                    .collect()
            })
            .collect()
    }

    /// Is there a directed path from `from` to `to`?
    pub fn has_path(&self, from: &str, to: &str) -> bool {
        let (Some(from_idx), Some(to_idx)) =
            (self.node_index(from), self.node_index(to))
        else {
            return false;
        };
        has_path_connecting(&self.graph, from_idx, to_idx, None)
    }

    /// Nodes with no outgoing edges (terminal nodes).
    pub fn leaf_nodes(&self) -> Vec<&str> {
        self.graph
            .node_indices()
            .filter(|i| self.graph.edges_directed(*i, Direction::Outgoing).count() == 0)
            .map(|i| self.graph[i].name.as_str())
            .collect()
    }

    /// Nodes with no incoming edges (entry points).
    pub fn root_nodes(&self) -> Vec<&str> {
        self.graph
            .node_indices()
            .filter(|i| self.graph.edges_directed(*i, Direction::Incoming).count() == 0)
            .map(|i| self.graph[i].name.as_str())
            .collect()
    }

    /// Total node count.
    pub fn node_count(&self) -> usize {
        self.graph.node_count()
    }

    /// Total edge count.
    pub fn edge_count(&self) -> usize {
        self.graph.edge_count()
    }
}

impl Default for NetworkGraph {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{EdgeSpec, EdgeTrigger, NodeSpec, TerminationSpec};

    fn writer_node() -> NodeSpec {
        NodeSpec {
            name: "writer".into(),
            profile: "w".into(),
            initial_prompt: Some("go".into()),
        }
    }

    fn reviewer_node() -> NodeSpec {
        NodeSpec {
            name: "reviewer".into(),
            profile: "r".into(),
            initial_prompt: None,
        }
    }

    fn done_edge(from: &str, to: &str) -> EdgeSpec {
        EdgeSpec {
            from: from.into(),
            to: to.into(),
            trigger: EdgeTrigger::OnDone,
            transform: None,
        }
    }

    // ── Mutation tests ──

    #[test]
    fn add_and_remove_nodes() {
        let mut g = NetworkGraph::new();
        g.add_node(writer_node()).unwrap();
        g.add_node(reviewer_node()).unwrap();
        assert_eq!(g.node_count(), 2);

        g.remove_node("writer");
        assert_eq!(g.node_count(), 1);
        assert!(g.node("writer").is_none());
        assert!(g.node("reviewer").is_some());
    }

    #[test]
    fn add_and_remove_edges() {
        let mut g = NetworkGraph::new();
        g.add_node(writer_node()).unwrap();
        g.add_node(reviewer_node()).unwrap();

        g.add_edge(done_edge("writer", "reviewer")).unwrap();
        assert_eq!(g.edge_count(), 1);
        assert_eq!(g.out_edges("writer").len(), 1);

        let removed = g.remove_edges("writer", "reviewer");
        assert_eq!(removed, 1);
        assert_eq!(g.edge_count(), 0);
    }

    #[test]
    fn remove_node_cleans_up_edges() {
        let mut g = NetworkGraph::new();
        g.add_node(writer_node()).unwrap();
        g.add_node(reviewer_node()).unwrap();
        g.add_edge(done_edge("writer", "reviewer")).unwrap();
        g.add_edge(done_edge("reviewer", "writer")).unwrap();

        g.remove_node("writer");
        assert_eq!(g.node_count(), 1);
        assert_eq!(g.edge_count(), 0); // both edges gone
    }

    #[test]
    fn reject_duplicate_node() {
        let mut g = NetworkGraph::new();
        g.add_node(writer_node()).unwrap();
        assert!(g.add_node(writer_node()).is_err());
    }

    #[test]
    fn reject_edge_to_missing_node() {
        let mut g = NetworkGraph::new();
        g.add_node(writer_node()).unwrap();
        assert!(g.add_edge(done_edge("writer", "ghost")).is_err());
    }

    #[test]
    fn remove_edges_where_predicate() {
        let mut g = NetworkGraph::new();
        g.add_node(writer_node()).unwrap();
        g.add_node(reviewer_node()).unwrap();
        g.add_edge(done_edge("writer", "reviewer")).unwrap();
        g.add_edge(EdgeSpec {
            from: "writer".into(),
            to: "reviewer".into(),
            trigger: EdgeTrigger::OnPattern {
                pattern: "ready".into(),
            },
            transform: None,
        })
        .unwrap();
        assert_eq!(g.edge_count(), 2);

        g.remove_edges_where(|e| matches!(e.trigger, EdgeTrigger::OnPattern { .. }));
        assert_eq!(g.edge_count(), 1);
    }

    // ── Query tests ──

    #[test]
    fn arena_is_cyclic_pipeline_is_not() {
        let mut g = NetworkGraph::new();
        g.add_node(writer_node()).unwrap();
        g.add_node(reviewer_node()).unwrap();
        g.add_edge(done_edge("writer", "reviewer")).unwrap();
        g.add_edge(done_edge("reviewer", "writer")).unwrap();
        assert!(g.is_cyclic());

        let mut p = NetworkGraph::new();
        for name in ["a", "b", "c"] {
            p.add_node(NodeSpec {
                name: name.into(),
                profile: name.into(),
                initial_prompt: None,
            })
            .unwrap();
        }
        p.add_edge(done_edge("a", "b")).unwrap();
        p.add_edge(done_edge("b", "c")).unwrap();
        assert!(!p.is_cyclic());
    }

    #[test]
    fn successors_and_predecessors() {
        let mut g = NetworkGraph::new();
        for name in ["a", "b", "c"] {
            g.add_node(NodeSpec {
                name: name.into(),
                profile: name.into(),
                initial_prompt: None,
            })
            .unwrap();
        }
        g.add_edge(done_edge("a", "b")).unwrap();
        g.add_edge(done_edge("b", "c")).unwrap();

        assert_eq!(g.successors("a"), vec!["b"]);
        assert_eq!(g.predecessors("c"), vec!["b"]);
        assert!(g.successors("c").is_empty());
        assert!(g.predecessors("a").is_empty());
    }

    #[test]
    fn leaf_and_root_nodes() {
        let mut g = NetworkGraph::new();
        for name in ["a", "b", "c"] {
            g.add_node(NodeSpec {
                name: name.into(),
                profile: name.into(),
                initial_prompt: None,
            })
            .unwrap();
        }
        g.add_edge(done_edge("a", "b")).unwrap();
        g.add_edge(done_edge("b", "c")).unwrap();

        assert_eq!(g.root_nodes(), vec!["a"]);
        assert_eq!(g.leaf_nodes(), vec!["c"]);
    }

    #[test]
    fn has_path() {
        let mut g = NetworkGraph::new();
        for name in ["a", "b", "c"] {
            g.add_node(NodeSpec {
                name: name.into(),
                profile: name.into(),
                initial_prompt: None,
            })
            .unwrap();
        }
        g.add_edge(done_edge("a", "b")).unwrap();
        g.add_edge(done_edge("b", "c")).unwrap();

        assert!(g.has_path("a", "c"));
        assert!(!g.has_path("c", "a"));
    }

    #[test]
    fn node_mut() {
        let mut g = NetworkGraph::new();
        g.add_node(writer_node()).unwrap();
        g.node_mut("writer").unwrap().initial_prompt = Some("new prompt".into());
        assert_eq!(g.node("writer").unwrap().initial_prompt.as_deref(), Some("new prompt"));
    }
}
