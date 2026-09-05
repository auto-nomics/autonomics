//! Journal-level metrics cache — impact factor and quartiles keyed by
//! normalized journal name.
//!
//! Journal metrics are a **journal-level** attribute, not an article-level
//! one: a library with 100 *Nature Medicine* papers needs exactly one
//! EasyScholar lookup. This table therefore caches metrics per journal
//! (`journal_key = lower(trim(journal_name))`), and read paths join articles
//! to it by normalized name. The [`Article`] model deliberately stays
//! unaware of these fields.

use std::collections::HashMap;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use turso::Value;

use crate::bib_base::{BibBase, opt_int, opt_string};
use crate::error::Result;

/// Cached journal metrics for one journal.
///
/// Field semantics mirror [`easyscholar::JournalRank`](https://easyscholar.cc);
/// every dimension is optional because EasyScholar coverage is partial.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct JournalMetrics {
    /// Cache key: `lower(trim(journal_name))`.
    pub journal_key: String,
    /// Journal name as first seen (display / re-fetch friendly).
    pub journal_name: String,
    /// JCR impact factor (2-year window).
    pub impact_factor: Option<f64>,
    /// JCR 5-year impact factor.
    pub impact_factor_5: Option<f64>,
    /// JCR quartile (`"Q1"`–`"Q4"`).
    pub jcr_quartile: Option<String>,
    /// SSCI quartile (`"Q1"`–`"Q4"`).
    pub ssci_quartile: Option<String>,
    /// CAS (中科院) upgraded quartile — 大类.
    pub cas_quartile: Option<String>,
    /// CAS (中科院) base-edition quartile — 基础版.
    pub cas_quartile_base: Option<String>,
    /// CAS (中科院) small-class quartile — 小类.
    pub cas_small: Option<String>,
    /// CAS (中科院) top-journal flag (顶级期刊).
    pub cas_top: Option<bool>,
    /// CAS (中科院) early-warning list membership (预警期刊).
    pub cas_warning: Option<String>,
    /// RFC 3339 timestamp of the last successful EasyScholar fetch.
    pub fetched_at: String,
}

/// Normalize a journal name into its cache key: trimmed + lowercased.
///
/// This is the single canonical normalization — the write path (upsert) and
/// every read path (article → metrics join, frontend matching) must agree
/// on it, so it lives here as the shared definition.
pub fn journal_key_of(name: &str) -> String {
    name.trim().to_lowercase()
}

/// Fetch-and-cache journal metrics for one journal name (best-effort).
///
/// The single entry point shared by every enrichment caller — the HTTP
/// import path, the manual `fetch-metrics` endpoint, and the agent
/// `bib_save` tool. Every failure mode (no key, unknown journal, transport
/// error, cache write error) collapses to `None`: enrichment must never
/// fail the caller's primary operation.
pub async fn enrich_journal_metrics(
    bib: &BibBase,
    client: &easyscholar::EasyscholarClient,
    journal_name: &str,
) -> Option<JournalMetrics> {
    let rank = client.fetch_journal_rank(journal_name).await.ok()??;
    let metrics = JournalMetrics::from_rank(journal_name, &rank);
    bib.upsert_journal_metrics(&metrics).await.ok()?;
    Some(metrics)
}

impl JournalMetrics {
    /// Build a cache row from a journal name and a fetched rank.
    pub fn from_rank(journal_name: &str, rank: &easyscholar::JournalRank) -> Self {
        Self {
            journal_key: journal_key_of(journal_name),
            journal_name: journal_name.trim().to_string(),
            impact_factor: rank.impact_factor,
            impact_factor_5: rank.impact_factor_5,
            jcr_quartile: rank.jcr_quartile.clone(),
            ssci_quartile: rank.ssci_quartile.clone(),
            cas_quartile: rank.cas_quartile.clone(),
            cas_quartile_base: rank.cas_quartile_base.clone(),
            cas_small: rank.cas_small.clone(),
            cas_top: rank.cas_top,
            cas_warning: rank.cas_warning.clone(),
            fetched_at: Utc::now().to_rfc3339(),
        }
    }
}

