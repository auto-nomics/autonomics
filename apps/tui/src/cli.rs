//! CLI argument definitions for `autonomics-tui`.
//!
//! Kept separate from command implementations so subcommand behavior can be
//! changed without touching clap metadata.
//!
//! Layout mirrors `commands/`: each subcommand's clap types live in a sibling
//! module here (`cli/tui.rs`, `cli/bib.rs`, …), paired with its implementation
//! module (`commands/tui.rs`, `commands/bib.rs`, …). Adding a subcommand means
//! adding one module on each side.
//!
//! All argument types are re-exported at this module's root so importers can
//! keep using flat paths like `tui::cli::{Command, TuiArgs}`.

mod bib;
mod cache;
mod kms;
mod run;
mod tui;

use clap::{Parser, Subcommand};

pub use bib::{BibAction, BibArgs, ExportArgs, InfoArgs, ListArgs, RequestsArgs, UploadArgs};
pub use cache::{CacheAction, CacheArgs, ClearOpengwasArgs, RefreshOpengwasArgs};
pub use kms::KmsArgs;
pub use run::RunArgs;
pub use tui::TuiArgs;

#[derive(Debug, Parser)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Launch the interactive TUI (default when no subcommand is given).
    Tui(TuiArgs),

    /// Launch the dedicated KMS tree TUI.
    Kms(KmsArgs),

    /// Local cache management helpers (refresh, inspect, purge).
    Cache(CacheArgs),

    /// Bibliography management — upload full-text PDFs, list pending requests.
    Bib(BibArgs),

}

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

#[derive(Debug, Args)]
pub struct KmsArgs {
    /// Agent database containing the shared KMS tables. Defaults to the
    /// runtime agent database.
    #[arg(long, value_name = "PATH")]
    pub agent_db: Option<PathBuf>,
}

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

// ---------------------------------------------------------------------------
// bib subcommand
// ---------------------------------------------------------------------------

#[derive(Debug, Args)]
pub struct BibArgs {
    #[command(subcommand)]
    pub action: BibAction,

    /// Path to the bibliography database file.
    #[arg(long, global = true, value_name = "PATH", default_value = "bib.db")]
    pub db: PathBuf,
}

/// Subcommands under `autonomics-tui bib ...`.
#[derive(Debug, Subcommand)]
pub enum BibAction {
    /// Upload a PDF (or other document) as the full text for an article.
    /// Extracts plain text automatically and stores it in the library.
    Upload(UploadArgs),

    /// Re-run text extraction for stored full texts (single article or
    /// everything still pending/failed). Uses the MinerU cloud extractor
    /// with local fallback.
    Reextract(ReextractArgs),

    /// List articles that have been marked as needing full-text upload
    /// (fetch_status = fulltext_requested).
    Requests(RequestsArgs),

    /// Show the current full-text status for a specific article.
    Info(InfoArgs),

    /// List all articles in the library.
    List(ListArgs),

    /// Export articles in a citation format (BibTeX, RIS, Markdown).
    Export(ExportArgs),
}

#[derive(Debug, Args)]
pub struct UploadArgs {
    /// Path to the document file (PDF, HTML, or TXT).
    #[arg(long, value_name = "PATH")]
    pub pdf: PathBuf,

    /// Article ID in the local library (from bib_save or lit_search).
    #[arg(long)]
    pub article_id: String,

    /// Collection ID — if provided, updates the fetch_status to
    /// "fulltext_available" within this collection.
    #[arg(long)]
    pub collection_id: Option<String>,
}

#[derive(Debug, Args)]
pub struct ReextractArgs {
    /// Re-extract a single article's full text by its local ID.
    #[arg(long, value_name = "ID")]
    pub article_id: Option<String>,

    /// Re-extract every article whose extraction is pending or failed.
    #[arg(long)]
    pub all_missing: bool,
}

#[derive(Debug, Args)]
pub struct RequestsArgs {
    /// Filter to a specific collection.
    #[arg(long)]
    pub collection_id: Option<String>,
}

#[derive(Debug, Args)]
pub struct InfoArgs {
    /// Article ID to inspect.
    #[arg(long)]
    pub article_id: String,
}

#[derive(Debug, Args)]
pub struct ListArgs {
    /// Filter by keyword (searches title + abstract).
    #[arg(long)]
    pub query: Option<String>,
    /// Maximum results (default 50).
    #[arg(long)]
    pub limit: Option<usize>,
}


    /// Headless mode
    Run(RunArgs),
}
