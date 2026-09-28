//! `autonomics panels` — explicit provisioning of plugin panel data bundles.

use clap::Subcommand;

#[derive(Debug, clap::Args)]
pub struct PanelsArgs {
    #[command(subcommand)]
    pub action: Option<PanelsAction>,
}

#[derive(Debug, Subcommand)]
pub enum PanelsAction {
    /// Download and checksum-verify every panel bundle declared by the
    /// installed plugins (cached bundles are verified locally and skipped).
    Sync,
}
