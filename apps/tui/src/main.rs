use std::path::PathBuf;
use std::sync::Arc;

use clap::{Args, Parser, Subcommand};
use time::macros::format_description;
use tracing::Level;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::fmt::time::OffsetTime;
use tracing_subscriber::fmt::writer::MakeWriterExt;

use opengwas::OpengwasClient;
use tui::app::App;

fn init_logging(nocapture: bool) -> color_eyre::Result<()> {
    color_eyre::install()?;

    let log_dir = PathBuf::from("logs");
    std::fs::create_dir_all(&log_dir)?;

    let file_appender = RollingFileAppender::new(Rotation::DAILY, &log_dir, "autonomics-tui.log");
    let file_writer = file_appender.with_max_level(Level::DEBUG);

    let timer = OffsetTime::new(
        time::UtcOffset::current_local_offset().expect("timezone"),
        format_description!("[year]-[month]-[day] [hour]:[minute]:[second].[subsecond digits:3]"),
    );

    // The TUI's `set_panic_hook` (app.rs) wraps this hook and handles
    // terminal restoration + readable stderr output. Here we only need
    // to log the full panic + backtrace to the log file. The readable
    // message to stderr is handled by `set_panic_hook`.
    std::panic::set_hook(Box::new(|info| {
        let bt = std::backtrace::Backtrace::force_capture();
        tracing::error!(
            target: "panic",
            payload = %info,
            backtrace = %bt,
            "thread panicked"
        );
    }));

    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("tui=debug,agentik_core=debug,agentik_sdk=debug,runtime=debug")
    });

    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_ansi(nocapture) // ANSI colors only when writing to a terminal
        .with_target(false)
        .with_file(true)
        .with_line_number(true)
        .with_span_events(FmtSpan::NONE)
        .with_timer(timer);

    if nocapture {
        // Write to BOTH the log file and stderr so all output is visible
        // in the terminal during debugging.
        subscriber
            .with_writer(file_writer.and(std::io::stderr))
            .init();
    } else {
        subscriber.with_writer(file_writer).init();
    }

    tracing::info!(
        "logging initialized — logs directory: {} (nocapture: {nocapture})",
        log_dir.display()
    );
    Ok(())
}

/// Top-level CLI for the autonomics-tui binary.
///
/// The default behaviour (no subcommand given) is identical to running
/// `autonomics-tui tui` — launching the interactive TUI.
#[derive(Debug, Parser)]
#[command(
    name = "autonomics-tui",
    about = "Autonomics interactive terminal UI",
    version,
    propagate_version = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
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
    Resource(ResourceArgs),
}

#[derive(Debug, Args)]
struct TuiArgs {
    /// Optional path to a TUI configuration file.
    #[arg(long, short, value_name = "PATH")]
    config: Option<PathBuf>,

    /// Disable output capture: tracing logs and panics are written to stderr
    /// in addition to the log file. Use this for debugging startup failures.
    #[arg(long)]
    nocapture: bool,
}

#[derive(Debug, Args)]
struct CacheArgs {
    #[command(subcommand)]
    action: CacheAction,
}

/// Subcommands under `autonomics-tui cache ...`.
#[derive(Debug, Subcommand)]
enum CacheAction {
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
struct RefreshOpengwasArgs {
    /// Print the resolved on-disk cache file path before refreshing.
    #[arg(long)]
    show_cache_path: bool,
}

#[derive(Debug, Args)]
struct ClearOpengwasArgs {
    /// Skip the interactive confirmation prompt.
    #[arg(long, short = 'y')]
    yes: bool,
}

// ---------------------------------------------------------------------------
// bib subcommand
// ---------------------------------------------------------------------------

#[derive(Debug, Args)]
struct BibArgs {
    #[command(subcommand)]
    action: BibAction,

    /// Path to the bibliography database file.
    #[arg(long, global = true, value_name = "PATH", default_value = "bib.db")]
    db: PathBuf,
}

/// Subcommands under `autonomics-tui bib ...`.
#[derive(Debug, Subcommand)]
enum BibAction {
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
struct UploadArgs {
    /// Path to the document file (PDF, HTML, or TXT).
    #[arg(long, value_name = "PATH")]
    pdf: PathBuf,

    /// Article ID in the local library (from bib_save or lit_search).
    #[arg(long)]
    article_id: String,

    /// Collection ID — if provided, updates the fetch_status to
    /// "fulltext_available" within this collection.
    #[arg(long)]
    collection_id: Option<String>,
}

#[derive(Debug, Args)]
struct RequestsArgs {
    /// Filter to a specific collection.
    #[arg(long)]
    collection_id: Option<String>,
}

#[derive(Debug, Args)]
struct InfoArgs {
    /// Article ID to inspect.
    #[arg(long)]
    article_id: String,
}

#[derive(Debug, Args)]
struct ListArgs {
    /// Filter by keyword (searches title + abstract).
    #[arg(long)]
    query: Option<String>,
    /// Maximum results (default 50).
    #[arg(long)]
    limit: Option<usize>,
}

#[derive(Debug, Args)]
struct ExportArgs {
    /// Output format: bibtex, ris, markdown.
    #[arg(long, default_value = "bibtex")]
    format: String,
    /// Collection ID to export. If omitted, exports entire library.
    #[arg(long)]
    collection_id: Option<String>,
    /// Write to file instead of stdout.
    #[arg(long)]
    output: Option<PathBuf>,
    /// Maximum articles to export (default 100).
    #[arg(long)]
    limit: Option<usize>,
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
struct ResourceArgs {
    #[command(subcommand)]
    action: ResourceAction,
}

/// Subcommands under `autonomics-tui resource ...`.
#[derive(Debug, Subcommand)]
enum ResourceAction {
    /// List all registered resources in a table. Supports filtering by kind,
    /// tag, or substring match on the name.
    List(ResourceListArgs),

    /// Show full details for a single resource (address, metadata, tags,
    /// archive status, ingestion spec).
    Show(ResourceShowArgs),

