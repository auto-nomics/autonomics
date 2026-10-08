//! Executor-facing task contracts.
//!
//! The scheduler currently dispatches every task through
//! [`LocalTaskExecutor`]. The trait boundary keeps that path replaceable: a
//! future SLURM, Kubernetes, or batch executor can consume the same task
//! identity, process inputs, resources, and terminal result protocol.
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

mod contract;
mod local;
mod process;
mod remote;
mod resources;
mod traits;
mod workspace;

pub use contract::{
    TaskAttemptReceipt, TaskDispatch, TaskExecution, TaskInputBinding, TaskInputSource,
    TaskOutputBinding, TaskResources, TaskSpec, TaskSubmission, task_timeout,
};
pub use local::{LocalTaskExecutor, local_task_executor};
pub use process::{ProcessTaskOutput, ProcessTaskSpec, ProcessTaskTransport};
pub use remote::{LocalDirectoryArtifactStore, RemoteTaskExecutor};
pub use traits::{TaskArtifactStore, TaskExecutor, TaskLease, TaskTransport};
