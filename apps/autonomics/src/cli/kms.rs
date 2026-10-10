//! Arguments for the dedicated KMS tree TUI subcommand.

use std::path::PathBuf;

use clap::Args;

#[derive(Debug, Args)]
pub struct KmsArgs {
    /// KMS knowledge database to open. Defaults to the runtime knowledge
    /// database (`AUTONOMICS_KMS_DB` or `~/.autonomics/knowledge.db`).
    #[arg(long, value_name = "PATH")]
    pub db: Option<PathBuf>,
    /// Legacy agent database holding KMS tables from older versions; its
    /// rows are migrated into the knowledge database once. Defaults to the
    /// runtime agent database.
    #[arg(long, value_name = "PATH")]
    pub agent_db: Option<PathBuf>,
}
