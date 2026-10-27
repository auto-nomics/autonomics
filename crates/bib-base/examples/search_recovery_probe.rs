//! Isolated child-process fixture for real interruption/replay qualification.
//! Never point this at a real library: seed requires an empty database.
use bib_base::{BibBase, SearchEngineMode};
use bib_types::Article;
use std::io::Write;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let phase = args.next().ok_or("phase required")?;
    let path = args.next().ok_or("isolated database path required")?;
    let count: usize = args.next().unwrap_or_else(|| "1000".into()).parse()?;
    let engine = if phase == "pending" {
        SearchEngineMode::Legacy
    } else {
        SearchEngineMode::Tantivy
    };
    let db = BibBase::open_with_search_engine(&path, engine).await?;
    match phase.as_str() {
        "seed" => {
            if db.article_count().await? != 0 {
                return Err("seed requires an empty fixture database".into());
            }
            for start in (0..count).step_by(250) {
                let articles = (start..(start + 250).min(count))
                    .map(|i| Article::new(format!("probe-{i}"), format!("Oldmarker evidence {i}")))
                    .collect::<Vec<_>>();
                db.upsert_articles(&articles).await?;
            }
            db.upsert_article(&Article::new("probe-delete", "Deletedmarker evidence"))
                .await?;
            db.rebuild_search_index().await?;
            println!(
                "{}",
                serde_json::json!({"phase":phase,"status":db.search_index_status().await?})
            );
        }
        "pending" => {
            if db.get_article("probe-0").await?.is_none() {
                return Err("fixture sentinel missing".into());
            }
            for start in (0..count).step_by(250) {
                let articles = (start..(start + 250).min(count))
                    .map(|i| {
                        Article::new(format!("probe-{i}"), format!("Freshmarker evidence {i}"))
                    })
                    .collect::<Vec<_>>();
                db.upsert_articles(&articles).await?;
            }
            db.delete_article("probe-delete").await?;
            println!(
                "{}",
                serde_json::json!({"phase":"committed_pending","status":db.search_index_status().await?})
            );
            std::io::stdout().flush()?;
            std::future::pending::<()>().await;
        }
        "build" => {
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                let status = db.search_index_status().await?;
                if status.building
                    && status.processed_documents > 0
                    && status.indexed_revision.unwrap_or(0) < status.source_revision
                {
                    println!(
                        "{}",
                        serde_json::json!({"phase":"building_uncommitted","status":status})
                    );
                    std::io::stdout().flush()?;
                    std::future::pending::<()>().await;
                }
                if tokio::time::Instant::now() > deadline {
                    return Err(
                        "could not observe an uncommitted build; interruption test is inconclusive"
                            .into(),
                    );
                }
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }
        }
        "hold" => {
            if db.search_articles("freshmarker", count + 1, 0).await?.len() != count {
                return Err("holder source mismatch".into());
            }
            println!(
                "{}",
                serde_json::json!({"phase":"holding_durable_writer","status":db.search_index_status().await?})
            );
            std::io::stdout().flush()?;
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
            loop {
                if db
                    .search_articles("concurrentprocessmarker", 10, 0)
                    .await?
                    .len()
                    == 1
                {
                    println!(
                        "{}",
                        serde_json::json!({"phase":"observed_secondary_process","status":db.search_index_status().await?})
                    );
                    break;
                }
                if tokio::time::Instant::now() > deadline {
                    return Err("holder did not observe secondary-process mutation".into());
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }
        "secondary" => {
            if db.search_articles("freshmarker", count + 1, 0).await?.len() != count {
                return Err("secondary source mismatch".into());
            }
            if db.search_index_status().await?.durable {
                return Err("secondary unexpectedly acquired durable writer".into());
            }
            db.upsert_article(&Article::new(
                "probe-secondary",
                "Concurrentprocessmarker evidence",
            ))
            .await?;
            if db
                .search_articles("concurrentprocessmarker", 10, 0)
                .await?
                .len()
                != 1
            {
                return Err("secondary index did not catch up".into());
            }
            println!(
                "{}",
                serde_json::json!({"phase":"secondary_process_committed","status":db.search_index_status().await?})
            );
        }
        "verify" => {
            let fresh = db.search_articles("freshmarker", count + 1, 0).await?;
            let old = db.search_articles("oldmarker", count + 1, 0).await?;
            let deleted = db.search_articles("deletedmarker", 10, 0).await?;
            if fresh.len() != count || !old.is_empty() || !deleted.is_empty() {
                return Err("replayed results differ from committed fixture data".into());
            }
            let status = db.search_index_status().await?;
            if status.pending_articles != 0
                || status.indexed_revision != Some(status.source_revision)
            {
                return Err("index cursor did not catch up".into());
            }
            println!(
                "{}",
                serde_json::json!({"phase":"verified","fresh":fresh.len(),"old":old.len(),"deleted":deleted.len(),"anchor_example":fresh[0].anchor_key,"status":status})
            );
        }
        _ => return Err("unknown phase".into()),
    }
    Ok(())
}
