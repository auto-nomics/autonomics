//! SQLite (turso) storage layer for the bibliography library.
//!
//! [`BibBase`] owns a [`Database`] handle. All public methods take `&self`;
//! every read opens a fresh connection (independent WAL snapshot), while
//! writes share a dedicated connection, a process-local write gate, and
//! transactions for multi-statement updates.

use std::sync::Arc;

use tokio::sync::Mutex;
use turso::{
    Builder, Connection, Database, Value,
    transaction::{Transaction, TransactionBehavior},
};

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
    data        TEXT,
    created_at  TEXT
);

CREATE TABLE IF NOT EXISTS collections (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    description TEXT,
    tags        TEXT,
    status      TEXT NOT NULL DEFAULT 'active',
    parent_id   TEXT,
    sort_order  INTEGER NOT NULL DEFAULT 0,
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

CREATE TABLE IF NOT EXISTS search_terms (
    term       TEXT NOT NULL,
    article_id TEXT NOT NULL,
    field      TEXT NOT NULL,
    PRIMARY KEY (term, article_id, field)
);

CREATE INDEX IF NOT EXISTS idx_search_terms_article ON search_terms(article_id);

CREATE TABLE IF NOT EXISTS bib_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS journal_metrics (
    journal_key       TEXT PRIMARY KEY,
    journal_name      TEXT NOT NULL,
    impact_factor     REAL,
    impact_factor_5   REAL,
    jcr_quartile      TEXT,
    ssci_quartile     TEXT,
    cas_quartile      TEXT,
    cas_quartile_base TEXT,
    cas_small         TEXT,
    cas_top           INTEGER,
    cas_warning       TEXT,
    fetched_at        TEXT NOT NULL
);
";

// ---------------------------------------------------------------------------
// Paged listing — request types
// ---------------------------------------------------------------------------

/// How many matches a keyword query may contribute to a paged listing.
///
/// The HTTP layer caps `limit` well below this; the headroom exists so the
/// reported `total` stays accurate for filtered pages instead of silently
/// stopping at the page size.
const PAGED_SEARCH_CANDIDATE_CAP: usize = 10_000;

/// Whitelisted sort keys for [`BibBase::list_articles_paged`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortField {
    /// `articles.created_at`.
    CreatedAt,
    /// `articles.updated_at`.
    UpdatedAt,
    /// Article title, case-insensitively.
    Title,
    /// Publication year.
    Year,
}

impl SortField {
    /// Parse the wire form used by the HTTP API.
    ///
    /// Unknown values return `None` so the caller can reject them: silently
    /// falling back to a default sort would hide a client typo behind a
    /// page that looks correct but is ordered differently than requested.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "created_at" => Some(Self::CreatedAt),
            "updated_at" => Some(Self::UpdatedAt),
            "title" => Some(Self::Title),
            "year" => Some(Self::Year),
            _ => None,
        }
    }
}

/// Sort direction for [`BibBase::list_articles_paged`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortOrder {
    /// Smallest first.
    Ascending,
    /// Largest first.
    Descending,
}

impl SortOrder {
    /// Parse the wire form used by the HTTP API (see
    /// [`SortField::from_wire`] for why this rejects rather than defaults).
    pub fn from_wire(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "asc" | "ascending" => Some(Self::Ascending),
            "desc" | "descending" => Some(Self::Descending),
            _ => None,
        }
    }
}

/// Filters and pagination for [`BibBase::list_articles_paged`].
#[derive(Debug, Clone)]
pub struct ListParams {
    /// Keyword query; empty means "no keyword filter".
    pub query: String,
    /// Number of articles to skip.
    pub offset: usize,
    /// Maximum number of articles to return.
    pub limit: usize,
    /// Sort key.
    pub sort: SortField,
    /// Sort direction.
    pub order: SortOrder,
    /// Only articles that are members of this collection.
    ///
    /// Takes precedence over `unfiled` when both are set.
    pub collection_id: Option<String>,
    /// Only articles that are not members of any collection.
    pub unfiled: bool,
}

impl Default for ListParams {
    fn default() -> Self {
        Self {
            query: String::new(),
            offset: 0,
            limit: 100,
            sort: SortField::CreatedAt,
            order: SortOrder::Descending,
            collection_id: None,
            unfiled: false,
        }
    }
}

