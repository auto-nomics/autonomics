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

    /// Data operations — ingest source files into Iceberg, archive/restore
    /// via cloud object storage (rclone).
    Data(DataArgs),
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

// ---------------------------------------------------------------------------
// data subcommand
// ---------------------------------------------------------------------------

#[derive(Debug, Args)]
struct DataArgs {
    #[command(subcommand)]
    action: DataAction,

    /// Specific resource name to operate on. If omitted, operates on ALL
    /// resources that have the relevant spec (ingestion_spec or archive_spec).
    #[arg(long, short = 'r', global = true)]
    resource: Option<String>,
}

/// Subcommands under `autonomics-tui data ...`.
#[derive(Debug, Subcommand)]
enum DataAction {
    /// Register a new parquet/csv source → Iceberg table ingestion job.
    /// Creates a ResourceEntry with ingestion_spec in the catalog.
    Add(AddDataArgs),

    /// Remove a resource entry from the catalog (does NOT drop the Iceberg
    /// table or delete source files — only unregisters the catalog entry).
    Remove(RemoveDataArgs),

    /// Ingest source files (parquet/csv/tsv) into Iceberg tables.
    /// Iterates all resources with an `ingestion_spec`, or a single
    /// resource when `--resource` is given.
    Ingest,

    /// Restore resources from cloud object storage (rclone pull).
    /// Iterates all resources with an `archive_spec`.
    Restore,

    /// Archive resources to cloud object storage (rclone push).
    /// Iterates all resources with an `archive_spec`.
    Archive,

    /// List resources that have ingestion or archive specs configured.
    /// Shows current status (archived_at, verified, etc.).
    List,
}

/// Arguments for `data add`.
#[derive(Debug, Args)]
struct AddDataArgs {
    /// Logical resource name, e.g. "iceberg.gwas.sumstats".
    #[arg(long)]
    name: String,

    /// Human-readable description.
    #[arg(long)]
    description: Option<String>,

    /// Source file path or glob (e.g. "/data/gwas/*.parquet").
    #[arg(long)]
    source: String,

    /// Source format: parquet, csv, or tsv.
    #[arg(long, default_value = "parquet")]
    format: String,

    /// Iceberg schema name (e.g. "gwas").
    #[arg(long)]
    schema: String,

    /// Iceberg table name (e.g. "sumstats").
    #[arg(long)]
    table: String,

    /// Partition columns (repeat for multiple, e.g. --partition chrom --partition pop).
    #[arg(long)]
    partition: Vec<String>,

    /// Write mode: create_if_not_exists, create_or_replace, or append.
    #[arg(long, default_value = "create_if_not_exists")]
    mode: String,

    /// Archive source to cloud AND ingest into Iceberg in one step.
    /// Reads ARCHIVE_REMOTE and ARCHIVE_BUCKET env vars for cloud config.
    /// Archive path is auto-derived as $ARCHIVE_BUCKET/<schema>/<table>/.
    #[arg(long)]
    archive: bool,

    /// CSV delimiter character (default: ','). For tsv, '\t' is used.
    #[arg(long)]
    delimiter: Option<char>,

    /// CSV has no header row.
    #[arg(long)]
    no_header: bool,

    /// Metadata key=value pair (repeat for multiple, e.g. --metadata source=Zenodo --metadata n_snps=1187349).
    #[arg(long, value_parser = parse_key_value)]
    metadata: Vec<(String, String)>,

    /// Tag (repeat for multiple, e.g. --tag ld_score --tag reference_panel).
    #[arg(long)]
    tag: Vec<String>,
}

/// Parse `key=value` into a tuple for `--metadata`.
fn parse_key_value(s: &str) -> color_eyre::Result<(String, String)> {
    s.split_once('=')
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .ok_or_else(|| color_eyre::eyre::eyre!("expected key=value, got '{s}'"))
}

/// Arguments for `data remove`.
#[derive(Debug, Args)]
struct RemoveDataArgs {
    /// Logical resource name to remove.
    #[arg(long)]
    name: String,
}

