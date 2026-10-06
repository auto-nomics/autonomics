//! Read-only topology: predecessor/successor/edge queries, topological
//! order, cycle-path recovery, dot rendering, and the TUI snapshot.

use std::collections::HashSet;

use petgraph::Direction;
use petgraph::algo::{kosaraju_scc, toposort};
use petgraph::dot::Dot;
use petgraph::graph::NodeIndex;
use petgraph::visit::EdgeRef;

use super::{DAG, DagEdge, EdgeLabel, Result};
use crate::dag::error::DagError;
use crate::dag::{DagNode, NodeId};

impl DAG {
    /// All node ids, in graph (arbitrary) order.
    pub fn node_ids(&self) -> Vec<NodeId> {
        self.nodes.keys().cloned().collect()
    }

    /// Direct predecessors of `id`.
    pub fn predecessors(&self, id: &str) -> Vec<NodeId> {
        let Some(&idx) = self.id_to_idx.get(id) else {
            return Vec::new();
        };
        self.graph
            .neighbors_directed(idx, Direction::Incoming)
            .map(|i| self.graph[i].clone())
            .collect()
    }

    /// Direct successors of `id`.
    pub fn successors(&self, id: &str) -> Vec<NodeId> {
        let Some(&idx) = self.id_to_idx.get(id) else {
            return Vec::new();
        };
        self.graph
            .neighbors_directed(idx, Direction::Outgoing)
            .map(|i| self.graph[i].clone())
            .collect()
    }

    /// Incoming edges for `id`, in insertion order. Returns predecessor ids.
    pub fn incoming_edges(&self, id: &str) -> Vec<NodeId> {
        self.incoming_edges_with_ports(id)
            .into_iter()
            .map(|(nid, _)| nid)
            .collect()
    }

    /// Incoming edges for `id` with their port labels, in insertion order.
    /// Returns `(predecessor_id, edge_label)` pairs.
    pub fn incoming_edges_with_ports(&self, id: &str) -> Vec<(NodeId, EdgeLabel)> {
        let Some(&idx) = self.id_to_idx.get(id) else {
            return Vec::new();
        };
        self.graph
            .edges_directed(idx, Direction::Incoming)
            .map(|e| (self.graph[e.source()].clone(), e.weight().clone()))
            .collect()
    }

    /// All edges in graph insertion order.
    ///
    /// This is the stable topology view needed by exporters and UI snapshots;
    /// per-node predecessor/successor queries do not expose edge ports.
    pub fn edges(&self) -> Vec<DagEdge> {
        self.graph
            .edge_references()
            .map(|edge| DagEdge {
                from_node: self.graph[edge.source()].clone(),
                to_node: self.graph[edge.target()].clone(),
                from_port: edge.weight().from_port,
                to_port: edge.weight().to_port,
            })
            .collect()
    }

    /// Topological order (predecessors before successors). Errors on a cycle.
    pub fn topo_order(&self) -> Result<Vec<NodeId>> {
        match toposort(&self.graph, None) {
            Ok(order) => Ok(order.iter().map(|i| self.graph[*i].clone()).collect()),
            Err(_) => Err(DagError::Cycle(self.cycle_node_names())),
        }
    }

    /// Borrow a node payload by id. Returns the trait object directly — no
    /// `Box` in the return type, since callers only want to call methods on
    /// the node (or take a fresh `Box<dyn DagNode>` themselves if they need
    /// ownership).
    pub fn get_node(&self, id: &str) -> Option<&dyn DagNode> {
        self.nodes.get(id).map(|b| b.as_ref())
    }