// ---------------------------------------------------------------------------
// BibBase
// ---------------------------------------------------------------------------

/// Bibliography database handle.
///
/// Wraps a turso [`Database`]; every method takes `&self` — you can share
/// `BibBase` behind `Arc<BibBase>`. Writes coordinate through the internal
/// write gate; reads open a fresh connection per call so they stay
/// concurrently callable (turso connections forbid concurrent use).
pub struct BibBase {
    /// Database handle — [`Self::conn`] opens a fresh read connection per
    /// call, because turso connections forbid concurrent use and the HTTP
    /// layer fans out parallel requests on page load.
    db: Database,
    write_conn: Connection,
    /// Serializes write operations started by this process handle.
    ///
    /// SQLite allows concurrent readers in WAL mode, but writers still commit
    /// one at a time. This gate prevents logical writes from interleaving on
    /// the shared connection and keeps multi-statement operations atomic.
    pub(crate) write_gate: Arc<Mutex<()>>,
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
        let write_conn = db.connect()?;
        if path != ":memory:" {
            write_conn.pragma_update("busy_timeout", 5000).await?;
        }
        // Enable FK enforcement so the `ON DELETE CASCADE` clauses declared in
        // [`SCHEMA_SQL`] actually fire. Without this pragma, SQLite parses and
        // stores FK constraints but does not enforce them, leaving orphan rows
        // (e.g. annotations referencing a deleted article). This matters most
        // for [`Self::delete_article`] — callers expect the cascade to clean
        // up authors, identifiers, annotations, `collection_articles`
        // memberships, and `fulltexts` pointer rows.
        write_conn.pragma_update("foreign_keys", true).await?;
        let base = Self {
            db,
            write_conn,
            write_gate: Arc::new(Mutex::new(())),
        };
        base.migrate().await?;
        Ok(base)
    }

    /// Open an in-memory database (useful for tests).
    pub async fn open_in_memory() -> Result<Self> {
        Self::open(":memory:").await
    }

    /// Open a fresh connection for a read.
    ///
    /// turso connections reject concurrent use ("concurrent use forbidden"),
    /// and page load fires parallel requests (collections + journal metrics
    /// + article list), so a single shared read connection races and 500s.
    /// A per-call connection gives every read an independent WAL snapshot.
    /// Reads never mutate, so they need neither the FK pragma nor
    /// busy_timeout (WAL readers don't block). `connect()` on an
    /// already-open local Database cannot realistically fail.
    pub(crate) fn conn(&self) -> Connection {
        self.db.connect().expect("connect on an open local database")
    }

    /// Clone the dedicated writer connection. All mutations use this
    /// connection so readers on [`Self::conn`] retain WAL snapshot isolation.
    pub(crate) fn write_conn(&self) -> Connection {
        self.write_conn.clone()
    }

    // -----------------------------------------------------------------------
    // Schema
    // -----------------------------------------------------------------------

    /// Run all DDL statements (idempotent — safe to call on every open).
    pub async fn migrate(&self) -> Result<()> {
        let _write = self.write_gate.lock().await;
        let conn = self.write_conn();
        conn.execute_batch(SCHEMA_SQL).await?;

        // `CREATE TABLE IF NOT EXISTS` never amends a table that already
        // exists, so columns introduced after a database was created would
        // stay missing forever. Backfill them here; the DDL strings match
        // the definitions in [`SCHEMA_SQL`].
        ensure_column(&conn, "collections", "parent_id", "parent_id TEXT").await?;
        ensure_column(
            &conn,
            "collections",
            "sort_order",
            "sort_order INTEGER NOT NULL DEFAULT 0",
        )
        .await?;
        ensure_column(&conn, "annotations", "data", "data TEXT").await?;

        let mut rows = conn
            .query(
                "SELECT value FROM bib_meta WHERE key = 'search_index_version'",
                turso::params![],
            )
            .await?;
        let version = match rows.next().await? {
            Some(row) => row.get::<String>(0)?,
            None => String::new(),
        };
        if version != "1" {
            conn.execute("DELETE FROM search_terms", turso::params![])
                .await?;
            let mut article_rows = conn
                .query("SELECT id FROM articles", turso::params![])
                .await?;
            let mut article_ids = Vec::new();
            while let Some(row) = article_rows.next().await? {
                article_ids.push(row.get::<String>(0)?);
            }
            drop(article_rows);
            for article_id in &article_ids {
                sync_search_index(&conn, article_id).await?;
            }
            conn.execute(
                "INSERT INTO bib_meta(key, value) VALUES ('search_index_version', '1') \
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                turso::params![],
            )
            .await?;
        }

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Articles — CRUD
    // -----------------------------------------------------------------------

    /// Insert or update a single article and its child rows (authors,
    /// identifiers).
    pub async fn upsert_article(&self, article: &Article) -> Result<()> {
        self.upsert_articles(std::slice::from_ref(article)).await
    }

    /// Insert or update articles and their child rows in one transaction.
    ///
    /// The batch is atomic: if any article fails, no article from the batch is
    /// committed. Article metadata is updated in place; annotations, full-text
    /// records, and collection memberships are preserved.
    pub async fn upsert_articles(&self, articles: &[Article]) -> Result<()> {
        if articles.is_empty() {
            return Ok(());
        }

        let _write = self.write_gate.lock().await;
        let conn = self.write_conn();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        let mut result = Ok(());
        for article in articles {
            if let Err(err) = Self::upsert_article_in_tx(&tx, article).await {
                result = Err(err);
                break;
            }
            sync_search_index(&tx, &article.id).await?;
        }

        if result.is_err() {
            // Roll back immediately instead of leaving a dangling transaction
            // for the next connection operation to discover.
            let _ = tx.rollback().await;
            return result;
        }

        tx.commit().await?;
        result
    }

    /// Insert or update one article, preserving rows that are not synchronized
    /// from `Article` metadata.
    async fn upsert_article_in_tx(conn: &Connection, article: &Article) -> Result<()> {
        // A real UPSERT is important here. `INSERT OR REPLACE` deletes the old
        // parent row first, which can cascade to annotations, fulltexts, and
        // collection memberships that should survive a metadata refresh.
        conn.execute(
            "INSERT INTO articles \
             (id, title, abstract, year, month, journal, volume, issue, pages, \
              issn, essn, language, pub_types, keywords, source, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17) \
             ON CONFLICT(id) DO UPDATE SET \
                title = excluded.title, \
                abstract = excluded.abstract, \
                year = excluded.year, \
                month = excluded.month, \
                journal = excluded.journal, \
                volume = excluded.volume, \
                issue = excluded.issue, \
                pages = excluded.pages, \
                issn = excluded.issn, \
                essn = excluded.essn, \
                language = excluded.language, \
                pub_types = excluded.pub_types, \
                keywords = excluded.keywords, \
                source = excluded.source, \
                created_at = COALESCE(articles.created_at, excluded.created_at), \
                updated_at = excluded.updated_at",
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

        // Replace synchronized child rows with one statement per table.
        conn.execute(
            "DELETE FROM authors WHERE article_id = ?1",
            turso::params![article.id.clone()],
        )
        .await?;
        for (chunk_index, author_chunk) in article.authors.chunks(500).enumerate() {
            let values = (0..author_chunk.len())
                .map(|row| {
                    let start = row * 8 + 1;
                    format!(
                        "(?{start}, ?{}, ?{}, ?{}, ?{}, ?{}, ?{}, ?{})",
                        start + 1,
                        start + 2,
                        start + 3,
                        start + 4,
                        start + 5,
                        start + 6,
                        start + 7
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "INSERT INTO authors \
                 (article_id, position, last_name, fore_name, initials, \
                  affiliation, orcid, corresponding) \
                 VALUES {values}"
            );
            let mut params = Vec::with_capacity(author_chunk.len() * 8);
            for (offset, author) in author_chunk.iter().enumerate() {
                let position = chunk_index * 500 + offset;
                params.push(Value::Text(article.id.clone()));
                params.push(Value::Integer(position as i64));
                params.push(Value::Text(author.last_name.clone()));
                params.push(opt_value(author.fore_name.clone()));
                params.push(opt_value(author.initials.clone()));
                params.push(opt_value(author.affiliation.clone()));
                params.push(opt_value(author.orcid.clone()));
                params.push(Value::Integer(author.corresponding as i64));
            }
            conn.execute(sql, turso::params_from_iter(params)).await?;
        }

        // Replace identifiers only after successfully replacing authors.
        conn.execute(
            "DELETE FROM identifiers WHERE article_id = ?1",
            turso::params![article.id.clone()],
        )
        .await?;
        for identifier_chunk in article.identifiers.chunks(500) {
            let values = (0..identifier_chunk.len())
                .map(|row| {
                    let start = row * 3 + 1;
                    format!("(?{start}, ?{}, ?{})", start + 1, start + 2)
                })
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!("INSERT INTO identifiers (article_id, kind, value) VALUES {values}");
            let mut params = Vec::with_capacity(identifier_chunk.len() * 3);
            for id in identifier_chunk {
                params.push(Value::Text(article.id.clone()));
                params.push(Value::Text(id.kind.as_str().to_owned()));
                params.push(Value::Text(id.value.clone()));
            }
            conn.execute(sql, turso::params_from_iter(params)).await?;
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

    /// Delete an article and (via FK `ON DELETE CASCADE`) all of its child
    /// rows: `authors`, `identifiers`, `annotations`, `collection_articles`
    /// memberships, and `fulltexts` pointer rows.
    ///
    /// Returns the number of `articles` rows actually removed — `0` when
    /// `id` did not exist (idempotent miss), `1` on a normal delete.
    /// Stored full-text *files* on disk are not touched by this method;
    /// full-text lifecycle is the caller's responsibility (see the HTTP
    /// `delete_article` handler in `tui-http` for the on-disk cleanup
    /// pattern).
    pub async fn delete_article(&self, id: &str) -> Result<u64> {
        let _write = self.write_gate.lock().await;
        let conn = self.write_conn();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        let n = tx
            .execute("DELETE FROM articles WHERE id = ?1", turso::params![id])
            .await?;
        if n > 0 {
            tx.execute(
                "DELETE FROM search_terms WHERE article_id = ?1",
                turso::params![id],
            )
            .await?;
        }
        tx.commit().await?;
        Ok(n)
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
    // Articles — paged listing
    // -----------------------------------------------------------------------

    /// List articles with offset/limit pagination, a whitelisted sort key,
    /// and optional collection / "unfiled" filtering.
    ///
    /// Returns the requested page plus the **filtered** total (the count
    /// before `offset`/`limit` are applied), which is what a paginated
    /// client needs to render page controls. Callers that only want the
    /// library size should keep using [`Self::article_count`].
    ///
    /// The sort column is mapped to a Rust comparator rather than to a SQL
    /// fragment: the candidate sets below are already plain ID lists, and
    /// sorting in Rust keeps every ordering decision out of string-built
    /// SQL. Local libraries are small enough (the HTTP layer caps `limit`
    /// at 2000) that the extra copy is not measurable.
    pub async fn list_articles_paged(&self, params: &ListParams) -> Result<(Vec<Article>, usize)> {
        // 1. Candidates — exactly one of three mutually exclusive ID sets.
        //    `collection_id` wins over `unfiled` so a caller that sends both
        //    gets a well-defined answer instead of a surprising empty page.
        let mut ids = if let Some(collection_id) = params.collection_id.as_deref() {
            let conn = self.conn();
            let mut rows = conn
                .query(
                    "SELECT article_id FROM collection_articles \
                     WHERE collection_id = ?1 ORDER BY position",
                    turso::params![collection_id],
                )
                .await?;
            let mut ids = Vec::new();
            while let Some(row) = rows.next().await? {
                ids.push(row.get::<String>(0)?);
            }
            ids
        } else if params.unfiled {
            // Everything that is not a member of any collection.
            let conn = self.conn();
            let mut rows = conn
                .query(
                    "SELECT id FROM articles \
                     WHERE id NOT IN (SELECT article_id FROM collection_articles) \
                     ORDER BY id",
                    turso::params![],
                )
                .await?;
            let mut ids = Vec::new();
            while let Some(row) = rows.next().await? {
                ids.push(row.get::<String>(0)?);
            }
            ids
        } else {
            self.list_article_ids().await?
        };
        // A collection membership list is already ordered by position, which
        // is a meaningful default; the other two branches are ID-ordered.
        // Either way the sort below makes the final order deterministic, so
        // deduping here (a membership table can technically hold an article
        // twice only if the PK were dropped) is just cheap insurance.
        ids.dedup();

        // 2. Load the candidates. Reusing `search_articles` keeps query
        //    semantics identical to the dedicated search endpoint; its
        //    relevance order is discarded by the sort below but its
        //    *filtering* is what narrows the page.
        let mut articles: Vec<Article> = Vec::new();
        if params.query.trim().is_empty() {
            for id in &ids {
                if let Some(article) = self.get_article(id).await? {
                    articles.push(article);
                }
            }
        } else {
            let hits = self
                .search_articles(&params.query, PAGED_SEARCH_CANDIDATE_CAP)
                .await?;
            let allowed: std::collections::HashSet<&str> =
                ids.iter().map(String::as_str).collect();
            for hit in hits {
                if !allowed.contains(hit.article_id.as_str()) {
                    continue;
                }
                if let Some(article) = self.get_article(&hit.article_id).await? {
                    articles.push(article);
                }
            }
        }

        // 3. Sort. `Option` keys compare with `None` first so a missing year
        //    or timestamp sorts deterministically instead of being
        //    implementation-defined.
        articles.sort_by(|left, right| {
            let ordering = match params.sort {
                SortField::CreatedAt => left.created_at.cmp(&right.created_at),
                SortField::UpdatedAt => left.updated_at.cmp(&right.updated_at),
                // Case-folded so "alpha" and "Beta" interleave the way a
                // reader expects rather than by code-point order.
                SortField::Title => left.title.to_lowercase().cmp(&right.title.to_lowercase()),
                SortField::Year => left.year.cmp(&right.year),
            };
            // Tiebreak on ID: without it, two articles sharing a key can be
            // split across pages in a different order on every request.
            let ordering = ordering.then_with(|| left.id.cmp(&right.id));
            if params.order == SortOrder::Descending {
                ordering.reverse()
            } else {
                ordering
            }
        });

        let total = articles.len();
        let page = articles
            .into_iter()
            .skip(params.offset)
            .take(params.limit)
            .collect::<Vec<_>>();
        Ok((page, total))
    }

    // -----------------------------------------------------------------------
    // Metadata — the `bib_meta` key/value store
    // -----------------------------------------------------------------------

    /// Read one value from the `bib_meta` key/value store.
    pub async fn get_meta(&self, key: &str) -> Result<Option<String>> {
        let conn = self.conn();
        let mut rows = conn
            .query(
                "SELECT value FROM bib_meta WHERE key = ?1",
                turso::params![key],
            )
            .await?;
        match rows.next().await? {
            Some(row) => Ok(Some(row.get::<String>(0)?)),
            None => Ok(None),
        }
    }

    /// Insert or update one value in the `bib_meta` key/value store.
    pub async fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        let _write = self.write_gate.lock().await;
        self.write_conn()
            .execute(
                "INSERT INTO bib_meta(key, value) VALUES (?1, ?2) \
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                turso::params![key, value],
            )
            .await?;
        Ok(())
    }

    /// List every `bib_meta` entry whose key starts with `prefix`.
    ///
    /// Uses a key range scan instead of `LIKE` so the caller's prefix needs
    /// no escaping of `%`/`_`, and so large blobs under unrelated prefixes
    /// (chat transcripts, caches) are never read just to be thrown away.
    pub async fn list_meta_prefixed(&self, prefix: &str) -> Result<Vec<(String, String)>> {
        let conn = self.conn();
        // The exclusive upper bound is the prefix with its last character
        // incremented; UTF-8 byte order matches code-point order, so no key
        // under this prefix can sort at or after it. `char::from_u32` only
        // fails on the surrogate gap, in which case the unbounded tail is
        // scanned and filtered in Rust instead.
        let upper = prefix_upper_bound(prefix);
        let sql = match upper.as_ref() {
            Some(_) => "SELECT key, value FROM bib_meta WHERE key >= ?1 AND key < ?2",
            None => "SELECT key, value FROM bib_meta WHERE key >= ?1",
        };
        let mut rows = match upper.as_ref() {
            Some(upper) => conn.query(sql, turso::params![prefix, upper.as_str()]).await?,
            None => conn.query(sql, turso::params![prefix]).await?,
        };

        let mut out = Vec::new();
        while let Some(row) = rows.next().await? {
            let key = row.get::<String>(0)?;
            // Re-check the prefix in Rust: the range scan is a superset
            // whenever the upper bound could not be built.
            if key.starts_with(prefix) {
                out.push((key, row.get::<String>(1)?));
            }
        }
        Ok(out)
    }

    // -----------------------------------------------------------------------
    // Search — indexed full-text search
    // -----------------------------------------------------------------------

    /// Search across article titles, abstracts, stored full-text
    /// content, **and user annotations** (notes/highlights/comments).
    /// Returns results ordered by match priority:
    /// title matches first, then abstract, then annotations, then full-text body.
    ///
    /// Uses an inverted index maintained in the same transaction as article,
    /// full-text, and annotation writes. A query matches when all of its
    /// lexical terms occur somewhere in the indexed fields.
    pub async fn search_articles(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        if query.trim().is_empty() {
            return self.list_search_hits(limit).await;
        }

        let terms = tokenize(query);
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        let term_placeholders = (1..=terms.len())
            .map(|index| format!("?{index}"))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT a.id, a.title, a.abstract, f.text_content, \
                    (SELECT GROUP_CONCAT(an.content, ' ') FROM annotations an \
                     WHERE an.article_id = a.id), \
                    SUM(CASE WHEN s.field = 'title' THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN s.field = 'abstract' THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN s.field = 'annotation' THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN s.field = 'fulltext' THEN 1 ELSE 0 END) \
             FROM search_terms s \
             JOIN articles a ON a.id = s.article_id \
             LEFT JOIN fulltexts f ON f.article_id = a.id \
             WHERE s.term IN ({term_placeholders}) \
             GROUP BY a.id, a.title, a.abstract, f.text_content \
             HAVING COUNT(DISTINCT s.term) = ?{} \
             ORDER BY CASE \
                 WHEN SUM(CASE WHEN s.field = 'title' THEN 1 ELSE 0 END) > 0 THEN 0 \
                 WHEN SUM(CASE WHEN s.field = 'abstract' THEN 1 ELSE 0 END) > 0 THEN 1 \
                 WHEN SUM(CASE WHEN s.field = 'annotation' THEN 1 ELSE 0 END) > 0 THEN 2 \
                 ELSE 3 \
             END, COUNT(*) DESC \
             LIMIT ?{}",
            terms.len() + 1,
            terms.len() + 2
        );
        let mut params: Vec<Value> = terms.iter().map(|term| Value::Text(term.clone())).collect();
        params.push(Value::Integer(terms.len() as i64));
        params.push(Value::Integer(limit as i64));

        let conn = self.conn();
        let mut rows = conn.query(sql, turso::params_from_iter(params)).await?;

        let mut hits = Vec::new();
        while let Some(row) = rows.next().await? {
            let article_id = row.get::<String>(0)?;
            let title = row.get::<String>(1)?;
            let abstract_text = opt_string(row.get_value(2)?);
            let fulltext = opt_string(row.get_value(3)?);
            let annotation = opt_string(row.get_value(4)?);
            let title_matches = row.get::<i64>(5)?;
            let abstract_matches = row.get::<i64>(6)?;
            let annotation_matches = row.get::<i64>(7)?;
            let _fulltext_matches = row.get::<i64>(8)?;
            let score = if title_matches > 0 {
                0.0
            } else if abstract_matches > 0 {
                1.0
            } else if annotation_matches > 0 {
                2.0
            } else {
                3.0
            };

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
                score,
                snippet,
            });
        }
        Ok(hits)
    }

    async fn list_search_hits(&self, limit: usize) -> Result<Vec<SearchHit>> {
        let conn = self.conn();
        let mut rows = conn
            .query(
                "SELECT id, title FROM articles ORDER BY title LIMIT ?1",
                turso::params![limit as i64],
            )
            .await?;
        let mut hits = Vec::new();
        while let Some(row) = rows.next().await? {
            let title = row.get::<String>(1)?;
            hits.push(SearchHit {
                article_id: row.get::<String>(0)?,
                snippet: extract_snippet("", &title, None, None, None),
                title,
                score: 0.0,
            });
        }
        Ok(hits)
    }
}

/// Add `column` to `table` when the database predates it.
///
/// `ddl` is the full column definition (name included) so this mirrors the
/// `CREATE TABLE` text in [`SCHEMA_SQL`]. Both identifiers come from
/// compile-time constants in this module — never from user input — so the
/// `format!` here is not an injection vector.
///
/// Returns `true` when the column had to be added.
///
/// The column list comes from the `PRAGMA table_info` *statement*. It must
/// not come from the equivalent table-valued form
/// (`SELECT name FROM pragma_table_info(…)`, which SQLite also accepts):
/// turso 0.7 answers that query correctly but leaves the connection pinned
/// to the pre-write snapshot, so every later read on that connection sees a
/// stale database. The statement form has no such side effect.
async fn ensure_column(conn: &Connection, table: &str, column: &str, ddl: &str) -> Result<bool> {
    let mut rows = conn
        .query(&format!("PRAGMA table_info({table})"), turso::params![])
        .await?;
    let mut present = false;
    while let Some(row) = rows.next().await? {
        if row.get::<String>(1)? == column {
            present = true;
            break;
        }
    }
    drop(rows);

    if present {
        return Ok(false);
    }
    conn.execute(
        &format!("ALTER TABLE {table} ADD COLUMN {ddl}"),
        turso::params![],
    )
    .await?;
    Ok(true)
}

pub(crate) async fn sync_search_index(conn: &Connection, article_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM search_terms WHERE article_id = ?1",
        turso::params![article_id],
    )
    .await?;

    let mut rows = conn
        .query(
            "SELECT a.title, a.abstract, f.text_content, \
                    (SELECT GROUP_CONCAT(an.content, ' ') FROM annotations an \
                     WHERE an.article_id = a.id) \
             FROM articles a LEFT JOIN fulltexts f ON f.article_id = a.id \
             WHERE a.id = ?1",
            turso::params![article_id],
        )
        .await?;
    let Some(row) = rows.next().await? else {
        return Ok(());
    };

    let fields = [
        ("title", row.get::<String>(0)?),
        (
            "abstract",
            opt_string(row.get_value(1)?).unwrap_or_default(),
        ),
        (
            "fulltext",
            opt_string(row.get_value(2)?).unwrap_or_default(),
        ),
        (
            "annotation",
            opt_string(row.get_value(3)?).unwrap_or_default(),
        ),
    ];

    for (field, text) in fields {
        let terms: Vec<_> = tokenize(&text).into_iter().collect();
        if terms.is_empty() {
            continue;
        }
        let placeholders = (0..terms.len())
            .map(|row| {
                let start = row * 3 + 1;
                format!("(?{start}, ?{}, ?{})", start + 1, start + 2)
            })
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "INSERT OR IGNORE INTO search_terms(term, article_id, field) VALUES {placeholders}"
        );
        let mut params = Vec::with_capacity(terms.len() * 3);
        for term in terms {
            params.push(Value::Text(term));
            params.push(Value::Text(article_id.to_owned()));
            params.push(Value::Text(field.to_owned()));
        }
        conn.execute(sql, turso::params_from_iter(params)).await?;
    }
    Ok(())
}

