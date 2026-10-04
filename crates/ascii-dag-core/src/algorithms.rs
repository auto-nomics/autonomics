//! Graph algorithms: cycle detection, generic analysis, and Sugiyama layout.
//!
//! ## Submodules
//!
//! - [`cycles`] - Cycle detection for DAGs and generic graphs
//! - `generic` (feature `generic`) - Generic graph algorithms (topological sort, impact analysis, metrics, traversal)
//! - [`sugiyama`] - Sugiyama hierarchical layout algorithm (heap and arena variants)

pub mod cycles;

pub mod sugiyama;
