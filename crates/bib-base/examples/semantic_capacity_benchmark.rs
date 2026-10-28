//! Synthetic index capacity, not model throughput or relevance qualification.
//! semantic_capacity_benchmark FRESH_DIRECTORY COUNT
use async_trait::async_trait;
use bib_base::{
    BibBase, SearchEngineMode,
    semantic_index::{SemanticIndex, SemanticProfile},
};
use bib_types::Article;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
    sync::Arc,
    time::Instant,
};
use text_embedding::{EmbeddingInput, EmbeddingOutput, TextEmbedder};

struct Fixture;
#[async_trait]
impl TextEmbedder for Fixture {
    fn signature(&self) -> String {
        "capacity-fixture-sha256-1024-v1".into()
    }
    async fn embed(
        &self,
        input: &EmbeddingInput,
    ) -> text_embedding::provider::error::Result<EmbeddingOutput> {
        let digest = Sha256::digest(input.text.as_bytes());
        let mut seed = u64::from_le_bytes(digest[..8].try_into().unwrap()).max(1);
        let vector = (0..1024)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                (seed as u32 as f64 / u32::MAX as f64 * 2. - 1.) as f32
            })
            .collect();
        Ok(EmbeddingOutput::new(self.signature(), vector))
    }
}
fn memory() -> Value {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let read = |key: &str| {
        status
            .lines()
            .find(|line| line.starts_with(key))
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|v| v.parse::<u64>().ok())
    };
    json!({"rss_kib":read("VmRSS:"),"peak_rss_kib":read("VmHWM:")})
}
fn files(root: &Path) -> BTreeMap<String, u64> {
    std::fs::read_dir(root)
        .unwrap()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            (name.starts_with("vectors.db") && entry.file_type().ok()?.is_file())
                .then(|| (name, entry.metadata().unwrap().len()))
        })
        .collect()
}
fn article(i: usize) -> Article {
    let mut article = Article::new(
        format!("capacity-{i:09}"),
        format!("Immune regulatory genetics study {i}"),
    );
    article.abstract_text = Some(format!("Study {i} investigates gene expression, cellular regulation and reproducible evidence in human cohorts. ").repeat(8));
    article.year = Some(2000 + (i % 27) as u16);
    article
}
fn elapsed(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.
}
fn distribution(mut values: Vec<f64>) -> Value {
    values.sort_by(f64::total_cmp);
    json!({"samples":values.len(),"p50_ms":values[values.len()/2],"p95_ms":values[(values.len()*95).div_ceil(100)-1],"max_ms":values.last()})
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 2 {
        return Err("usage: semantic_capacity_benchmark FRESH_DIRECTORY COUNT".into());
    }
    let count: usize = args[1].parse()?;
    if count == 0 {
        return Err("count must be positive".into());
    }
    let root = Path::new(&args[0]);
    std::fs::create_dir(root)?;
    let database = root.join("library.db").to_string_lossy().into_owned();
    let sidecar = root.join("vectors.db").to_string_lossy().into_owned();
    let library = BibBase::open_with_search_engine(&database, SearchEngineMode::Tantivy).await?;
    let start = Instant::now();
    for batch in (0..count).collect::<Vec<_>>().chunks(500) {
        library
            .upsert_articles(&batch.iter().map(|i| article(*i)).collect::<Vec<_>>())
            .await?;
    }
    let import_ms = elapsed(start);
    eprintln!("imported {count} articles in {import_ms:.0}ms");
    let source = turso::Builder::new_local(&database)
        .experimental_multiprocess_wal(true)
        .build()
        .await?;
    let provider = Arc::new(Fixture);
    let profile = SemanticProfile::default();
    let start = Instant::now();
    let mut index = SemanticIndex::open(
        source.connect()?,
        &sidecar,
        profile.clone(),
        provider.clone(),
    )
    .await?;
    let (snapshot, initial) = index
        .sync_with_progress(false, |p| {
            if p.processed_articles > 0 && p.processed_articles % 10_000 == 0 {
                eprintln!(
                    "committed {} articles, {} chunks",
                    p.processed_articles, p.processed_chunks
                );
            }
        })
        .await?;
    let initial_ms = elapsed(start);
    assert_eq!(snapshot.chunks.len(), count);
    assert_eq!(initial.embedded_chunks, count);
    let built_memory = memory();
    let query = provider
        .embed(&profile.query_input("immune cellular expression")?)
        .await?
        .vector;
    let mut rankings = BTreeMap::new();
    for (name, stride) in [("all", 1), ("one_percent", 100)] {
        let allowed: HashSet<_> = (0..count).step_by(stride).map(|i| article(i).id).collect();
        let start = Instant::now();
        let first = snapshot.rank(&query, &allowed, 100)?;
        let first_ms = elapsed(start);
        assert_eq!(first.len(), allowed.len().min(100));
        assert!(first.iter().all(|hit| allowed.contains(&hit.0.article_id)));
        let mut samples = Vec::new();
        for _ in 0..30 {
            let start = Instant::now();
            let repeated = snapshot.rank(&query, &allowed, 100)?;
            assert_eq!(
                repeated
                    .iter()
                    .map(|hit| (&hit.0.article_id, hit.1))
                    .collect::<Vec<_>>(),
                first
                    .iter()
                    .map(|hit| (&hit.0.article_id, hit.1))
                    .collect::<Vec<_>>()
            );
            samples.push(elapsed(start));
        }
        rankings.insert(name,json!({"allowed_articles":allowed.len(),"first_observed_ms":first_ms,"hot":distribution(samples)}));
    }
    let start = Instant::now();
    let (idle, _) = index.sync().await?;
    let idle_ms = elapsed(start);
    assert!(Arc::ptr_eq(&snapshot, &idle));
    drop(idle);
    let mut changed = article(0);
    changed.year = Some(2026);
    library.upsert_article(&changed).await?;
    let start = Instant::now();
    let (updated, metadata) = index.sync().await?;
    let metadata_ms = elapsed(start);
    assert_eq!(metadata.embedded_chunks, 0);
    assert_eq!(metadata.cached_chunks, 1);
    let dual_generation_memory = memory();
    drop(snapshot);
    drop(updated);
    drop(index);
    let start = Instant::now();
    let mut reopened = SemanticIndex::open(source.connect()?, &sidecar, profile, provider).await?;
    let (restored, restart) = reopened.sync().await?;
    let reopen_ms = elapsed(start);
    assert_eq!(restart.embedded_chunks, 0);
    assert_eq!(restored.chunks.len(), count);
    let store_files = files(root);
    let receipt = json!({"schema":"bib-semantic-capacity-v1","count":count,"dimensions":1024,
        "synthetic_provider":true,"relevance_evaluation":false,"model_throughput_evaluation":false,
        "build_profile":if cfg!(debug_assertions){"debug"}else{"release"},
        "import_ms":import_ms,"initial_ms":initial_ms,"initial":initial,"rankings":rankings,
        "idle_ms":idle_ms,"idle_snapshot_reused":true,"metadata_ms":metadata_ms,"metadata":metadata,
        "reopen_ms":reopen_ms,"restart":restart,"built_memory":built_memory,"dual_generation_memory":dual_generation_memory,
        "final_memory":memory(),"vector_store_total_bytes":store_files.values().sum::<u64>(),"vector_store_files":store_files,
        "limitations":["Direct SemanticIndex exercise, excludes HTTP/GUI, hard-filter computation and model inference","First observed query is after index construction; not operating-system cold cache","One title/abstract chunk per article; no fulltext fanout","Single query worker, no concurrency SLO claim"]});
    std::fs::write(
        root.join("receipt.json"),
        serde_json::to_vec_pretty(&receipt)?,
    )?;
    println!("{receipt}");
    Ok(())
}