impl BibBase {
    /// Insert or refresh one journal's cached metrics.
    ///
    /// The UPSERT **never clears existing values**: each column takes the
    /// incoming value only when it is non-NULL (`COALESCE`). A re-fetch
    /// that misses a dimension the previous fetch had (EasyScholar data
    /// revisions do happen) keeps the older value instead of wiping it.
    /// `fetched_at` always advances so staleness stays observable.
    pub async fn upsert_journal_metrics(&self, metrics: &JournalMetrics) -> Result<()> {
        let _write = self.write_gate.lock().await;
        let conn = self.write_conn();
        conn.execute(
            "INSERT INTO journal_metrics \
             (journal_key, journal_name, impact_factor, impact_factor_5, jcr_quartile, \
              ssci_quartile, cas_quartile, cas_quartile_base, cas_small, cas_top, \
              cas_warning, fetched_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12) \
             ON CONFLICT(journal_key) DO UPDATE SET \
                journal_name = COALESCE(excluded.journal_name, journal_name), \
                impact_factor = COALESCE(excluded.impact_factor, impact_factor), \
                impact_factor_5 = COALESCE(excluded.impact_factor_5, impact_factor_5), \
                jcr_quartile = COALESCE(excluded.jcr_quartile, jcr_quartile), \
                ssci_quartile = COALESCE(excluded.ssci_quartile, ssci_quartile), \
                cas_quartile = COALESCE(excluded.cas_quartile, cas_quartile), \
                cas_quartile_base = COALESCE(excluded.cas_quartile_base, cas_quartile_base), \
                cas_small = COALESCE(excluded.cas_small, cas_small), \
                cas_top = COALESCE(excluded.cas_top, cas_top), \
                cas_warning = COALESCE(excluded.cas_warning, cas_warning), \
                fetched_at = excluded.fetched_at",
            turso::params![
                metrics.journal_key.clone(),
                metrics.journal_name.clone(),
                metrics.impact_factor,
                metrics.impact_factor_5,
                metrics.jcr_quartile.clone(),
                metrics.ssci_quartile.clone(),
                metrics.cas_quartile.clone(),
                metrics.cas_quartile_base.clone(),
                metrics.cas_small.clone(),
                metrics.cas_top.map(|b| b as i64),
                metrics.cas_warning.clone(),
                metrics.fetched_at.clone(),
            ],
        )
        .await?;
        Ok(())
    }

    /// Look up cached metrics for a batch of journal keys.
    ///
    /// Returns only the keys that have a cache row; absent journals are
    /// simply missing from the map.
    pub async fn get_journal_metrics(
        &self,
        keys: &[String],
    ) -> Result<HashMap<String, JournalMetrics>> {
        let mut map = HashMap::with_capacity(keys.len());
        if keys.is_empty() {
            return Ok(map);
        }

        let conn = self.conn();
        let placeholders: Vec<String> = (1..=keys.len()).map(|i| format!("?{i}")).collect();
        let sql = format!(
            "SELECT journal_key, journal_name, impact_factor, impact_factor_5, jcr_quartile, \
             ssci_quartile, cas_quartile, cas_quartile_base, cas_small, cas_top, cas_warning, \
             fetched_at FROM journal_metrics WHERE journal_key IN ({})",
            placeholders.join(", ")
        );
        let params: Vec<Value> = keys.iter().map(|k| Value::Text(k.clone())).collect();
        let mut rows = conn.query(&sql, params).await?;
        while let Some(row) = rows.next().await? {
            let metrics = journal_metrics_from_row(&row)?;
            map.insert(metrics.journal_key.clone(), metrics);
        }
        Ok(map)
    }

    /// List every cached journal metrics row (frontend joins by journal name).
    pub async fn list_journal_metrics(&self) -> Result<Vec<JournalMetrics>> {
        let conn = self.conn();
        let mut rows = conn
            .query(
                "SELECT journal_key, journal_name, impact_factor, impact_factor_5, \
                 jcr_quartile, ssci_quartile, cas_quartile, cas_quartile_base, cas_small, \
                 cas_top, cas_warning, fetched_at \
                 FROM journal_metrics ORDER BY journal_name",
                turso::params![],
            )
            .await?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await? {
            out.push(journal_metrics_from_row(&row)?);
        }
        Ok(out)
    }

    /// Distinct journal names among stored articles that have no metrics
    /// cache row yet (backfill work-list).
    ///
    /// Deduplication is by normalized key (`lower(trim(...))`), but the
    /// returned names keep their stored casing — EasyScholar matches on the
    /// human-readable name. When one key has several spellings, `MIN`
    /// deterministically picks the lexicographically smallest.
    pub async fn missing_metric_journals(&self) -> Result<Vec<String>> {
        let conn = self.conn();
        let mut rows = conn
            .query(
                "SELECT MIN(journal) FROM articles \
                 WHERE journal IS NOT NULL AND TRIM(journal) != '' \
                   AND LOWER(TRIM(journal)) NOT IN (SELECT journal_key FROM journal_metrics) \
                 GROUP BY LOWER(TRIM(journal)) \
                 ORDER BY 1",
                turso::params![],
            )
            .await?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await? {
            out.push(row.get::<String>(0)?);
        }
        Ok(out)
    }
}

