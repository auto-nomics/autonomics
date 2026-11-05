//! Process-grade workflow engine.
//!
//! This crate owns the *model* and *scheduling* of the stage/channel
//! workflow paradigm (Nextflow-style). It deliberately does **not**
//! reimplement execution: node construction goes through [`dag_core`'s
//! node registry](dag_core::NodeRegistry) and task dispatch goes through
//! the existing [`TaskExecutor`](dag_core::TaskExecutor) stack (local,
//! compute-router, Slurm), so the HPC path is inherited as-is.
//!
//! # Core invariant — two states, never mixed
//!
//! 1. **Authored state** is process-granular: a [`FlowGraph`] holds processes
//!    (one node per workflow process) and wires (routing between stages).
//!    It is the only editable, persisted, snapshotted representation.
//! 2. **Instance state** is run-scoped: tasks materialize from channel
//!    deliveries and never modify the authored graph. Eligible results can
//!    be cached for later runs.
//!
//! # Planning and routing
//!
//! The reactive scheduler consumes sources incrementally, creates tasks as
//! inputs arrive, and closes downstream channels after producers finish. The
//! older batch planner remains available for its existing callers.

pub mod cache;
pub mod channel;
pub mod dataflow;
pub mod error;
pub mod instance;
pub mod model;
pub mod plan;
mod reactive;
pub mod scheduler;
pub mod source;

pub use cache::{CachedInstance, CachedOutput, InstanceCache, plan_fingerprint};
pub use error::{Error, Result};
pub use instance::{
    INSTANCE_ID_SEPARATOR, Instance, InstanceId, UpstreamBinding, instance_fingerprint, item_key,
    render_spec,
};
pub use model::{
    ErrorMode, ErrorStrategy, FlowGraph, FlowGraphBuilder, PathPatterns, PortIndex, Process,
    ProcessName, Source, SourceSpec, Wire, WireOp,
};
pub use plan::{ExecutionPlan, InstanceRoute, ProcessPlan, SourceResolution, build_plan};
pub use scheduler::{
    ChannelBinaryFunction, ChannelFunction, FlowRunConfig, FlowRunReport, FlowScheduler,
    InstanceRecord, NodeBuilder, ProcessRunSummary,
};
pub use source::{
    SourceStreams, enumerate_glob, resolve_sources, stream_sources, stream_sources_with_functions,
};
