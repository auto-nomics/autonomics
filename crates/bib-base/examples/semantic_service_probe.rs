//! Actual installed-model qualification of BibBase's background service.
//! semantic_service_probe CORPUS_JSONL FRESH_DIRECTORY MODEL SHA256
use bib_base::{
    BibBase, SearchEngineMode,
    semantic_service::{SemanticConfig, SemanticPhase, SemanticStatus},
};
use bib_types::{Article, SearchEvidenceRequest, SearchMode, SearchRequestV2};
use serde::Deserialize;
use serde_json::json;
use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader},
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Deserialize)]
struct Record {
    #[serde(rename = "_id")]
    id: String,
    title: String,
    text: String,
}

async fn ready(
    library: &BibBase,
    revision: u64,
) -> Result<SemanticStatus, Box<dyn std::error::Error>> {
    tokio::time::timeout(Duration::from_secs(180), async {
        loop {
            let status = library.semantic_search_status()?;
            if status.phase == SemanticPhase::Failed {
                return Err(status
                    .last_error
                    .unwrap_or_else(|| "semantic service failed".into())
                    .into());
            }
            if status.phase == SemanticPhase::Ready && status.indexed_revision == Some(revision) {
                return Ok(status);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await?
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 4 {
        return Err(
            "usage: semantic_service_probe CORPUS_JSONL FRESH_DIRECTORY MODEL SHA256".into(),
        );
    }
    let root = PathBuf::from(&args[1]);
    std::fs::create_dir(&root)?;
    let path = root.join("library.db").to_string_lossy().into_owned();
    let library = BibBase::open_with_search_engine(&path, SearchEngineMode::Tantivy).await?;
    assert_eq!(
        library.semantic_search_status()?.phase,
        SemanticPhase::Disabled
    );
    let mut articles = Vec::new();
    for line in BufReader::new(std::fs::File::open(&args[0])?)
        .lines()
        .take(32)
    {
        let record: Record = serde_json::from_str(&line?)?;
        let mut article = Article::new(record.id, record.title);
        article.abstract_text = Some(record.text);
        articles.push(article);
    }
    if articles.len() != 32 {
        return Err("probe requires 32 public corpus documents".into());
    }
    library.upsert_articles(&articles).await?;
    let revision = library.search_index_status().await?.source_revision;
    let started = Instant::now();
    let configuration = library
        .configure_semantic_search(SemanticConfig {
            enabled: true,
            model: args[2].clone(),
            model_digest: args[3].clone(),
            ..Default::default()
        })
        .await?;
    let lexical_started = Instant::now();
    library
        .search_v2(SearchRequestV2 {
            query: "gene".into(),
            ..Default::default()
        })
        .await?;
    let lexical_ms = lexical_started.elapsed().as_secs_f64() * 1000.;
    let initial = ready(&library, revision).await?;
    let initial_ms = started.elapsed().as_secs_f64() * 1000.;
    assert_eq!(initial.indexed_chunks, 32);
    assert_eq!(initial.configuration_generation, configuration);
    let hybrid_started = Instant::now();
    let topic = SearchRequestV2 {
        mode: SearchMode::Hybrid,
        semantic_query: Some("immune cells and gene expression".into()),
        page_size: 5,
        include_articles: true,
        ..Default::default()
    };
    let first = library.search_v2(topic.clone()).await?;
    let hybrid_ms = hybrid_started.elapsed().as_secs_f64() * 1000.;
    assert_eq!(first.total, 32);
    assert_eq!(first.hits.len(), 5);
    assert_eq!(
        first.retrieval.as_ref().unwrap().total_kind,
        "candidate_pool"
    );
    let mut next = topic.clone();
    next.cursor = first.next_cursor.clone();
    let second = library.search_v2(next).await?;
    assert_eq!(first.index_generation, second.index_generation);
    assert!(!second.hits.iter().any(|hit| {
        first
            .hits
            .iter()
            .any(|previous| previous.article_id == hit.article_id)
    }));
    let evidence = library
        .search_evidence(
            &first.hits[0].article_id,
            SearchEvidenceRequest {
                search: topic.clone(),
                index_generation: Some(first.index_generation.clone()),
            },
        )
        .await?;
    assert!(
        evidence
            .evidence
            .iter()
            .any(|row| row.semantic_evidence.is_some())
    );
    let exported = library.export_search(topic.clone()).await?;
    assert_eq!(exported.len(), 32);
    articles[0].year = Some(2026);
    library.upsert_article(&articles[0]).await?;
    let metadata = ready(
        &library,
        library.search_index_status().await?.source_revision,
    )
    .await?;
    assert_ne!(metadata.generation, initial.generation);
    let mut restricted = topic.clone();
    restricted.query = "year:2026".into();
    let filtered = library.search_v2(restricted).await?;
    assert_eq!(filtered.total, 1);
    assert_eq!(filtered.hits[0].article_id, articles[0].id);
    assert_eq!(filtered.retrieval.as_ref().unwrap().eligible_articles, 1);
    articles[0]
        .abstract_text
        .as_mut()
        .unwrap()
        .push_str("\nBackground semantic service replacement probe.");
    library.upsert_article(&articles[0]).await?;
    let replaced = ready(
        &library,
        library.search_index_status().await?.source_revision,
    )
    .await?;
    assert_ne!(replaced.generation, metadata.generation);
    library.delete_article(&articles[1].id).await?;
    let revision = library.search_index_status().await?.source_revision;
    let deletion = ready(&library, revision).await?;
    assert_eq!(deletion.indexed_chunks, 31);
    let after_delete = library.export_search(topic).await?;
    assert_eq!(after_delete.len(), 31);
    assert!(
        !after_delete
            .iter()
            .any(|article| article.id == articles[1].id)
    );
    drop(library);
    let reopened = BibBase::open_with_search_engine(&path, SearchEngineMode::Tantivy).await?;
    let restart = ready(&reopened, revision).await?;
    assert_eq!(restart.generation, deletion.generation);
    assert_eq!(restart.configuration_generation, configuration);
    reopened
        .configure_semantic_search(SemanticConfig::default())
        .await?;
    assert_eq!(
        reopened.semantic_search_status()?.phase,
        SemanticPhase::Disabled
    );
    drop(reopened);
    let disabled = BibBase::open_with_search_engine(&path, SearchEngineMode::Tantivy).await?;
    assert_eq!(
        disabled.semantic_search_status()?.phase,
        SemanticPhase::Disabled
    );
    let mut files = BTreeMap::new();
    for entry in std::fs::read_dir(&root)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("library.db.semantic-v1") && entry.file_type()?.is_file() {
            files.insert(name, entry.metadata()?.len());
        }
    }
    println!(
        "{}",
        json!({"schema":"bib-semantic-service-probe-v2","production_hybrid_search_integrated":true,
        "relevance_evaluation":false,"initial_ms":initial_ms,"lexical_during_enable_ms":lexical_ms,
        "hybrid_first_page_ms":hybrid_ms,"hybrid_first_page":first,"hybrid_second_page_ids":second.hits.iter().map(|hit|&hit.article_id).collect::<Vec<_>>(),
        "evidence":evidence,"hard_filter":filtered,"exported_candidate_count":exported.len(),"deleted_article_absent_from_export":true,
        "initial":initial,"metadata_update":metadata,"content_replacement":replaced,"deletion":deletion,
        "restart":restart,"disable_persisted":true,"vector_store_total_bytes":files.values().sum::<u64>(),"vector_store_files":files})
    );
    Ok(())
}
