//! Core DAG engine: node traits, factory registry, and graph scheduler,
//! free of any concrete node implementations.
//!
//! Concrete node bundles live in separate `nodes-*` crates and register
//! themselves via the [`plugin::NodePlugin`] trait.

pub mod arrow_util;
pub mod dag;
pub mod dataset;
pub mod error;
pub mod node;
pub mod plugin;
pub mod registry;
pub mod sink;
pub mod value;

pub use node::{
    BundleRegistry, BundleRegistryError, DEFAULT_PORT, DagNode, DataBundle, DataBundleBinding,
    NodeId, NodeInput, NodePorts, Port, PortId, ResolvedDataBundle,
};
pub use plugin::NodePlugin;
pub use registry::{NodeCtx, NodeFactory, NodeInfo, NodeRegistry, new_isolated_ctx};
// pub use sink::SinkMode;
pub use value::{ArtifactRef, DataRef, FileFingerprint, FileRef, NodeValue, PortType};
