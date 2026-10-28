//! Installed-model capacity-policy lifecycle, using twenty public corpus records.
//! semantic_resource_probe CORPUS_JSONL FRESH_DIRECTORY MODEL SHA256
use bib_base::{
    BibBase, SearchEngineMode,
    semantic_service::{
        SemanticConfig, SemanticMaintenanceAction, SemanticMaintenanceRequest, SemanticPhase,
        SemanticStatus,
    },
};
use bib_types::{Article, SearchMode, SearchRequestV2};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    io::{BufRead, BufReader},
    path::PathBuf,
    time::Duration,
};
const DOCUMENTS: usize = 20;
const INITIAL: usize = DOCUMENTS - 1;
const VECTOR_BYTES: u64 = 4096;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
#[derive(Deserialize)]
struct Record {
    #[serde(rename = "_id")]
    id: String,
    title: String,
    text: String,
}

async fn status(
    library: &BibBase,
    epoch: &str,
    phase: SemanticPhase,
    revision: u64,
) -> Result<SemanticStatus> {
    tokio::time::timeout(Duration::from_secs(180), async {
        loop {
            let value = library.semantic_search_status()?;
            if value.configuration_generation == epoch {
                if value.phase == phase
                    && (phase != SemanticPhase::Ready || value.indexed_revision == Some(revision))
                {
                    return Ok(value);
                }
                if value.phase == SemanticPhase::Failed && phase != SemanticPhase::Failed {
                    return Err(value.last_error.unwrap_or_default().into());
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await?
}
fn topic() -> SearchRequestV2 {
    SearchRequestV2 {
        mode: SearchMode::Hybrid,
        semantic_query: Some("immune cells and gene expression".into()),
        ..Default::default()
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 4 {
        return Err(
            "usage: semantic_resource_probe CORPUS_JSONL FRESH_DIRECTORY MODEL SHA256".into(),
        );
    }
    let bytes = std::fs::read(&args[0])?;
    let mut articles = Vec::new();
    for line in BufReader::new(bytes.as_slice()).lines().take(DOCUMENTS) {
        let r: Record = serde_json::from_str(&line?)?;
        let mut a = Article::new(r.id, r.title);
        a.abstract_text = Some(r.text);
        articles.push(a);
    }
    if articles.len() != DOCUMENTS {
        return Err("twenty public records required".into());
    }
    let root = PathBuf::from(&args[1]);
    std::fs::create_dir(&root)?;
    let path = root.join("library.db").to_string_lossy().into_owned();
    let library = BibBase::open_with_search_engine(&path, SearchEngineMode::Tantivy).await?;
    library.upsert_articles(&articles[..INITIAL]).await?;
    let revision = library.search_index_status().await?.source_revision;
    let mut config = SemanticConfig {
        enabled: true,
        model: args[2].clone(),
        model_digest: args[3].clone(),
        ..Default::default()
    };
    config.limits.max_chunks = 1;
    let epoch = library.configure_semantic_search(config.clone()).await?;
    let chunks_limited = status(&library, &epoch, SemanticPhase::Failed, revision).await?;
    assert!(
        chunks_limited
            .last_error
            .as_deref()
            .unwrap()
            .contains("index chunks")
    );
    assert!(library.search_v2(topic()).await.is_err());
    assert_eq!(
        library.search_v2(SearchRequestV2::default()).await?.total,
        INITIAL
    );
    config.limits.max_chunks = INITIAL;
    config.limits.max_vector_bytes = INITIAL as u64 * VECTOR_BYTES;
    let epoch = library.configure_semantic_search(config.clone()).await?;
    let initial = status(&library, &epoch, SemanticPhase::Ready, revision).await?;
    assert_eq!(
        initial.resources.as_ref().unwrap().vector_bytes,
        INITIAL as u64 * VECTOR_BYTES
    );
    assert_eq!(library.search_v2(topic()).await?.total, INITIAL);
    eprintln!("Chunk budget recovery used actual installed model; initial vectors accounted");

    library.upsert_article(&articles[INITIAL]).await?;
    let revision = library.search_index_status().await?.source_revision;
    let chunk_growth = status(&library, &epoch, SemanticPhase::Failed, revision).await?;
    assert!(
        chunk_growth
            .last_error
            .as_deref()
            .unwrap()
            .contains("index chunks")
    );
    assert_eq!(
        library.search_v2(SearchRequestV2::default()).await?.total,
        DOCUMENTS
    );
    config.limits.max_chunks = DOCUMENTS;
    let epoch = library.configure_semantic_search(config.clone()).await?;
    let vector_limited = status(&library, &epoch, SemanticPhase::Failed, revision).await?;
    assert!(
        vector_limited
            .last_error
            .as_deref()
            .unwrap()
            .contains("vector payload bytes")
    );
    config.limits.max_vector_bytes = DOCUMENTS as u64 * VECTOR_BYTES;
    let epoch = library.configure_semantic_search(config.clone()).await?;
    let grown = status(&library, &epoch, SemanticPhase::Ready, revision).await?;
    assert_eq!(grown.signature, initial.signature);
    assert_eq!(grown.progress.as_ref().unwrap().embedded_chunks, 1);
    assert_eq!(
        grown.resources.as_ref().unwrap().vector_bytes,
        DOCUMENTS as u64 * VECTOR_BYTES
    );
    assert_eq!(library.search_v2(topic()).await?.total, DOCUMENTS);

    config.limits.max_vector_bytes = 16 * VECTOR_BYTES;
    let epoch = library.configure_semantic_search(config.clone()).await?;
    status(&library, &epoch, SemanticPhase::Failed, revision).await?;
    let repair = library
        .maintain_semantic_search(SemanticMaintenanceRequest {
            action: SemanticMaintenanceAction::Repair,
            expected_configuration_generation: epoch,
            request_id: uuid::Uuid::new_v4().to_string(),
        })
        .await?;
    let repair_epoch = repair.configuration_generation.clone();
    let partial_repair = status(&library, &repair_epoch, SemanticPhase::Failed, revision).await?;
    assert_eq!(
        partial_repair.maintenance.as_ref().unwrap().request_id,
        repair.request_id
    );
    assert_eq!(
        partial_repair
            .maintenance
            .as_ref()
            .unwrap()
            .progress
            .as_ref()
            .unwrap()
            .embedded_chunks,
        16
    );
    config.limits.max_vector_bytes = DOCUMENTS as u64 * VECTOR_BYTES;
    let epoch = library.configure_semantic_search(config.clone()).await?;
    let repaired = status(&library, &epoch, SemanticPhase::Ready, revision).await?;
    assert_eq!(repaired.store_generation, repair.store_generation.unwrap());
    let progress = repaired
        .maintenance
        .as_ref()
        .unwrap()
        .progress
        .as_ref()
        .unwrap();
    assert_eq!(progress.cached_chunks, 16);
    assert_eq!(progress.embedded_chunks, 4);
    assert_eq!(
        repaired.maintenance.as_ref().unwrap().request_id,
        repair.request_id
    );
    eprintln!(
        "Resource-failed private repair retained 16 actual vectors and embedded only four on budget increase"
    );

    config.limits.max_disk_bytes = 1;
    let epoch = library.configure_semantic_search(config.clone()).await?;
    let disk_limited = status(&library, &epoch, SemanticPhase::Failed, revision).await?;
    assert!(
        disk_limited
            .last_error
            .as_deref()
            .unwrap()
            .contains("cache disk bytes")
    );
    assert_eq!(
        library.search_v2(SearchRequestV2::default()).await?.total,
        DOCUMENTS
    );
    config.limits.max_disk_bytes = 8 * 1024 * 1024 * 1024;
    let epoch = library.configure_semantic_search(config.clone()).await?;
    let restored = status(&library, &epoch, SemanticPhase::Ready, revision).await?;
    assert_eq!(restored.generation, repaired.generation);
    assert_eq!(restored.progress.as_ref().unwrap().embedded_chunks, 0);
    assert!(restored.resources.as_ref().unwrap().disk_bytes.unwrap() > 0);
    drop(library);
    let reopened = BibBase::open_with_search_engine(&path, SearchEngineMode::Tantivy).await?;
    let restart = status(&reopened, &epoch, SemanticPhase::Ready, revision).await?;
    assert_eq!(restart.config, config);
    assert_eq!(restart.generation, restored.generation);
    assert_eq!(reopened.search_v2(topic()).await?.total, DOCUMENTS);
    let receipt = json!({"schema":"bib-semantic-resource-probe-v2","relevance_evaluation":false,
        "actual_installed_model":true,"corpus_sha256":format!("{:x}",Sha256::digest(bytes)),
        "article_ids":articles.iter().map(|a|&a.id).collect::<Vec<_>>(),
        "chunk_limit_initial":chunks_limited,"initial":initial,"chunk_growth_limit":chunk_growth,
        "vector_limit":vector_limited,"grown":grown,"disk_limit":disk_limited,
        "resource_failed_repair":partial_repair,"resumed_same_repair":repaired,
        "restored":restored,"restart":restart,"canonical_write_and_lexical_available":true,
        "limitations":["Twenty public abstract records, not capacity or relevance percentiles",
            "Vector payload limit is per store; disk budget includes all managed store families and is cooperative",
            "No automatic eviction of published models or filesystem compaction tested"]});
    std::fs::write(
        root.join("receipt.json"),
        serde_json::to_vec_pretty(&receipt)?,
    )?;
    println!("{}", receipt);
    Ok(())
}
