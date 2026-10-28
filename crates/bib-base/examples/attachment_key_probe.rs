//! Compare an existing key lookup with a nonunique key index on an owned,
//! closed-fixture copy. Never opens the original fixture or the running worker.
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{path::PathBuf, time::Instant};
use turso::Connection;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
async fn plan(conn: &Connection) -> Result<Vec<String>> {
    let mut rows = conn
        .query(
            "EXPLAIN QUERY PLAN SELECT itemID FROM items WHERE key=?1",
            ["FAKEKEY1"],
        )
        .await?;
    let mut details = Vec::new();
    while let Some(row) = rows.next().await? {
        details.push(row.get::<String>(3)?);
    }
    Ok(details)
}
async fn lookup(conn: &Connection, key: &str) -> Result<Vec<i64>> {
    let mut rows = conn
        .query("SELECT itemID FROM items WHERE key=?1", [key])
        .await?;
    let mut ids = Vec::new();
    while let Some(row) = rows.next().await? {
        ids.push(row.get::<i64>(0)?);
    }
    ids.sort_unstable();
    Ok(ids)
}
fn distribution(mut values: Vec<f64>) -> serde_json::Value {
    values.sort_by(f64::total_cmp);
    let count = values.len();
    json!({"samples":count,"p50_ms":values[count/2],"p95_ms":values[(count*95).div_ceil(100)-1],"max_ms":values[count-1]})
}
#[tokio::main]
async fn main() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 1 {
        return Err("usage: attachment_key_probe OWNED_COPY_DB".into());
    }
    let path = PathBuf::from(&args[0]);
    let parent = path.parent().ok_or("missing parent")?;
    let marker = parent.join("owned-key-lookup-profile.json");
    if !marker.is_file() || parent.join("key-lookup-receipt.json").exists() {
        return Err("requires a fresh owned copy marker and no prior receipt".into());
    }
    let db = turso::Builder::new_local(path.to_str().ok_or("invalid path")?)
        .experimental_multiprocess_wal(true)
        .build()
        .await?;
    let conn = db.connect()?;
    let mut rows = conn.query("SELECT COUNT(*) FROM items", ()).await?;
    let count: i64 = rows.next().await?.ok_or("no count")?.get(0)?;
    drop(rows);
    let mut rows = conn
        .query("SELECT key FROM items ORDER BY itemID DESC LIMIT 32", ())
        .await?;
    let mut keys = Vec::new();
    while let Some(row) = rows.next().await? {
        keys.push(row.get::<String>(0)?);
    }
    drop(rows);
    if keys.len() != 32 {
        return Err("requires at least 32 fixture rows".into());
    }
    let before_plan = plan(&conn).await?;
    let mut expected = Vec::new();
    let mut before = Vec::new();
    for _ in 0..10 {
        for key in &keys {
            let start = Instant::now();
            let ids = lookup(&conn, key).await?;
            before.push(start.elapsed().as_secs_f64() * 1000.);
            expected.push(ids);
        }
    }
    let start = Instant::now();
    conn.execute(
        "CREATE INDEX IF NOT EXISTS items_key_probe ON items(key)",
        (),
    )
    .await?;
    let index_ms = start.elapsed().as_secs_f64() * 1000.;
    let after_plan = plan(&conn).await?;
    let mut after = Vec::new();
    let mut index = 0;
    for _ in 0..10 {
        for key in &keys {
            let start = Instant::now();
            let ids = lookup(&conn, key).await?;
            after.push(start.elapsed().as_secs_f64() * 1000.);
            assert_eq!(ids, expected[index]);
            index += 1;
        }
    }
    let receipt = json!({"schema":"bib-attachment-key-diagnostic-v1","fixture":serde_json::from_slice::<serde_json::Value>(&std::fs::read(marker)?)?,"items":count,"before_plan":before_plan,"after_plan":after_plan,"before":distribution(before),"after":distribution(after),"index_build_ms":index_ms,"all_results_identical":true,"index_is_nonunique":true,"probe_sha256":format!("{:x}",Sha256::digest(include_bytes!("attachment_key_probe.rs"))),"limitations":["Closed partial fixture copy; not complete 100k/700k attachment ingestion","Warm serial exact SQL lookup; not HTTP/extraction/model/GUI performance","Only the owned copy receives the diagnostic index; running worker and original fixture are unchanged"]});
    std::fs::write(
        parent.join("key-lookup-receipt.json"),
        serde_json::to_vec_pretty(&receipt)?,
    )?;
    println!("{receipt}");
    Ok(())
}
