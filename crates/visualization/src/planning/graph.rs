//! Observations about declared topology, never scientific or causal validation.
use crate::sketch::Direction;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GraphReview {
    pub scope: String,
    pub verification: String,
    pub weak_components: Vec<Vec<String>>,
    pub source_node_ids: Vec<String>,
    pub sink_node_ids: Vec<String>,
    pub isolated_node_ids: Vec<String>,
    pub invalid_edge_indices: Vec<usize>,
}
impl super::Brief {
    pub fn graph_review(&self) -> GraphReview {
        let index: HashMap<_, _> = self
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.id.as_str(), i))
            .collect();
        let mut neighbors = vec![Vec::new(); self.nodes.len()];
        let mut incoming = vec![0; self.nodes.len()];
        let mut outgoing = vec![0; self.nodes.len()];
        let mut invalid_edge_indices = Vec::new();
        for (i, edge) in self.edges.iter().enumerate() {
            let (Some(&from), Some(&to)) =
                (index.get(edge.from.as_str()), index.get(edge.to.as_str()))
            else {
                invalid_edge_indices.push(i);
                continue;
            };
            neighbors[from].push(to);
            neighbors[to].push(from);
            incoming[to] += 1;
            outgoing[from] += 1;
            if edge.direction != Direction::Forward {
                // Both endpoints are incident in both directions for topology only.
                incoming[from] += 1;
                outgoing[to] += 1;
            }
        }
        let mut seen = vec![false; self.nodes.len()];
        let mut components = Vec::new();
        for start in 0..self.nodes.len() {
            if seen[start] {
                continue;
            }
            let mut stack = vec![start];
            let mut members = Vec::new();
            seen[start] = true;
            while let Some(node) = stack.pop() {
                members.push(node);
                for &neighbor in &neighbors[node] {
                    if !seen[neighbor] {
                        seen[neighbor] = true;
                        stack.push(neighbor);
                    }
                }
            }
            members.sort_unstable();
            components.push(
                members
                    .into_iter()
                    .map(|i| self.nodes[i].id.clone())
                    .collect(),
            );
        }
        let ids = |predicate: &dyn Fn(usize) -> bool| {
            self.nodes
                .iter()
                .enumerate()
                .filter(|(i, _)| predicate(*i))
                .map(|(_, n)| n.id.clone())
                .collect()
        };
        GraphReview {
            scope: "declared_graph_only".into(),
            verification: "not_verified".into(),
            weak_components: components,
            source_node_ids: ids(&|i| incoming[i] == 0 && outgoing[i] > 0),
            sink_node_ids: ids(&|i| incoming[i] > 0 && outgoing[i] == 0),
            isolated_node_ids: ids(&|i| neighbors[i].is_empty()),
            invalid_edge_indices,
        }
    }
}
