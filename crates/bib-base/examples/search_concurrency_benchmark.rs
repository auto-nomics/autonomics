//! Reopen an existing synthetic scale fixture and measure maintenance behavior.
//! Usage: search_concurrency_benchmark DATABASE OUTPUT_JSON
use bib_base::{BibBase, SearchEngineMode};
use bib_types::SearchRequestV2;
use serde_json::{Value, json};
use std::{sync::Arc, time::Instant};
fn distribution(mut values: Vec<f64>) -> Value {
    values.sort_by(f64::total_cmp);
    let at = |p: f64| {
        values[((values.len() as f64 * p).ceil() as usize)
            .saturating_sub(1)
            .min(values.len() - 1)]
    };
    json!({"p50_ms":at(0.5),"p95_ms":at(0.95),"max_ms":at(1.0),"samples_ms":values})
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut args = std::env::args().skip(1);
    let database = args.next().ok_or("synthetic fixture database required")?;
    let output = args.next().ok_or("output path required")?;
    let started = Instant::now();
    let db =
        Arc::new(BibBase::open_with_search_engine(&database, SearchEngineMode::Tantivy).await?);
    let open_ms = started.elapsed().as_secs_f64() * 1000.0;
    let request = SearchRequestV2 {
        query: "title:\"gene editing\"".into(),
        ..Default::default()
    };
    let started = Instant::now();
    let initial = db.search_v2(request.clone()).await?;
    let first_search_ms = started.elapsed().as_secs_f64() * 1000.0;
    if db.get_article("bench-000000000").await?.is_none() {
        return Err("not a synthetic benchmark fixture".into());
    }
    db.request_search_rebuild().await?;
    let writer_db = db.clone();
    let writer = tokio::spawn(async move {
        let mut samples = Vec::new();
        for i in 0..30 {
            let mut article = writer_db
                .get_article(&format!("bench-{i:09}"))
                .await?
                .ok_or("fixture article missing")?;
            article
                .abstract_text
                .get_or_insert_default()
                .push_str(" Background revision evidence.");
            let start = Instant::now();
            writer_db.upsert_article(&article).await?;
            samples.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(samples)
    });
    let observer_db = db.clone();
    let observer = tokio::spawn(async move {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        let mut samples = Vec::new();
        let mut observed_building = 0;
        loop {
            let start = Instant::now();
            let progress = observer_db.search_index_status().await?;
            samples.push(start.elapsed().as_secs_f64() * 1000.0);
            observed_building += usize::from(progress.building);
            if samples.len() >= 20 && !progress.building && progress.pending_articles == 0 {
                break;
            }
            if tokio::time::Instant::now() > deadline {
                return Err("maintenance did not finish".into());
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>((samples, observed_building))
    });
    let mut queries = Vec::new();
    for _ in 0..20 {
        let start = Instant::now();
        let page = db.search_v2(request.clone()).await?;
        if page.total != initial.total {
            return Err("maintenance changed unrelated title results".into());
        }
        queries.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    let writes = writer.await??;
    db.search_v2(request).await?; // explicit catch-up barrier after the final write
    let maintenance_first_query_ms = queries[0];
    let (status_reads, observed_building) = observer.await??;
    if observed_building == 0 {
        return Err("no in-progress index work observed; concurrency run is inconclusive".into());
    }
    let status = db.search_index_status().await?;
    if status.pending_articles != 0 {
        return Err("index did not catch up after concurrent writes".into());
    }
    let report = json!({"schema_version":1,"profile":if cfg!(debug_assertions){"debug"}else{"release"},"database":database,
        "reopen_process_os_cache":"warm_or_uncontrolled","open_ms":open_ms,"first_search_after_open_ms":first_search_ms,
        "observed_building_samples":observed_building,"metadata_writes":distribution(writes),"status_reads":distribution(status_reads),
        "queries_during_maintenance_first_ms":maintenance_first_query_ms,"queries_during_maintenance":distribution(queries),"query_service_serializes_replay_and_queries":true,"final_status":status});
    if let Some(parent) = std::path::Path::new(&output).parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&output, serde_json::to_vec_pretty(&report)?)?;
    println!("{output}");
    Ok(())
}