    /// Build an owned snapshot for interactive DAG consumers.
    ///
    /// Node order is id-sorted so repeated snapshots have stable selection and
    /// layout, independent of the graph payload HashMap's iteration order.
    pub fn tui_snapshot(&self) -> super::super::view::DagTuiSnapshot {
        let mut nodes = Vec::with_capacity(self.nodes.len());
        for id in self.node_ids() {
            let Some(node) = self.nodes.get(&id) else {
                continue;
            };
            let ports = node.ports();
            nodes.push(super::super::view::DagNodeView {
                kind: node.kind().to_string(),
                status: self.status(&id).unwrap_or_default(),
                dirty: self.is_dirty(&id),
                inputs: ports
                    .input_ports()
                    .iter()
                    .map(|port| super::super::view::DagPortView {
                        index: port.index,
                        label: port.label.clone(),
                        data_type: port.data_type.to_string(),
                    })
                    .collect(),
                outputs: ports
                    .output_ports()
                    .iter()
                    .map(|port| super::super::view::DagPortView {
                        index: port.index,
                        label: port.label.clone(),
                        data_type: port.data_type.to_string(),
                    })
                    .collect(),
                id,
            });
        }
        nodes.sort_unstable_by(|a, b| a.id.cmp(&b.id));

        let edges = self
            .edges()
            .into_iter()
            .map(|edge| super::super::view::DagEdgeView {
                from: edge.from_node,
                from_port: edge.from_port,
                to: edge.to_node,
                to_port: edge.to_port,
            })
            .collect();

        super::super::view::DagTuiSnapshot { nodes, edges }
    }

    /// Build a human-readable cycle path like `A → B → C → A` from the first
    /// strongly-connected component that contains a cycle.
    ///
    /// Uses DFS within the SCC to recover an actual cycle (not just the node set).
    pub(super) fn cycle_node_names(&self) -> String {
        let sccs = kosaraju_scc(&self.graph);
        for scc in sccs {
            let cyclic = scc.len() > 1
                || scc
                    .first()
                    .map(|&i| self.graph.neighbors(i).any(|j| j == i))
                    .unwrap_or(false);
            if !cyclic {
                continue;
            }
            // Collect node ids in this SCC and map from NodeIndex → node id.
            let ids: Vec<String> = scc.iter().map(|&i| self.graph[i].clone()).collect();
            let idx_set: HashSet<NodeIndex> = scc.iter().copied().collect();
            // DFS to find an actual cycle path within the SCC.
            if let Some(path) = self.find_cycle_path(&idx_set) {
                let names: Vec<&str> = path.iter().map(|&i| self.graph[i].as_str()).collect();
                return names.join(" → ");
            }
            // Fallback: list the SCC members (shouldn't happen for a cyclic SCC).
            return ids.join(", ");
        }
        String::from("<unknown>")
    }

    /// DFS within a known SCC to recover one concrete cycle path.
    ///
    /// Returns a vec of [`NodeIndex`] forming a cycle (first element == last).
    fn find_cycle_path(&self, idx_set: &HashSet<NodeIndex>) -> Option<Vec<NodeIndex>> {
        // Try DFS from each node in the SCC until we find a back-edge.
        let start = *idx_set.iter().next()?;
        let mut stack: Vec<NodeIndex> = vec![start];
        let mut on_stack: HashSet<NodeIndex> = HashSet::from([start]);
        let mut visited: HashSet<NodeIndex> = HashSet::from([start]);

        loop {
            let &current = stack.last()?;
            // Look for a successor that is still on the stack (back-edge = cycle).
            for neighbor in self.graph.neighbors_directed(current, Direction::Outgoing) {
                if !idx_set.contains(&neighbor) {
                    continue;
                }
                if on_stack.contains(&neighbor) {
                    // Found a cycle: extract the portion from `neighbor` to end.
                    let cycle_start = stack.iter().position(|&n| n == neighbor).unwrap();
                    let mut path: Vec<NodeIndex> = stack[cycle_start..].to_vec();
                    path.push(neighbor);
                    return Some(path);
                }
                if !visited.contains(&neighbor) {
                    visited.insert(neighbor);
                    on_stack.insert(neighbor);
                    stack.push(neighbor);
                    break; // continue DFS from the pushed neighbor
                }
            }
            // If no unvisited successor was pushed, backtrack.
            if *stack.last().unwrap() == current {
                // We processed all neighbors without finding a cycle — pop and try next.
                on_stack.remove(&current);
                stack.pop();
                if stack.is_empty() {
                    return None;
                }
            }
        }
    }

    /// Render DAG topology into dot code
    pub fn to_dot(&self) -> String {
        format!("{:?}", Dot::with_config(&self.graph, &[]))
    }
}
