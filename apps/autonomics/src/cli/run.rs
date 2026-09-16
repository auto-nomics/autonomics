//! Arguments for the headless run subcommand.

use std::path::PathBuf;

use clap::Args;

#[derive(Debug, Args)]
pub struct RunArgs {
    /// Task prompt. If omitted (or `-`), read from stdin; piped stdin
    /// alongside a positional prompt is appended as a `<stdin>` block.
    #[arg(value_name = "PROMPT")]
    pub prompt: Option<String>,

    /// List persisted sessions for the --name agent instead of running a
    /// prompt. Combine with --json for machine-readable output.
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

    /// Create or restore the agent at /root/<NAME>. Concurrent live agents
    /// with the same name fall back to a unique suffix.
    #[arg(long, value_name = "NAME")]
    pub name: String,

    /// Per-agent runtime overrides as JSON, e.g.
    /// `--agent-config '{"use_memory":false,"generate_memory":false}'`.
    #[arg(long = "agent-config", value_name = "JSON")]
    pub agent_config: Option<String>,

    /// Disable memory injection, tools, and generation for this run.
    #[arg(long)]
    pub no_memory: bool,

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

    /// Write a JSON run manifest (prompt hash, model, usage, status) to
    /// this file after the run, whatever the outcome.
    #[arg(long, value_name = "FILE")]
    pub manifest: Option<PathBuf>,
}
