//! Full-corpus production lexical/hybrid relevance runs, with durable import/cache.
//! semantic_relevance DATASET DIRECTORY MODEL DIGEST lexical|hybrid
//! Reads corpus.jsonl and queries.jsonl only; never reads relevance judgments.
use bib_base::{
    BibBase, SearchEngineMode,
    semantic_service::{SemanticConfig, SemanticPhase},
};
use bib_types::{Article, SearchMode, SearchRequestV2};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    io::{BufRead, BufReader, BufWriter, Read, Write},
    path::Path,
    time::{Duration, Instant},
};
type Error = Box<dyn std::error::Error + Send + Sync>;
type Result<T> = std::result::Result<T, Error>;
#[derive(Deserialize)]
struct Record {
    #[serde(rename = "_id")]
    id: String,
    #[serde(default)]
    title: String,
    text: String,
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn file_hash(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let read = file.read(&mut buf)?;
        if read == 0 {
            break;
        }
        hash.update(&buf[..read]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn atomic_json(path: &Path, value: &Value) -> Result<()> {
    let temporary = path.with_extension("json.tmp");
    let mut file = std::fs::File::create(&temporary)?;
    file.write_all(&serde_json::to_vec_pretty(value)?)?;
    file.sync_all()?;
    std::fs::rename(temporary, path)?;
    Ok(())
}
fn memory() -> Value {
    let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let read = |key: &str| {
        s.lines()
            .find(|l| l.starts_with(key))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<u64>().ok())
    };
    json!({"rss_kib":read("VmRSS:"),"peak_rss_kib":read("VmHWM:")})
}
#[tokio::main]
async fn main() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 5 || !matches!(args[4].as_str(), "lexical" | "hybrid") {
        return Err(
            "usage: semantic_relevance DATASET DIRECTORY MODEL DIGEST lexical|hybrid".into(),
        );
    }
    let dataset = Path::new(&args[0]);
    let root = Path::new(&args[1]);
    let mode = &args[4];
    let config = SemanticConfig {
        enabled: true,
        model: args[2].clone(),
        model_digest: args[3].clone(),
        ..Default::default()
    };
    config.validate()?;
    let identity = json!({"schema":"bib-production-relevance-v1","corpus_sha256":file_hash(&dataset.join("corpus.jsonl"))?,
        "queries_sha256":file_hash(&dataset.join("queries.jsonl"))?,"model":config.model,"digest":config.model_digest,"profile":config.profile,"inference":config.inference,
        "tool_source_sha256":hash(include_bytes!("semantic_relevance.rs")),
        "ranking_source_sha256":hash(include_bytes!("../src/hybrid_search.rs")),
        "index_source_sha256":hash(include_bytes!("../src/semantic_index.rs")),
        "local_source_sha256":hash(include_bytes!("../src/local_index.rs"))});
    if root.exists() {
        let existing: Value =
            serde_json::from_slice(&std::fs::read(root.join("input-identity.json"))?)?;
        if existing != identity {
            return Err("existing run input/model/code identity mismatch".into());
        }
    } else {
        std::fs::create_dir(root)?;
        atomic_json(&root.join("input-identity.json"), &identity)?;
    }
    if root.join(format!("{mode}.jsonl")).exists() {
        return Err("completed mode output exists; select the next phase instead of overwriting a frozen run".into());
    }
    let library = BibBase::open_with_search_engine(
        root.join("library.db")
            .to_str()
            .ok_or("invalid database path")?,
        SearchEngineMode::Tantivy,
    )
    .await?;
    let checkpoint = root.join("import-progress.json");
    let completed = if checkpoint.exists() {
        serde_json::from_slice::<Value>(&std::fs::read(&checkpoint)?)?["imported"]
            .as_u64()
            .ok_or("invalid import checkpoint")? as usize
    } else {
        0
    };
    let started = Instant::now();
    let mut count = 0;
    let mut chunks = 0;
    let mut batch = Vec::new();
    let mut ids = HashSet::new();
    for line in BufReader::new(std::fs::File::open(dataset.join("corpus.jsonl"))?).lines() {
        let row: Record = serde_json::from_str(&line?)?;
        if row.id.is_empty() || !ids.insert(row.id.clone()) {
            return Err("empty or duplicate corpus ID".into());
        }
        count += 1;
        chunks += usize::from(!row.title.trim().is_empty() || !row.text.trim().is_empty());
        if count <= completed {
            continue;
        }
        if mode == "hybrid" {
            return Err("complete lexical/import phase before enabling hybrid".into());
        }
        let mut article = Article::new(row.id, row.title);
        article.abstract_text = Some(row.text);
        batch.push(article);
        if batch.len() == 500 {
            library.upsert_articles(&batch).await?;
            batch.clear();
            atomic_json(&checkpoint, &json!({"imported":count}))?;
            if count % 10000 == 0 {
                eprintln!(
                    "imported {count} documents ({:.1}s)",
                    started.elapsed().as_secs_f64()
                );
            }
        }
    }
    if !batch.is_empty() {
        library.upsert_articles(&batch).await?;
        atomic_json(&checkpoint, &json!({"imported":count}))?;
    }
    if count == 0 || completed > count {
        return Err("empty corpus or checkpoint beyond corpus".into());
    }
    drop(ids);
    let import_ms = started.elapsed().as_secs_f64() * 1000.;
    let mut query_ids = HashSet::new();
    let mut queries = Vec::new();
    for line in BufReader::new(std::fs::File::open(dataset.join("queries.jsonl"))?).lines() {
        let row: Record = serde_json::from_str(&line?)?;
        if row.id.is_empty() || row.text.trim().is_empty() || !query_ids.insert(row.id.clone()) {
            return Err("invalid or duplicate query".into());
        }
        queries.push(row);
    }
    if queries.is_empty() {
        return Err("empty query list".into());
    }
    let start = Instant::now();
    if mode == "lexical" {
        library.rebuild_search_index().await?;
    }
    let index_ms = start.elapsed().as_secs_f64() * 1000.;
    let initial = library.search_v2(SearchRequestV2::default()).await?;
    assert_eq!(
        initial.total, count,
        "entire corpus must be indexed, without judgment-based document filtering"
    );
    let source_revision = library.search_index_status().await?.source_revision;
    let start = Instant::now();
    if mode == "hybrid" {
        if !root.join("lexical.jsonl").is_file() {
            return Err("lexical baseline must be completed first".into());
        }
        library.configure_semantic_search(config.clone()).await?;
        let mut last_report = Instant::now();
        loop {
            let status = library.semantic_search_status()?;
            if status.phase == SemanticPhase::Failed {
                atomic_json(
                    &root.join("semantic-failure.json"),
                    &serde_json::to_value(&status)?,
                )?;
                return Err(status.last_error.unwrap_or_default().into());
            }
            if status.phase == SemanticPhase::Ready
                && status.indexed_revision == Some(source_revision)
            {
                assert_eq!(status.indexed_chunks, chunks);
                break;
            }
            if last_report.elapsed() >= Duration::from_secs(10) {
                eprintln!(
                    "semantic progress {}",
                    serde_json::to_string(&status.progress)?
                );
                atomic_json(
                    &root.join("semantic-progress.json"),
                    &serde_json::to_value(&status)?,
                )?;
                last_report = Instant::now();
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
    let ready_ms = start.elapsed().as_secs_f64() * 1000.;
    let partial = root.join(format!("{mode}.jsonl.partial"));
    let mut writer = BufWriter::new(std::fs::File::create(&partial)?);
    let mut timings = Vec::new();
    for row in &queries {
        let req = if mode == "hybrid" {
            SearchRequestV2 {
                mode: SearchMode::Hybrid,
                semantic_query: Some(row.text.clone()),
                page_size: 100,
                ..Default::default()
            }
        } else {
            SearchRequestV2 {
                expression: Some(bib_base::topic_search_expression(&row.text)?),
                page_size: 100,
                ..Default::default()
            }
        };
        let start = Instant::now();
        let page = library.search_v2(req).await?;
        if mode == "hybrid" {
            let retrieval = page
                .retrieval
                .as_ref()
                .ok_or("missing production retrieval provenance")?;
            assert_eq!(retrieval.eligible_articles, count);
            assert_eq!(retrieval.indexed_chunks, chunks);
            assert_eq!(retrieval.rrf_constant, 10);
            assert_eq!(retrieval.semantic_weight, 4.);
            assert_eq!(retrieval.candidate_depth, 100);
        }
        timings.push(start.elapsed().as_secs_f64() * 1000.);
        let hits = page
            .hits
            .iter()
            .map(|hit| json!({"article_id":hit.article_id,"score":hit.score}))
            .collect::<Vec<_>>();
        writeln!(writer, "{}", json!({"query_id":row.id,"hits":hits}))?;
        writer.flush()?;
        eprintln!(
            "{mode} query {}/{} {:.0}ms",
            timings.len(),
            queries.len(),
            timings.last().unwrap()
        );
    }
    writer.flush()?;
    writer.get_ref().sync_all()?;
    drop(writer);
    let output = root.join(format!("{mode}.jsonl"));
    std::fs::rename(partial, &output)?;
    timings.sort_by(f64::total_cmp);
    let receipt = json!({"schema":"bib-production-relevance-run-v1","mode":mode,"corpus_documents":count,"expected_chunks":chunks,"queries":queries.len(),"identity":identity,"source_revision":source_revision,
        "import_or_resume_scan_ms":import_ms,"lexical_rebuild_ms":index_ms,"semantic_ready_ms":ready_ms,
        "query_latency":{"samples":timings.len(),"p50_ms":timings[timings.len()/2],"p95_ms":timings[(timings.len()*95).div_ceil(100)-1],"max_ms":timings.last()},
        "memory":memory(),"run_sha256":file_hash(&output)?,"planned_hybrid_configuration":config,"semantic_status":library.semantic_search_status()?,
        "judgments_read":false,"relevance_score_computed":false,"transport":"production actor/service; excludes GUI and client HTTP overhead"});
    atomic_json(&root.join(format!("{mode}.receipt.json")), &receipt)?;
    println!("{}", serde_json::to_string(&receipt)?);
    Ok(())
}
