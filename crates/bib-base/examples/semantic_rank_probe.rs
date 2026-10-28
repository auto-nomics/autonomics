//! Exact ranking microbenchmark; no DB/API/model/relevance or end-to-end SLO claim.
//! semantic_rank_probe FRESH_ROOT ARTICLE_COUNT CHUNKS_PER_ARTICLE DIMENSIONS SAMPLES
use bib_base::semantic_index::{SemanticChunk, SemanticSnapshot};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
    time::Instant,
};
fn normalize(vector: &[f32]) -> Vec<f32> {
    let norm = vector
        .iter()
        .map(|v| f64::from(*v).powi(2))
        .sum::<f64>()
        .sqrt();
    vector
        .iter()
        .map(|v| (f64::from(*v) / norm) as f32)
        .collect()
}
// Frozen pre-optimization production algorithm, including cloning every
// article winner before the full sort. Its source is archived with the run.
fn baseline(
    snapshot: &SemanticSnapshot,
    query: &[f32],
    allowed: &HashSet<String>,
    limit: usize,
) -> Vec<(SemanticChunk, f64)> {
    let query = normalize(query);
    let mut matches: HashMap<&str, (&SemanticChunk, f64)> = HashMap::new();
    for chunk in &snapshot.chunks {
        if !allowed.contains(&chunk.article_id) {
            continue;
        }
        let score = chunk
            .vector
            .iter()
            .zip(&query)
            .map(|(a, b)| f64::from(*a) * f64::from(*b))
            .sum::<f64>();
        let entry = matches.entry(&chunk.article_id).or_insert((chunk, score));
        if score > entry.1 || (score == entry.1 && chunk.chunk_id < entry.0.chunk_id) {
            *entry = (chunk, score);
        }
    }
    let mut result = matches
        .into_values()
        .map(|(chunk, score)| (chunk.clone(), score))
        .collect::<Vec<_>>();
    result.sort_by(|a, b| {
        b.1.total_cmp(&a.1)
            .then_with(|| a.0.article_id.cmp(&b.0.article_id))
    });
    result.truncate(limit);
    result
}
fn distribution(mut samples: Vec<f64>) -> Value {
    samples.sort_by(f64::total_cmp);
    json!({"samples":samples.len(),"p50_ms":samples[samples.len()/2],"p95_ms":samples[(samples.len()*95).div_ceil(100)-1],"max_ms":samples.last()})
}
fn run(
    snapshot: &SemanticSnapshot,
    query: &[f32],
    allowed: &HashSet<String>,
    old: bool,
) -> (f64, Vec<(SemanticChunk, f64)>) {
    let start = Instant::now();
    let hits = if old {
        baseline(snapshot, query, allowed, 100)
    } else {
        snapshot.rank(query, allowed, 100).unwrap()
    };
    (start.elapsed().as_secs_f64() * 1000., hits)
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 5 {
        return Err(
            "usage: semantic_rank_probe FRESH_ROOT ARTICLES CHUNKS DIMENSIONS SAMPLES".into(),
        );
    }
    let root = PathBuf::from(&args[0]);
    let count = args[1].parse::<usize>()?;
    let parts = args[2].parse::<usize>()?;
    let dimensions = args[3].parse::<usize>()?;
    let samples = args[4].parse::<usize>()?;
    if count == 0
        || count > 100000
        || parts == 0
        || parts > 7
        || dimensions == 0
        || dimensions > 1024
        || !(2..=100).contains(&samples)
    {
        return Err("invalid bounded probe geometry".into());
    }
    std::fs::create_dir(&root)?;
    let mut seed = 71_u64;
    let mut vector = || {
        normalize(
            &(0..dimensions)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    (seed as u32 as f64 / u32::MAX as f64 * 2. - 1.) as f32
                })
                .collect::<Vec<_>>(),
        )
    };
    let mut chunks = Vec::with_capacity(count * parts);
    for n in 0..count {
        let first = Arc::new(vector());
        for part in 0..parts {
            chunks.push(SemanticChunk {
                chunk_id: format!("chunk-{n:09}-{part}"),
                doc_id: format!("doc-{n:09}-{part}"),
                article_id: format!("article-{n:09}"),
                anchor_key: format!("attachment-{part}"),
                kind: if part == 0 { "item" } else { "attachment" }.into(),
                content_hash: format!("content-hash-{n:09}-{part}"),
                start_byte: part * 512,
                end_byte: (part + 1) * 512,
                vector: if part == 0 {
                    first.clone()
                } else {
                    Arc::new(vector())
                },
            });
        }
    }
    let snapshot = SemanticSnapshot {
        signature: "synthetic-rank-probe".into(),
        generation: "probe".into(),
        revision: 1,
        token: "probe".into(),
        dimensions,
        chunks,
    };
    let query = vector();
    let mut cases = serde_json::Map::new();
    for stride in [1, 100] {
        let allowed = (0..count)
            .step_by(stride)
            .map(|n| format!("article-{n:09}"))
            .collect::<HashSet<_>>();
        let mut old_samples = Vec::new();
        let mut new_samples = Vec::new();
        for round in 0..samples + 1 {
            let (old, new) = if round % 2 == 0 {
                (
                    run(&snapshot, &query, &allowed, true),
                    run(&snapshot, &query, &allowed, false),
                )
            } else {
                let new = run(&snapshot, &query, &allowed, false);
                (run(&snapshot, &query, &allowed, true), new)
            };
            assert_eq!(old.1.len(), new.1.len());
            for ((a, x), (b, y)) in old.1.iter().zip(&new.1) {
                assert_eq!(a.chunk_id, b.chunk_id);
                assert_eq!(x.to_bits(), y.to_bits());
                assert!(Arc::ptr_eq(&a.vector, &b.vector));
            }
            if round > 0 {
                old_samples.push(old.0);
                new_samples.push(new.0);
            }
            eprintln!(
                "stride={stride} round={round} old_ms={:.3} new_ms={:.3}",
                old.0, new.0
            );
        }
        cases.insert(format!("stride_{stride}"),json!({"eligible_articles":allowed.len(),"baseline":distribution(old_samples),"optimized":distribution(new_samples),"all_compared_results_bit_identical":true}));
    }
    let receipt = json!({"schema":"bib-semantic-rank-probe-v1","build_profile":if cfg!(debug_assertions){"debug"}else{"release"},"articles":count,"chunks":count*parts,"dimensions":dimensions,"synthetic_vectors":true,"same_vectors_and_process":true,"vector_storage":"one distinct allocation per chunk","available_parallelism":std::thread::available_parallelism().map_or(1,|n|n.get()),"ranking_thread_cap":4,"alternating_order":true,"warmup_pairs_excluded":1,"cases":cases,"scope":"Ranking only; no service/API/GUI latency or model relevance claim"});
    std::fs::write(
        root.join("receipt.json"),
        serde_json::to_vec_pretty(&receipt)?,
    )?;
    println!("{receipt}");
    Ok(())
}
