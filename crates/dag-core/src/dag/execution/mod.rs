//! Executor-facing task contracts.
//!
//! The scheduler dispatches through the executor contract: the default
//! in-process [`LocalTaskExecutor`], the `process` transport, the
//! coordinator-side `remote` executor with its artifact store, and the
//! `slurm` HPC backend. The trait boundary keeps that set open: a future
//! Kubernetes or batch executor can consume the same task identity,
//! process inputs, resources, and terminal result protocol.
//!
//! The implementation is split by concern; every public item is re-exported
//! here so existing `dag::execution::…` paths stay stable:
//!
//! - `contract` — task identity, resources, submissions, dispatches, receipts
//! - `traits` — the executor / transport / artifact-store contracts
//! - `resources` — the local CPU/memory admission budget
//! - `workspace` — the per-attempt on-disk task workspace
//! - `local` — the default in-process executor
//! - `process` — the OS-process transport behind `process` tasks
//! - `remote` — coordinator-side remote executor + artifact store
//! - `slurm` — the SLURM batch backend behind the same transport protocol
//! - `apptainer` — SIF image and container settings for Slurm container runs
//! - `gc` — retention sweep for the persistent dag-tasks receipt tree

mod apptainer;
mod channel_value;
mod contract;
mod gc;
pub mod heartbeat;
mod limited;
mod local;
mod placement;
pub mod private_native;
mod process;
mod remote;
mod resources;
mod retained_arrow;
mod slurm;
mod ssh_transport;
mod traits;
mod typed_publication;
pub mod typed_worker;
mod workspace;

pub use apptainer::{
    ApptainerContainer, ApptainerMode, ApptainerNetwork, ApptainerSettings, SifImage,
};
pub use channel_value::ChannelTransportValue;
pub use contract::{
    TaskAttemptReceipt, TaskDispatch, TaskExecution, TaskInputBinding, TaskInputSource,
    TaskOutputBinding, TaskResources, TaskSpec, TaskSubmission, task_timeout,
};
pub use gc::{
    DagTaskGcPolicy, DagTaskGcReport, DagTaskWatermarkReport, dag_task_gc_age,
    dag_task_gc_interval, dag_task_gc_max_bytes, dag_tasks_root, sweep_dag_tasks,
    sweep_dag_tasks_watermark,
};
pub use limited::{LimitedTaskExecutor, MAX_REMOTE_TASKS};
pub use local::{LocalTaskExecutor, local_task_executor};
pub use process::{ProcessTaskOutput, ProcessTaskSpec, ProcessTaskTransport};
pub use remote::{LocalDirectoryArtifactStore, RemoteTaskExecutor};
pub use slurm::{
    ProcessRoutingTaskExecutor, SlurmRecordedSubmission, SlurmSettings, SlurmSubmissionIntent,
    SlurmSubmissionObservation, SlurmSubmissionRecorder, SlurmTaskTransport, SshArtifactStore,
    SshDataHome, SshTarget, slurm_backed_executor, slurm_queue_absent,
};
pub use ssh_transport::{SshSettings, SshTaskTransport};
pub use traits::{TaskArtifactStore, TaskExecutor, TaskLease, TaskTransport};
pub use typed_worker::{
    PlacementTaskExecutor, TypedPodmanResourceControl, TypedPodmanSettings,
    TypedPodmanUserNamespace, TypedRemoteTaskExecutor, TypedSlurmSwapPolicy,
    TypedWorkerEnvironment, TypedWorkerSettings,
};

/// Provenance-bound remote Arrow identity; never authorizes local row reads.
pub use retained_arrow::pure_retained_arrow_origin;
