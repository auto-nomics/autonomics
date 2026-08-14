//! CLI argument definitions for `autonomics-tui`.
//!
//! Kept separate from command implementations so subcommand behavior can be
//! changed without touching clap metadata.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Launch the interactive TUI (default when no subcommand is given).
    Tui(TuiArgs),

    /// Local cache management helpers (refresh, inspect, purge).
    Cache(CacheArgs),

    /// Bibliography management — upload full-text PDFs, list pending requests.
    Bib(BibArgs),

    /// Resource catalog management — browse, inspect, register, ingest,
    /// archive/restore/verify, drift check, and export. Covers all resource
    /// types (Iceberg tables, file paths, API endpoints, databases, docs,
    /// config).
    Resource(Box<ResourceArgs>),
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

/// Parse `key=value` into a tuple for `--metadata`.
fn parse_key_value(s: &str) -> color_eyre::Result<(String, String)> {
    s.split_once('=')
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .ok_or_else(|| color_eyre::eyre::eyre!("expected key=value, got '{s}'"))
}

// ---------------------------------------------------------------------------
// resource subcommand
// ---------------------------------------------------------------------------

#[derive(Debug, Args)]
pub struct ResourceArgs {
    #[command(subcommand)]
    pub action: ResourceAction,
}

/// Subcommands under `autonomics-tui resource ...`.
#[derive(Debug, Subcommand)]
pub enum ResourceAction {
    /// List all registered resources in a table. Supports filtering by kind,
    /// tag, or substring match on the name.
    List(ResourceListArgs),

    /// Show full details for a single resource (address, metadata, tags,
    /// archive status, ingestion spec).
    Show(ResourceShowArgs),

    /// Update mutable fields on an existing resource (currently:
    /// description). Other fields (`address`, `metadata`, `tags`,
    /// `archive_status`, `ingestion_spec`) are left untouched. This
    /// avoids the remove-and-re-add round-trip that would clobber
    /// runtime state.
    Update(ResourceUpdateArgs),

    /// Register a new resource entry in the catalog. The `--kind` determines
    /// which address fields are required. For `iceberg_table`, optional
    /// `--source` registers an ingestion spec; `--archive` additionally
    /// pushes source to cloud and ingests into Iceberg in one step.
    Add(Box<ResourceAddArgs>),

    /// Remove a resource from the catalog manifest (does NOT drop Iceberg
    /// tables or delete files — only unregisters the catalog entry).
    Remove(ResourceRemoveArgs),

    /// Ingest source files into Iceberg tables. Operates on resources with
    /// an ingestion_spec, or a single resource via `--resource`.
    Ingest(ResourceTargetArgs),

    /// Push a resource's content to its configured cloud archive (rclone).
    Archive(ResourceTargetArgs),

    /// Pull a resource's content from its configured cloud archive (rclone).
    Restore(ResourceTargetArgs),

    /// Verify that local content matches the cloud archive (rclone check).
    Verify(ResourceTargetArgs),

    /// Check all registered resources against live state (Iceberg catalog +
    /// filesystem) and report mismatches. Never mutates the catalog.
    Drift(ResourceDriftArgs),

    /// Export the full catalog manifest as JSON to stdout or a file.
    Export(ResourceExportArgs),
}

/// Arguments for `resource list`.
#[derive(Debug, Args)]
pub struct ResourceListArgs {
    /// Filter by resource kind: iceberg_table, file_path, endpoint, config,
    /// database, doc.
    #[arg(long)]
    pub kind: Option<String>,

    /// Filter by tag (resources matching ANY of the given tags are shown).
    #[arg(long)]
    pub tag: Vec<String>,

    /// Substring filter on the resource name (case-insensitive).
    #[arg(long)]
    pub name: Option<String>,

    /// Show only resources that have an archive_spec.
    #[arg(long)]
    pub archivable: bool,

    /// Show only resources that have an ingestion_spec.
    #[arg(long)]
    pub ingestible: bool,

    /// Output format: table (default) or json.
    #[arg(long, default_value = "table")]
    pub format: String,
}

/// Arguments for `resource show`.
#[derive(Debug, Args)]
pub struct ResourceShowArgs {
    /// Logical resource name.
    pub name: String,
}

/// Arguments for `resource update`.
#[derive(Debug, Args)]
pub struct ResourceUpdateArgs {
    /// Logical resource name.
    pub name: String,

    /// New description text. Replaces the existing description in full.
    /// Pass an empty string ("") to clear it.
    #[arg(long)]
    pub description: Option<String>,
}

/// Arguments for `resource add`.
#[derive(Debug, Args)]
pub struct ResourceAddArgs {
    /// Logical resource name (stable identifier, e.g. "ldscore.1000g_eur").
    #[arg(long)]
    pub name: String,

    /// Human-readable description.
    #[arg(long)]
    pub description: String,

    /// Resource kind: iceberg_table, file_path, endpoint, config, database, doc.
    #[arg(long)]
    pub kind: String,

    // ── IcebergTable address fields ───────────────────────────────────
    /// Iceberg schema name (for kind=iceberg_table).
    #[arg(long)]
    pub schema: Option<String>,

    /// Iceberg table name (for kind=iceberg_table).
    #[arg(long)]
    pub table: Option<String>,