    /// Register a new resource entry in the catalog. The `--kind` determines
    /// which address fields are required. For `iceberg_table`, optional
    /// `--source` registers an ingestion spec; `--archive` additionally
    /// pushes source to cloud and ingests into Iceberg in one step.
    Add(ResourceAddArgs),

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
struct ResourceListArgs {
    /// Filter by resource kind: iceberg_table, file_path, endpoint, config,
    /// database, doc.
    #[arg(long)]
    kind: Option<String>,

    /// Filter by tag (resources matching ANY of the given tags are shown).
    #[arg(long)]
    tag: Vec<String>,

    /// Substring filter on the resource name (case-insensitive).
    #[arg(long)]
    name: Option<String>,

    /// Show only resources that have an archive_spec.
    #[arg(long)]
    archivable: bool,

    /// Show only resources that have an ingestion_spec.
    #[arg(long)]
    ingestible: bool,

    /// Output format: table (default) or json.
    #[arg(long, default_value = "table")]
    format: String,
}

/// Arguments for `resource show`.
#[derive(Debug, Args)]
struct ResourceShowArgs {
    /// Logical resource name.
    name: String,
}

/// Arguments for `resource add`.
#[derive(Debug, Args)]
struct ResourceAddArgs {
    /// Logical resource name (stable identifier, e.g. "ldscore.1000g_eur").
    #[arg(long)]
    name: String,

    /// Human-readable description.
    #[arg(long)]
    description: String,

    /// Resource kind: iceberg_table, file_path, endpoint, config, database, doc.
    #[arg(long)]
    kind: String,

    // ── IcebergTable address fields ───────────────────────────────────
    /// Iceberg schema name (for kind=iceberg_table).
    #[arg(long)]
    schema: Option<String>,

    /// Iceberg table name (for kind=iceberg_table).
    #[arg(long)]
    table: Option<String>,

    // ── FilePath / Database / Doc address fields ──────────────────────
    /// Filesystem path (for kind=file_path, database, doc).
    #[arg(long)]
    path: Option<String>,

    // ── Endpoint address fields ───────────────────────────────────────
    /// API base URL (for kind=endpoint).
    #[arg(long)]
    url: Option<String>,

    // ── Config address fields ─────────────────────────────────────────
    /// Config key (for kind=config).
    #[arg(long)]
    key: Option<String>,

    /// Config value (for kind=config).
    #[arg(long)]
    value: Option<String>,

    // ── Database-specific ─────────────────────────────────────────────
    /// Database backend: sqlite, turso, postgres (for kind=database).
    #[arg(long)]
    db_kind: Option<String>,

    // ── Doc-specific ──────────────────────────────────────────────────
    /// Doc category: docs, logs, archive, notes, fixtures (for kind=doc).
    #[arg(long)]
    doc_kind: Option<String>,

    // ── Ingestion spec fields (iceberg_table only) ────────────────────
    /// Source file path or glob for ingestion (e.g. "/data/gwas/*.parquet").
    /// When set, an ingestion_spec is attached to the resource.
    #[arg(long)]
    source: Option<String>,

    /// Source format: parquet, csv, or tsv (default: parquet).
    #[arg(long)]
    format: Option<String>,

    /// Partition columns (repeatable, e.g. --partition chrom).
    #[arg(long)]
    partition: Vec<String>,

    /// Write mode: create_if_not_exists, create_or_replace, or append
    /// (default: create_if_not_exists).
    #[arg(long)]
    mode: Option<String>,

    /// CSV delimiter character (default: ','). For tsv, '\t' is used.
    #[arg(long)]
    delimiter: Option<char>,

    /// CSV has no header row.
    #[arg(long)]
    no_header: bool,

    // ── Compound: --archive does register + rclone push + Iceberg ingest ─
    /// Archive source to cloud AND ingest into Iceberg in one step.
    /// Requires ARCHIVE_REMOTE and ARCHIVE_BUCKET env vars.
    /// Only meaningful with --source for kind=iceberg_table.
    #[arg(long)]
    archive: bool,

    // ── Common optional fields ────────────────────────────────────────
    /// Metadata key=value pair (repeatable).
    #[arg(long, value_parser = parse_key_value)]
    metadata: Vec<(String, String)>,

    /// Tag (repeatable).
    #[arg(long)]
    tag: Vec<String>,
}

/// Arguments for `resource remove`.
#[derive(Debug, Args)]
struct ResourceRemoveArgs {
    /// Logical resource name to remove.
    name: String,

