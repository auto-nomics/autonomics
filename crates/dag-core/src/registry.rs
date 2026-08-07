//! Node factory registry: the bridge between typed JSON specs and live `DagNode` instances.

pub mod error;
pub mod registry;
pub mod spec_normalize;

pub use registry::{NodeCtx, NodeFactory, NodeInfo, NodeRegistry, new_isolated_ctx};
