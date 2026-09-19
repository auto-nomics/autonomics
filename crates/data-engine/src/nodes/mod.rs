//! Node trait re-exports.
//!
//! All concrete node implementations now live in bundle crates under
//! `crates/node-bundles/`. This module provides re-export shims so that
//! existing `use crate::nodes::meta::*` / `super::meta::*` paths in
//! data-engine's own code keeps working.

pub mod meta {
    pub use dag_core::node::*;
}
pub mod numeric_util {
    pub use dag_core::arrow_util::*;
}

pub use meta::{DEFAULT_PORT, DagNode, NodeId, NodeInput, NodePorts, Port};
