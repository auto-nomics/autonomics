//! Bibliography management commands.

use crate::cli::{BibAction, BibArgs, ExportArgs, InfoArgs, ListArgs, RequestsArgs, UploadArgs};
use bib_base::{stored_fulltext, vfs_virtual_path};
use vfs::OpendalFileStorage;

pub async fn run_bib(bib: BibArgs) -> color_eyre::Result<()> {
    let config = runtime::RuntimeConfig::builder()
        .bib_db_path(&bib.db)
        .build();
    let file_storage = runtime::bibliography_file_storage(&config)?;
    let db_path = bib.db.to_string_lossy().to_string();
    let db = bib_base::BibBase::open(&db_path).await?;
    match bib.action {
        BibAction::Upload(args) => run_bib_upload(&db, &file_storage, args).await,
        BibAction::Requests(args) => run_bib_requests(&db, args).await,
        BibAction::Info(args) => run_bib_info(&db, args).await,
        BibAction::List(args) => run_bib_list(&db, args).await,
        BibAction::Export(args) => run_bib_export(&db, args).await,
    }
}

async fn run_bib_upload(
    db: &bib_base::BibBase,
    file_storage: &OpendalFileStorage,
    args: UploadArgs,
) -> color_eyre::Result<()> {
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
    let extractor = bib_base::OcrFallbackExtractor::new();
    let extracted = match extractor.extract(&content, format).await {
        Ok(extracted) => Some(extracted),
        Err(error) => {
            tracing::warn!(
                filename = %args.pdf.display(),
                error = %error,
                "failed to extract full-text content; storing original bytes only"
            );
            None
        }
    };
    let text_len = extracted
        .as_ref()
        .map_or(0, |text| text.text.chars().count());

    // 5. Store the original in the same VFS used by the runtime and HTTP API.
    let filename = args
        .pdf
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("upload.txt");
    let stored = stored_fulltext(&args.article_id, filename, &content);
    let virtual_path = vfs_virtual_path(&stored.path)
        .ok_or_else(|| color_eyre::eyre::eyre!("stored full-text path is not a VFS path"))?;
    file_storage
        .write_bytes(&virtual_path, content.clone())
        .await?;
    let ft = bib_base::FullText {
        article_id: args.article_id.clone(),
        file_path: stored.path,
        file_format: format,
        text_content: extracted.as_ref().map(|text| text.text.clone()),
        source: bib_base::FullTextSource::UserUpload,
        file_hash: Some(stored.file_hash),
        file_size: Some(file_size),
        uploaded_at: Some(chrono::Utc::now()),
        // TUI attach stays on the synchronous local-extract path.
        parse_status: "done".to_owned(),
        parse_engine: Some("builtin".to_owned()),
        parse_error: None,
    };
    let previous = db.get_fulltext(&args.article_id).await?;
    if let Err(error) = db.upsert_fulltext(&ft).await {
        if previous.as_ref().map(|old| old.file_path.clone()) != Some(ft.file_path.clone()) {
            let _ = file_storage.delete_object(&virtual_path).await;
        }
        return Err(error.into());
    }
    if let Some(previous) = previous {
        if previous.file_path != ft.file_path {
            if let Some(old_path) = vfs_virtual_path(&previous.file_path) {
                let _ = file_storage.delete_object(&old_path).await;
            }
        }
    }

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