    /// Skip confirmation prompt.
    #[arg(long, short = 'y')]
    yes: bool,
}

/// Arguments for archive/restore/verify (single resource or all archivable).
#[derive(Debug, Args)]
struct ResourceTargetArgs {
    /// Logical resource name. If omitted, operates on ALL archivable resources.
    #[arg(long, short = 'r')]
    resource: Option<String>,
}

/// Arguments for `resource drift`.
#[derive(Debug, Args)]
struct ResourceDriftArgs {
    /// Also check Iceberg tables (requires the datalake to be reachable).
    #[arg(long)]
    check_iceberg: bool,
}

/// Arguments for `resource export`.
#[derive(Debug, Args)]
struct ResourceExportArgs {
    /// Write to file instead of stdout.
    #[arg(long, short = 'o')]
    output: Option<PathBuf>,
}

// ---------------------------------------------------------------------------
// resource subcommand implementation
// ---------------------------------------------------------------------------

/// Open the resource catalog from the default RuntimeConfig location.
/// Uses `load_or_error` so that a locked or corrupt manifest DB produces
/// a clear error instead of silently returning an empty catalog.
async fn open_catalog() -> color_eyre::Result<Arc<dag_core::resource_catalog::ResourceCatalog>> {
    use dag_core::resource_catalog::ResourceCatalog;
    use runtime::config::RuntimeConfig;

    let config = RuntimeConfig::default();
    let manifest_db = config.state_dir.join("resource-manifest.db");
    if let Some(parent) = manifest_db.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let catalog = Arc::new(
        ResourceCatalog::load_or_error(&config.data_dir, &manifest_db)
            .await
            .map_err(|e| {
                let msg = e.to_string();
                // Detect the common "database is locked" case and give
                // actionable advice.
                if msg.contains("locked") || msg.contains("busy") {
                    color_eyre::eyre::eyre!(
                        "resource-manifest.db is locked — another TUI process is likely running.\n\
                         \n\
                         Options:\n\
                         1. Close the other TUI instance and retry.\n\
                         2. DB path: {}\n\
                         \n\
                         Underlying error: {msg}",
                        manifest_db.display()
                    )
                } else {
                    color_eyre::eyre::eyre!(
                        "failed to open resource catalog at {}: {msg}",
                        manifest_db.display()
                    )
                }
            })?,
    );
    let _ = ResourceCatalog::set_global(catalog.clone());
    Ok(catalog)
}

fn run_resource(args: ResourceArgs) -> color_eyre::Result<()> {
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| color_eyre::eyre::eyre!("failed to build tokio runtime: {e}"))?;
    runtime.block_on(async { run_resource_async(args).await })
}

async fn run_resource_async(args: ResourceArgs) -> color_eyre::Result<()> {
    let catalog = open_catalog().await?;

    match args.action {
        ResourceAction::List(a) => resource_list(&catalog, a).await,
        ResourceAction::Show(a) => resource_show(&catalog, a),
        ResourceAction::Add(a) => resource_add(&catalog, a).await,
        ResourceAction::Remove(a) => resource_remove(&catalog, a).await,
        ResourceAction::Ingest(a) => resource_ingest(&catalog, a).await,
        ResourceAction::Archive(a) => resource_archive(&catalog, a).await,
        ResourceAction::Restore(a) => resource_restore(&catalog, a).await,
        ResourceAction::Verify(a) => resource_verify(&catalog, a).await,
        ResourceAction::Drift(a) => resource_drift(&catalog, a).await,
        ResourceAction::Export(a) => resource_export(&catalog, a),
    }
}

async fn resource_list(
    catalog: &Arc<dag_core::resource_catalog::ResourceCatalog>,
    args: ResourceListArgs,
) -> color_eyre::Result<()> {
    let mut entries = catalog.list();

    // ── Filters ─────────────────────────────────────────────────────
    if let Some(ref kind) = args.kind {
        entries.retain(|e| e.kind.as_str() == kind.as_str());
    }
    if !args.tag.is_empty() {
        entries.retain(|e| args.tag.iter().any(|t| e.tags.contains(t)));
    }
    if let Some(ref name) = args.name {
        let lower = name.to_lowercase();
        entries.retain(|e| e.name.to_lowercase().contains(&lower));
    }
    if args.archivable {
        entries.retain(|e| e.archive_spec.is_some());
    }
    if args.ingestible {
        entries.retain(|e| e.ingestion_spec.is_some());
    }

    if entries.is_empty() {
        println!("No resources match the given filters.");
        return Ok(());
    }

    // Sort by kind then name for readability.
    entries.sort_by(|a, b| {
        a.kind
            .as_str()
            .cmp(b.kind.as_str())
            .then_with(|| a.name.cmp(&b.name))
    });

    match args.format.as_str() {
        "json" => {
            let json = serde_json::to_string_pretty(&entries)?;
            println!("{json}");
        }
        _ => {
            // Table output.
            println!("{} resources:\n", entries.len());
            println!(
                "  {:<14} {:<30} {:<10} {:<8} {:<8} {}",
                "KIND", "NAME", "ARCHIVE", "INGEST", "", "DESCRIPTION"
            );
            println!("  {:-<120}", "");
            for e in &entries {
                let desc = if e.description.len() > 50 {
                    format!("{}…", &e.description[..47])
                } else {
                    e.description.clone()
                };
                let has_archive = if e.archive_spec.is_some() { "✓" } else { "" };
                let has_ingest = if e.ingestion_spec.is_some() {
                    "✓"
                } else {
                    ""
                };
                println!(
                    "  {:<14} {:<30} {:<8} {:<8} {}",
                    e.kind.as_str(),
                    e.name,
                    has_archive,
                    has_ingest,
                    desc
                );
            }
        }
    }

    Ok(())
}

fn resource_show(
    catalog: &Arc<dag_core::resource_catalog::ResourceCatalog>,
    args: ResourceShowArgs,
) -> color_eyre::Result<()> {
    let entry = catalog.get(&args.name).ok_or_else(|| {
        color_eyre::eyre::eyre!(
            "resource '{}' not found. Use 'autonomics-tui resource list' to see names.",
            args.name
        )
    })?;

    println!("Name:        {}", entry.name);
    println!("Kind:        {}", entry.kind.as_str());
    println!("Description: {}", entry.description);
    println!("Address:     {:?}", entry.address);

    if !entry.metadata.is_empty() {
        println!("Metadata:");
        for (k, v) in &entry.metadata {
            println!("  {k} = {v}");
        }
    }

    if !entry.tags.is_empty() {
        println!("Tags:        {}", entry.tags.join(", "));
    }

    if let Some(ref spec) = entry.archive_spec {
        println!("\nArchive Spec:");
        println!("  remote:      {}:{}", spec.remote, spec.remote_path);
        println!("  checksum:    {}", spec.checksum);
    }

    if let Some(ref status) = entry.archive_status {
        println!("\nArchive Status:");
        if let Some(ref ts) = status.archived_at {
            println!("  archived_at: {ts}");
        }
        if let Some(ref ts) = status.restored_at {
            println!("  restored_at: {ts}");
        }
        if let Some(n) = status.file_count {
            println!("  file_count:  {n}");
        }
        if let Some(s) = status.size_bytes {
            println!("  size_bytes:  {s}");
        }
        if let Some(v) = status.verified {
            println!("  verified:    {}", if v { "✓" } else { "✗" });
        }
    }

    if let Some(ref spec) = entry.ingestion_spec {
        println!("\nIngestion Spec:");
        println!("  source_path:   {}", spec.source_path);
        println!("  source_format: {}", spec.source_format.as_str());
        if !spec.partition_by.is_empty() {
            println!("  partition_by:  {}", spec.partition_by.join(", "));
        }
        println!("  mode:          {}", spec.mode.as_str());
        if spec.restore_before {
            println!("  restore_before: ✓");
        }
        if spec.archive_after {
            println!("  archive_after:  ✓");
        }
        if let Some(ref src) = spec.source_resource {
            println!("  source_resource: {src}");
        }
    }

    // Show the rclone commands for archivable resources.
    if entry.archive_spec.is_some() {
        if let Ok(cmd) = catalog.archive_command(&entry.name) {
            println!("\nArchive command:  {cmd}");
        }
        if let Ok(cmd) = catalog.restore_command(&entry.name) {
            println!("Restore command:  {cmd}");
        }
    }

    Ok(())
}

async fn resource_add(
    catalog: &Arc<dag_core::resource_catalog::ResourceCatalog>,
    args: ResourceAddArgs,
) -> color_eyre::Result<()> {
    use dag_core::resource_catalog::{
        ArchiveSpec, CsvOptions, DbKind, DocKind, IngestionSpec, ResourceAddress, ResourceEntry,
        ResourceKind, SourceFormat, WriteMode,
    };

    let kind = ResourceKind::from_str(&args.kind).ok_or_else(|| {
        color_eyre::eyre::eyre!(
            "unknown kind '{}': expected iceberg_table, file_path, endpoint, config, database, or doc",
            args.kind
        )
    })?;

    let address =
        match kind {
            ResourceKind::IcebergTable => {
                let schema = args.schema.as_deref().ok_or_else(|| {
                    color_eyre::eyre::eyre!("--schema is required for kind=iceberg_table")
                })?;
                let table = args.table.as_deref().ok_or_else(|| {
                    color_eyre::eyre::eyre!("--table is required for kind=iceberg_table")
                })?;
                ResourceAddress::iceberg(schema, table)
            }
            ResourceKind::FilePath => {
                let path = args.path.as_deref().ok_or_else(|| {
                    color_eyre::eyre::eyre!("--path is required for kind=file_path")
                })?;
                ResourceAddress::path(path)
            }
            ResourceKind::Endpoint => {
                let url = args.url.as_deref().ok_or_else(|| {
                    color_eyre::eyre::eyre!("--url is required for kind=endpoint")
                })?;
                ResourceAddress::endpoint(url)
            }
            ResourceKind::Config => {
                let key = args
                    .key
                    .as_deref()
                    .ok_or_else(|| color_eyre::eyre::eyre!("--key is required for kind=config"))?;
                let value = args.value.as_deref().ok_or_else(|| {
                    color_eyre::eyre::eyre!("--value is required for kind=config")
                })?;
                ResourceAddress::config(key, value)
            }
            ResourceKind::Database => {
                let path = args.path.as_deref().ok_or_else(|| {
                    color_eyre::eyre::eyre!("--path is required for kind=database")
                })?;
                let db_kind = match args.db_kind.as_deref() {
                    Some("turso") => DbKind::Turso,
                    Some("postgres") => DbKind::Postgres,
                    _ => DbKind::Sqlite,
                };
                ResourceAddress::database(db_kind, path)
            }
            ResourceKind::Doc => {
                let path = args
                    .path
                    .as_deref()
                    .ok_or_else(|| color_eyre::eyre::eyre!("--path is required for kind=doc"))?;
                let doc_kind = match args.doc_kind.as_deref() {
                    Some("logs") => DocKind::Logs,
                    Some("archive") => DocKind::Archive,
                    Some("notes") => DocKind::Notes,
                    Some("fixtures") => DocKind::Fixtures,
                    _ => DocKind::Docs,
                };
                ResourceAddress::doc(doc_kind, path)
            }
        };

    let mut entry_builder = ResourceEntry::new(&args.name, kind, &args.description, address);

    if !args.metadata.is_empty() {
        entry_builder = entry_builder.with_metadata(args.metadata.iter().cloned().collect());
    }
    if !args.tag.is_empty() {
        entry_builder = entry_builder.with_tags(args.tag.clone());
    }

    // ── Ingestion spec (--source, iceberg_table only) ───────────────────
    let mut ingestion_spec: Option<IngestionSpec> = None;
    let mut archive_spec: Option<ArchiveSpec> = None;
    let mut archive_local_path: Option<PathBuf> = None;
    let mut source_resource_name: Option<String> = None;

    if let Some(ref source_path) = args.source {
        if kind != ResourceKind::IcebergTable {
            return Err(color_eyre::eyre::eyre!(
                "--source is only valid for kind=iceberg_table"
            ));
        }

        let format_str = args.format.as_deref().unwrap_or("parquet");
        let format = match format_str {
            "parquet" => SourceFormat::Parquet,
            "csv" => SourceFormat::Csv,
            "tsv" => SourceFormat::Tsv,
            other => {
                return Err(color_eyre::eyre::eyre!(
                    "unknown format '{other}': expected parquet, csv, or tsv"
                ));
            }
        };

        let mode_str = args.mode.as_deref().unwrap_or("create_if_not_exists");
        let mode = match mode_str {
            "create_if_not_exists" => WriteMode::CreateIfNotExists,
            "create_or_replace" => WriteMode::CreateOrReplace,
            "append" => WriteMode::Append,
            other => {
                return Err(color_eyre::eyre::eyre!(
                    "unknown mode '{other}': expected create_if_not_exists, create_or_replace, or append"
                ));
            }
        };

        let csv_options = match format {
            SourceFormat::Csv | SourceFormat::Tsv => Some(CsvOptions {
                has_header: !args.no_header,
                delimiter: args.delimiter.unwrap_or_else(|| {
                    if matches!(format, SourceFormat::Tsv) {
                        '\t'
                    } else {
                        ','
                    }
                }),
                file_extension: None,
                compression: None,
            }),
            _ => None,
        };

        // Resolve archive config: --archive flag + env vars.
        if args.archive {
            let remote = std::env::var("ARCHIVE_REMOTE").map_err(|_| {
                color_eyre::eyre::eyre!("--archive requires ARCHIVE_REMOTE env var (e.g. 'aliyun')")
            })?;
            let bucket = std::env::var("ARCHIVE_BUCKET").map_err(|_| {
                color_eyre::eyre::eyre!(
                    "--archive requires ARCHIVE_BUCKET env var (e.g. 'autonomics-data')"
                )
            })?;
            let schema = args.schema.as_deref().unwrap_or("data");
            let table = args.table.as_deref().unwrap_or("default");
            let remote_path = format!("{}/{}/{}/", bucket, schema, table);
            archive_spec = Some(ArchiveSpec {
                remote,
                remote_path,
                checksum: true,
            });

            // Resolve the local path to archive (file / dir / glob parent).
            let p = std::path::Path::new(source_path);
            archive_local_path = Some(if p.is_file() || p.is_dir() {
                p.to_path_buf()
            } else {
                let mut dir = p.parent().unwrap_or(p);
                while !dir.is_dir() {
                    dir = match dir.parent() {
                        Some(p) => p,
                        None => break,
                    };
                }
                dir.to_path_buf()
            });

            source_resource_name = Some(format!(
                "source.{}",
                args.name.strip_prefix("iceberg.").unwrap_or(&args.name)
            ));
        }

        ingestion_spec = Some(IngestionSpec {
            source_path: source_path.clone(),
            source_format: format,
            partition_by: args.partition.clone(),
            mode,
            restore_before: false,
            archive_after: false,
            source_resource: source_resource_name.clone(),
            csv_options,
        });
    }

    if let Some(ref spec) = ingestion_spec {
        entry_builder = entry_builder.with_ingestion(spec.clone());
    }

    let registered_name = args.name.clone();
    catalog.register(entry_builder)?;
    catalog.persist().await;

    // ── Register linked source FilePath resource for archive ────────────
    if let (Some(sn), Some(aspec), Some(local)) =
        (&source_resource_name, &archive_spec, &archive_local_path)
    {
        let src_entry = ResourceEntry::new(
            sn.clone(),
            ResourceKind::FilePath,
            &format!("Source files for {}", args.name),
            ResourceAddress::path(local),
        )
        .with_archive(aspec.clone());
        catalog.register(src_entry)?;
        catalog.persist().await;
    }

    // ── Summary output ──────────────────────────────────────────────────
    println!(
        "✓ Registered resource '{}' ({})",
        args.name,
        args.kind.as_str()
    );
    if let Some(ref spec) = ingestion_spec {
        println!(
            "  source: {} ({})",
            spec.source_path,
            spec.source_format.as_str()
        );
        if !spec.partition_by.is_empty() {
            println!("  partition: {}", spec.partition_by.join(", "));
        }
        println!("  mode: {}", spec.mode.as_str());
    }
    if let Some(ref aspec) = archive_spec {
        println!("  archive: {}:{}", aspec.remote, aspec.remote_path);
    }

    // ── Compound: --archive does register + push + ingest ───────────────
    if args.archive {
        let Some(ref src_name) = source_resource_name else {
            return Ok(());
        };

        // Step 1: Archive source to cloud.
        println!("\n[1/2] Archiving source…");
        match catalog.archive(src_name).await {
            Ok(o) => println!(
                "  ✓ {} files, {} bytes, {:.1}s",
                o.files_transferred,
                o.size_bytes,
                o.duration_ms as f64 / 1000.0
            ),
            Err(e) => {
                println!("  ✗ archive failed: {e}");
                println!("\nResource registered but archive/ingest incomplete.");
                println!("Retry: autonomics-tui resource archive -r {src_name}");
                return Ok(());
            }
        }

        // Step 2: Ingest into Iceberg.
        println!("\n[2/2] Ingesting into Iceberg…");
        let datalake = Arc::new(datalake::Datalake::new());
        let executor = runtime::ingestion::IngestionExecutor::new(catalog.clone(), datalake);
        match executor.ingest(&registered_name).await {
            Ok(o) if o.skipped => println!("  ⊘ skipped (table already has data)"),
            Ok(o) => println!(
                "  ✓ {} rows, {} files, {:.1}s",
                o.rows_written,
                o.files_processed,
                o.duration_ms as f64 / 1000.0
            ),
            Err(e) => {
                println!("  ✗ ingest failed: {e}");
                println!("\nSource archived ✓ but ingest incomplete.");
                println!("Retry: autonomics-tui resource ingest -r {registered_name}");
                return Ok(());
            }
        }
        println!("\n✓ Done — registered + archived + ingested.");
    } else if ingestion_spec.is_some() {
        println!("\nTo ingest: autonomics-tui resource ingest -r {registered_name}");
    }

    Ok(())
}

async fn resource_remove(
    catalog: &Arc<dag_core::resource_catalog::ResourceCatalog>,
    args: ResourceRemoveArgs,
) -> color_eyre::Result<()> {
    // Show what would be removed.
    let entry = catalog
        .get(&args.name)
        .ok_or_else(|| color_eyre::eyre::eyre!("resource '{}' not found", args.name))?;

    if !args.yes {
        eprint!(
            "Remove '{}' ({}) from the catalog? [y/N] ",
            entry.name,
            entry.kind.as_str()
        );
        let mut buf = String::new();
        std::io::stdin().read_line(&mut buf)?;
        if !matches!(buf.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("aborted");
            return Ok(());
        }
    }

    catalog.deregister(&args.name);
    catalog.persist().await;
    println!("✓ Removed resource '{}'", args.name);
    Ok(())
}

async fn resource_archive(
    catalog: &Arc<dag_core::resource_catalog::ResourceCatalog>,
    args: ResourceTargetArgs,
) -> color_eyre::Result<()> {
    let targets = resolve_archive_targets(catalog, &args.resource);

    if targets.is_empty() {
        println!("No archivable resources found.");
        return Ok(());
    }

    println!("Archiving {} resource(s) to cloud…\n", targets.len());
    let mut ok = 0u32;
    let mut fail = 0u32;
    for name in &targets {
        print!("  {name}: ");
        use std::io::Write;
        let _ = std::io::stdout().flush();
        match catalog.archive(name).await {
            Err(e) => {
                println!("✗ {e}");
                fail += 1;
            }
            Ok(o) => {
                println!(
                    "✓ {} files, {} bytes, {:.1}s",
                    o.files_transferred,
                    o.size_bytes,
                    o.duration_ms as f64 / 1000.0
                );
                ok += 1;
            }
        }
    }
    println!("\nDone: {ok} ok, {fail} failed.");
    catalog.persist().await;
    Ok(())
}

async fn resource_restore(
    catalog: &Arc<dag_core::resource_catalog::ResourceCatalog>,
    args: ResourceTargetArgs,
) -> color_eyre::Result<()> {
    let targets = resolve_archive_targets(catalog, &args.resource);

    if targets.is_empty() {
        println!("No archivable resources found.");
        return Ok(());
    }

    println!("Restoring {} resource(s) from cloud…\n", targets.len());
    let mut ok = 0u32;
    let mut fail = 0u32;
    for name in &targets {
        print!("  {name}: ");
        use std::io::Write;
        let _ = std::io::stdout().flush();
        match catalog.restore(name).await {
            Err(e) => {
                println!("✗ {e}");
                fail += 1;
            }
            Ok(o) => {
                println!(
                    "✓ {} files, {} bytes, {:.1}s",
                    o.files_transferred,
                    o.size_bytes,
                    o.duration_ms as f64 / 1000.0
                );
                ok += 1;
            }
        }
    }
    println!("\nDone: {ok} ok, {fail} failed.");
    Ok(())
}

async fn resource_verify(
    catalog: &Arc<dag_core::resource_catalog::ResourceCatalog>,
    args: ResourceTargetArgs,
) -> color_eyre::Result<()> {
    let targets = resolve_archive_targets(catalog, &args.resource);

    if targets.is_empty() {
        println!("No archivable resources found.");
        return Ok(());
    }

    println!("Verifying {} resource(s)…\n", targets.len());
    let mut ok = 0u32;
    let mut fail = 0u32;
    for name in &targets {
        print!("  {name}: ");
        use std::io::Write;
        let _ = std::io::stdout().flush();
        match catalog.verify_archive(name).await {
            Ok(true) => {
                println!("✓ verified");
                ok += 1;
            }
            Ok(false) => {
                println!("✗ mismatch (local ≠ remote)");
                fail += 1;
            }
            Err(e) => {
                println!("✗ {e}");
                fail += 1;
            }
        }
    }
    println!("\nDone: {ok} ok, {fail} failed.");
    catalog.persist().await;
    Ok(())
}

/// Resolve the list of resource names for archive/restore/verify.
/// Single resource when `--resource` is given; all archivable otherwise.
fn resolve_archive_targets(
    catalog: &Arc<dag_core::resource_catalog::ResourceCatalog>,
    resource: &Option<String>,
) -> Vec<String> {
    match resource {
        Some(name) => vec![name.clone()],
        None => catalog
            .list_archivable()
            .into_iter()
            .map(|r| r.name)
            .collect(),
    }
}

/// Resolve the list of resource names for ingest.
/// Single resource when `--resource` is given; all ingestible otherwise.
fn resolve_ingest_targets(
    catalog: &Arc<dag_core::resource_catalog::ResourceCatalog>,
    resource: &Option<String>,
) -> Vec<String> {
    match resource {
        Some(name) => vec![name.clone()],
        None => catalog
            .list()
            .into_iter()
            .filter(|e| e.ingestion_spec.is_some())
            .map(|e| e.name)
            .collect(),
    }
}

async fn resource_ingest(
    catalog: &Arc<dag_core::resource_catalog::ResourceCatalog>,
    args: ResourceTargetArgs,
) -> color_eyre::Result<()> {
    let targets = resolve_ingest_targets(catalog, &args.resource);

    if targets.is_empty() {
        println!("No resources with ingestion_spec registered.");
        println!("Register with: autonomics-tui resource add --kind iceberg_table --source …");
        return Ok(());
    }

    let datalake = Arc::new(datalake::Datalake::new());
    let executor = runtime::ingestion::IngestionExecutor::new(catalog.clone(), datalake);

    println!("Ingesting {} resource(s)…\n", targets.len());
    let mut ok = 0u32;
    let mut fail = 0u32;
    for name in &targets {
        print!("  {name}: ");
        use std::io::Write;
        let _ = std::io::stdout().flush();
        match executor.ingest(name).await {
            Err(e) => {
                println!("✗ {e}");
                fail += 1;
            }
            Ok(o) if o.skipped => {
                println!("⊘ skipped (already has data)");
                ok += 1;
            }
            Ok(o) => {
                println!(
                    "✓ {} rows, {} files, {:.1}s",
                    o.rows_written,
                    o.files_processed,
                    o.duration_ms as f64 / 1000.0
                );
                ok += 1;
            }
        }
    }
    println!("\nDone: {ok} ok, {fail} failed.");
    Ok(())
}

async fn resource_drift(
    catalog: &Arc<dag_core::resource_catalog::ResourceCatalog>,
    args: ResourceDriftArgs,
) -> color_eyre::Result<()> {
    use dag_core::resource_catalog::CatalogSnapshot;

    let mut snapshot = CatalogSnapshot::default();

    if args.check_iceberg {
        let datalake = datalake::Datalake::new();
        match datalake.list_all_tables().await {
            Ok(tables) => {
                snapshot.tables = tables
                    .into_iter()
                    .filter_map(|(ns, table)| {
                        let schema = ns.last()?.clone();
                        Some((schema, table))
                    })
                    .collect();
            }
            Err(e) => {
                eprintln!("⚠ could not reach datalake for drift check: {e}");
                eprintln!("  (file-path/database/doc checks still run)");
            }
        }
    }

    let warnings = catalog.check_drift(&snapshot);

    if warnings.is_empty() {
        println!("✓ No drift detected — all registered resources are present.");
        return Ok(());
    }

    println!("⚠ {} drift warning(s):\n", warnings.len());
    for w in &warnings {
        println!("  [{}] {}", w.name, w.detail);
    }
    println!("\n(Index is unchanged — these are warnings only.)");
    Ok(())
}

fn resource_export(
    catalog: &Arc<dag_core::resource_catalog::ResourceCatalog>,
    args: ResourceExportArgs,
) -> color_eyre::Result<()> {
    let entries = catalog.list();
    let json = serde_json::to_string_pretty(&entries)?;

    match &args.output {
        Some(path) => {
            std::fs::write(path, &json)?;
            println!("Exported {} resources to {}", entries.len(), path.display());
        }
        None => {
            println!("{json}");
        }
    }
    Ok(())
}

fn run_tui(_args: TuiArgs) -> color_eyre::Result<()> {
    let mut app = App::new();
    app.start()
}

/// Resolve the OpenGWAS token from env. Mirrors the convention used by
/// the `runtime` crate and `OpengwasClient::new`.
fn opengwas_token() -> color_eyre::Result<String> {
    std::env::var("OPENGWAS_TOKEN")
        .map_err(|_| color_eyre::eyre::eyre!("OPENGWAS_TOKEN env var is not set"))
}

fn run_refresh_opengwas(args: RefreshOpengwasArgs) -> color_eyre::Result<()> {
    let token = opengwas_token()?;
    let client = OpengwasClient::with_cache_dir(Some(&token), default_opengwas_cache_dir())?;

    if args.show_cache_path {
        println!(
            "OpenGWAS cache file: {}",
            client.cache_file_path().display()
        );
    }

    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| color_eyre::eyre::eyre!("failed to build tokio runtime: {e}"))?;