    // ── FilePath / Database / Doc address fields ──────────────────────
    /// Filesystem path (for kind=file_path, database, doc).
    #[arg(long)]
    pub path: Option<String>,

    // ── Endpoint address fields ───────────────────────────────────────
    /// API base URL (for kind=endpoint).
    #[arg(long)]
    pub url: Option<String>,

    // ── Config address fields ─────────────────────────────────────────
    /// Config key (for kind=config).
    #[arg(long)]
    pub key: Option<String>,

    /// Config value (for kind=config).
    #[arg(long)]
    pub value: Option<String>,

    // ── Database-specific ─────────────────────────────────────────────
    /// Database backend: sqlite, turso, postgres (for kind=database).
    #[arg(long)]
    pub db_kind: Option<String>,

    // ── Doc-specific ──────────────────────────────────────────────────
    /// Doc category: docs, logs, archive, notes, fixtures (for kind=doc).
    #[arg(long)]
    pub doc_kind: Option<String>,

    // ── ObjectStorage address fields ──────────────────────────────────
    /// Object-store bucket name (for kind=object_storage).
    #[arg(long)]
    pub bucket: Option<String>,

    /// Object-store prefix, must start with '/' (for kind=object_storage).
    #[arg(long)]
    pub prefix: Option<String>,

    /// Backend type for object_storage: oss, s3, or local
    /// (default: oss when --endpoint/--access-key provided, else local).
    /// Determines which connection fields apply.
    #[arg(long, value_parser = ["oss", "s3", "local"])]
    pub backend: Option<String>,

    /// Cloud backend endpoint URL
    /// (e.g. "https://oss-cn-hangzhou.aliyuncs.com"). Defaults: aliyun OSS for
    /// --backend=oss; AWS S3 default region for --backend=s3. Ignored for local.
    #[arg(long)]
    pub endpoint: Option<String>,

    /// Cloud backend region (e.g. "us-east-1" for S3, "oss-cn-hangzhou" for OSS).
    /// Ignored for local backend.
    #[arg(long)]
    pub region: Option<String>,

    /// Explicit access-key id for OSS / S3. Omit to defer to opendal's
    /// default credential chain (env vars / IAM role).
    #[arg(long)]
    pub access_key: Option<String>,

    /// Explicit secret access-key for OSS / S3. Omit to defer to opendal's
    /// default credential chain. Should accompany --access-key; if only one is
    /// set the entry will fail loudly at first read.
    #[arg(long)]
    pub secret: Option<String>,

    /// Local filesystem root for backend=local (ignored otherwise).
    /// The bucket's URL resolves under this directory.
    #[arg(long)]
    pub local_root: Option<String>,

    // ── Ingestion spec fields (iceberg_table only) ────────────────────
    /// Source file path or glob for ingestion (e.g. "/data/gwas/*.parquet").
    /// When set, an ingestion_spec is attached to the resource.
    #[arg(long)]
    pub source: Option<String>,

    /// Source format: parquet, csv, or tsv (default: parquet).
    #[arg(long)]
    pub format: Option<String>,

    /// Partition columns (repeatable, e.g. --partition chrom).
    #[arg(long)]
    pub partition: Vec<String>,

    /// Write mode: create_if_not_exists, create_or_replace, or append
    /// (default: create_if_not_exists).
    #[arg(long)]
    pub mode: Option<String>,

    /// CSV delimiter character (default: ','). For tsv, '\t' is used.
    #[arg(long)]
    pub delimiter: Option<char>,

    /// CSV has no header row.
    #[arg(long)]
    pub no_header: bool,

    // ── Compound: --archive does register + rclone push + Iceberg ingest ─
    /// Archive source to cloud AND ingest into Iceberg in one step.
    /// Requires ARCHIVE_REMOTE and ARCHIVE_BUCKET env vars.
    /// Only meaningful with --source for kind=iceberg_table.
    #[arg(long)]
    pub archive: bool,

    // ── Common optional fields ────────────────────────────────────────
    /// Metadata key=value pair (repeatable).
    #[arg(long, value_parser = parse_key_value)]
    pub metadata: Vec<(String, String)>,

    /// Tag (repeatable).
    #[arg(long)]
    pub tag: Vec<String>,
}

/// Arguments for `resource remove`.
#[derive(Debug, Args)]
pub struct ResourceRemoveArgs {
    /// Logical resource name to remove.
    pub name: String,

    /// Skip confirmation prompt.
    #[arg(long, short = 'y')]
    pub yes: bool,
}

/// Arguments for archive/restore/verify (single resource or all archivable).
#[derive(Debug, Args)]
pub struct ResourceTargetArgs {
    /// Logical resource name. If omitted, operates on ALL archivable resources.
    #[arg(long, short = 'r')]
    pub resource: Option<String>,
}

/// Arguments for `resource drift`.
#[derive(Debug, Args)]
pub struct ResourceDriftArgs {
    /// Also check Iceberg tables (requires the datalake to be reachable).
    #[arg(long)]
    pub check_iceberg: bool,
}

/// Arguments for `resource export`.
#[derive(Debug, Args)]
pub struct ResourceExportArgs {
    /// Write to file instead of stdout.
    #[arg(long, short = 'o')]
    pub output: Option<PathBuf>,
}
