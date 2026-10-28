//! Real installed-model, long public abstract and cache-isolation qualification.
//! semantic_long_input_probe CORPUS_JSONL FRESH_DIRECTORY MODEL SHA256
//! No retrieval-quality claim: documents are selected by length, not relevance.
use bib_base::{
    BibBase, SearchEngineMode,
    semantic_service::{SemanticConfig, SemanticPhase, SemanticStatus},
};
use bib_types::{Article, SearchEvidenceRequest, SearchMode, SearchRequestV2};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    io::{BufRead, BufReader},
    path::PathBuf,
    time::{Duration, Instant},
};
use text_embedding::OllamaInferenceOptions;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
#[derive(Deserialize)]
struct Record {
    #[serde(rename = "_id")]
    id: String,
    title: String,
    text: String,
}

async fn wait(
    library: &BibBase,
    epoch: &str,
    revision: u64,
    expected: SemanticPhase,
) -> Result<(SemanticStatus, Value)> {
    let started = Instant::now();
    let mut lexical_times = Vec::new();
    let status = tokio::time::timeout(Duration::from_secs(240), async {
        loop {
            let status = library.semantic_search_status()?;
            if status.configuration_generation == epoch {
                if status.phase == expected
                    && (expected != SemanticPhase::Ready
                        || status.indexed_revision == Some(revision))
                {
                    return Ok::<_, Box<dyn std::error::Error>>(status);
                }
                if status.phase == SemanticPhase::Failed && expected != SemanticPhase::Failed {
                    return Err(status.last_error.unwrap_or_default().into());
                }
            }
            let lexical_started = Instant::now();
            let page = library.search_v2(SearchRequestV2::default()).await?;
            assert!(!page.hits.is_empty());
            lexical_times.push(lexical_started.elapsed().as_secs_f64() * 1000.);
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await??;
    lexical_times.sort_by(f64::total_cmp);
    let observations = json!({"wait_ms":started.elapsed().as_secs_f64()*1000.,
        "lexical_samples":lexical_times.len(),"lexical_max_ms":lexical_times.last(),
        "lexical_p95_ms":lexical_times.get((lexical_times.len()*95).div_ceil(100).saturating_sub(1))});
    Ok((status, observations))
}

fn request() -> SearchRequestV2 {
    SearchRequestV2 {
        mode: SearchMode::Hybrid,
        semantic_query: Some("immune cells and gene expression".into()),
        include_articles: true,
        page_size: 2,
        ..Default::default()
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 4 {
        return Err(
            "usage: semantic_long_input_probe CORPUS_JSONL FRESH_DIRECTORY MODEL SHA256".into(),
        );
    }
    let bytes = std::fs::read(&args[0])?;
    let mut seen = HashSet::new();
    let mut records = Vec::new();
    for line in BufReader::new(bytes.as_slice()).lines() {
        let row: Record = serde_json::from_str(&line?)?;
        if row.id.is_empty() || !seen.insert(row.id.clone()) || row.text.trim().is_empty() {
            return Err("invalid, duplicate or empty public corpus record".into());
        }
        records.push(row);
    }
    records.sort_by(|a, b| {
        b.text
            .chars()
            .count()
            .cmp(&a.text.chars().count())
            .then(a.id.cmp(&b.id))
    });
    records.truncate(4);
    if records.len() != 4 || records.iter().any(|r| r.text.chars().count() < 6000) {
        return Err("requires at least four public abstracts of >=6000 characters".into());
    }
    let inputs = records
        .iter()
        .map(|r| {
            json!({"article_id":r.id,"abstract_chars":r.text.chars().count(),
        "abstract_sha256":format!("{:x}",Sha256::digest(r.text.as_bytes()))})
        })
        .collect::<Vec<_>>();
    let mut articles = records
        .into_iter()
        .map(|r| {
            let mut a = Article::new(r.id, r.title);
            a.abstract_text = Some(r.text);
            a
        })
        .collect::<Vec<_>>();
    let root = PathBuf::from(&args[1]);
    std::fs::create_dir(&root)?;
    let path = root.join("library.db").to_string_lossy().into_owned();
    let library = BibBase::open_with_search_engine(&path, SearchEngineMode::Tantivy).await?;
    library.upsert_articles(&articles).await?;
    let revision = library.search_index_status().await?.source_revision;
    let mut config = SemanticConfig {
        enabled: true,
        model: args[2].clone(),
        model_digest: args[3].clone(),
        batch_size: 4,
        inference: OllamaInferenceOptions {
            num_ctx: Some(128),
            num_batch: Some(128),
        },
        ..Default::default()
    };
    // BGE has no task instruction requirement. This probe does not change the
    // Qwen/claim-retrieval parameters frozen in the relevance experiment.
    config.profile.query_instruction.clear();
    let rejected_epoch = library.configure_semantic_search(config.clone()).await?;
    let (rejected, rejected_wait) =
        wait(&library, &rejected_epoch, revision, SemanticPhase::Failed).await?;
    assert!(rejected.generation.is_none());
    assert!(
        rejected
            .last_error
            .as_deref()
            .is_some_and(|error| error.contains("400 Bad Request")),
        "the small-context probe must observe an input rejection, not an unrelated outage"
    );
    assert!(library.search_v2(request()).await.is_err());
    eprintln!("Over-limit complete abstract rejected; lexical search remains available");

    config.inference = OllamaInferenceOptions {
        num_ctx: Some(8192),
        num_batch: Some(8192),
    };
    let good_epoch = library.configure_semantic_search(config.clone()).await?;
    let (whole, whole_wait) = wait(&library, &good_epoch, revision, SemanticPhase::Ready).await?;
    assert_eq!(whole.indexed_chunks, 4);
    let first = library.search_v2(request()).await?;
    assert_eq!(first.retrieval.as_ref().unwrap().eligible_articles, 4);
    assert_eq!(first.hits.len(), 2);

    let mut changed = config.clone();
    changed.inference.num_batch = Some(4096);
    let changed_epoch = library.configure_semantic_search(changed).await?;
    let (changed, changed_wait) =
        wait(&library, &changed_epoch, revision, SemanticPhase::Ready).await?;
    assert_ne!(whole.signature, changed.signature);
    assert_ne!(whole.generation, changed.generation);
    assert_eq!(changed.progress.as_ref().unwrap().embedded_chunks, 4);
    let mut old_cursor = request();
    old_cursor.cursor = first.next_cursor.clone();
    assert!(old_cursor.cursor.is_some());
    assert!(library.search_v2(old_cursor).await.is_err());

    let restored_epoch = library.configure_semantic_search(config.clone()).await?;
    let (restored, restored_wait) =
        wait(&library, &restored_epoch, revision, SemanticPhase::Ready).await?;
    assert_eq!(whole.signature, restored.signature);
    assert_eq!(whole.generation, restored.generation);
    assert_eq!(restored.progress.as_ref().unwrap().embedded_chunks, 0);
    eprintln!(
        "Token-batch change isolated cache; restoring original options reused its generation"
    );

    config.profile.chunk_chars = Some(1000);
    config.profile.overlap_chars = 100;
    let window_epoch = library.configure_semantic_search(config.clone()).await?;
    let (windowed, window_wait) =
        wait(&library, &window_epoch, revision, SemanticPhase::Ready).await?;
    assert!(windowed.indexed_chunks > 4);
    assert_ne!(windowed.signature, whole.signature);
    let page = library.search_v2(request()).await?;
    let evidence = library
        .search_evidence(
            &page.hits[0].article_id,
            SearchEvidenceRequest {
                search: request(),
                index_generation: Some(page.index_generation.clone()),
            },
        )
        .await?;
    let original = articles
        .iter()
        .find(|a| a.id == page.hits[0].article_id)
        .unwrap();
    let canonical = format!(
        "{}\n{}",
        original.title,
        original.abstract_text.as_ref().unwrap()
    );
    assert!(
        evidence
            .evidence
            .iter()
            .any(|row| row.semantic_evidence.is_some())
    );
    for row in &evidence.evidence {
        if row.semantic_evidence.is_some() {
            assert!(
                canonical.contains(&row.snippet),
                "semantic evidence must be verbatim canonical text"
            );
        }
    }

    articles[0].year = Some(2026);
    library.upsert_article(&articles[0]).await?;
    let revision = library.search_index_status().await?.source_revision;
    let (metadata, metadata_wait) =
        wait(&library, &window_epoch, revision, SemanticPhase::Ready).await?;
    assert_eq!(metadata.progress.as_ref().unwrap().embedded_chunks, 0);
    let mut restricted = request();
    restricted.query = "year:2026".into();
    let filtered = library.search_v2(restricted).await?;
    assert_eq!(filtered.total, 1);
    assert_eq!(filtered.hits[0].article_id, articles[0].id);

    // This suffix is explicitly a controlled mutation, not part of the public source.
    articles[0]
        .abstract_text
        .as_mut()
        .unwrap()
        .push_str("\nControlled integration fixture content-replacement suffix.");
    library.upsert_article(&articles[0]).await?;
    let revision = library.search_index_status().await?.source_revision;
    let (replacement, replacement_wait) =
        wait(&library, &window_epoch, revision, SemanticPhase::Ready).await?;
    assert!(replacement.progress.as_ref().unwrap().embedded_chunks > 0);
    library.delete_article(&articles[1].id).await?;
    let revision = library.search_index_status().await?.source_revision;
    let (deletion, deletion_wait) =
        wait(&library, &window_epoch, revision, SemanticPhase::Ready).await?;
    let exported = library.export_search(request()).await?;
    assert_eq!(exported.len(), 3);
    assert!(exported.iter().all(|a| a.id != articles[1].id));
    drop(library);
    let reopened = BibBase::open_with_search_engine(&path, SearchEngineMode::Tantivy).await?;
    let (restart, restart_wait) =
        wait(&reopened, &window_epoch, revision, SemanticPhase::Ready).await?;
    assert_eq!(restart.config, config);
    assert_eq!(restart.generation, deletion.generation);
    assert_eq!(reopened.export_search(request()).await?.len(), 3);
    let receipt = json!({"schema":"bib-long-input-probe-v1","relevance_evaluation":false,
        "corpus_sha256":format!("{:x}",Sha256::digest(&bytes)),"inputs":inputs,
        "over_limit":{"status":rejected,"observations":rejected_wait},
        "whole":{"status":whole,"observations":whole_wait},
        "changed_token_batch":{"status":changed,"observations":changed_wait},
        "restored":{"status":restored,"observations":restored_wait},
        "windowed":{"status":windowed,"observations":window_wait},
        "metadata":{"status":metadata,"observations":metadata_wait},
        "replacement":{"status":replacement,"observations":replacement_wait},
        "deletion":{"status":deletion,"observations":deletion_wait},
        "restart":{"status":restart,"observations":restart_wait},
        "semantic_evidence_verbatim":true,"old_cursor_rejected":true,"deleted_absent_from_export":true,
        "limitations":["Four long public abstract records, not full papers or judged quality",
            "Only the replacement suffix is synthetic and explicitly marked",
            "Single local installed-model lifecycle, not capacity percentiles or GUI/native qualification"]});
    std::fs::write(
        root.join("receipt.json"),
        serde_json::to_vec_pretty(&receipt)?,
    )?;
    println!("{}", receipt);
    Ok(())
}