/// Smallest string that sorts after every key beginning with `prefix`.
///
/// Used to turn a prefix search into a bounded key range. Returns `None`
/// when no such string exists (the last character sits just below the
/// surrogate gap), signalling "scan the unbounded tail instead".
fn prefix_upper_bound(prefix: &str) -> Option<String> {
    let last = prefix.chars().last()?;
    let next = char::from_u32(last as u32 + 1)?;
    let head = &prefix[..prefix.len() - last.len_utf8()];
    Some(format!("{head}{next}"))
}

fn tokenize(text: &str) -> Vec<String> {
    let mut terms: Vec<String> = Vec::new();
    let mut current = String::new();
    for character in text.chars() {
        if character.is_alphanumeric() {
            current.extend(character.to_lowercase());
        } else if !current.is_empty() {
            terms.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        terms.push(current);
    }
    terms
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
            // Snap to UTF-8 char boundaries so we never slice mid-character.
            let start = text.floor_char_boundary(start);
            let end = text.ceil_char_boundary(end);
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

pub(crate) fn opt_string(v: Value) -> Option<String> {
    match v {
        Value::Text(s) => Some(s),
        Value::Null => None,
        _ => None,
    }
}

fn opt_value(value: Option<String>) -> Value {
    value.map_or(Value::Null, Value::Text)
}

pub(crate) fn opt_int(v: Value) -> Option<i64> {
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