    let refreshed = runtime.block_on(async { client.refresh_disk_cache().await })?;
    println!(
        "OpenGWAS cache refreshed: {} dataset(s) persisted to {}",
        refreshed.len(),
        client.cache_file_path().display()
    );
    Ok(())
}

fn run_clear_opengwas(args: ClearOpengwasArgs) -> color_eyre::Result<()> {
    let token = opengwas_token()?;
    let client = OpengwasClient::with_cache_dir(Some(&token), default_opengwas_cache_dir())?;
    let path = client.cache_file_path();

    if !args.yes {
        eprint!(
            "About to delete OpenGWAS cache at {}. Continue? [y/N] ",
            path.display()
        );
        let mut buf = String::new();
        std::io::stdin().read_line(&mut buf)?;
        if !matches!(buf.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("aborted");
            return Ok(());
        }
    }

    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| color_eyre::eyre::eyre!("failed to build tokio runtime: {e}"))?;
    runtime.block_on(async { client.clear_disk_cache().await })?;
    println!("OpenGWAS cache deleted: {}", path.display());
    Ok(())
}

/// Resolve the cache directory the same way `OpengwasClient::new` does,
/// without round-tripping through env twice.
fn default_opengwas_cache_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("OPENGWAS_CACHE_DIR") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir);
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".cache").join("opengwas");
    }
    std::env::temp_dir().join("opengwas")
}

