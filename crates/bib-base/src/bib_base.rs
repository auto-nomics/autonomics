//! SQLite (turso) storage layer for the bibliography library.
//!
//! [`BibBase`] owns a single [`Connection`] which is [`Clone`] (cheap —
//! internally an `Arc`). All public methods take `&self` and clone the
//! connection as needed, so the struct is freely shareable behind an
//! `Arc<BibBase>` without any `&mut self`.

use turso::{Builder, Connection, Value};

use crate::error::{Error, Result};
use bib_types::{Article, ArticleSource, Author, CollectionStatus, IdKind, Identifier, SearchHit};

// ---------------------------------------------------------------------------
// Schema — single source of truth for DDL
// ---------------------------------------------------------------------------

const SCHEMA_SQL: &str = "\
CREATE TABLE IF NOT EXISTS articles (
    id          TEXT PRIMARY KEY,
    title       TEXT NOT NULL,
    abstract    TEXT,
    year        INTEGER,
    month       INTEGER,
    journal     TEXT,
    volume      TEXT,
    issue       TEXT,
    pages       TEXT,
    issn        TEXT,
    essn        TEXT,
    language    TEXT,
    pub_types   TEXT,
    keywords    TEXT,
    source      TEXT,
    created_at  TEXT,
    updated_at  TEXT
);

CREATE TABLE IF NOT EXISTS authors (
    article_id  TEXT NOT NULL REFERENCES articles(id) ON DELETE CASCADE,
    position    INTEGER NOT NULL,
    last_name   TEXT NOT NULL,
    fore_name   TEXT,
    initials    TEXT,
    affiliation TEXT,
    orcid       TEXT,
    corresponding INTEGER DEFAULT 0,
    PRIMARY KEY (article_id, position)
);

CREATE TABLE IF NOT EXISTS identifiers (
    article_id  TEXT NOT NULL REFERENCES articles(id) ON DELETE CASCADE,
    kind        TEXT NOT NULL,
    value       TEXT NOT NULL,
    PRIMARY KEY (article_id, kind)
);

CREATE INDEX IF NOT EXISTS idx_identifiers_value ON identifiers(value);

CREATE TABLE IF NOT EXISTS annotations (
    id          TEXT PRIMARY KEY,
    article_id  TEXT NOT NULL REFERENCES articles(id) ON DELETE CASCADE,
    kind        TEXT NOT NULL,
    content     TEXT NOT NULL,
    page        INTEGER,
    created_at  TEXT
);

CREATE TABLE IF NOT EXISTS collections (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    description TEXT,
    tags        TEXT,
    status      TEXT NOT NULL DEFAULT 'active',
    created_at  TEXT,
    updated_at  TEXT
);

CREATE TABLE IF NOT EXISTS collection_articles (
    collection_id  TEXT NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
    article_id     TEXT NOT NULL REFERENCES articles(id) ON DELETE CASCADE,
    position       INTEGER NOT NULL,
    role           TEXT NOT NULL DEFAULT 'referenced',
    fetch_status   TEXT NOT NULL DEFAULT 'metadata_only',
    added_by       TEXT NOT NULL DEFAULT 'agent',
    note           TEXT,
    added_at       TEXT,
    PRIMARY KEY (collection_id, article_id)
);

CREATE INDEX IF NOT EXISTS idx_collection_articles_fetch
    ON collection_articles(fetch_status) WHERE fetch_status = 'fulltext_requested';

CREATE TABLE IF NOT EXISTS fulltexts (
    article_id   TEXT PRIMARY KEY REFERENCES articles(id) ON DELETE CASCADE,
    file_path    TEXT NOT NULL,
    file_format  TEXT NOT NULL,
    text_content TEXT,
    source       TEXT NOT NULL,
    file_hash    TEXT,
    file_size    INTEGER,
    uploaded_at  TEXT
);
";

// ---------------------------------------------------------------------------
// BibBase
// ---------------------------------------------------------------------------

/// Bibliography database handle.
///
/// Wraps a single turso [`Connection`]. Because `Connection` is cheaply
/// cloneable (internally an `Arc`), every method takes `&self` — you can
/// share `BibBase` behind `Arc<BibBase>` across tasks without any locking.
pub struct BibBase {
    conn: Connection,
}

