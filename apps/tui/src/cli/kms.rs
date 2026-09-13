//! Arguments for the dedicated KMS tree TUI subcommand.

use std::path::PathBuf;

use clap::Args;

#[derive(Debug, Args)]
pub struct KmsArgs {
    /// Agent database containing the shared KMS tables. Defaults to the
    /// runtime agent database.
    #[arg(long, value_name = "PATH")]
    pub agent_db: Option<PathBuf>,
}
