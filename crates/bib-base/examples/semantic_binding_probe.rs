//! Inspect an owned, closed semantic-store copy. Optional COPY_LIMIT performs
//! one unpublished binding copy on that copy only; heads/vectors are unchanged.
//! Optional PAGE_SIZE uses keyset-paged transactions instead of one statement.
use serde_json::json;
use std::{path::PathBuf, time::Instant};
use turso::transaction::{Transaction, TransactionBehavior};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
#[tokio::main]
async fn main() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.is_empty() || args.len() > 4 {
        return Err("usage: semantic_binding_probe OWNED_COPY_DB [COPY_LIMIT [PAGE_SIZE]]".into());
    }
    let path = PathBuf::from(&args[0]);
    let deleting = args.get(1).is_some_and(|s| s == "--delete-unpublished");
    if !deleting && args.len() > 3 {
        return Err("copy mode accepts at most three arguments".into());
    }
    let copy_limit = if deleting {
        None
    } else {
        args.get(1).map(|s| s.parse::<u32>()).transpose()?
    };
    let page_size = if deleting {
        args.get(3).map(|s| s.parse::<u32>()).transpose()?
    } else {
        args.get(2).map(|s| s.parse::<u32>()).transpose()?
    };
    if copy_limit.is_some_and(|limit| limit == 0 || limit > 1_000_000) {
        return Err("COPY_LIMIT must be 1..1000000".into());
    }
    if page_size.is_some_and(|size| size == 0 || size > 2048) {
        return Err("PAGE_SIZE must be 1..2048".into());
    }
    let parent = path.parent().ok_or("missing parent")?;
    if !parent.join("owned-semantic-binding-profile.json").is_file() {
        return Err("requires an owned copy marker".into());
    }
    let db = turso::Builder::new_local(path.to_str().ok_or("invalid path")?)
        .experimental_multiprocess_wal(true)
        .build()
        .await?;
    let conn = db.connect()?;
    conn.pragma_update("foreign_keys", true).await?;
    conn.pragma_update("busy_timeout", 5000).await?;
    let queries = [
        (
            "generations",
            "SELECT g.generation,g.revision,g.touchedAt,h.signature FROM semanticGenerations g LEFT JOIN semanticHeads h USING(generation)",
        ),
        (
            "copy_plan",
            "EXPLAIN QUERY PLAN INSERT INTO semanticBindings SELECT 'probe',chunkID,docID,articleID,anchorKey,kind,contentHash,startByte,endByte,inputHash FROM semanticBindings WHERE generation='old'",
        ),
        (
            "page_plan",
            "EXPLAIN QUERY PLAN SELECT chunkID FROM semanticBindings WHERE generation='old' AND chunkID>'a' ORDER BY chunkID LIMIT 2048",
        ),
        (
            "copy_range_plan",
            "EXPLAIN QUERY PLAN INSERT INTO semanticBindings SELECT 'probe',chunkID,docID,articleID,anchorKey,kind,contentHash,startByte,endByte,inputHash FROM semanticBindings WHERE generation='old' AND chunkID>'a' AND chunkID<='b'",
        ),
        (
            "copy_range_forced_plan",
            "EXPLAIN QUERY PLAN INSERT INTO semanticBindings SELECT 'probe',chunkID,docID,articleID,anchorKey,kind,contentHash,startByte,endByte,inputHash FROM semanticBindings INDEXED BY sqlite_autoindex_semanticBindings_1 WHERE generation='old' AND chunkID>'a' AND chunkID<='b'",
        ),
        (
            "gc_plan",
            "EXPLAIN QUERY PLAN DELETE FROM semanticVectors WHERE signature='probe' AND inputHash NOT IN(SELECT b.inputHash FROM semanticBindings b JOIN semanticGenerations g USING(generation) WHERE g.signature='probe')",
        ),
        (
            "bindings",
            "SELECT generation,COUNT(*) FROM semanticBindings GROUP BY generation",
        ),
    ];
    for (name, sql) in queries {
        eprintln!("starting {name}");
        let start = Instant::now();
        let mut rows = conn.query(sql, ()).await?;
        let mut values = Vec::new();
        while let Some(row) = rows.next().await? {
            let mut fields = Vec::new();
            for column in 0..row.column_count() {
                fields.push(format!("{:?}", row.get_value(column)?));
            }
            values.push(fields);
        }
        println!(
            "{}",
            json!({"stage":name,"elapsed_ms":start.elapsed().as_secs_f64()*1000.,"rows":values})
        );
    }
    if deleting {
        let generation = args.get(2).ok_or("missing probe generation")?;
        let uuid = generation
            .strip_prefix("binding-probe-")
            .ok_or("only probe-created generations may be deleted")?;
        uuid::Uuid::parse_str(uuid)?;
        if let Some(page_size) = page_size {
            let start = Instant::now();
            conn.execute("CREATE INDEX IF NOT EXISTS semanticBindings_input ON semanticBindings(inputHash,generation)",()).await?;
            println!(
                "{}",
                json!({"stage":"input_index_migration","elapsed_ms":start.elapsed().as_secs_f64()*1000.})
            );
            let start = Instant::now();
            let mut total = 0;
            loop {
                let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
                let mut rows=conn.query("SELECT 1 FROM semanticHeads WHERE generation=?1 UNION ALL SELECT 1 FROM semanticBindingAliases WHERE bindingGeneration=?1",[generation.as_str()]).await?;
                if rows.next().await?.is_some() {
                    return Err("probe generation is referenced; cleanup refused".into());
                }
                drop(rows);
                let mut rows=conn.query("SELECT chunkID FROM semanticBindings WHERE generation=?1 ORDER BY chunkID LIMIT ?2",turso::params![generation.clone(),i64::from(page_size)]).await?;
                let mut last = None;
                let mut expected = 0;
                while let Some(row) = rows.next().await? {
                    last = Some(row.get::<String>(0)?);
                    expected += 1;
                }
                drop(rows);
                if let Some(last) = last {
                    let count=conn.execute("DELETE FROM semanticBindings INDEXED BY sqlite_autoindex_semanticBindings_1 WHERE generation=?1 AND chunkID<=?2",turso::params![generation.clone(),last]).await?;
                    if count != expected {
                        return Err("binding delete count changed".into());
                    }
                    total += count;
                }
                let removed=conn.execute("DELETE FROM semanticGenerations WHERE generation=?1 AND NOT EXISTS(SELECT 1 FROM semanticHeads WHERE generation=?1) AND NOT EXISTS(SELECT 1 FROM semanticBindingAliases WHERE bindingGeneration=?1) AND NOT EXISTS(SELECT 1 FROM semanticBindings WHERE generation=?1)",[generation.as_str()]).await?;
                tx.commit().await?;
                println!(
                    "{}",
                    json!({"stage":"delete_page","deleted":total,"elapsed_ms":start.elapsed().as_secs_f64()*1000.})
                );
                if removed == 1 {
                    break;
                }
                if expected == 0 {
                    return Err("probe generation missing; cleanup did not remove metadata".into());
                }
            }
            println!(
                "{}",
                json!({"stage":"delete_unpublished","generation":generation,"page_size":page_size,"deleted_bindings":total,"elapsed_ms":start.elapsed().as_secs_f64()*1000.,"result":"ok"})
            );
            return Ok(());
        }
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        eprintln!("starting deletion of unpublished probe generation {generation}");
        let start = Instant::now();
        let result=conn.execute("DELETE FROM semanticGenerations WHERE generation=?1 AND NOT EXISTS(SELECT 1 FROM semanticHeads WHERE generation=?1) AND NOT EXISTS(SELECT 1 FROM semanticBindingAliases WHERE bindingGeneration=?1)",[generation.as_str()]).await;
        println!(
            "{}",
            json!({"stage":"delete_unpublished","generation":generation,"elapsed_ms":start.elapsed().as_secs_f64()*1000.,"result":format!("{result:?}")})
        );
        match result {
            Ok(1) => tx.commit().await?,
            Ok(_) => {
                tx.rollback().await?;
                return Err("probe generation is missing or referenced; nothing deleted".into());
            }
            Err(e) => {
                let _ = tx.rollback().await;
                return Err(e.into());
            }
        }
    }
    if let Some(limit) = copy_limit {
        let mut rows = conn
            .query(
                "SELECT generation FROM semanticHeads ORDER BY signature LIMIT 1",
                (),
            )
            .await?;
        let old: String = rows.next().await?.ok_or("no published head")?.get(0)?;
        drop(rows);
        let new = format!("binding-probe-{}", uuid::Uuid::new_v4());
        conn.execute("INSERT INTO semanticGenerations SELECT ?1,signature,revision,token,dimensions,touchedAt FROM semanticGenerations WHERE generation=?2", turso::params![new.clone(),old.clone()]).await?;
        eprintln!("starting copy of at most {limit} bindings into {new}");
        let start = Instant::now();
        let result = if let Some(page_size) = page_size {
            let mut after = String::new();
            let mut total = 0;
            while total < u64::from(limit) {
                let mut rows = conn.query("SELECT chunkID FROM semanticBindings WHERE generation=?1 AND chunkID>?2 ORDER BY chunkID LIMIT ?3", turso::params![old.clone(),after.clone(),i64::from(page_size).min(i64::from(limit)-total as i64)]).await?;
                let mut last = None;
                let mut expected = 0;
                while let Some(row) = rows.next().await? {
                    last = Some(row.get::<String>(0)?);
                    expected += 1;
                }
                drop(rows);
                let Some(last) = last else {
                    break;
                };
                let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
                conn.execute(
                    "UPDATE semanticGenerations SET touchedAt=?1 WHERE generation=?2",
                    turso::params![chrono::Utc::now().timestamp_millis(), new.clone()],
                )
                .await?;
                let copied = conn.execute("INSERT INTO semanticBindings SELECT ?1,chunkID,docID,articleID,anchorKey,kind,contentHash,startByte,endByte,inputHash FROM semanticBindings INDEXED BY sqlite_autoindex_semanticBindings_1 WHERE generation=?2 AND chunkID>?3 AND chunkID<=?4", turso::params![new.clone(),old.clone(),after,last.clone()]).await?;
                if copied != expected {
                    return Err("copy count changed".into());
                }
                tx.commit().await?;
                total += copied;
                after = last;
                println!(
                    "{}",
                    json!({"stage":"copy_page","copied":total,"elapsed_ms":start.elapsed().as_secs_f64()*1000.})
                );
            }
            Ok(total)
        } else {
            conn.execute("INSERT INTO semanticBindings SELECT ?1,chunkID,docID,articleID,anchorKey,kind,contentHash,startByte,endByte,inputHash FROM semanticBindings WHERE generation=?2 LIMIT ?3", turso::params![new.clone(),old, i64::from(limit)]).await
        };
        println!(
            "{}",
            json!({"stage":"copy","generation":new,"limit":limit,"page_size":page_size,"elapsed_ms":start.elapsed().as_secs_f64()*1000.,"result":format!("{result:?}")})
        );
        result?;
    }
    Ok(())
}
