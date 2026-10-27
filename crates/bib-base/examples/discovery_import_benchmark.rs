//! Measure the production importer against a private copy of a closed synthetic
//! search_benchmark fixture. Never writes to the supplied fixture directory.
//! cargo run --release -p bib-base --example discovery_import_benchmark --
//!   --fixture /tmp/bib-bench-100000-... --output report.json
use bib_base::BibBase;
use bib_types::{Article, Identifier, SearchRequestV2, discovery::*};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    time::Instant,
};
fn argument(name: &str, default: &str) -> String {
    std::env::args()
        .collect::<Vec<_>>()
        .windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].clone())
        .unwrap_or_else(|| default.into())
}
fn record(i: usize, existing: bool, session_id: &str) -> DiscoveryImportRecord {
    let mut article = Article::new(format!("source-{i}"), format!("Upstream title {i}"));
    article.identifiers.push(Identifier::doi(if existing {
        format!("HTTPS://DOI.ORG/10.9999/bench-{i:09}")
    } else {
        format!("10.9999/not-in-library-{i}")
    }));
    let selected = DiscoveryRecord {
        source: "fixture".into(),
        article,
        source_rank: i + 1,
        query_fingerprint: "fixture-query".into(),
        retrieved_at: "2026-10-28T00:00:00Z".into(),
        executed_parameters: [("query".into(), "synthetic benchmark".into())].into(),
        query_translation: None,
    };
    let records = vec![selected.clone()];
    DiscoveryImportRecord {
        schema_version: 1,
        session_id: session_id.into(),
        candidate_id: format!("candidate-{i}-{existing}"),
        fingerprint: format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&records).unwrap())
        ),
        selected,
        records,
        request: DiscoveryRequest {
            query: "synthetic benchmark".into(),
            sources: vec!["fixture".into()],
            ..Default::default()
        },
        plans: Vec::new(),
    }
}
fn stats(mut samples: Vec<f64>) -> Value {
    samples.sort_by(f64::total_cmp);
    let pick = |percent: f64| {
        samples[((samples.len() as f64 * percent).ceil() as usize).saturating_sub(1)]
    };
    json!({"samples":samples.len(),"p50_ms":pick(0.50),"p95_ms":pick(0.95),"max_ms":samples.last()})
}
fn rss() -> Value {
    let text = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let value = |key: &str| {
        text.lines()
            .find(|line| line.starts_with(key))
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|value| value.parse::<u64>().ok())
    };
    json!({"rss_kib":value("VmRSS:"),"peak_rss_kib":value("VmHWM:")})
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = PathBuf::from(argument("--fixture", ""));
    let source = fixture.join("library.db");
    if !source.is_file() {
        return Err("--fixture must contain a closed synthetic library.db".into());
    }
    let wal = fixture.join("library.db-wal");
    if wal.is_file() && std::fs::metadata(wal)?.len() > 32 {
        return Err(
            "fixture has uncheckpointed WAL frames; close and checkpoint it before copying".into(),
        );
    }
    let repeats: usize = argument("--repeats", "20").parse()?;
    if !(2..=100).contains(&repeats) {
        return Err("repeats must be between 2 and 100".into());
    }
    let root =
        std::env::temp_dir().join(format!("discovery-import-bench-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root)?;
    let copy_started = Instant::now();
    std::fs::copy(source, root.join("library.db"))?;
    let copy_ms = copy_started.elapsed().as_secs_f64() * 1000.0;
    let db = BibBase::open(root.join("library.db").to_string_lossy()).await?;
    let count = db.article_count().await? as usize;
    if count < 100 || db.get_article("bench-000000001").await?.is_none() {
        return Err("fixture is not a search_benchmark dataset".into());
    }
    // Keep first-build indexing work out of the timed importer measurements.
    db.search_v2(SearchRequestV2 {
        query: "doi:10.9999/bench-000000001".into(),
        ..Default::default()
    })
    .await?;
    let session_id = uuid::Uuid::new_v4().to_string();
    let mut previews = Vec::new();
    for (name, size, mixed) in [
        ("one_existing", 1, false),
        ("hundred_existing", 100, false),
        ("hundred_half_new", 100, true),
    ] {
        let records: Vec<_> = (0..size)
            .map(|i| record(i, !mixed || i % 2 == 0, &session_id))
            .collect();
        let mut samples = Vec::new();
        for _ in 0..repeats {
            let started = Instant::now();
            let preview = db
                .preview_discovery_import(records.clone(), Some(1), None)
                .await?;
            samples.push(started.elapsed().as_secs_f64() * 1000.0);
            for (i, item) in preview.items.iter().enumerate() {
                assert!(item.error.is_none(), "{:?}", item.error);
                assert_eq!(item.existing.len(), usize::from(!mixed || i % 2 == 0));
                if let Some(article) = item.existing.first() {
                    assert_eq!(article.id, format!("bench-{i:09}"));
                }
            }
        }
        previews.push(json!({"case":name,"timing":stats(samples)}));
    }
    let records: Vec<_> = (0..100).map(|i| record(i, true, &session_id)).collect();
    let started = Instant::now();
    let initial = db.import_discovery(records.clone(), Some(1), None).await?;
    let first_audit_import_ms = started.elapsed().as_secs_f64() * 1000.0;
    assert!(
        initial
            .iter()
            .all(|item| item.status == DiscoveryImportStatus::AlreadyPresent && item.changed)
    );
    let mut samples = Vec::new();
    for _ in 0..repeats {
        let started = Instant::now();
        let outcomes = db.import_discovery(records.clone(), Some(1), None).await?;
        samples.push(started.elapsed().as_secs_f64() * 1000.0);
        assert!(
            outcomes
                .iter()
                .all(|item| item.status == DiscoveryImportStatus::AlreadyPresent && !item.changed)
        );
    }
    assert_eq!(db.article_count().await? as usize, count);
    assert!(
        !db.get_article("bench-000000001")
            .await?
            .unwrap()
            .title
            .starts_with("Upstream")
    );
    let result = json!({"schema_version":1,"qualified_at":chrono::Utc::now().to_rfc3339(),"profile":if cfg!(debug_assertions){"debug-diagnostic"}else{"release"},"os":std::env::consts::OS,"arch":std::env::consts::ARCH,"fixture_directory":fixture,"private_copy_directory":root,"fixture_copy_ms":copy_ms,"library_articles":count,"preview":previews,"hundred_existing_first_audit_import_ms":first_audit_import_ms,"hundred_existing_idempotent_import":stats(samples),"memory":rss(),"checks":["normalized DOI lookup matches exact local ids","mixed new/existing preview","existing metadata preserved","idempotent repeat has no changes","article count unchanged"],"limitations":["synthetic local actor benchmark, not HTTP/GUI latency or real-library relevance","warm operating-system cache; copying and initial index build excluded","identifier normalization scans current library once per batch; this is not an O(1) normalized-id lookup index"]});
    let output = argument("--output", "");
    if !output.is_empty() {
        if let Some(parent) = Path::new(&output)
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(output, serde_json::to_vec_pretty(&result)?)?;
    }
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}
