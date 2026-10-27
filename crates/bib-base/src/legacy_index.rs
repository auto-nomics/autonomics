//! On-demand rollback index. The caller holds an immediate transaction so
//! replay, its acknowledgement, and results observe the same source state.
//! This shared SQL table has one transactional cursor; Tantivy consumers retain
//! their own independent cursors and the outbox is never pruned here.
use crate::{Error, Result};
use std::collections::HashMap;
use turso::Connection;

pub(crate) async fn refresh(conn: &Connection) -> Result<()> {
    let mut rows = conn.query("SELECT key,value FROM bib_meta WHERE key IN ('search_revision','search_revision_token','legacy_terms_revision','legacy_terms_token','legacy_terms_version')", ()).await?;
    let mut meta = HashMap::new();
    while let Some(row) = rows.next().await? {
        meta.insert(row.get::<String>(0)?, row.get::<String>(1)?);
    }
    drop(rows);
    let revision = meta
        .get("search_revision")
        .map(String::as_str)
        .unwrap_or("0");
    let revision = revision
        .parse::<i64>()
        .map_err(|_| Error::Unknown("invalid search revision".into()))?;
    let token = meta
        .get("search_revision_token")
        .ok_or_else(|| Error::Unknown("missing search revision token".into()))?;
    let indexed = meta
        .get("legacy_terms_revision")
        .and_then(|value| value.parse::<i64>().ok());
    let version_ok = meta
        .get("legacy_terms_version")
        .is_some_and(|value| value == "1");
    if version_ok && indexed == Some(revision) && meta.get("legacy_terms_token") == Some(token) {
        return Ok(());
    }
    let mut rebuild = !version_ok
        || indexed.is_none_or(|indexed| indexed >= revision)
        || !meta.contains_key("legacy_terms_token");
    let mut ids = Vec::new();
    if !rebuild {
        let mut rows = conn
            .query(
                "SELECT articleID FROM searchOutbox WHERE revision>?1 ORDER BY revision",
                turso::params![indexed.unwrap_or(0)],
            )
            .await?;
        while let Some(row) = rows.next().await? {
            ids.push(row.get::<String>(0)?);
        }
        rebuild = ids.is_empty();
    }
    if rebuild {
        conn.execute("DELETE FROM searchTerms", ()).await?;
        let mut rows = conn
            .query("SELECT id FROM articles ORDER BY id", ())
            .await?;
        ids.clear();
        while let Some(row) = rows.next().await? {
            ids.push(row.get::<String>(0)?);
        }
    }
    for id in ids {
        crate::bib_base::sync_legacy_search_terms(conn, &id).await?;
    }
    for (key, value) in [
        ("legacy_terms_revision", revision.to_string()),
        ("legacy_terms_token", token.clone()),
        ("legacy_terms_version", "1".into()),
    ] {
        conn.execute("INSERT INTO bib_meta(key,value) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", turso::params![key,value]).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::{SearchEngineMode, bib_base::BibCore};
    use bib_types::Article;

    async fn term_count(core: &BibCore) -> i64 {
        core.conn()
            .query("SELECT COUNT(*) FROM searchTerms", ())
            .await
            .unwrap()
            .next()
            .await
            .unwrap()
            .unwrap()
            .get(0)
            .unwrap()
    }

    #[tokio::test]
    async fn rollback_replays_modern_writes_updates_and_deletes_without_default_dual_write() {
        struct TempDir(std::path::PathBuf);
        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let dir =
            TempDir(std::env::temp_dir().join(format!("bib-rollback-{}", uuid::Uuid::new_v4())));
        std::fs::create_dir_all(&dir.0).unwrap();
        let path = dir.0.join("rollback.db");
        let path = path.to_str().unwrap();
        let modern = BibCore::open_with_search_engine(path, SearchEngineMode::Tantivy)
            .await
            .unwrap();
        modern
            .upsert_article(&Article::new("a", "Originalmarker evidence"))
            .await
            .unwrap();
        assert_eq!(term_count(&modern).await, 0);
        let legacy = BibCore::open_with_search_engine(path, SearchEngineMode::Legacy)
            .await
            .unwrap();
        assert_eq!(
            legacy
                .search_articles("originalmarker", 10, 0)
                .await
                .unwrap()
                .len(),
            1
        );
        let original_count = term_count(&modern).await;
        modern
            .upsert_article(&Article::new("a", "Replacementmarker evidence"))
            .await
            .unwrap();
        // Modern writes leave the legacy projection untouched until requested.
        assert_eq!(term_count(&modern).await, original_count);
        assert!(
            legacy
                .search_articles("originalmarker", 10, 0)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            legacy
                .search_articles("replacementmarker", 10, 0)
                .await
                .unwrap()
                .len(),
            1
        );
        modern.delete_article("a").await.unwrap();
        assert!(
            legacy
                .search_articles("replacementmarker", 10, 0)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(term_count(&legacy).await, 0);
        modern
            .upsert_article(&Article::new("b", "Newmarker evidence"))
            .await
            .unwrap();
        assert_eq!(
            legacy.search_articles("newmarker", 10, 0).await.unwrap()[0].article_id,
            "b"
        );
    }

    #[tokio::test]
    async fn divergent_tokens_and_missing_acknowledgements_rebuild_rollback_terms() {
        let core = BibCore::open_with_search_engine(":memory:", SearchEngineMode::Legacy)
            .await
            .unwrap();
        core.upsert_article(&Article::new("a", "Recoverymarker evidence"))
            .await
            .unwrap();
        assert_eq!(
            core.search_articles("recoverymarker", 10, 0)
                .await
                .unwrap()
                .len(),
            1
        );
        for sabotage in [
            "UPDATE bib_meta SET value='divergent-history' WHERE key='legacy_terms_token'",
            "UPDATE bib_meta SET value='999999' WHERE key='legacy_terms_revision'",
            "DELETE FROM bib_meta WHERE key='legacy_terms_version'",
        ] {
            core.write_conn()
                .execute("DELETE FROM searchTerms", ())
                .await
                .unwrap();
            core.write_conn().execute(sabotage, ()).await.unwrap();
            assert_eq!(
                core.search_articles("recoverymarker", 10, 0)
                    .await
                    .unwrap()
                    .len(),
                1,
                "{sabotage}"
            );
        }
    }
}