impl BibBase {
    /// Open (or create) a local SQLite database file and run migrations.
    ///
    /// Uses turso's `multiprocess_wal` engine feature so that multiple
    /// processes (e.g. two TUI instances) can open the same file without
    /// one blocking the other with a `database is locked` error. WAL mode
    /// is implied by that feature; `busy_timeout` is set per-connection so
    /// contended writes wait briefly instead of erroring immediately.
    pub async fn open(path: impl AsRef<str>) -> Result<Self> {
        let path = path.as_ref();
        let mut builder = Builder::new_local(path);
        if path != ":memory:" {
            builder = builder.experimental_multiprocess_wal(true);
        }
        let db = builder.build().await?;
        let conn = db.connect()?;
        if path != ":memory:" {
            conn.pragma_update("busy_timeout", 5000).await?;
        }
        let base = Self { conn };
        base.migrate().await?;
        Ok(base)
    }

    /// Open an in-memory database (useful for tests).
    pub async fn open_in_memory() -> Result<Self> {
        Self::open(":memory:").await
    }

    /// Clone the underlying connection — cheap (Arc-based).
    pub(crate) fn conn(&self) -> Connection {
        self.conn.clone()
    }

    // -----------------------------------------------------------------------
    // Schema
    // -----------------------------------------------------------------------

    /// Run all DDL statements (idempotent — safe to call on every open).
    pub async fn migrate(&self) -> Result<()> {
        self.conn().execute_batch(SCHEMA_SQL).await?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Articles — CRUD
    // -----------------------------------------------------------------------

    /// Insert or replace a single article and its child rows (authors,
    /// identifiers).
    pub async fn upsert_article(&self, article: &Article) -> Result<()> {
        let conn = self.conn();

        // Upsert the article row.
        conn.execute(
            "INSERT OR REPLACE INTO articles \
             (id, title, abstract, year, month, journal, volume, issue, pages, \
              issn, essn, language, pub_types, keywords, source, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
            turso::params![
                article.id.clone(),
                article.title.clone(),
                article.abstract_text.clone(),
                article.year.map(|y| y as i64),
                article.month.map(|m| m as i64),
                article.journal.clone(),
                article.volume.clone(),
                article.issue.clone(),
                article.pages.clone(),
                article.issn.clone(),
                article.essn.clone(),
                article.language.clone(),
                serde_json::to_string(&article.pub_types)?,
                serde_json::to_string(&article.keywords)?,
                serde_json::to_string(&article.source)?,
                article.created_at.map(|t| t.to_rfc3339()),
                article.updated_at.map(|t| t.to_rfc3339()),
            ],
        )
        .await?;

        // Replace authors.
        conn.execute(
            "DELETE FROM authors WHERE article_id = ?1",
            turso::params![article.id.clone()],
        )
        .await?;
        for (i, author) in article.authors.iter().enumerate() {
            conn.execute(
                "INSERT INTO authors \
                 (article_id, position, last_name, fore_name, initials, \
                  affiliation, orcid, corresponding) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                turso::params![
                    article.id.clone(),
                    i as i64,
                    author.last_name.clone(),
                    author.fore_name.clone(),
                    author.initials.clone(),
                    author.affiliation.clone(),
                    author.orcid.clone(),
                    author.corresponding as i64,
                ],
            )
            .await?;
        }

        // Replace identifiers.
        conn.execute(
            "DELETE FROM identifiers WHERE article_id = ?1",
            turso::params![article.id.clone()],
        )
        .await?;
        for id in &article.identifiers {
            conn.execute(
                "INSERT INTO identifiers (article_id, kind, value) VALUES (?1, ?2, ?3)",
                turso::params![
                    article.id.clone(),
                    id.kind.as_str().to_owned(),
                    id.value.clone()
                ],
            )
            .await?;
        }

        Ok(())
    }

