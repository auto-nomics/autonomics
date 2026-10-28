//! Exercise the durable index with actual installed embeddings and public corpus.
//! semantic_index_probe CORPUS_JSONL FRESH_DIRECTORY MODEL SHA256
use bib_base::{
    BibBase, SearchEngineMode,
    semantic_index::{SemanticIndex, SemanticProfile},
};
use bib_types::Article;
use serde::Deserialize;
use serde_json::json;
use std::{
    collections::{BTreeMap, HashSet},
    io::{BufRead, BufReader},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use text_embedding::{PinnedOllamaEmbedder, TextEmbedder};

#[derive(Deserialize)]
struct Record {
    #[serde(rename = "_id")]
    id: String,
    title: String,
    text: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 4 {
        return Err("usage: semantic_index_probe CORPUS_JSONL FRESH_DIRECTORY MODEL SHA256".into());
    }
    let root = PathBuf::from(&args[1]);
    std::fs::create_dir(&root)?;
    let database = root.join("library.db").to_string_lossy().into_owned();
    let sidecar = root.join("vectors.db").to_string_lossy().into_owned();
    let library = BibBase::open_with_search_engine(&database, SearchEngineMode::Tantivy).await?;
    let mut articles = Vec::new();
    for line in BufReader::new(std::fs::File::open(&args[0])?)
        .lines()
        .take(128)
    {
        let row: Record = serde_json::from_str(&line?)?;
        let mut article = Article::new(row.id, row.title);
        article.abstract_text = Some(row.text);
        articles.push(article);
    }
    if articles.len() != 128 {
        return Err("probe requires at least 128 corpus records".into());
    }
    library.upsert_articles(&articles).await?;
    let source = turso::Builder::new_local(&database)
        .experimental_multiprocess_wal(true)
        .build()
        .await?;
    let reader = source.connect()?;
    let provider = Arc::new(
        PinnedOllamaEmbedder::connect(
            "http://127.0.0.1:11434",
            &args[2],
            &args[3],
            1024,
            16,
            Duration::from_secs(60),
        )
        .await?,
    );
    let profile = SemanticProfile::default();
    let mut index =
        SemanticIndex::open(reader.clone(), &sidecar, profile.clone(), provider.clone()).await?;
    let started = Instant::now();
    let (initial, first) = index.sync().await?;
    let initial_ms = started.elapsed().as_secs_f64() * 1000.;
    assert_eq!(first.embedded_chunks, 128);
    assert_eq!(initial.chunks.len(), 128);
    articles[0].year = Some(2026);
    library.upsert_article(&articles[0]).await?;
    let (_, metadata) = index.sync().await?;
    assert_eq!(metadata.embedded_chunks, 0);
    assert_eq!(metadata.cached_chunks, 1);
    articles[0]
        .abstract_text
        .as_mut()
        .unwrap()
        .push_str("\nSemantic index content replacement probe.");
    library.upsert_article(&articles[0]).await?;
    let (changed, content) = index.sync().await?;
    assert_eq!(content.embedded_chunks, 1);
    let old_hash = &initial
        .chunks
        .iter()
        .find(|row| row.article_id == articles[0].id)
        .unwrap()
        .content_hash;
    let new_hash = &changed
        .chunks
        .iter()
        .find(|row| row.article_id == articles[0].id)
        .unwrap()
        .content_hash;
    assert_ne!(old_hash, new_hash);
    library.delete_article(&articles[1].id).await?;
    let (deleted, deletion) = index.sync().await?;
    assert_eq!(deleted.chunks.len(), 127);
    assert_eq!(deletion.embedded_chunks, 0);
    assert!(
        !deleted
            .chunks
            .iter()
            .any(|row| row.article_id == articles[1].id)
    );
    let generation = deleted.generation.clone();
    drop(index);
    let mut reopened =
        SemanticIndex::open(reader, &sidecar, profile.clone(), provider.clone()).await?;
    let (durable, restart) = reopened.sync().await?;
    assert_eq!(durable.generation, generation);
    assert_eq!(restart.embedded_chunks, 0);
    let query = profile.query_input("immune cells and gene expression")?;
    let vector = provider.embed(&query).await?;
    let allowed = HashSet::from([articles[2].id.clone()]);
    let hits = durable.rank(&vector.vector, &allowed, 10)?;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0.article_id, articles[2].id);
    let mut files = BTreeMap::new();
    for entry in std::fs::read_dir(&root)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("vectors.db") && entry.file_type()?.is_file() {
            files.insert(name, entry.metadata()?.len());
        }
    }
    println!(
        "{}",
        json!({"schema":"bib-semantic-index-probe-v1","production_search_integrated":false,
        "relevance_evaluation":false,"provider_signature":provider.signature(),"profile_signature":durable.signature,
        "source_revision":durable.revision,"initial_ms":initial_ms,"initial":first,"metadata_only":metadata,
        "content_replacement":content,"deletion":deletion,"restart":restart,"scope_checked_before_top_k":true,
        "unchanged_generation_after_restart":true,"immutable_old_snapshot":initial.chunks.len()==128,
        "vector_store_total_bytes":files.values().sum::<u64>(),"vector_store_files":files})
    );
    Ok(())
}
