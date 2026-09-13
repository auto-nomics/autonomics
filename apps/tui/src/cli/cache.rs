//! CLI definitions for the `autonomics-tui cache ...` subcommand tree.

use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct CacheArgs {
    #[command(subcommand)]
    pub action: CacheAction,
}

/// Subcommands under `autonomics-tui cache ...`.
#[derive(Debug, Subcommand)]
pub enum CacheAction {
    /// Re-fetch the OpenGWAS gwasinfo catalog from the remote API and
    /// persist the snapshot to the on-disk SQLite cache.
    ///
    /// Equivalent to `OpengwasClient::refresh_disk_cache`. On the next
    /// process restart the new data is picked up automatically without
    /// any further network call.
    RefreshOpengwas(RefreshOpengwasArgs),

    /// Delete the on-disk OpenGWAS gwasinfo cache file. The next query
    /// will trigger a fresh fetch from the remote API.
    ClearOpengwas(ClearOpengwasArgs),
}

#[derive(Debug, Args)]
pub struct RefreshOpengwasArgs {
    /// Print the resolved on-disk cache file path before refreshing.
    #[arg(long)]
    pub show_cache_path: bool,
}

#[derive(Debug, Args)]
pub struct ClearOpengwasArgs {
    /// Skip the interactive confirmation prompt.
    #[arg(long, short = 'y')]
    pub yes: bool,
}