// ---------------------------------------------------------------------------
// bib subcommand implementation
// ---------------------------------------------------------------------------

async fn run_bib(bib: BibArgs) -> color_eyre::Result<()> {
    let db_path = bib.db.to_string_lossy().to_string();
    let db = bib_base::BibBase::open(&db_path).await?;
    match bib.action {
        BibAction::Upload(args) => run_bib_upload(&db, args).await,
        BibAction::Requests(args) => run_bib_requests(&db, args).await,
        BibAction::Info(args) => run_bib_info(&db, args).await,
        BibAction::List(args) => run_bib_list(&db, args).await,
        BibAction::Export(args) => run_bib_export(&db, args).await,
    }
}

async fn run_bib_upload(db: &bib_base::BibBase, args: UploadArgs) -> color_eyre::Result<()> {
    // 1. Verify article exists.
    let article = db.get_article(&args.article_id).await?.ok_or_else(|| {
        color_eyre::eyre::eyre!(
            "Article '{}' not found in library. \
                 Use bib_save to add the article first.",
            args.article_id,
        )
    })?;

    // 2. Read the file.
    let content = std::fs::read(&args.pdf)
        .map_err(|e| color_eyre::eyre::eyre!("Failed to read '{}': {e}", args.pdf.display()))?;
    let file_size = content.len() as i64;

    // 3. Detect format from extension.
    let ext = args
        .pdf
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("txt");
    let format = bib_base::FileFormat::from_extension(ext);

    // 4. Extract text.
    use bib_base::TextExtractor;
    let extractor = bib_base::SimpleExtractor::new();
    let extracted = extractor.extract(&content, format).await?;
    let text_len = extracted.text.len();

    // 5. Store in BibBase.
    let abs_path = args.pdf.canonicalize().unwrap_or(args.pdf.clone());
    let ft = bib_base::FullText {
        article_id: args.article_id.clone(),
        file_path: abs_path.to_string_lossy().to_string(),
        file_format: format,
        text_content: Some(extracted.text),
        source: bib_base::FullTextSource::UserUpload,
        file_hash: None,
        file_size: Some(file_size),
        uploaded_at: Some(chrono::Utc::now()),
    };
    db.upsert_fulltext(&ft).await?;

    // 6. Update fetch_status if collection context provided.
    if let Some(ref cid) = args.collection_id {
        db.update_fetch_status(
            cid,
            &args.article_id,
            bib_base::FetchStatus::FulltextAvailable,
        )
        .await?;
        println!(
            "✓ Full text uploaded for '{}' ({} bytes, {} chars extracted)\n  \
             Collection '{}' status → fulltext_available",
            article.title, file_size, text_len, cid
        );
    } else {
        println!(
            "✓ Full text uploaded for '{}' ({} bytes, {} chars extracted)\n  \
             Tip: use --collection-id to also update the fetch_status.",
            article.title, file_size, text_len
        );
    }

    Ok(())
}

