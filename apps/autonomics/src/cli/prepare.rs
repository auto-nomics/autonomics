//! `autonomics prepare` — provision everything the daemon needs.

use clap::Args;

#[derive(Debug, Args)]
pub struct PrepareArgs {
    /// Diagnostics only: verify plugins, the embedding model, images,
    /// and panel bundles against local state without touching the
    /// network. Exits 1 when a fatal category (plugins, model, images)
    /// has a gap.
    #[arg(long)]
    pub check: bool,
}
