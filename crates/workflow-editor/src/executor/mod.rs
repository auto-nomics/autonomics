//! Executor — topologically walks a [`WorkflowManifest`] and dispatches
//! each node to its registered factory.
//!
//! ## Modules
//!
//! - [`ports`] — runtime port types + the [`NodeExecutor`](ports::NodeExecutor) trait.
//! - [`sop_context`] — SOP prompt / tool-whitelist stack with RAII push/pop.
//! - [`scheduler`] — the main [`Scheduler`] that builds the petgraph DAG, validates it,
//!   and runs nodes in topological order.
//! - [`subgraph_node`] — `SubgraphNode` wraps a `Skill` and (in Phase 3) recurses.
//!
//! ## Phase 1 status
//!
//! `Scheduler::run` is fully wired for ordinary nodes (any
//! `NodeFactory::build_executor` implementation). `SubgraphNode::execute`
//! is a stub that returns `ExecutorError::Internal("...Phase 3...")` —
//! Phase 3 will implement the recursive descent + SOP push/pop.

pub mod ports;
pub mod scheduler;
pub mod sop_context;
pub mod subgraph_node;

pub use ports::{NodeCtx, NodeExecutor, NodeReporter, PortInputs, PortOutputs};
pub use scheduler::{Scheduler, ValidationReport, WorkflowResult};
pub use sop_context::{SopContext, SopGuard};
pub use subgraph_node::SubgraphNode;