async fn run_bib_requests(db: &bib_base::BibBase, args: RequestsArgs) -> color_eyre::Result<()> {
    let requests = db
        .list_fulltext_requests(args.collection_id.as_deref())
        .await?;

    if requests.is_empty() {
        println!("No pending full-text requests.");
        return Ok(());
    }

    println!("Pending full-text requests ({}):", requests.len());
    println!("{:-<80}", "");

    for ca in &requests {
        let article = db.get_article(&ca.article_id).await.ok().flatten();
        let title = article
            .as_ref()
            .map(|a| a.title.as_str())
            .unwrap_or("(metadata missing)");
        let doi = article.as_ref().and_then(|a| a.doi().map(String::from));
        let year = article.as_ref().and_then(|a| a.year);

        println!("  Article : {}", ca.article_id);
        println!("  Title   : {title}");
        if let Some(ref doi) = doi {
            println!("  DOI     : {doi}");
        }
        if let Some(year) = year {
            println!("  Year    : {year}");
        }
        println!("  Collection: {}", ca.collection_id);
        if let Some(ref note) = ca.note {
            println!("  Note    : {note}");
        }
        println!();
    }

    println!(
        "To upload: autonomics-tui bib upload --pdf <file> --article-id <id> --collection-id <id>"
    );
    Ok(())
}