    /// Look up a single article by its internal ID.
    pub async fn get_article(&self, id: &str) -> Result<Option<Article>> {
        let conn = self.conn();
        let mut rows = conn
            .query(
                "SELECT id, title, abstract, year, month, journal, volume, issue, pages, \
                 issn, essn, language, pub_types, keywords, source, created_at, updated_at \
                 FROM articles WHERE id = ?1",
                turso::params![id],
            )
            .await?;

        let row = match rows.next().await? {
            Some(row) => row,
            None => return Ok(None),
        };

        let mut article = Article::new(row.get::<String>(0)?, row.get::<String>(1)?);
        article.abstract_text = opt_string(row.get_value(2)?);
        article.year = opt_int(row.get_value(3)?).map(|y| y as u16);
        article.month = opt_int(row.get_value(4)?).map(|m| m as u8);
        article.journal = opt_string(row.get_value(5)?);
        article.volume = opt_string(row.get_value(6)?);
        article.issue = opt_string(row.get_value(7)?);
        article.pages = opt_string(row.get_value(8)?);
        article.issn = opt_string(row.get_value(9)?);
        article.essn = opt_string(row.get_value(10)?);
        article.language = opt_string(row.get_value(11)?);
        article.pub_types = parse_json_col(&opt_string(row.get_value(12)?));
        article.keywords = parse_json_col(&opt_string(row.get_value(13)?));
        article.source = opt_string(row.get_value(14)?)
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();

        // Load authors.
        let mut author_rows = conn
            .query(
                "SELECT last_name, fore_name, initials, affiliation, orcid, corresponding \
                 FROM authors WHERE article_id = ?1 ORDER BY position",
                turso::params![id],
            )
            .await?;
        while let Some(a_row) = author_rows.next().await? {
            article.authors.push(Author {
                last_name: a_row.get::<String>(0)?,
                fore_name: opt_string(a_row.get_value(1)?),
                initials: opt_string(a_row.get_value(2)?),
                affiliation: opt_string(a_row.get_value(3)?),
                orcid: opt_string(a_row.get_value(4)?),
                corresponding: a_row.get::<i64>(5)? != 0,
            });
        }

        // Load identifiers.
        let mut id_rows = conn
            .query(
                "SELECT kind, value FROM identifiers WHERE article_id = ?1",
                turso::params![id],
            )
            .await?;
        while let Some(id_row) = id_rows.next().await? {
            let kind_str = id_row.get::<String>(0)?;
            let kind = match kind_str.as_str() {
                "doi" => IdKind::Doi,
                "pmid" => IdKind::Pmid,
                "pmc" => IdKind::Pmc,
                "arxiv" => IdKind::Arxiv,
                "s2" => IdKind::S2,
                "openalex" => IdKind::OpenAlex,
                _ => IdKind::Other,
            };
            article
                .identifiers
                .push(Identifier::new(kind, id_row.get::<String>(1)?));
        }

        Ok(Some(article))
    }

    /// Find an article by an external identifier (DOI, PMID, …).
    pub async fn find_by_identifier(&self, kind: IdKind, value: &str) -> Result<Option<Article>> {
        let conn = self.conn();
        let mut rows = conn
            .query(
                "SELECT article_id FROM identifiers WHERE kind = ?1 AND value = ?2 LIMIT 1",
                turso::params![kind.as_str(), value],
            )
            .await?;

        match rows.next().await? {
            Some(row) => self.get_article(&row.get::<String>(0)?).await,
            None => Ok(None),
        }
    }

    /// Delete an article and all its child rows (FK ON DELETE CASCADE).
    pub async fn delete_article(&self, id: &str) -> Result<()> {
        self.conn()
            .execute("DELETE FROM articles WHERE id = ?1", turso::params![id])
            .await?;
        Ok(())
    }

    /// Count articles in the library.
    pub async fn article_count(&self) -> Result<i64> {
        let conn = self.conn();
        let mut rows = conn
            .query("SELECT COUNT(*) FROM articles", turso::params![])
            .await?;
        let row = rows
            .next()
            .await?
            .ok_or_else(|| Error::Unknown("COUNT(*) returned no rows".into()))?;
        Ok(row.get::<i64>(0)?)
    }

    /// List all article IDs in the library.
    ///
    /// Lightweight scan — returns only IDs.  Use [`get_article`] to load
    /// full records.  Useful for building cite-key indexes.
    pub async fn list_article_ids(&self) -> Result<Vec<String>> {
        let conn = self.conn();
        let mut rows = conn
            .query("SELECT id FROM articles ORDER BY id", turso::params![])
            .await?;
        let mut ids = Vec::new();
        while let Some(row) = rows.next().await? {
            ids.push(row.get::<String>(0)?);
        }
        Ok(ids)
    }

    /// List all articles (full records) in the library.
    ///
    /// Loads every article with authors and identifiers.  For large
    /// libraries prefer [`list_article_ids`] + selective [`get_article`].
    pub async fn list_all_articles(&self) -> Result<Vec<Article>> {
        let ids = self.list_article_ids().await?;
        let mut articles = Vec::with_capacity(ids.len());
        for id in &ids {
            if let Some(article) = self.get_article(id).await? {
                articles.push(article);
            }
        }
        Ok(articles)
    }