fn journal_metrics_from_row(row: &turso::Row) -> Result<JournalMetrics> {
    Ok(JournalMetrics {
        journal_key: row.get::<String>(0)?,
        journal_name: row.get::<String>(1)?,
        impact_factor: opt_real(row.get_value(2)?),
        impact_factor_5: opt_real(row.get_value(3)?),
        jcr_quartile: opt_string(row.get_value(4)?),
        ssci_quartile: opt_string(row.get_value(5)?),
        cas_quartile: opt_string(row.get_value(6)?),
        cas_quartile_base: opt_string(row.get_value(7)?),
        cas_small: opt_string(row.get_value(8)?),
        cas_top: opt_int(row.get_value(9)?).map(|i| i != 0),
        cas_warning: opt_string(row.get_value(10)?),
        fetched_at: row.get::<String>(11)?,
    })
}

fn opt_real(v: Value) -> Option<f64> {
    match v {
        Value::Real(f) => Some(f),
        Value::Integer(i) => Some(i as f64),
        Value::Null => None,
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bib_base::BibBase;
    use crate::types::Article;

    fn rank(if_: f64, jcr: &str) -> easyscholar::JournalRank {
        easyscholar::JournalRank {
            impact_factor: Some(if_),
            jcr_quartile: Some(jcr.to_string()),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn upsert_then_batch_lookup() {
        let db = BibBase::open_in_memory().await.unwrap();
        db.upsert_journal_metrics(&JournalMetrics::from_rank("Nature Medicine", &rank(82.9, "Q1")))
            .await
            .unwrap();
        db.upsert_journal_metrics(&JournalMetrics::from_rank("Spine", &rank(3.2, "Q2")))
            .await
            .unwrap();

        let keys = vec![
            journal_key_of("Nature Medicine"),
            journal_key_of("  spine  "),
            journal_key_of("Unknown Journal"),
        ];
        let map = db.get_journal_metrics(&keys).await.unwrap();
        assert_eq!(map.len(), 2);
        let nm = &map[&journal_key_of("Nature Medicine")];
        assert_eq!(nm.impact_factor, Some(82.9));
        assert_eq!(nm.jcr_quartile.as_deref(), Some("Q1"));
        assert_eq!(nm.journal_name, "Nature Medicine");
        assert!(!map.contains_key(&journal_key_of("Unknown Journal")));
    }

    #[tokio::test]
    async fn refetch_does_not_clear_existing_values() {
        let db = BibBase::open_in_memory().await.unwrap();
        db.upsert_journal_metrics(&JournalMetrics::from_rank("Spine", &rank(3.2, "Q2")))
            .await
            .unwrap();
        // A later fetch that lost both dimensions must not wipe them.
        db.upsert_journal_metrics(&JournalMetrics::from_rank(
            "Spine",
            &easyscholar::JournalRank::default(),
        ))
        .await
        .unwrap();

        let map = db
            .get_journal_metrics(&[journal_key_of("Spine")])
            .await
            .unwrap();
        let spine = &map[&journal_key_of("Spine")];
        assert_eq!(spine.impact_factor, Some(3.2));
        assert_eq!(spine.jcr_quartile.as_deref(), Some("Q2"));
    }

    #[tokio::test]
    async fn missing_journals_only_uncached_distinct() {
        let db = BibBase::open_in_memory().await.unwrap();
        let mut a = Article::new("a", "Paper A");
        a.journal = Some("Nature Medicine".to_string());
        let mut b = Article::new("b", "Paper B");
        b.journal = Some(" nature medicine ".to_string()); // same key after trim+lower
        let mut c = Article::new("c", "Paper C");
        c.journal = Some("Spine".to_string());
        let mut d = Article::new("d", "Paper D");
        d.journal = None;
        db.upsert_articles(&[a, b, c, d]).await.unwrap();

        let missing = db.missing_metric_journals().await.unwrap();
        assert_eq!(missing.len(), 2); // "Nature Medicine" (deduped) + "Spine"

        db.upsert_journal_metrics(&JournalMetrics::from_rank("Nature Medicine", &rank(82.9, "Q1")))
            .await
            .unwrap();
        let missing = db.missing_metric_journals().await.unwrap();
        assert_eq!(missing, vec!["Spine".to_string()]);
    }
}