async fn run_bib_info(db: &bib_base::BibBase, args: InfoArgs) -> color_eyre::Result<()> {
    let article = db
        .get_article(&args.article_id)
        .await?
        .ok_or_else(|| color_eyre::eyre::eyre!("Article '{}' not found.", args.article_id))?;

    let fulltext = db.get_fulltext(&args.article_id).await?;

    println!("Article: {}", article.title);
    if let Some(ref journal) = article.journal {
        println!("Journal: {journal}");
    }
    if let Some(year) = article.year {
        println!("Year   : {year}");
    }
    if let Some(ref doi) = article.doi() {
        println!("DOI    : {doi}");
    }
    println!();

    match fulltext {
        Some(ft) => {
            println!("Full text: ✓ available");
            println!("  Format  : {}", ft.file_format.as_str());
            println!("  Source  : {}", ft.source.as_str());
            println!("  Path    : {}", ft.file_path);
            if let Some(size) = ft.file_size {
                println!("  Size    : {} bytes", size);
            }
            if let Some(ref text) = ft.text_content {
                println!("  Text    : {} chars", text.len());
                let preview = if text.len() > 200 {
                    format!("{}…", &text[..200])
                } else {
                    text.clone()
                };
                println!("  Preview : {preview}");
            }
        }
        None => {
            println!("Full text: ✗ not uploaded");
        }
    }

    Ok(())
}

