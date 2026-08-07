//! Petgraph-backed structural index for a [`NetworkSpec`](crate::NetworkSpec).
//!
//! `NetworkSpec` stores nodes/edges as flat `Vec`s for serializability and
//! human-friendly editing. `NetworkGraph` wraps a
//! [`petgraph::graph::DiGraph`] to provide O(out-degree) routing queries,
//! cycle detection, topological sort, and reachability — the same pattern
//! used by `dag-core`.

use std::collections::HashMap;

use petgraph::algo::{has_path_connecting, is_cyclic_directed, kosaraju_scc};
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;
use petgraph::Direction;

use crate::spec::{EdgeSpec, NetworkSpec, NodeSpec};

/// A petgraph-backed structural index built from a [`NetworkSpec`].
///
/// Node weights are [`NodeSpec`] clones; edge weights are [`EdgeSpec`]
/// clones. A `name → NodeIndex` map provides O(1) node look-up by name.
///
/// Built once via [`NetworkGraph::from_spec`] and then used for all
/// graph-level queries. The original [`NetworkSpec`] is the canonical
/// serializable form; this struct is the canonical queryable form.
pub struct NetworkGraph {
    graph: DiGraph<NodeSpec, EdgeSpec>,
    name_to_idx: HashMap<String, NodeIndex>,
}

