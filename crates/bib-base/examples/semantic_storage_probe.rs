//! Diagnose copied synthetic sidecar reads, not search latency or relevance.
//! semantic_storage_probe COPIED_SIDECAR join|scan|rowid|plan
use sha2::{Digest, Sha256};
use std::{collections::HashSet, time::Instant};
use turso::transaction::{Transaction, TransactionBehavior};

const JOIN: &str = "SELECT b.inputHash,v.vector,v.checksum FROM semanticBindings b JOIN semanticVectors v ON v.inputHash=b.inputHash AND v.signature=?1 WHERE b.generation=?2 ORDER BY b.chunkID";
const SCAN: &str =
    "SELECT inputHash,vector,checksum FROM semanticVectors NOT INDEXED WHERE signature=?1";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 2 || !matches!(args[1].as_str(), "join" | "scan" | "rowid" | "plan") {
        return Err("usage: semantic_storage_probe COPIED_SIDECAR join|scan|rowid|plan".into());
    }
    let db = turso::Builder::new_local(&args[0])
        .experimental_multiprocess_wal(true)
        .build()
        .await?;
    let connection = db.connect()?;
    let tx = Transaction::new_unchecked(&connection, TransactionBehavior::Deferred).await?;
    let mut rows=connection.query("SELECT h.signature,COALESCE(a.bindingGeneration,h.generation) FROM semanticHeads h LEFT JOIN semanticBindingAliases a USING(generation) ORDER BY h.signature LIMIT 1",()).await?;
    let row = rows.next().await?.ok_or("no head")?;
    let signature: String = row.get(0)?;
    let binding: String = row.get(1)?;
    drop(rows);
    let mut rows = connection
        .query(
            "SELECT DISTINCT inputHash FROM semanticBindings WHERE generation=?1",
            turso::params![binding.clone()],
        )
        .await?;
    let mut required = HashSet::<String>::new();
    while let Some(row) = rows.next().await? {
        required.insert(row.get(0)?);
    }
    drop(rows);
    if args[1] == "plan" {
        for (name, sql) in [("join", JOIN), ("scan", SCAN)] {
            let sql = format!("EXPLAIN QUERY PLAN {sql}");
            let params: Vec<turso::Value> = if name == "join" {
                vec![signature.clone().into(), binding.clone().into()]
            } else {
                vec![signature.clone().into()]
            };
            let mut rows = connection.query(&sql, params).await?;
            while let Some(row) = rows.next().await? {
                println!("{}: {}", name, row.get::<String>(3)?);
            }
        }
        tx.commit().await?;
        return Ok(());
    }
    let started = Instant::now();
    let queries: Vec<(String, Vec<turso::Value>)> = if args[1] == "rowid" {
        let mut rows = connection
            .query(
                "SELECT rowid,inputHash FROM semanticVectors WHERE signature=?1",
                turso::params![signature.clone()],
            )
            .await?;
        let mut locators = Vec::new();
        while let Some(row) = rows.next().await? {
            if required.contains(&row.get::<String>(1)?) {
                locators.push(row.get::<i64>(0)?);
            }
        }
        drop(rows);
        locators.sort_unstable();
        locators.chunks(256).map(|ids| {
            let slots=vec!["?";ids.len()].join(",");
            (format!("SELECT inputHash,vector,checksum FROM semanticVectors WHERE rowid IN ({slots}) ORDER BY rowid"),ids.iter().copied().map(Into::into).collect())
        }).collect()
    } else if args[1] == "join" {
        vec![(JOIN.into(), vec![signature.clone().into(), binding.into()])]
    } else {
        vec![(SCAN.into(), vec![signature.clone().into()])]
    };
    let mut values = Vec::new();
    let mut seen = HashSet::new();
    let mut vector_bytes = 0usize;
    for (sql, params) in queries {
        let mut rows = connection.query(sql, params).await?;
        while let Some(row) = rows.next().await? {
            let input: String = row.get(0)?;
            if !required.contains(&input) || !seen.insert(input.clone()) {
                continue;
            }
            let vector: Vec<u8> = row.get(1)?;
            let checksum: String = row.get(2)?;
            if format!("{:x}", Sha256::digest(&vector)) != checksum {
                return Err("checksum mismatch".into());
            }
            vector_bytes += vector.len();
            values.push((input, checksum));
        }
    }
    if seen != required {
        return Err("missing referenced vector".into());
    }
    let read_ms = started.elapsed().as_secs_f64() * 1000.;
    values.sort();
    let fingerprint = format!("{:x}", Sha256::digest(serde_json::to_vec(&values)?));
    tx.commit().await?;
    println!(
        "{}",
        serde_json::json!({"schema":"bib-vector-storage-diagnostic-v1","mode":args[1],"signature":signature,"unique_vectors":values.len(),"vector_bytes":vector_bytes,"read_ms":read_ms,"content_fingerprint":fingerprint,"search_latency":false,"limitations":["Copied sidecar projected read diagnostic only","Excludes metadata materialization, normalization, model, source validation, HTTP and GUI","Cache conditions supplied externally; not guaranteed cold by this worker"]})
    );
    Ok(())
}