    // -----------------------------------------------------------------------
    // Search — LIKE-based full-text search
    // -----------------------------------------------------------------------

    /// Search across article titles, abstracts, stored full-text
    /// content, **and user annotations** (notes/highlights/comments).
    /// Returns results ordered by match priority:
    /// title matches first, then abstract, then annotations, then full-text body.
    ///
    /// Uses SQL `LIKE` (case-insensitive for ASCII). Snippets are extracted
    /// in Rust from the first matching field.
    pub async fn search_articles(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        let pattern = format!("%{query}%");
        let conn = self.conn();
        let mut rows = conn
            .query(
                "SELECT a.id, a.title, a.abstract, f.text_content, \
                        (SELECT an.content FROM annotations an \
                         WHERE an.article_id = a.id AND an.content LIKE ?1 \
                         ORDER BY an.created_at LIMIT 1) AS annotation_hit, \
                 CASE \
                     WHEN a.title LIKE ?1 THEN 0 \
                     WHEN COALESCE(a.abstract, '') LIKE ?1 THEN 1 \
                     WHEN EXISTS (SELECT 1 FROM annotations an2 \
                                  WHERE an2.article_id = a.id AND an2.content LIKE ?1) THEN 2 \
                     ELSE 3 \
                 END AS rank \
                 FROM articles a \
                 LEFT JOIN fulltexts f ON f.article_id = a.id \
                 WHERE a.title LIKE ?1 \
                    OR COALESCE(a.abstract, '') LIKE ?1 \
                    OR COALESCE(f.text_content, '') LIKE ?1 \
                    OR EXISTS (SELECT 1 FROM annotations an3 \
                               WHERE an3.article_id = a.id AND an3.content LIKE ?1) \
                 ORDER BY rank \
                 LIMIT ?2",
                turso::params![pattern, limit as i64],
            )
            .await?;

        let mut hits = Vec::new();
        while let Some(row) = rows.next().await? {
            let article_id = row.get::<String>(0)?;
            let title = row.get::<String>(1)?;
            let abstract_text = opt_string(row.get_value(2)?);
            let fulltext = opt_string(row.get_value(3)?);
            let annotation = opt_string(row.get_value(4)?);
            let rank = row.get::<i64>(5)?;

            let snippet = extract_snippet(
                query,
                &title,
                abstract_text.as_deref(),
                fulltext.as_deref(),
                annotation.as_deref(),
            );

            hits.push(SearchHit {
                article_id,
                title,
                score: rank as f64,
                snippet,
            });
        }
        Ok(hits)
    }
}

/// Extract a context snippet around the first occurrence of `query` in
/// the available text fields. Tries title first, then abstract, then
/// annotations, then full-text body.
fn extract_snippet(
    query: &str,
    title: &str,
    abstract_text: Option<&str>,
    fulltext: Option<&str>,
    annotation: Option<&str>,
) -> String {
    const SNIPPET_RADIUS: usize = 80;

    let query_lower = query.to_ascii_lowercase();

    for text in [Some(title), abstract_text, annotation, fulltext]
        .into_iter()
        .flatten()
    {
        let text_lower = text.to_ascii_lowercase();
        if let Some(pos) = text_lower.find(&query_lower) {
            let start = pos.saturating_sub(SNIPPET_RADIUS);
            let end = (pos + query.len() + SNIPPET_RADIUS).min(text.len());
            let prefix = if start > 0 { "…" } else { "" };
            let suffix = if end < text.len() { "…" } else { "" };
            return format!("{prefix}{}{suffix}", &text[start..end]);
        }
    }

    // No match found in any field — return a truncated title as fallback.
    format!("{title:.120}")
}

// ---------------------------------------------------------------------------
// Value extraction helpers — turso's `Row::get_value` returns `Value`,
// we convert to Option<String> / Option<i64>.
// ---------------------------------------------------------------------------

fn opt_string(v: Value) -> Option<String> {
    match v {
        Value::Text(s) => Some(s),
        Value::Null => None,
        _ => None,
    }
}

fn opt_int(v: Value) -> Option<i64> {
    match v {
        Value::Integer(i) => Some(i),
        Value::Null => None,
        _ => None,
    }
}

fn parse_json_col(raw: &Option<String>) -> Vec<String> {
    raw.as_ref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default()
}
