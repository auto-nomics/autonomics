use std::panic;
use std::path::PathBuf;

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

fn init_logging() -> color_eyre::Result<()> {
    color_eyre::install()?;

    let log_dir = PathBuf::from("logs");
    std::fs::create_dir_all(&log_dir)?;

    let file_appender = RollingFileAppender::new(Rotation::DAILY, &log_dir, "phloem-tui.log");
    let file_writer = file_appender.with_max_level(Level::DEBUG);

    let timer = OffsetTime::new(
        time::UtcOffset::current_local_offset().expect("timezone"),
        format_description!("[year]-[month]-[day] [hour]:[minute]:[second].[subsecond digits:3]"),
    );

    // Retain the previously-installed hook so it can be re-invoked once the
    // TUI's own panic handling is finalised; currently we log only.
    let _default = panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let bt = std::backtrace::Backtrace::force_capture();
        tracing::error!(
            target: "panic",
            payload = %info,           // "panicked at src/...: xxx"
            backtrace = %bt,
            "thread panicked"
        );
        // _default(info); // 保留默认行为(打到 stderr)
    }));

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            EnvFilter::new("autonomics_tui=debug,agentik_core=debug,agentik_sdk=debug")
        }))
        .with_writer(file_writer)
        .with_ansi(false)
        .with_target(false)
        .with_file(true)
        .with_line_number(true)
        .with_span_events(FmtSpan::NONE)
        .with_timer(timer)
        .init();

    tracing::info!(
        "logging initialized — logs directory: {}",
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
}

#[derive(Debug, Args)]
struct TuiArgs {
    /// Optional path to a TUI configuration file.
    #[arg(long, short, value_name = "PATH")]
    config: Option<PathBuf>,
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

    println!("{:<20} {:<6} {:<10} {}", "ID", "Year", "DOI", "Title");
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
    init_logging()?;

    let cli = Cli::parse();
    match cli
        .command
        .unwrap_or(Command::Tui(TuiArgs { config: None }))
    {
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
    }
}
