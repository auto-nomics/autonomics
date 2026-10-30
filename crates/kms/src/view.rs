//! Local view types for stateless knowledge-tree navigation.
//!
//! These types support a "local view" pattern for read-only agents, which
//! lets them obtain structured information about any tree node without
//! mutating the global pointer.

use uuid::Uuid;

use crate::storage::types::{Index, TargetType};

pub const SUBTREE_TITLES_LIMIT: usize = 30;

/// A compact projection of a single tree node for use in `LocalView` and
/// tool output.
#[derive(Debug, Clone)]
pub struct IndexView {
    pub id: Uuid,
    pub title: String,
    pub target_type: TargetType,
    pub position: i64,
}

/// Aggregate statistics about a node's subtree.
#[derive(Debug, Clone)]
pub struct SubtreeSummary {
    pub total_nodes: usize,
    pub knowledge_count: usize,
    pub group_count: usize,
    pub max_depth: usize,
    pub knowledge_titles: Vec<String>,
    pub truncated: bool,
}

/// A stateless snapshot of a tree node and its immediate context.
#[derive(Debug, Clone)]
pub struct LocalView {
    pub node: Index,
    pub path: Vec<Index>,
    pub children: Vec<IndexView>,
    pub sibling_count: usize,
    pub subtree_summary: SubtreeSummary,
}
