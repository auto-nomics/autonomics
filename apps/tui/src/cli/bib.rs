//! CLI definitions for the `autonomics-tui bib ...` subcommand tree:
//! bibliography management — upload full-text PDFs, list pending requests.

use std::path::PathBuf;

use clap::{Args, Subcommand};

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

#[derive(Debug, Args)]
pub struct ExportArgs {
    /// Output format: bibtex, ris, markdown.
    #[arg(long, default_value = "bibtex")]
    pub format: String,
    /// Collection ID to export. If omitted, exports entire library.
    #[arg(long)]
    pub collection_id: Option<String>,
    /// Write to file instead of stdout.
    #[arg(long)]
    pub output: Option<PathBuf>,
    /// Maximum articles to export (default 100).
    #[arg(long)]
    pub limit: Option<usize>,
}
