//! Arguments for the headless run subcommand.

use std::path::PathBuf;

use clap::Args;
use clap::ValueEnum;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum RunBackend {
    InProcess,
    Gateway,
}

#[derive(Debug, Args)]
pub struct RunArgs {
    /// Task prompt. If omitted (or `-`), read from stdin; piped stdin
    /// alongside a positional prompt is appended as a `<stdin>` block.
    #[arg(value_name = "PROMPT")]
    pub prompt: Option<String>,

    /// List persisted sessions for the stable headless agent instead of
    /// running a prompt. Combine with --json for machine-readable output.
    #[arg(long)]
    pub list_sessions: bool,

    /// Print events to stdout as JSONL, one per line. Progress and
    /// warnings still go to stderr.
    #[arg(long)]
    pub json: bool,

    /// Write the agent's final message to FILE (empty content when the
    /// turn produced no message).
    #[arg(long, short = 'o', value_name = "FILE")]
    pub output_last_message: Option<PathBuf>,

    /// Spawn from this profile path (default: first stored profile).
    #[arg(long, value_name = "PATH")]
    pub profile: Option<String>,

    /// Model override as `provider_name:model_name` (default: the
    /// gateway daemon's active model).
    #[arg(long, value_name = "SPEC")]
    pub model: Option<String>,

    /// Wall-clock budget in seconds for the whole run. On expiry the run
    /// is cancelled (exit code 2).
    #[arg(long, value_name = "SECS")]
    pub timeout: Option<u64>,

    /// Resume this session (id comes from a previous run's
    /// `turn.started` event) instead of the agent's active one.
    #[arg(long, value_name = "UUID")]
    pub session: Option<uuid::Uuid>,

    /// Run in-process with all conversation state in a throwaway temp
    /// dir (benchmark isolation) instead of through the gateway daemon.
    /// Credentials still come from the app DB.
    #[arg(long)]
    pub ephemeral: bool,

    /// Ephemeral runtime implementation. Defaults to `in-process`;
    /// non-ephemeral runs continue through the resident gateway.
    #[arg(long, value_enum)]
    pub backend: Option<RunBackend>,

    /// Map an absolute host workspace directory to an absolute virtual
    /// path, for example `/path/to/work=/app`.
    #[arg(long, value_name = "SOURCE=VPATH")]
    pub workspace: Option<String>,

    /// Map an absolute read-only host data source to an absolute virtual
    /// path, for example `/path/to/data=/data`. May be repeated.
    #[arg(long, value_name = "SOURCE=VPATH")]
    pub data_mount: Vec<String>,

    /// Read workspace/data mount intent from a TOML file.
    #[arg(long, value_name = "FILE")]
    pub mount_manifest: Option<PathBuf>,

    /// Reuse an existing non-empty `--workspace`.
    #[arg(long)]
    pub resume_workspace: bool,

    /// Keep the ephemeral state root after the run for debugging.
    #[arg(long)]
    pub keep_state: bool,

    /// Write a JSON run manifest (prompt hash, model, usage, status) to
    /// this file after the run, whatever the outcome.
    #[arg(long, value_name = "FILE")]
    pub manifest: Option<PathBuf>,
}
