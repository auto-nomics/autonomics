//! The Slurm task backend: a transport that submits `process` tasks as
//! sbatch jobs over ssh to an HPC login node, waits for terminal state via
//! `squeue`/`sacct`, and moves artifacts through the same ssh channel.
//!
//! Mirrors [`super::process::ProcessTaskTransport`]: the remote job sees the
//! identical `AUTONOMICS_*` environment contract, staged inputs, and
//! output-glob collection, but runs under Slurm admission instead of the
//! local resource budget. Wrap the transport and [`SshArtifactStore`] in the

mod artifact;
mod routing;
mod settings;
mod ssh;
mod transport;

pub use artifact::{SshArtifactStore, SshDataHome};
pub use routing::{ProcessRoutingTaskExecutor, slurm_backed_executor};
pub use settings::SlurmSettings;
pub use ssh::{RemoteStream, SshTarget, slurm_queue_absent};
pub(crate) use ssh::{ensure_ssh_success, hex_encode, sh_single_quote, ssh_stdout_text};
pub use transport::SlurmTaskTransport;

pub(crate) use transport::{
    attempt_key, heredoc_delimiter, match_uploaded_inputs, remote_task_dir, render_collect_script,
    resolve_remote_prefix,
};

#[path = "slurm_submission.rs"]
mod submission;
pub use submission::{
    SlurmRecordedSubmission, SlurmSubmissionIntent, SlurmSubmissionObservation,
    SlurmSubmissionRecorder,
};

#[cfg(all(test, unix))]
#[path = "data_home_tests.rs"]
mod data_home_tests;
