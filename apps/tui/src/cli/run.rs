//! Arguments for the headless run subcommand.

use std::path::PathBuf;

use clap::Args;

#[derive(Debug, Args)]
pub struct RunArgs {
    /// Task prompt. If omitted (or `-`), read from stdin; piped stdin
    /// alongside a positional prompt is appended as a `<stdin>` block.
    #[arg(value_name = "PROMPT")]
    pub prompt: Option<String>,

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

    /// Write a JSON run manifest (prompt hash, model, usage, status) to
    /// this file after the run, whatever the outcome.
    #[arg(long, value_name = "FILE")]
    pub manifest: Option<PathBuf>,
}
