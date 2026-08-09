//! Data analysis engine — aggregates the DAG core with concrete node bundles.
//!
//! All infrastructure (DagNode trait, NodeRegistry, DAG scheduler, codegen)
//! lives in [`dag_core`]. This crate re-exports those modules under their
//! familiar `data_engine::dag`, `data_engine::codegen`, etc. paths so that
//! existing consumers need no changes.

// Re-export core infrastructure from dag-core under the original paths.
pub use dag_core::codegen;
pub use dag_core::dag;
pub use dag_core::dataset;
pub use dag_core::error;
// Node-level traits and types.
pub use dag_core::arrow_util;
pub use dag_core::node;
pub use dag_core::sink;
// Registry under its original `node_registry` path for backward compat.
pub use dag_core::registry as node_registry;
pub use dag_core::types;

pub mod data_engine;
pub mod default_registry;
pub mod nodes;
pub mod runtime;

// Convenience re-exports (backward compat with existing `use data_engine::*`).
pub use dag_core::{DagNode, NodeCtx, NodeFactory, NodeId, NodeInput, NodePorts, NodeRegistry};

// Codegen tests require concrete node types, so they live here not in dag-core.
#[cfg(test)]
mod codegen_tests;
#[cfg(test)]
mod xval_tests;
