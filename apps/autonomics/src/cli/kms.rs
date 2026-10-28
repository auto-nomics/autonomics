//! Arguments for the dedicated KMS tree TUI subcommand.

use std::path::PathBuf;

use clap::Args;

#[derive(Debug, Args)]
pub struct KmsArgs {
    /// KMS knowledge database to open. Defaults to the runtime knowledge
    /// database (`AUTONOMICS_KMS_DB` or `~/.autonomics/knowledge.db`).
    #[arg(long, value_name = "PATH")]
    pub db: Option<PathBuf>,
}