async fn run_bib_list(db: &bib_base::BibBase, args: ListArgs) -> color_eyre::Result<()> {
    let count = db.article_count().await?;
    println!("Library: {} articles total\n", count);

    let limit = args.limit.unwrap_or(50).clamp(1, 500);
    let query = args.query.as_deref().unwrap_or("");
    let hits = db.search_articles(query, limit).await?;

    if hits.is_empty() {
        println!("No articles found.");
        return Ok(());
    }

    println!("{:<20} {:<6} {:<10}Title", "ID", "Year", "DOI");
    println!("{:-<80}", "");

    for hit in &hits {
        let article = db.get_article(&hit.article_id).await.ok().flatten();
        let year = article
            .as_ref()
            .and_then(|a| a.year)
            .map(|y| y.to_string())
            .unwrap_or_default();
        let doi = article
            .as_ref()
            .and_then(|a| a.doi().map(String::from))
            .unwrap_or_default();
        let title = &hit.title;
        println!("{:<20} {:<6} {:<10} {}", hit.article_id, year, doi, title);
    }

    Ok(())
}

async fn run_bib_export(db: &bib_base::BibBase, args: ExportArgs) -> color_eyre::Result<()> {
    let format = match args.format.to_lowercase().as_str() {
        "ris" => bib_base::ExportFormat::Ris,
        "markdown" | "md" => bib_base::ExportFormat::Markdown,
        "csl_json" | "json" => bib_base::ExportFormat::CslJson,
        _ => bib_base::ExportFormat::Bibtex,
    };

    let limit = args.limit.unwrap_or(100).clamp(1, 500);

    // Gather articles.
    let articles: Vec<bib_base::Article> = match &args.collection_id {
        Some(cid) => {
            let cas = db.list_collection_articles(cid, None, None).await?;
            let mut out = Vec::new();
            for ca in cas.into_iter().take(limit) {
                if let Some(a) = db.get_article(&ca.article_id).await? {
                    out.push(a);
                }
            }
            out
        }
        None => {
            let hits = db.search_articles("", limit).await?;
            let mut out = Vec::new();
            for hit in hits {
                if let Some(a) = db.get_article(&hit.article_id).await? {
                    out.push(a);
                }
            }
            out
        }
    };

    if articles.is_empty() {
        println!("No articles to export.");
        return Ok(());
    }

    let rendered = bib_base::render_all(&articles, format);

    match &args.output {
        Some(path) => {
            std::fs::write(path, &rendered)?;
            println!(
                "Exported {} articles to {} ({})",
                articles.len(),
                path.display(),
                format.extension()
            );
        }
        None => {
            println!("{}", rendered);
        }
    }

    Ok(())
}

fn main() -> color_eyre::Result<()> {
    // Parse CLI first so we can read --nocapture before init_logging.
    let cli = Cli::parse();

    // Determine nocapture: true if any subcommand (or the default Tui) has it.
    let nocapture = match &cli.command {
        Some(Command::Tui(args)) => args.nocapture,
        _ => false,
    };

    init_logging(nocapture)?;

    match cli.command.unwrap_or(Command::Tui(TuiArgs {
        config: None,
        nocapture: false,
    })) {
        Command::Tui(args) => run_tui(args),
        Command::Cache(cache) => match cache.action {
            CacheAction::RefreshOpengwas(args) => run_refresh_opengwas(args),
            CacheAction::ClearOpengwas(args) => run_clear_opengwas(args),
        },
        Command::Bib(bib) => {
            let runtime = tokio::runtime::Runtime::new()
                .map_err(|e| color_eyre::eyre::eyre!("failed to build tokio runtime: {e}"))?;
            runtime.block_on(run_bib(bib))
        }
        Command::Resource(res) => run_resource(res),
    }
}
