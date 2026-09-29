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
pub mod resource;
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

/// The git commit (short form) this crate was built from, recorded alongside
/// run history for reproducibility evidence.
///
/// Injected by `build.rs` from the workspace repository; `"unknown"` when the
/// build tree has no `.git` (e.g. a release tarball).
pub fn source_revision() -> &'static str {
    option_env!("AUTONOMICS_SOURCE_REVISION").unwrap_or("unknown")
}

/// The engine version recorded in history records (this crate's version).
///
/// Exposed so downstream crates (data-engine) stamp snapshots and run records
/// with the same version string the history store itself uses.
pub fn engine_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
