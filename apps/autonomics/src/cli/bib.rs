//! CLI definitions for the `autonomics bib ...` subcommand tree:
//! bibliography management — upload full-text PDFs, list pending requests.

use std::path::PathBuf;

use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct BibArgs {
    #[command(subcommand)]
    pub action: BibAction,

    /// Path to the bibliography database file. Defaults to the runtime
    /// bibliography database (`AUTONOMICS_BIB_DB` or `~/.autonomics/bib.db`)
    /// — the same path the daemon uses.
    #[arg(long, global = true, value_name = "PATH")]
    pub db: Option<PathBuf>,
}

/// Subcommands under `autonomics bib ...`.
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

    /// Article ID in the local library (from a `bib_save` DAG node or
    /// `source_literature_fetch`).
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
