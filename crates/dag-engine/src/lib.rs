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
//! 2. **Instance state** is run-scoped: item instances (`process#key`)
//!    materialize at run start (or on data arrival, later) and die with
//!    the run. Only their fingerprints/outputs survive, in a per-stage
//!    instance result cache keyed by stable `stage#key` ids — enabling
//!    incremental reuse without polluting the authored graph.
//!
//! # Cardinality and routing
//!
//! - A stage is either `Singleton` (one instance) or a `Fanout` over a
//!   [`Source`] resolved at run start (glob / explicit items; channel
//!   streams later).
//! - `Direct` wires: fanout→fanout zips by item key; singleton→any
//!   broadcasts to every target instance.
//! - `Collect` wires: fanout→singleton collapses all instances into the
//!   target's single input port (Nextflow's `collect` as an edge op).
//!
//! Incremental identity: `fingerprint(process definition revision, rendered
//! spec, item, upstream instance fingerprints)`. A spec edit invalidates
//! every instance of that process; an item-set change affects only the
//! added/removed keys.

pub mod error;
pub mod instance;
pub mod model;
pub mod plan;
pub mod scheduler;
pub mod source;

pub use error::{Error, Result};
pub use instance::{
    Instance, InstanceId, UpstreamBinding, INSTANCE_ID_SEPARATOR, instance_fingerprint, item_key,
    render_spec,
};
pub use model::{
    Cardinality, FlowGraph, FlowGraphBuilder, PortIndex, Process, ProcessName, Source, Wire,
    WireOp,
};
pub use plan::{
    ExecutionPlan, InstanceRoute, ProcessPlan, SourceResolution, build_plan,
};
pub use scheduler::{
    FlowRunConfig, FlowRunReport, FlowScheduler, InstanceRecord, NodeBuilder, ProcessRunSummary,
};
pub use source::{enumerate_glob, resolve_sources};
