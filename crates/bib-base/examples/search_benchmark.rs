//! Repeatable synthetic P1 scale benchmark through the production storage actor.
//! cargo run -p bib-base --example search_benchmark -- --count 10000 --output report.json
//! Use --release for latency qualification; debug runs are diagnostic only.
use bib_base::BibBase;
use bib_types::{
    Article, Author, FileFormat, FullText, FullTextSource, Identifier, SearchRequestV2,
};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    time::Instant,
};

fn argument(name: &str, default: &str) -> String {
    let args = std::env::args().collect::<Vec<_>>();
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].clone())
        .unwrap_or_else(|| default.into())
}
fn directory_bytes(path: &Path) -> u64 {
    if path.is_file() {
        return path.metadata().map(|m| m.len()).unwrap_or(0);
    }
    std::fs::read_dir(path)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| directory_bytes(&entry.path()))
                .sum()
        })
        .unwrap_or(0)
}
fn memory() -> Value {
    let text = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let read = |name: &str| {
        text.lines()
            .find(|line| line.starts_with(name))
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|value| value.parse::<u64>().ok())
    };
    json!({"rss_kib":read("VmRSS:"),"peak_rss_kib":read("VmHWM:")})
}
fn article(i: usize, abstract_chars: usize) -> Article {
    let subject = match i % 10 {
        0 => "Gene editing with CRISPR",
        1 => "APOE and Alzheimer disease",
        2 => "Single cell transcriptomics",
        _ => "Regulatory genetics and population biology",
    };
    let mut article = Article::new(format!("bench-{i:09}"), format!("{subject}: study {i}"));
    let paragraph = format!(
        "Study {i} evaluates regulatory evidence in human cohorts. Replicated observations connect genetic variation with cellular mechanisms and population outcomes. "
    );
    let paragraph = if i % 50 == 0 {
        format!("肿瘤免疫治疗机制。{paragraph}")
    } else {
        paragraph
    };
    if i % 100 == 42 {
        article.title.push_str(" IL-6");
    }
    if i % 100 == 43 {
        article.title.push_str(" IL-10 chromosome 6");
    }
    article.abstract_text = Some(
        paragraph
            .repeat(abstract_chars / paragraph.chars().count().max(1) + 1)
            .chars()
            .take(abstract_chars)
            .collect(),
    );
    article.year = Some(2000 + (i % 27) as u16);
    article.journal = Some(format!("Research Journal {}", i % 20));
    article.authors.push(Author {
        last_name: format!("Researcher{}", i % 100),
        fore_name: Some("Avery".into()),
        initials: None,
        affiliation: None,
        orcid: None,
        corresponding: false,
    });
    article
        .identifiers
        .push(Identifier::doi(format!("10.9999/bench-{i:09}")));
    article.keywords = vec![format!("topic{}", i % 50)];
    article
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let gui = argument("--gui", "false") == "true";
    let count: usize = argument("--count", "10000").parse()?;
    let abstract_chars: usize = argument("--abstract-chars", "1000").parse()?;
    let fulltext_every: usize = argument("--fulltext-every", "10").parse()?;
    let fulltext_chars: usize = argument("--fulltext-chars", "10000").parse()?;
    let repeats: usize = argument("--query-repeats", "10").parse()?;
    if count < 2 || repeats == 0 {
        return Err("count must be >=2 and query-repeats >0".into());
    }
    let root = PathBuf::from(argument(
        "--directory",
        &std::env::temp_dir()
            .join(format!("bib-bench-{count}-{}", uuid::Uuid::new_v4()))
            .to_string_lossy(),
    ));
    std::fs::create_dir_all(&root)?;
    let database = root.join("library.db");
    let reuse = argument("--reuse", "false") == "true";
    if database.exists() && !reuse {
        return Err("benchmark requires a fresh database directory (or --reuse true)".into());
    }
    if reuse && !database.exists() {
        return Err("--reuse requires an existing synthetic fixture".into());
    }
    let opened = Instant::now();
    let db = BibBase::open_with_search_engine(
        database.to_string_lossy(),
        bib_base::SearchEngineMode::Tantivy,
    )
    .await?;
    let open_ms = opened.elapsed().as_secs_f64() * 1000.0;
    if reuse
        && (db.article_count().await? != count as i64
            || db.get_article("bench-000000001").await?.is_none())
    {
        return Err("existing fixture does not match --count".into());
    }
    let started = Instant::now();
    if !reuse {
        for start in (0..count).step_by(500) {
            let articles = (start..(start + 500).min(count))
                .map(|i| article(i, abstract_chars))
                .collect::<Vec<_>>();
            db.upsert_articles(&articles).await?;
            let imported = (start + 500).min(count);
            if imported % 10000 == 0 || imported == count {
                eprintln!("imported {imported} / {count}");
            }
        }
    }
    let metadata_import_ms = (!reuse).then(|| started.elapsed().as_secs_f64() * 1000.0);
    let started = Instant::now();
    let mut attachments = if reuse && fulltext_every > 0 {
        count.div_ceil(fulltext_every)
    } else {
        0
    };
    if !reuse && fulltext_every > 0 {
        for i in (0..count).step_by(fulltext_every) {
            let paragraph = format!(
                "Attachmentmarker{i} provides evidence about gene editing mechanisms. Section results describe cellular pathways and replicated experimental observations. "
            );
            db.upsert_fulltext(&FullText {
                article_id: format!("bench-{i:09}"),
                attachment_key: None,
                file_path: format!("/synthetic/{i}.txt"),
                file_format: FileFormat::Txt,
                text_content: Some(
                    paragraph
                        .repeat(fulltext_chars / paragraph.len() + 1)
                        .chars()
                        .take(fulltext_chars)
                        .collect(),
                ),
                source: FullTextSource::UserUpload,
                file_hash: None,
                file_size: None,
                uploaded_at: None,
                extract_status: None,
                text_format: None,
                extracted_by: None,
                extract_error: None,
            })
            .await?;
            attachments += 1;
            if attachments % 2000 == 0 {
                eprintln!("imported {attachments} full texts");
            }
        }
    }
    let fulltext_import_ms = (!reuse).then(|| started.elapsed().as_secs_f64() * 1000.0);
    let ready_started = Instant::now();
    db.search_v2(SearchRequestV2 {
        query: "identifier:10.9999/bench-000000001".into(),
        page_size: 1,
        ..Default::default()
    })
    .await?;
    let initial_index_catchup_ms = ready_started.elapsed().as_secs_f64() * 1000.0;
    let started = Instant::now();
    db.rebuild_search_index().await?;
    let rebuild_ms = started.elapsed().as_secs_f64() * 1000.0;
    eprintln!("full rebuild completed in {rebuild_ms:.1} ms");
    let queries = [
        "identifier:10.9999/bench-000000001",
        "title:\"gene editing\"",
        "APOE Alzheimer",
        "author:Researcher42",
        "keyword:topic7",
        "title:genetics AND year:2020..2026",
        "NOT title:CRISPR",
        "fulltext:attachmentmarker0",
        "abstract:免疫",
        "abstract:\"肿瘤免疫治疗\"",
        "IL-6",
    ];
    let mut results = Vec::new();
    for query in queries {
        if fulltext_every == 0 && query.starts_with("fulltext:") {
            continue;
        }
        let request = SearchRequestV2 {
            query: query.into(),
            page_size: 50,
            include_facets: gui,
            include_articles: gui,
            ..Default::default()
        };
        let cold = Instant::now();
        let page = db.search_v2(request.clone()).await?;
        let first_query_ms = cold.elapsed().as_secs_f64() * 1000.0;
        if query.starts_with("identifier:")
            && (page.total != 1 || page.hits[0].article_id != "bench-000000001")
        {
            return Err("exact identifier correctness check failed".into());
        }
        if query.starts_with("fulltext:") && page.total != 1 {
            return Err("attachment identity correctness check failed".into());
        }
        if query == "IL-6" && page.total != (count + 57) / 100 {
            return Err("scientific symbol boundary check failed".into());
        }
        if query.starts_with("abstract:") && page.total != count.div_ceil(50) {
            return Err("Chinese recall count check failed".into());
        }
        let mut samples = Vec::new();
        for _ in 0..repeats {
            let started = Instant::now();
            let again = db.search_v2(request.clone()).await?;
            if again.total != page.total {
                return Err("stable-count check failed".into());
            }
            samples.push(started.elapsed().as_secs_f64() * 1000.0);
        }
        samples.sort_by(f64::total_cmp);
        let percentile = |p: f64| {
            samples[((samples.len() as f64 * p).ceil() as usize)
                .saturating_sub(1)
                .min(samples.len() - 1)]
        };
        let p50 = percentile(0.5);
        let p95 = percentile(0.95);
        eprintln!(
            "{query}: total={}, p50={p50:.2}ms p95={p95:.2}ms",
            page.total
        );
        results.push(json!({"query":query,"total":page.total,"first_query_ms":first_query_ms,"p50_ms":p50,"p95_ms":p95,"samples_ms":samples}));
    }
    let known_checks = 100.min(count);
    for check in 0..known_checks {
        let i = check * count / known_checks;
        let page = db
            .search_v2(SearchRequestV2 {
                query: format!("identifier:10.9999/bench-{i:09}"),
                page_size: 1,
                ..Default::default()
            })
            .await?;
        if page.total != 1 || page.hits[0].article_id != format!("bench-{i:09}") {
            return Err("known identifier Top-1 check failed".into());
        }
    }
    let report = json!({"schema_version":2,"synthetic":true,"gui_facets_and_articles":gui,"reused_existing_fixture":reuse,"open_ms":open_ms,"profile":if cfg!(debug_assertions){"debug"}else{"release"},"os":std::env::consts::OS,"arch":std::env::consts::ARCH,
        "dataset":{"articles":count,"abstract_chars_per_article":abstract_chars,"attachments":attachments,"fulltext_chars_per_attachment":fulltext_chars,"han_abstract_every":50,"scientific_symbol_every":100},
        "metadata_import_with_background_indexing_ms":metadata_import_ms,"fulltext_import_with_background_indexing_ms":fulltext_import_ms,"explicit_full_rebuild_ms":rebuild_ms,"initial_index_catchup_ms":initial_index_catchup_ms,"known_identifier_checks":{"queries":known_checks,"top1_correct":known_checks},
        "memory":memory(),"database_bytes":directory_bytes(&database),"index_bytes":directory_bytes(&PathBuf::from(format!("{}.search-v2",database.to_string_lossy()))),"all_artifact_bytes":directory_bytes(&root),
        "index_status":db.search_index_status().await?,"queries":results,"directory":root});
    let output = PathBuf::from(argument(
        "--output",
        &root.join("report.json").to_string_lossy(),
    ));
    if let Some(parent) = output.parent().filter(|path| !path.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&output, serde_json::to_vec_pretty(&report)?)?;
    println!("{}", output.display());
    Ok(())
}