fn run_data(args: DataArgs) -> color_eyre::Result<()> {
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| color_eyre::eyre::eyre!("failed to build tokio runtime: {e}"))?;
    runtime.block_on(async { run_data_async(args).await })
}

async fn run_data_async(args: DataArgs) -> color_eyre::Result<()> {
    use dag_core::resource_catalog::ResourceCatalog;
    use runtime::config::RuntimeConfig;

    let config = RuntimeConfig::default();
    let manifest_db = config.state_dir.join("resource-manifest.db");
    if let Some(parent) = manifest_db.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let catalog = Arc::new(
        ResourceCatalog::load_or_new(&config.data_dir, &manifest_db).await,
    );

    // Register built-in resources (same as SharedInfra::open would do).
    let _ = ResourceCatalog::set_global(catalog.clone());
    // Re-register providers by opening a minimal engine.
    // For now, the catalog should have persisted entries from a prior TUI run.

    match args.action {
        DataAction::Add(a) => {
            use dag_core::resource_catalog::{
                ArchiveSpec, CsvOptions, IngestionSpec, ResourceAddress,
                ResourceEntry, ResourceKind, SourceFormat, WriteMode,
            };

            let format = match a.format.as_str() {
                "parquet" => SourceFormat::Parquet,
                "csv" => SourceFormat::Csv,
                "tsv" => SourceFormat::Tsv,
                other => {
                    return Err(color_eyre::eyre::eyre!(
                        "unknown format '{other}': expected parquet, csv, or tsv"
                    ))
                }
            };

            let mode = match a.mode.as_str() {
                "create_if_not_exists" => WriteMode::CreateIfNotExists,
                "create_or_replace" => WriteMode::CreateOrReplace,
                "append" => WriteMode::Append,
                other => {
                    return Err(color_eyre::eyre::eyre!(
                        "unknown mode '{other}': expected create_if_not_exists, create_or_replace, or append"
                    ))
                }
            };

            let csv_options = match format {
                SourceFormat::Csv | SourceFormat::Tsv => Some(CsvOptions {
                    has_header: !a.no_header,
                    delimiter: a.delimiter.unwrap_or_else(|| {
                        if matches!(format, SourceFormat::Tsv) { '\t' } else { ',' }
                    }),
                    file_extension: None,
                    compression: None,
                }),
                _ => None,
            };

            // Resolve archive config: --archive flag + env vars.
            let archive_spec = if a.archive {
                let remote = std::env::var("ARCHIVE_REMOTE")
                    .map_err(|_| color_eyre::eyre::eyre!(
                        "--archive requires ARCHIVE_REMOTE env var (e.g. 'aliyun')"
                    ))?;
                let bucket = std::env::var("ARCHIVE_BUCKET")
                    .map_err(|_| color_eyre::eyre::eyre!(
                        "--archive requires ARCHIVE_BUCKET env var (e.g. 'autonomics-data')"
                    ))?;
                // Auto-derive archive path: bucket/schema/table/
                let remote_path = format!("{}/{}/", a.schema, a.table);
                Some(ArchiveSpec { remote, remote_path: format!("{bucket}/{remote_path}"), checksum: true })
            } else {
                None
            };

            // For archiving, resolve what to upload:
            // - Single file → archive just that file
            // - Directory → archive that directory
            // - Glob pattern → walk up to the first real directory
            let archive_local_path = {
                let p = std::path::Path::new(&a.source);
                if p.is_file() {
                    // Single file — rclone copies just this file.
                    p.to_path_buf()
                } else if p.is_dir() {
                    // Directory — rclone copies the directory contents.
                    p.to_path_buf()
                } else {
                    // Glob pattern (e.g. /data/**/*.parquet) — walk up
                    // to the first existing directory.
                    let mut dir = p.parent().unwrap_or(p);
                    while !dir.is_dir() {
                        dir = match dir.parent() {
                            Some(p) => p,
                            None => break,
                        };
                    }
                    dir.to_path_buf()
                }
            };

            // Build the ingestion spec.
            let src_name = if archive_spec.is_some() {
                Some(format!("source.{}", a.name.strip_prefix("iceberg.").unwrap_or(&a.name)))
            } else {
                None
            };

            let spec = IngestionSpec {
                source_path: a.source.clone(),
                source_format: format,
                partition_by: a.partition.clone(),
                mode,
                restore_before: false,
                archive_after: false,
                source_resource: src_name.clone(),
                csv_options,
            };

            // Register source FilePath resource (for archive linkage).
            if let Some(ref sn) = src_name {
                if let Some(ref aspec) = archive_spec {
                    let src_entry = ResourceEntry::new(
                        sn.clone(),
                        ResourceKind::FilePath,
                        &format!("Source files for {}", a.name),
                        ResourceAddress::path(&archive_local_path),
                    )
                    .with_archive(aspec.clone());
                    catalog.register(src_entry)?;
                }
            }

            // Register the target IcebergTable entry.
            let mut entry_builder = ResourceEntry::new(
                a.name.clone(),
                ResourceKind::IcebergTable,
                a.description.as_deref().unwrap_or(""),
                ResourceAddress::iceberg(&a.schema, &a.table),
            )
            .with_ingestion(spec);

            if !a.metadata.is_empty() {
                entry_builder = entry_builder.with_metadata(a.metadata.iter().cloned().collect());
            }
            if !a.tag.is_empty() {
                entry_builder = entry_builder.with_tags(a.tag.clone());
            }

            catalog.register(entry_builder)?;
            catalog.persist().await;

            println!("✓ Registered resource '{}'", a.name);
            println!("  target: iceberg.{}.{}", a.schema, a.table);
            println!("  source: {} ({})", a.source, a.format);
            if !a.partition.is_empty() {
                println!("  partition: {}", a.partition.join(", "));
            }
            println!("  mode: {}", a.mode);
            if !a.metadata.is_empty() {
                println!("  metadata:");
                for (k, v) in &a.metadata {
                    println!("    {k} = {v}");
                }
            }
            if !a.tag.is_empty() {
                println!("  tags: {}", a.tag.join(", "));
            }
            if let Some(ref aspec) = archive_spec {
                println!("  archive: {}:{}", aspec.remote, aspec.remote_path);
            }

            // ── If --archive: execute archive + ingest in one shot ──────
            if a.archive {
                let Some(ref src_name) = src_name else { unreachable!() };

                // Step 1: Archive source to cloud.
                println!("\n[1/2] Archiving source…");
                match catalog.archive(src_name).await {
                    Ok(o) => println!("  ✓ {} files, {} bytes, {:.1}s",
                        o.files_transferred, o.size_bytes, o.duration_ms as f64 / 1000.0),
                    Err(e) => {
                        println!("  ✗ archive failed: {e}");
                        println!("\nResource registered but archive/ingest incomplete.");
                        println!("Retry: autonomics-tui data archive -r {src_name}");
                        return Ok(());
                    }
                }

                // Step 2: Ingest into Iceberg.
                println!("\n[2/2] Ingesting into Iceberg…");
                let datalake = Arc::new(datalake::Datalake::new());
                let executor = runtime::ingestion::IngestionExecutor::new(catalog.clone(), datalake);
                match executor.ingest(&a.name).await {
                    Ok(o) if o.skipped => println!("  ⊘ skipped (table already has data)"),
                    Ok(o) => println!("  ✓ {} rows, {} files, {:.1}s",
                        o.rows_written, o.files_processed, o.duration_ms as f64 / 1000.0),
                    Err(e) => {
                        println!("  ✗ ingest failed: {e}");
                        println!("\nSource archived ✓ but ingest incomplete.");
                        println!("Retry: autonomics-tui data ingest -r {}", a.name);
                        return Ok(());
                    }
                }
                println!("\n✓ Done — registered + archived + ingested.");
            } else {
                println!("\nTo ingest: autonomics-tui data ingest -r {}", a.name);
            }
            return Ok(());
        }

        DataAction::Remove(a) => {
            if catalog.deregister(&a.name).is_some() {
                catalog.persist().await;
                println!("✓ Removed resource '{}'", a.name);
            } else {
                println!("— resource '{}' not found in catalog", a.name);
            }
            return Ok(());
        }

        DataAction::Ingest => {
            // Need Datalake for Iceberg table creation/insertion.
            let datalake = Arc::new(datalake::Datalake::new());
            let executor = runtime::ingestion::IngestionExecutor::new(catalog.clone(), datalake);

            let targets: Vec<String> = match &args.resource {
                Some(name) => vec![name.clone()],
                None => catalog
                    .list()
                    .into_iter()
                    .filter(|e| e.ingestion_spec.is_some())
                    .map(|e| e.name)
                    .collect(),
            };

            if targets.is_empty() {
                println!("No resources with ingestion_spec registered.");
                println!("Register resources with IngestionSpec via the ResourceCatalog first.");
                return Ok(());
            }

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
        }

        DataAction::Restore => {
            let targets: Vec<String> = match &args.resource {
                Some(name) => vec![name.clone()],
                None => catalog
                    .list_archivable()
                    .into_iter()
                    .map(|r| r.name)
                    .collect(),
            };

            if targets.is_empty() {
                println!("No resources with archive_spec registered.");
                return Ok(());
            }

            println!("Restoring {} resource(s) from archive…\n", targets.len());
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
        }

        DataAction::Archive => {
            let targets: Vec<String> = match &args.resource {
                Some(name) => vec![name.clone()],
                None => catalog
                    .list_archivable()
                    .into_iter()
                    .map(|r| r.name)
                    .collect(),
            };

            if targets.is_empty() {
                println!("No resources with archive_spec registered.");
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
                            "✓ {} files, {} bytes, {:.1}s → {}",
                            o.files_transferred,
                            o.size_bytes,
                            o.duration_ms as f64 / 1000.0,
                            name
                        );
                        ok += 1;
                    }
                }
            }
            println!("\nDone: {ok} ok, {fail} failed.");
            catalog.persist().await;
        }

        DataAction::List => {
            // List ingestible resources.
            let ingestible: Vec<_> = catalog
                .list()
                .into_iter()
                .filter(|e| e.ingestion_spec.is_some())
                .collect();
            if !ingestible.is_empty() {
                println!("Ingestible resources ({}):", ingestible.len());
                for e in &ingestible {
                    let spec = e.ingestion_spec.as_ref().unwrap();
                    println!(
                        "  {} [{} → {}]",
                        e.name,
                        spec.source_format.as_str(),
                        e.kind.as_str()
                    );
                    println!("    source: {}", spec.source_path);
                    if !spec.partition_by.is_empty() {
                        println!("    partition: {}", spec.partition_by.join(", "));
                    }
                    println!("    mode: {}", spec.mode.as_str());
                    if spec.restore_before {
                        println!("    restore_before: ✓");
                    }
                    if spec.archive_after {
                        println!("    archive_after: ✓");
                    }
                }
                println!();
            }

            // List archivable resources.
            let archivable = catalog.list_archivable();
            if !archivable.is_empty() {
                println!("Archivable resources ({}):", archivable.len());
                for r in &archivable {
                    let status = match (&r.archived_at, r.verified) {
                        (Some(ts), Some(true)) => format!("archived ✓ ({ts})"),
                        (Some(ts), _) => format!("archived ({ts})"),
                        _ => "not archived".to_string(),
                    };
                    println!("  {} → {} ({})", r.name, r.remote, status);
                }
            }

            if ingestible.is_empty() && archivable.is_empty() {
                println!("No data resources registered.");
                println!("Resources are registered via ResourceProvider in the runtime.");
                println!("Run the TUI once to populate the catalog, then use this command.");
            }
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
        Command::Data(data) => run_data(data),
    }
}
