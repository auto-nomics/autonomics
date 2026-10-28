//! Production actor/service capacity against an explicit local HTTP fixture.
//! semantic_service_benchmark FRESH_DIRECTORY COUNT FIXTURE_ORIGIN
//! No model relevance, actual inference speed or HTTP/GUI SLO claim.
use bib_base::{
    BibBase, Error, SearchEngineMode,
    semantic_service::{
        SemanticConfig, SemanticMaintenanceAction, SemanticMaintenanceRequest, SemanticPhase,
    },
};
use bib_types::{Article, SearchMode, SearchRequestV2};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

fn elapsed(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.
}
fn distribution(mut samples: Vec<f64>) -> Value {
    samples.sort_by(f64::total_cmp);
    json!({"samples":samples.len(),"p50_ms":samples[samples.len()/2],"p95_ms":samples[(samples.len()*95).div_ceil(100)-1],"max_ms":samples.last()})
}
fn article(i: usize) -> Article {
    let mut a = Article::new(
        format!("service-{i:09}"),
        format!("Immune genetics study {i} cohortg{:03}", i % 100),
    );
    a.abstract_text=Some(format!("Study {i} evaluates immune gene expression and cellular regulation in replicated human cohorts. ").repeat(8));
    a.year = Some(2000 + (i % 27) as u16);
    a
}
fn request(narrow: bool) -> SearchRequestV2 {
    SearchRequestV2 {
        mode: SearchMode::Hybrid,
        semantic_query: Some("immune gene expression".into()),
        query: if narrow {
            "title:cohortg001".into()
        } else {
            String::new()
        },
        include_facets: true,
        include_articles: true,
        page_size: 20,
        ..Default::default()
    }
}
async fn query(library: &BibBase, request: SearchRequestV2, eligible: usize) -> Result<f64, Error> {
    let start = Instant::now();
    let page = library.search_v2(request).await?;
    assert_eq!(page.retrieval.as_ref().unwrap().eligible_articles, eligible);
    assert!(!page.hits.is_empty());
    assert!(page.total <= 200);
    assert_eq!(page.articles.len(), page.hits.len());
    assert!(page.facets.is_some());
    Ok(elapsed(start))
}
async fn ready(library: &BibBase, revision: u64) -> Result<usize, Box<dyn std::error::Error>> {
    tokio::time::timeout(Duration::from_secs(600), async {
        let mut conflict_polls = 0;
        loop {
            let s = library.semantic_search_status()?;
            if s.phase == SemanticPhase::Failed {
                let error = s.last_error.unwrap_or_default();
                if !error.starts_with("search conflict:") {
                    return Err(error.into());
                }
                conflict_polls += 1;
            }
            if s.phase == SemanticPhase::Ready && s.indexed_revision == Some(revision) {
                return Ok(conflict_polls);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await?
}
fn memory() -> Value {
    let text = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let read = |key: &str| {
        text.lines()
            .find(|l| l.starts_with(key))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<u64>().ok())
    };
    json!({"rss_kib":read("VmRSS:"),"peak_rss_kib":read("VmHWM:")})
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let reuse = args.len() == 4 && args[3] == "--reuse-existing";
    if args.len() != 3 && !reuse {
        return Err(
            "usage: semantic_service_benchmark DIRECTORY COUNT FIXTURE_ORIGIN [--reuse-existing]"
                .into(),
        );
    }
    let count: usize = args[1].parse()?;
    if count < 100 || count % 100 != 0 {
        return Err("count must be a positive multiple of 100".into());
    }
    let root = Path::new(&args[0]);
    if reuse {
        if !root.join("library.db").is_file() {
            return Err("reuse requires a copied fixture database".into());
        }
    } else {
        std::fs::create_dir(root)?;
    }
    let path = root.join("library.db").to_string_lossy().into_owned();
    let library =
        Arc::new(BibBase::open_with_search_engine(&path, SearchEngineMode::Tantivy).await?);
    let start = Instant::now();
    let import_ms = if reuse {
        assert_eq!(
            library.search_v2(SearchRequestV2::default()).await?.total,
            count
        );
        eprintln!("reopened copied fixture with {count} articles");
        None
    } else {
        for batch in (0..count).collect::<Vec<_>>().chunks(500) {
            library
                .upsert_articles(&batch.iter().map(|i| article(*i)).collect::<Vec<_>>())
                .await?;
        }
        eprintln!("imported {count} articles");
        Some(elapsed(start))
    };
    let revision = library.search_index_status().await?.source_revision;
    let start = Instant::now();
    library
        .configure_semantic_search(SemanticConfig {
            enabled: true,
            origin: args[2].clone(),
            model: "fixture:latest".into(),
            model_digest: "a".repeat(64),
            ..Default::default()
        })
        .await?;
    let initial_conflict_polls = ready(&library, revision).await?;
    let initial_ready_ms = elapsed(start);
    eprintln!("{count} vectors ready in {initial_ready_ms:.0}ms");
    let built_memory = memory();
    let mut single = BTreeMap::new();
    for (name, narrow) in [("wide", false), ("one_percent", true)] {
        let expected = if narrow { count / 100 } else { count };
        let first = query(&library, request(narrow), expected).await?;
        let mut samples = Vec::new();
        for _ in 0..30 {
            samples.push(query(&library, request(narrow), expected).await?);
        }
        single.insert(
            name,
            json!({"first_observed_ms":first,"hot":distribution(samples)}),
        );
    }
    let start = Instant::now();
    let barrier = Arc::new(tokio::sync::Barrier::new(4));
    let mut workers = Vec::new();
    for _ in 0..4 {
        let library = library.clone();
        let barrier = barrier.clone();
        workers.push(tokio::spawn(async move {
            barrier.wait().await;
            let mut samples = Vec::new();
            for _ in 0..5 {
                samples.push(query(&library, request(false), count).await?);
            }
            Ok::<_, Error>(samples)
        }));
    }
    let mut concurrent = Vec::new();
    for worker in workers {
        concurrent.extend(worker.await??);
    }
    let concurrent_wall_ms = elapsed(start);
    let mut writes = Vec::new();
    let mut lexical = Vec::new();
    for i in 0..10 {
        let mut changed = article(i);
        changed.year = Some(if reuse { 2025 } else { 2026 });
        let start = Instant::now();
        library.upsert_article(&changed).await?;
        writes.push(elapsed(start));
        let start = Instant::now();
        let page = library
            .search_v2(SearchRequestV2 {
                query: "title:cohortg001".into(),
                page_size: 5,
                ..Default::default()
            })
            .await?;
        assert_eq!(page.total, count / 100);
        lexical.push(elapsed(start));
    }
    let revision = library.search_index_status().await?.source_revision;
    let start = Instant::now();
    let update_conflict_polls = ready(&library, revision).await?;
    let update_catchup_ms = elapsed(start);
    let status = library.semantic_search_status()?;
    let start = Instant::now();
    let job = library
        .maintain_semantic_search(SemanticMaintenanceRequest {
            action: SemanticMaintenanceAction::Rebuild,
            expected_configuration_generation: status.configuration_generation,
            request_id: uuid::Uuid::new_v4().to_string(),
        })
        .await?;
    let mut maintenance_hybrid = Vec::new();
    let mut maintenance_lexical = Vec::new();
    let mut conflicts = 0;
    for _ in 0..10 {
        match query(&library, request(false), count).await {
            Ok(ms) => maintenance_hybrid.push(ms),
            Err(Error::SearchConflict(_)) => conflicts += 1,
            Err(error) => return Err(error.into()),
        }
        let t = Instant::now();
        library
            .search_v2(SearchRequestV2 {
                query: "title:cohortg001".into(),
                page_size: 5,
                ..Default::default()
            })
            .await?;
        maintenance_lexical.push(elapsed(t));
    }
    let maintenance = tokio::time::timeout(Duration::from_secs(600), async {
        loop {
            let status = library.semantic_search_status()?;
            if let Some(current) = status
                .maintenance
                .filter(|m| m.request_id == job.request_id)
            {
                if current.state == "complete" {
                    return Ok::<_, Error>(current);
                }
                if current.state == "failed" {
                    return Err(Error::Unknown(current.last_error.unwrap_or_default()));
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await??;
    let rebuild_ms = elapsed(start);
    assert_eq!(maintenance.progress.as_ref().unwrap().embedded_chunks, 0);
    assert_eq!(maintenance.progress.as_ref().unwrap().cached_chunks, count);
    let files = std::fs::read_dir(root)?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            (name.starts_with("library.db.semantic-v1") && e.file_type().ok()?.is_file())
                .then(|| (name, e.metadata().unwrap().len()))
        })
        .collect::<BTreeMap<_, _>>();
    let receipt = json!({"schema":"bib-semantic-service-capacity-v1","count":count,"dimensions":1024,
        "build_profile":if cfg!(debug_assertions){"debug"}else{"release"},"controlled_http_model_fixture":true,"relevance_evaluation":false,"copied_fixture_reused":reuse,
        "import_ms":import_ms,"initial_ready_ms":initial_ready_ms,"initial_conflict_polls":initial_conflict_polls,"update_conflict_polls":update_conflict_polls,"built_memory":built_memory,"single_client":single,
        "four_clients":distribution(concurrent),"four_clients_wall_ms":concurrent_wall_ms,"writes_during_index_updates":distribution(writes),
        "lexical_during_index_updates":distribution(lexical),"update_catchup_ms":update_catchup_ms,
        "rebuild_ms":rebuild_ms,"rebuild_progress":maintenance.progress,"hybrid_during_rebuild":if maintenance_hybrid.is_empty(){Value::Null}else{distribution(maintenance_hybrid)},
        "hybrid_config_transition_conflicts":conflicts,"lexical_during_rebuild":distribution(maintenance_lexical),"final_memory":memory(),
        "vector_store_files":files,"vector_store_total_bytes":files.values().sum::<u64>(),
        "limitations":["Production BibBase service/actor path with local bounded model HTTP fixture; not HTTP API or GUI end-to-end","No actual model inference speed or relevance claim","First observed queries follow initial indexing, not OS-cold cache","One title/abstract chunk per article","Single host measurements; concurrent samples include queueing"]});
    std::fs::write(
        root.join("receipt.json"),
        serde_json::to_vec_pretty(&receipt)?,
    )?;
    println!("{receipt}");
    Ok(())
}