impl NetworkGraph {
    /// Build a graph index from a spec.
    ///
    /// Performs the same structural validation as
    /// [`NetworkSpec::validate`](crate::NetworkSpec::validate): unique node
    /// names, no dangling edge references.
    pub fn from_spec(spec: &NetworkSpec) -> Result<Self, String> {
        if spec.nodes.is_empty() {
            return Err("network must have at least one node".into());
        }

        let mut graph = DiGraph::with_capacity(spec.nodes.len(), spec.edges.len());
        let mut name_to_idx = HashMap::new();

        for node in &spec.nodes {
            if name_to_idx.contains_key(&node.name) {
                return Err(format!("duplicate node name: {}", node.name));
            }
            let idx = graph.add_node(node.clone());
            name_to_idx.insert(node.name.clone(), idx);
        }

        for edge in &spec.edges {
            let from = *name_to_idx
                .get(&edge.from)
                .ok_or_else(|| format!("edge references unknown source node: {}", edge.from))?;
            let to = *name_to_idx
                .get(&edge.to)
                .ok_or_else(|| format!("edge references unknown target node: {}", edge.to))?;
            graph.add_edge(from, to, edge.clone());
        }

        Ok(Self { graph, name_to_idx })
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

    /// All node names.
    pub fn node_names(&self) -> Vec<&str> {
        self.graph
            .node_weights()
            .map(|n| n.name.as_str())
            .collect()
    }

    // ── Edge queries (the core routing primitive) ──────────

    /// All outgoing edges from a node — used for routing.
    ///
    /// O(out-degree) instead of O(|E|) linear scan.
    pub fn out_edges(&self, name: &str) -> Vec<&EdgeSpec> {
        let Some(idx) = self.node_index(name) else {
            return vec![];
        };
        self.graph
            .edges_directed(idx, Direction::Outgoing)
            .map(|e| e.weight())
            .collect()
    }

    /// All incoming edges to a node — who can route *to* this node?
    pub fn in_edges(&self, name: &str) -> Vec<&EdgeSpec> {
        let Some(idx) = self.node_index(name) else {
            return vec![];
        };
        self.graph
            .edges_directed(idx, Direction::Incoming)
            .map(|e| e.weight())
            .collect()
    }

    /// Direct successors (by name) of a node.
    pub fn successors(&self, name: &str) -> Vec<&str> {
        let Some(idx) = self.node_index(name) else {
            return vec![];
        };
        self.graph
            .neighbors_directed(idx, Direction::Outgoing)
            .map(|i| self.graph[i].name.as_str())
            .collect()
    }

    /// Direct predecessors (by name) of a node.
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
    ///
    /// Cyclic = arena/loop topology (writer ↔ reviewer).
    /// Acyclic = pipeline/tree topology (A → B → C).
    pub fn is_cyclic(&self) -> bool {
        is_cyclic_directed(&self.graph)
    }

    /// Strongly connected components (each SCC is a maximal set of nodes
    /// that are mutually reachable).
    ///
    /// In an arena (writer ↔ reviewer) the whole graph is one SCC.
    /// In a pipeline each node is its own SCC.
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

    /// Nodes with no outgoing edges — potential terminal nodes in a
    /// pipeline.
    pub fn leaf_nodes(&self) -> Vec<&str> {
        self.graph
            .node_indices()
            .filter(|i| self.graph.edges_directed(*i, Direction::Outgoing).count() == 0)
            .map(|i| self.graph[i].name.as_str())
            .collect()
    }

    /// Nodes with no incoming edges — potential entry points.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{EdgeSpec, EdgeTrigger, NetworkSpec, NodeSpec, TerminationSpec};

    fn arena_spec() -> NetworkSpec {
        NetworkSpec {
            name: "arena".into(),
            nodes: vec![
                NodeSpec {
                    name: "writer".into(),
                    profile: "w".into(),
                    initial_prompt: Some("go".into()),
                },
                NodeSpec {
                    name: "reviewer".into(),
                    profile: "r".into(),
                    initial_prompt: None,
                },
            ],
            edges: vec![
                EdgeSpec {
                    from: "writer".into(),
                    to: "reviewer".into(),
                    trigger: EdgeTrigger::OnDone,
                    transform: None,
                },
                EdgeSpec {
                    from: "reviewer".into(),
                    to: "writer".into(),
                    trigger: EdgeTrigger::OnDone,
                    transform: None,
                },
            ],
            termination: TerminationSpec::MaxRounds { max: 10 },
        }
    }

    fn pipeline_spec() -> NetworkSpec {
        NetworkSpec {
            name: "pipe".into(),
            nodes: vec![
                NodeSpec {
                    name: "a".into(),
                    profile: "x".into(),
                    initial_prompt: Some("go".into()),
                },
                NodeSpec {
                    name: "b".into(),
                    profile: "y".into(),
                    initial_prompt: None,
                },
                NodeSpec {
                    name: "c".into(),
                    profile: "z".into(),
                    initial_prompt: None,
                },
            ],
            edges: vec![
                EdgeSpec {
                    from: "a".into(),
                    to: "b".into(),
                    trigger: EdgeTrigger::OnDone,
                    transform: None,
                },
                EdgeSpec {
                    from: "b".into(),
                    to: "c".into(),
                    trigger: EdgeTrigger::OnDone,
                    transform: None,
                },
            ],
            termination: TerminationSpec::AnyNodeDone {
                nodes: vec!["c".into()],
            },
        }
    }

    #[test]
    fn build_from_spec() {
        let g = NetworkGraph::from_spec(&arena_spec()).unwrap();
        assert_eq!(g.node_count(), 2);
        assert_eq!(g.edge_count(), 2);
    }

    #[test]
    fn rejects_duplicate_names() {
        let mut spec = arena_spec();
        spec.nodes.push(NodeSpec {
            name: "writer".into(),
            profile: "dup".into(),
            initial_prompt: None,
        });
        assert!(NetworkGraph::from_spec(&spec).is_err());
    }

    #[test]
    fn rejects_dangling_edge() {
        let mut spec = arena_spec();
        spec.edges.push(EdgeSpec {
            from: "writer".into(),
            to: "ghost".into(),
            trigger: EdgeTrigger::OnDone,
            transform: None,
        });
        assert!(NetworkGraph::from_spec(&spec).is_err());
    }

    #[test]
    fn arena_is_cyclic() {
        let g = NetworkGraph::from_spec(&arena_spec()).unwrap();
        assert!(g.is_cyclic());
    }

    #[test]
    fn pipeline_is_acyclic() {
        let g = NetworkGraph::from_spec(&pipeline_spec()).unwrap();
        assert!(!g.is_cyclic());
    }

    #[test]
    fn out_edges_for_routing() {
        let g = NetworkGraph::from_spec(&arena_spec()).unwrap();
        let writer_edges = g.out_edges("writer");
        assert_eq!(writer_edges.len(), 1);
        assert_eq!(writer_edges[0].to, "reviewer");
    }

    #[test]
    fn successors_and_predecessors() {
        let g = NetworkGraph::from_spec(&pipeline_spec()).unwrap();
        assert_eq!(g.successors("a"), vec!["b"]);
        assert_eq!(g.successors("c"), Vec::<&str>::new());
        assert_eq!(g.predecessors("a"), Vec::<&str>::new());
        assert_eq!(g.predecessors("c"), vec!["b"]);
    }

    #[test]
    fn arena_scc_is_single_component() {
        let g = NetworkGraph::from_spec(&arena_spec()).unwrap();
        let sccs = g.sccs();
        assert_eq!(sccs.len(), 1);
        assert_eq!(sccs[0].len(), 2);
    }

    #[test]
    fn pipeline_sccs_are_singletons() {
        let g = NetworkGraph::from_spec(&pipeline_spec()).unwrap();
        let sccs = g.sccs();
        assert_eq!(sccs.len(), 3);
        for scc in &sccs {
            assert_eq!(scc.len(), 1);
        }
    }

    #[test]
    fn has_path() {
        let g = NetworkGraph::from_spec(&pipeline_spec()).unwrap();
        assert!(g.has_path("a", "c"));
        assert!(g.has_path("a", "b"));
        assert!(!g.has_path("c", "a"));
        assert!(!g.has_path("b", "a"));
    }

    #[test]
    fn leaf_and_root_nodes() {
        let g = NetworkGraph::from_spec(&pipeline_spec()).unwrap();
        // Pipeline: a → b → c
        assert_eq!(g.root_nodes(), vec!["a"]);
        assert_eq!(g.leaf_nodes(), vec!["c"]);
    }

    #[test]
    fn arena_has_no_leaf_or_root() {
        let g = NetworkGraph::from_spec(&arena_spec()).unwrap();
        assert!(g.root_nodes().is_empty());
        assert!(g.leaf_nodes().is_empty());
    }
}
