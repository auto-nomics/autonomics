//! Core DAG engine: node traits, factory registry, and graph scheduler,
//! free of any concrete node implementations.
//!
//! Concrete node bundles live in separate `nodes-*` crates and register
//! themselves via the [`plugin::NodePlugin`] trait.

pub mod arrow_util;
pub mod dag;
pub mod dataset;
pub mod error;
pub mod fingerprint;
pub mod node;
pub mod plugin;
pub mod registry;
pub mod resource;
pub mod runtime_output;
pub mod sink;
pub mod value;

pub use dag::{
    ApptainerContainer, ApptainerMode, ApptainerNetwork, ApptainerSettings, ChannelBranch,
    ChannelNode, ChannelOperator, DagTaskGcPolicy, DagTaskGcReport, DagTaskWatermarkReport,
    LocalDirectoryArtifactStore, LocalTaskExecutor, PlacementTaskExecutor,
    ProcessRoutingTaskExecutor, ProcessTaskOutput, ProcessTaskSpec, ProcessTaskTransport,
    RemoteTaskExecutor, SifImage, SlurmRecordedSubmission, SlurmSettings, SlurmSubmissionIntent,
    SlurmSubmissionObservation, SlurmSubmissionRecorder, SlurmTaskTransport, SshArtifactStore,
    SshDataHome, SshSettings, SshTarget, SshTaskTransport, TaskArtifactStore, TaskAttemptReceipt,
    TaskDispatch, TaskExecution,
    TaskExecutor, TaskInputBinding, TaskInputSource, TaskLease, TaskOutputBinding, TaskResources,
    TaskSpec, TaskSubmission, TaskTransport, TypedPodmanResourceControl, TypedPodmanSettings,
    TypedPodmanUserNamespace, TypedRemoteTaskExecutor, TypedSlurmSwapPolicy,
    TypedWorkerEnvironment, TypedWorkerSettings, dag_task_gc_age, dag_task_gc_interval,
    dag_task_gc_max_bytes, dag_tasks_root, local_task_executor, slurm_backed_executor,
    sweep_dag_tasks, sweep_dag_tasks_watermark,
};
pub use node::{
    BundleRegistry, BundleRegistryError, DEFAULT_PORT, DagNode, DataBundle, DataBundleBinding,
    NodeId, NodeInput, NodePorts, Port, PortId, ResolvedDataBundle,
};
pub use plugin::NodePlugin;
pub use registry::{NodeCtx, NodeFactory, NodeInfo, NodeRegistry, new_isolated_ctx};
// pub use sink::SinkMode;
pub use value::{ArtifactRef, ChannelValue, FileFingerprint, FileRef, NodeValue, PortType};

/// The source identity this crate was built from: the full git commit, recorded alongside run history for reproducibility evidence and
/// folded into every node execution fingerprint — so same-version builds
/// from different commits never silently reuse each other's incremental
/// cache entries.
///
/// Injected by `build.rs` with the precedence: the `AUTONOMICS_SOURCE_REVISION`
/// env override (release/CI stamping) > the workspace's checked-out commit.
/// A frozen tree without `.git` requires an explicit nonempty stamp; the
/// caller must verify that stamp against the frozen source manifest.
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
