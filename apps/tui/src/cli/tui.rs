//! Arguments for the interactive TUI subcommand.

use std::path::PathBuf;

use clap::Args;

#[derive(Debug, Args)]
pub struct TuiArgs {
    /// Optional path to a TUI configuration file.
    #[arg(long, short, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// Disable output capture: tracing logs and panics are written to stderr
    /// in addition to the log file. Use this for debugging startup failures.
    #[arg(long)]
    pub nocapture: bool,
}
