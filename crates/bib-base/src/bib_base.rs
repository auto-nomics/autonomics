//! SQLite (turso) storage layer for the bibliography library.
//!
//! [`BibBase`] is a cheap handle to a spawned storage actor (see
//! [`crate::actor`]) that owns the turso connections exclusively. All
//! public methods take `&self`, send one command, and await its reply —
//! so an `Arc<BibBase>` shared by concurrent DAG nodes, the TUI, or
//! several agent sessions serializes on the actor instead of racing on
//! the connection (turso connections reject concurrent use with
//! `concurrent use forbidden`).

use tokio::sync::mpsc;
use turso::{
    Builder, Connection, Value,
    transaction::{Transaction, TransactionBehavior},
};

use crate::actor::BibCommand;
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
    color       TEXT,
    rects       TEXT,
    comment     TEXT NOT NULL DEFAULT '',
    tags        TEXT NOT NULL DEFAULT '[]',
    created_at  TEXT
);

CREATE TABLE IF NOT EXISTS fulltexts (
    article_id   TEXT PRIMARY KEY REFERENCES articles(id) ON DELETE CASCADE,
    file_path    TEXT NOT NULL,
    file_format  TEXT NOT NULL,
    text_content TEXT,
    source       TEXT NOT NULL,
    file_hash    TEXT,
    file_size    INTEGER,
    uploaded_at  TEXT,
    extract_status TEXT,
    text_format   TEXT,
    extracted_by  TEXT,
    extract_error TEXT
);

CREATE TABLE IF NOT EXISTS search_terms (
    term       TEXT NOT NULL,
    article_id TEXT NOT NULL,
    field      TEXT NOT NULL,
    PRIMARY KEY (term, article_id, field)
);

CREATE INDEX IF NOT EXISTS idx_search_terms_article ON search_terms(article_id);

CREATE TABLE IF NOT EXISTS searchOutbox (
    articleID TEXT PRIMARY KEY,
    revision INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS searchOutbox_revision ON searchOutbox(revision);

CREATE TABLE IF NOT EXISTS bib_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
";

// ---------------------------------------------------------------------------
// BibBase
// ---------------------------------------------------------------------------

/// Runtime rollout and rollback policy for local search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchEngineMode {
    Tantivy,
    Legacy,
}

/// Bibliography database handle.
///
/// A cheap handle to the storage actor ([`crate::actor`]) that owns the
/// turso connections. Every method sends one command and awaits the
/// reply, so any number of concurrent callers sharing an `Arc<BibBase>`
/// queue in FIFO order on the actor — two simultaneous readers can no
/// longer trip turso's `concurrent use forbidden` guard (issue #143).
pub struct BibBase {
    pub(crate) tx: mpsc::Sender<BibCommand>,
    pub(crate) changes: tokio::sync::watch::Sender<u64>,
    pub(crate) semantic: Option<crate::semantic_service::SemanticService>,
}

/// Implementation core owned by the storage actor.
///
/// Holds the two turso connections (reads and writes split so a write
/// commits on its own connection). Only the actor task ever touches
/// this struct — one command at a time — so no interior gating is
/// needed anywhere.
pub(crate) struct BibCore {
    conn: Connection,
    write_conn: Connection,
    pub(crate) search_engine: SearchEngineMode,
    pub(crate) search_index: std::sync::Arc<std::sync::Mutex<crate::local_index::LocalIndex>>,
    pub(crate) search_progress: std::sync::Arc<std::sync::Mutex<crate::local_index::IndexProgress>>,
    pub(crate) search_context: Option<Box<BibCore>>,
    pub(crate) semantic: Option<crate::semantic_service::SemanticService>,
}

#[derive(Clone, serde::Serialize)]
pub(crate) struct SearchCandidate {
    pub doc_id: String,
    pub article_id: String,
    pub anchor_key: String,
    pub kind: String,
    pub title: String,
    pub year: Option<i64>,
    pub score: f64,
    pub matched_field: String,
    pub snippet_query: String,
}

impl BibBase {
    /// Open (or create) a local SQLite database file and run migrations.
    ///
    /// Uses turso's `multiprocess_wal` engine feature so that multiple
    /// processes (e.g. two TUI instances) can open the same file without
    /// one blocking the other with a `database is locked` error. WAL mode
    /// is implied by that feature; `busy_timeout` is set per-connection so
    /// contended writes wait briefly instead of erroring immediately.
    ///
    /// The connections live inside a spawned storage actor (see
    /// [`crate::actor`]); this handle and its clones keep it alive.
    pub async fn open(path: impl AsRef<str>) -> Result<Self> {
        let core = BibCore::open(path).await?;
        Ok(crate::actor::spawn(core))
    }

    /// Select an engine explicitly, bypassing the process rollout flag.
    pub async fn open_with_search_engine(
        path: impl AsRef<str>,
        engine: SearchEngineMode,
    ) -> Result<Self> {
        Ok(crate::actor::spawn(
            BibCore::open_with_search_engine(path, engine).await?,
        ))
    }

    /// Open an in-memory database (useful for tests).
    pub async fn open_in_memory() -> Result<Self> {
        Self::open(":memory:").await
    }
}

impl BibCore {
    /// Open (or create) the connections and run migrations.
    pub(crate) async fn open(path: impl AsRef<str>) -> Result<Self> {
        let engine = match std::env::var("AUTONOMICS_BIB_SEARCH_ENGINE")
            .unwrap_or_else(|_| "tantivy".into())
            .to_ascii_lowercase()
            .as_str()
        {
            "tantivy" => SearchEngineMode::Tantivy,
            "legacy" => SearchEngineMode::Legacy,
            _ => {
                return Err(Error::Unknown(
                    "AUTONOMICS_BIB_SEARCH_ENGINE must be tantivy or legacy".into(),
                ));
            }
        };
        Self::open_with_search_engine(path, engine).await
    }

    pub(crate) async fn open_with_search_engine(
        path: impl AsRef<str>,
        engine: SearchEngineMode,
    ) -> Result<Self> {
        let path = path.as_ref();
        let mut builder = Builder::new_local(path);
        if path != ":memory:" {
            builder = builder.experimental_multiprocess_wal(true);
        }
        let db = builder.build().await?;
        let conn = db.connect()?;
        let write_conn = db.connect()?;
        if path != ":memory:" {
            conn.pragma_update("busy_timeout", 5000).await?;
            write_conn.pragma_update("busy_timeout", 5000).await?;
        }
        // Enable FK enforcement so the `ON DELETE CASCADE` clauses declared in
        // [`SCHEMA_SQL`] actually fire. Without this pragma, SQLite parses and
        // stores FK constraints but does not enforce them, leaving orphan rows
        // (e.g. annotations referencing a deleted article). This matters most
        // for [`Self::delete_article`] — callers expect the cascade to clean
        // up authors, identifiers, annotations, `collection_articles`
        // memberships, and `fulltexts` pointer rows.
        for conn in [&conn, &write_conn] {
            conn.pragma_update("foreign_keys", true).await?;
        }
        let index_path = if engine == SearchEngineMode::Legacy {
            ":memory:".to_owned()
        } else {
            path.to_owned()
        };
        let search_index =
            tokio::task::spawn_blocking(move || crate::local_index::LocalIndex::open(&index_path))
                .await
                .map_err(|e| Error::Unknown(format!("open search index task: {e}")))??;
        let search_progress = search_index.progress.clone();
        let mut base = Self {
            conn,
            write_conn,
            search_engine: engine,
            search_index: std::sync::Arc::new(std::sync::Mutex::new(search_index)),
            search_progress,
            search_context: None,
            semantic: None,
        };
        base.migrate().await?;
        if engine == SearchEngineMode::Tantivy {
            let vector_path = if path == ":memory:" {
                path.to_owned()
            } else {
                format!("{path}.semantic-v1")
            };
            base.semantic = Some(
                crate::semantic_service::SemanticService::open(
                    db.connect()?,
                    db.connect()?,
                    db.connect()?,
                    vector_path,
                )
                .await?,
            );
            let reader = db.connect()?;
            if path != ":memory:" {
                reader.pragma_update("busy_timeout", 5000).await?;
            }
            base.search_context = Some(Box::new(Self {
                conn: reader.clone(),
                write_conn: reader,
                search_engine: engine,
                search_index: base.search_index.clone(),
                search_progress: base.search_progress.clone(),
                search_context: None,
                semantic: base.semantic.clone(),
            }));
        }
        Ok(base)
    }

    /// Clone the read connection — cheap (Arc-based).
    pub(crate) fn conn(&self) -> Connection {
        self.conn.clone()
    }

    /// Clone the dedicated writer connection.
    pub(crate) fn write_conn(&self) -> Connection {
        self.write_conn.clone()
    }

    // -----------------------------------------------------------------------
    // Schema
    // -----------------------------------------------------------------------

    /// Run all DDL statements (idempotent — safe to call on every open).
    pub async fn migrate(&self) -> Result<()> {
        let conn = self.write_conn();
        conn.execute_batch(SCHEMA_SQL).await?;
        crate::collection_schema::prepare_legacy(&conn)
            .await
            .map_err(|e| Error::Unknown(format!("prepare collection schema: {e}")))?;

        // --- fulltexts extraction-tracking columns -----------------------
        // Databases created before background extraction tracking lack the
        // four status/provenance columns. `CREATE TABLE IF NOT EXISTS`
        // cannot add them to an existing table, so detect-and-ALTER here,
        // then backfill the initial status from text presence: rows with
        // text are already final; rows without it are failed extractions
        // that a later re-extract can retry.
        let mut has_extract_status = false;
        {
            let mut rows = conn
                .query("PRAGMA table_info(fulltexts)", turso::params![])
                .await?;
            while let Some(row) = rows.next().await? {
                if row.get::<String>(1)? == "extract_status" {
                    has_extract_status = true;
                }
            }
        }
        if !has_extract_status {
            conn.execute_batch(
                "ALTER TABLE fulltexts ADD COLUMN extract_status TEXT;
                 ALTER TABLE fulltexts ADD COLUMN text_format TEXT;
                 ALTER TABLE fulltexts ADD COLUMN extracted_by TEXT;
                 ALTER TABLE fulltexts ADD COLUMN extract_error TEXT;
                 UPDATE fulltexts SET extract_status = \
                  CASE WHEN text_content IS NOT NULL AND LENGTH(text_content) > 0 \
                  THEN 'done' ELSE 'failed' END;
                 UPDATE fulltexts SET text_format = 'plain' \
                  WHERE extract_status = 'done';",
            )
            .await?;
        }

        // --- annotation PDF anchors --------------------------------------
        // Databases created before PDF-reader highlights lack normalized
        // geometry and color. Existing notes remain valid with both null.
        let mut has_annotation_rects = false;
        {
            let mut rows = conn
                .query("PRAGMA table_info(annotations)", turso::params![])
                .await?;
            while let Some(row) = rows.next().await? {
                if row.get::<String>(1)? == "rects" {
                    has_annotation_rects = true;
                }
            }
        }
        if !has_annotation_rects {
            conn.execute_batch(
                "ALTER TABLE annotations ADD COLUMN color TEXT;
                 ALTER TABLE annotations ADD COLUMN rects TEXT;",
            )
            .await?;
        }

        let mut annotation_columns = std::collections::HashSet::new();
        let mut rows = conn
            .query("PRAGMA table_info(annotations)", turso::params![])
            .await?;
        while let Some(row) = rows.next().await? {
            annotation_columns.insert(row.get::<String>(1)?);
        }
        drop(rows);
        if !annotation_columns.contains("comment") {
            conn.execute(
                "ALTER TABLE annotations ADD COLUMN comment TEXT NOT NULL DEFAULT ''",
                turso::params![],
            )
            .await?;
        }
        if !annotation_columns.contains("tags") {
            conn.execute(
                "ALTER TABLE annotations ADD COLUMN tags TEXT NOT NULL DEFAULT '[]'",
                turso::params![],
            )
            .await?;
        }

        crate::saved_searches::ensure(&conn).await?;
        crate::saved_searches::ensure(&self.conn()).await?;
        crate::discovery_import::ensure(&conn).await?;
        crate::discovery_import::ensure(&self.conn()).await?;

        // Mirror existing legacy rows into Zotero-compatible item tables.
        // This runs after legacy column backfills because it reads
        // extraction provenance from `fulltexts`.
        crate::zotero_model::migrate(&conn).await?;
        // Turso prepares statements per connection. Applying the same
        // idempotent DDL to the read side ensures freshly added extension
        // columns are visible to queries made through `BibCore::conn()`.
        let read_conn = self.conn();
        crate::zotero_model::ensure_schema(&read_conn).await?;
        crate::collection_schema::migrate(&conn)
            .await
            .map_err(|e| Error::Unknown(format!("migrate collection schema: {e}")))?;

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
        drop(rows);
        let mut rows = conn
            .query(
                "SELECT value FROM bib_meta WHERE key='search_revision_token'",
                (),
            )
            .await?;
        let token_valid = rows
            .next()
            .await?
            .map(|row| row.get::<String>(0))
            .transpose()?
            .is_some_and(|token| uuid::Uuid::parse_str(&token).is_ok());
        drop(rows);
        if version != "11" || !token_valid {
            let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
            let result = async {
                crate::search_projection::ensure(&tx,true).await?;
                tx.execute("INSERT INTO bib_meta(key,value) VALUES ('search_revision','1') ON CONFLICT(key) DO UPDATE SET value=CAST(bib_meta.value AS INTEGER)+1",()).await?;
                tx.execute("INSERT INTO bib_meta(key,value) VALUES ('search_revision_token',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",turso::params![uuid::Uuid::new_v4().to_string()]).await?;
                tx.execute("INSERT INTO searchOutbox(articleID,revision) SELECT id,(SELECT CAST(value AS INTEGER) FROM bib_meta WHERE key='search_revision') FROM articles WHERE 1=1 ON CONFLICT(articleID) DO UPDATE SET revision=excluded.revision",()).await?;
                tx.execute("INSERT INTO bib_meta(key,value) VALUES ('search_index_version','11') ON CONFLICT(key) DO UPDATE SET value=excluded.value",()).await?;
                Ok::<_,Error>(())
            }.await;
            if let Err(error) = result {
                let _ = tx.rollback().await;
                return Err(error);
            }
            tx.commit().await?;
        } else {
            crate::search_projection::ensure(&conn, false).await?;
        }
        crate::search_projection::ensure(&read_conn, false).await?;

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

        let conn = self.write_conn();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        let mut result = Ok(());
        for article in articles {
            if let Err(err) = async {
                Self::upsert_article_in_tx(&tx, article).await?;
                crate::zotero_model::ensure_article_item(&tx, &article.id).await?;
                crate::zotero_model::sync_article_item(&tx, &article.id).await?;
                sync_search_index(&tx, &article.id).await
            }
            .await
            {
                result = Err(err);
                break;
            }
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
    pub(crate) async fn upsert_article_in_tx(conn: &Connection, article: &Article) -> Result<()> {
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

        // Close the header cursor before issuing the child-row queries —
        // an open turso cursor keeps the connection's statement state
        // alive, and a second statement overlapping it trips the
        // connection's concurrent-use guard even on a single task.
        drop(rows);

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

        let article_id = match rows.next().await? {
            Some(row) => row.get::<String>(0)?,
            None => return Ok(None),
        };
        drop(rows);
        self.get_article(&article_id).await
    }

    /// Delete an article and (via FK `ON DELETE CASCADE`) all of its child
    /// rows: `authors`, `identifiers`, `annotations`, `collection_articles`
    /// memberships, and `fulltexts` pointer rows.
    ///
    /// Returns the number of `articles` rows actually removed — `0` when
    /// `id` did not exist (idempotent miss), `1` on a normal delete.
    /// Stored full-text *files* on disk are not touched by this method;
    /// full-text lifecycle is the caller's responsibility (see the HTTP
    /// `delete_article` handler in `api-server` for the on-disk cleanup
    /// pattern).
    pub async fn delete_article(&self, id: &str) -> Result<u64> {
        let conn = self.write_conn();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        let result = async {
            crate::zotero_model::delete_article_item(&tx, id).await?;
            let n = tx
                .execute("DELETE FROM articles WHERE id = ?1", turso::params![id])
                .await?;
            if n > 0 {
                tx.execute(
                    "DELETE FROM searchTerms WHERE articleID = ?1",
                    turso::params![id],
                )
                .await?;
            }
            if n > 0 {
                bump_search_revision(&tx, id).await?;
            }
            Ok::<_, Error>(n)
        }
        .await;
        match result {
            Ok(n) => {
                tx.commit().await?;
                Ok(n)
            }
            Err(error) => {
                let _ = tx.rollback().await;
                Err(error)
            }
        }
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
    // Search — indexed full-text search
    // -----------------------------------------------------------------------

    pub async fn search_articles(
        &self,
        query: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<SearchHit>> {
        self.search_legacy(query, limit, offset).await
    }
}

// ---------------------------------------------------------------------------
// BibBase — thin handle methods, one command round-trip each
// ---------------------------------------------------------------------------

impl BibBase {
    /// Run all DDL statements (idempotent — safe to call on every open).
    pub async fn migrate(&self) -> Result<()> {
        self.call(|reply| BibCommand::Migrate { reply }).await
    }

    /// Insert or update a single article and its child rows (authors,
    /// identifiers).
    pub async fn upsert_article(&self, article: &Article) -> Result<()> {
        self.call(|reply| BibCommand::UpsertArticle {
            article: article.clone(),
            reply,
        })
        .await
    }

    /// Insert or update articles and their child rows in one transaction.
    pub async fn upsert_articles(&self, articles: &[Article]) -> Result<()> {
        self.call(|reply| BibCommand::UpsertArticles {
            articles: articles.to_vec(),
            reply,
        })
        .await
    }

    /// Look up a single article by its internal ID.
    pub async fn get_article(&self, id: &str) -> Result<Option<Article>> {
        self.call(|reply| BibCommand::GetArticle {
            id: id.to_owned(),
            reply,
        })
        .await
    }

    /// Find an article by an external identifier (DOI, PMID, …).
    pub async fn find_by_identifier(&self, kind: IdKind, value: &str) -> Result<Option<Article>> {
        self.call(|reply| BibCommand::FindByIdentifier {
            kind,
            value: value.to_owned(),
            reply,
        })
        .await
    }

    /// Delete an article and (via FK `ON DELETE CASCADE`) all of its child
    /// rows. Returns `0` on an idempotent miss, `1` on a normal delete.
    pub async fn delete_article(&self, id: &str) -> Result<u64> {
        self.call(|reply| BibCommand::DeleteArticle {
            id: id.to_owned(),
            reply,
        })
        .await
    }

    /// Count articles in the library.
    pub async fn article_count(&self) -> Result<i64> {
        self.call(|reply| BibCommand::ArticleCount { reply }).await
    }

    /// List all article IDs in the library.
    pub async fn list_article_ids(&self) -> Result<Vec<String>> {
        self.call(|reply| BibCommand::ListArticleIds { reply })
            .await
    }

    /// List all articles (full records) in the library.
    pub async fn list_all_articles(&self) -> Result<Vec<Article>> {
        self.call(|reply| BibCommand::ListAllArticles { reply })
            .await
    }

    /// Search across titles, abstracts, stored full text, and annotations.
    pub async fn search_articles(
        &self,
        query: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<SearchHit>> {
        self.call(|reply| BibCommand::SearchArticles {
            query: query.to_owned(),
            limit,
            offset,
            reply,
        })
        .await
    }

    /// Return the Zotero-style preferred attachment for an article.
    pub async fn zotero_attachment(
        &self,
        article_id: &str,
    ) -> Result<Option<crate::zotero_model::StoredAttachment>> {
        self.call(|reply| crate::actor::BibCommand::GetZoteroAttachment {
            article_id: article_id.to_owned(),
            reply,
        })
        .await
    }

    /// Return Zotero-style full-text index state for an attachment item.
    pub async fn zotero_fulltext_state(
        &self,
        item_id: i64,
    ) -> Result<Option<crate::zotero_model::FulltextItemState>> {
        self.call(|reply| crate::actor::BibCommand::GetZoteroFulltextState { item_id, reply })
            .await
    }

    /// List all Zotero-style attachment child items for an article.
    pub async fn zotero_attachments(
        &self,
        article_id: &str,
    ) -> Result<Vec<crate::zotero_model::StoredAttachment>> {
        self.call(|reply| crate::actor::BibCommand::ListZoteroAttachments {
            article_id: article_id.to_owned(),
            reply,
        })
        .await
    }

    /// Return compact attachment-type flags for many articles in one actor
    /// round trip.
    pub async fn attachment_type_summaries(
        &self,
        article_ids: &[String],
    ) -> Result<Vec<crate::zotero_model::AttachmentTypeSummary>> {
        self.call(
            |reply| crate::actor::BibCommand::ListAttachmentTypeSummaries {
                article_ids: article_ids.to_vec(),
                reply,
            },
        )
        .await
    }

    /// Return one Zotero attachment by its item key.
    pub async fn zotero_attachment_by_key(
        &self,
        article_id: &str,
        key: &str,
    ) -> Result<Option<crate::zotero_model::StoredAttachment>> {
        self.call(|reply| crate::actor::BibCommand::GetZoteroAttachmentByKey {
            article_id: article_id.to_owned(),
            key: key.to_owned(),
            reply,
        })
        .await
    }

    /// Create a new Zotero attachment child item.
    pub async fn create_zotero_attachment(
        &self,
        attachment: crate::zotero_model::NewAttachment,
        make_preferred: bool,
    ) -> Result<crate::zotero_model::StoredAttachment> {
        self.call(|reply| crate::actor::BibCommand::CreateZoteroAttachment {
            attachment,
            make_preferred,
            reply,
        })
        .await
    }

    /// Make an existing attachment the compatibility full text.
    pub async fn set_preferred_zotero_attachment(
        &self,
        article_id: &str,
        key: &str,
    ) -> Result<bool> {
        self.call(
            |reply| crate::actor::BibCommand::SetPreferredZoteroAttachment {
                article_id: article_id.to_owned(),
                key: key.to_owned(),
                reply,
            },
        )
        .await
    }

    /// Attach VFS object metadata to an already-created Zotero attachment.
    pub async fn set_zotero_attachment_storage(
        &self,
        article_id: &str,
        key: &str,
        vfs_path: &str,
        storage_hash: &str,
        file_size: i64,
    ) -> Result<Option<crate::zotero_model::StoredAttachment>> {
        self.call(
            |reply| crate::actor::BibCommand::SetZoteroAttachmentStorage {
                article_id: article_id.to_owned(),
                key: key.to_owned(),
                vfs_path: vfs_path.to_owned(),
                storage_hash: storage_hash.to_owned(),
                file_size,
                reply,
            },
        )
        .await
    }

    /// Delete an attachment child item.
    pub async fn delete_zotero_attachment(
        &self,
        article_id: &str,
        key: &str,
    ) -> Result<Option<crate::zotero_model::DeletedAttachment>> {
        self.call(|reply| crate::actor::BibCommand::DeleteZoteroAttachment {
            article_id: article_id.to_owned(),
            key: key.to_owned(),
            reply,
        })
        .await
    }
}

pub(crate) async fn sync_search_index(conn: &Connection, article_id: &str) -> Result<()> {
    bump_search_revision(conn, article_id).await
}

/// Rebuild only when the explicit rollback engine needs its derived terms.
pub(crate) async fn sync_legacy_search_terms(conn: &Connection, article_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM searchTerms WHERE articleID = ?1",
        turso::params![article_id],
    )
    .await?;

    let mut article_rows = conn
        .query(
            "SELECT a.title, COALESCE(a.abstract,''), I.key FROM articles a JOIN articleItems MAP ON MAP.article_id=a.id JOIN items I ON I.itemID=MAP.itemID WHERE a.id=?1",
            turso::params![article_id],
        )
        .await?;
    let Some(article) = article_rows.next().await? else {
        return Ok(());
    };
    let title = article.get::<String>(0)?;
    let abstract_text = article.get::<String>(1)?;
    let article_key = article.get::<String>(2)?;
    drop(article_rows);

    insert_search_terms(
        conn,
        &article_key,
        article_id,
        "title",
        &tokenize(&title).into_iter().collect::<Vec<_>>(),
    )
    .await?;
    insert_search_terms(
        conn,
        &article_key,
        article_id,
        "abstract",
        &tokenize(&abstract_text).into_iter().collect::<Vec<_>>(),
    )
    .await?;

    let mut attachment_rows = conn
        .query(
            "SELECT I.key, F.textContent \
             FROM articleItems MAP \
             JOIN itemAttachments AI ON AI.parentItemID = MAP.itemID \
             JOIN items I ON I.itemID = AI.itemID \
             LEFT JOIN fulltextItems F ON F.itemID = AI.itemID \
             WHERE MAP.article_id = ?1",
            turso::params![article_id],
        )
        .await?;
    let mut attachment_texts = Vec::new();
    while let Some(row) = attachment_rows.next().await? {
        attachment_texts.push((
            row.get::<String>(0)?,
            opt_string(row.get_value(1)?).unwrap_or_default(),
        ));
    }
    drop(attachment_rows);
    for (attachment_key, text) in attachment_texts {
        insert_search_terms(
            conn,
            &attachment_key,
            article_id,
            "fulltext",
            &tokenize(&text).into_iter().collect::<Vec<_>>(),
        )
        .await?;
    }

    let mut annotation_rows = conn
        .query(
            "SELECT COALESCE(MI.key, A.id), A.content || ' ' || A.comment || ' ' || A.tags \
             FROM annotations A \
             LEFT JOIN annotationItems MAP ON MAP.annotation_id = A.id \
             LEFT JOIN items MI ON MI.itemID = MAP.itemID \
             LEFT JOIN itemAnnotations IA ON IA.itemID = MAP.itemID \
             LEFT JOIN itemAttachments PA ON PA.itemID = IA.parentItemID \
             LEFT JOIN items PI ON PI.itemID = PA.itemID \
             WHERE A.article_id = ?1",
            turso::params![article_id],
        )
        .await?;
    let mut annotations = Vec::new();
    while let Some(row) = annotation_rows.next().await? {
        annotations.push((row.get::<String>(0)?, row.get::<String>(1)?));
    }
    drop(annotation_rows);
    for (anchor_key, text) in annotations {
        insert_search_terms(
            conn,
            &anchor_key,
            article_id,
            "annotation",
            &tokenize(&text).into_iter().collect::<Vec<_>>(),
        )
        .await?;
    }
    Ok(())
}

async fn bump_search_revision(conn: &Connection, article_id: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO bib_meta(key, value) VALUES ('search_revision', '1') \
         ON CONFLICT(key) DO UPDATE SET value = CAST(bib_meta.value AS INTEGER) + 1",
        (),
    )
    .await?;
    conn.execute("INSERT INTO bib_meta(key,value) VALUES ('search_revision_token',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",turso::params![uuid::Uuid::new_v4().to_string()]).await?;
    conn.execute("INSERT INTO searchOutbox(articleID, revision) VALUES (?1, (SELECT CAST(value AS INTEGER) FROM bib_meta WHERE key='search_revision')) ON CONFLICT(articleID) DO UPDATE SET revision=excluded.revision", turso::params![article_id]).await?;
    Ok(())
}

async fn insert_search_terms(
    conn: &Connection,
    anchor_key: &str,
    article_id: &str,
    field: &str,
    terms: &[String],
) -> Result<()> {
    for chunk in terms.chunks(200) {
        if chunk.is_empty() {
            continue;
        }
        let placeholders = (0..chunk.len())
            .map(|row| {
                let start = row * 4 + 1;
                format!("(?{start}, ?{}, ?{}, ?{})", start + 1, start + 2, start + 3)
            })
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "INSERT OR IGNORE INTO searchTerms(term, anchorKey, field, articleID) VALUES {placeholders}"
        );
        let mut params = Vec::with_capacity(chunk.len() * 4);
        for term in chunk {
            params.push(Value::Text(term.clone()));
            params.push(Value::Text(anchor_key.to_owned()));
            params.push(Value::Text(field.to_owned()));
            params.push(Value::Text(article_id.to_owned()));
        }
        conn.execute(sql, turso::params_from_iter(params)).await?;
    }
    Ok(())
}

pub(crate) fn tokenize(text: &str) -> Vec<String> {
    crate::search_analyzer::analyze(text)
        .into_iter()
        .map(|token| token.text)
        .collect()
}

/// Return the distinct lexical terms in first-seen order for boolean search.
/// Indexing continues to use [`tokenize`] directly; only query clauses need
/// duplicate removal because `searchTerms` is a set of terms per field.
pub(crate) fn unique_query_terms(text: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    tokenize(text)
        .into_iter()
        .filter(|term| seen.insert(term.clone()))
        .collect()
}

pub(crate) fn extract_snippet_from_text(query: &str, text: &str) -> Option<String> {
    const SNIPPET_RADIUS: usize = 80;
    let query_tokens = crate::search_analyzer::position_tokens(query);
    let query_terms = query_tokens
        .iter()
        .map(|token| token.text.clone())
        .collect::<Vec<_>>();
    if query_terms.is_empty() {
        return None;
    }
    // Retain original byte ranges while normalizing exactly as the index does.
    // Substring matches (gene inside genome) are not indexed word hits.
    let tokens = crate::search_analyzer::position_tokens(text)
        .into_iter()
        .map(|token| {
            (
                token.text,
                token.offset_from,
                token.offset_to,
                token.position,
            )
        })
        .collect::<Vec<_>>();
    let phrase = tokens.windows(query_terms.len()).position(|window| {
        window.iter().zip(&query_tokens).all(|(token, query)| {
            token.0 == query.text
                && token.3 - window[0].3 == query.position - query_tokens[0].position
        })
    });
    let (first, last) = if let Some(first) = phrase {
        (first, first + query_terms.len() - 1)
    } else {
        let terms = query_terms.iter().collect::<std::collections::HashSet<_>>();
        let first = tokens.iter().position(|token| terms.contains(&token.0))?;
        (first, first)
    };
    let start = text.floor_char_boundary(tokens[first].1.saturating_sub(SNIPPET_RADIUS));
    let end = text.ceil_char_boundary((tokens[last].2 + SNIPPET_RADIUS).min(text.len()));
    let prefix = if start > 0 { "…" } else { "" };
    let suffix = if end < text.len() { "…" } else { "" };
    Some(format!("{prefix}{}{suffix}", &text[start..end]))
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

pub(crate) fn parse_json_col(raw: &Option<String>) -> Vec<String> {
    raw.as_ref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use bib_types::Collection;

    use super::*;

    #[tokio::test]
    async fn search_index_migration_rebuilds_documents_from_existing_articles() {
        let core = BibCore::open(":memory:").await.unwrap();
        core.upsert_article(&Article::new("legacy-article", "Migrationmarker evidence"))
            .await
            .unwrap();

        let conn = core.write_conn();
        conn.execute("DROP VIEW searchDocuments", turso::params![])
            .await
            .unwrap();
        conn.execute(
            "UPDATE bib_meta SET value = '3' WHERE key = 'search_index_version'",
            turso::params![],
        )
        .await
        .unwrap();

        core.migrate().await.unwrap();
        let hits = core
            .search_articles("migrationmarker", 10, 0)
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].article_id, "legacy-article");
    }

    #[tokio::test]
    async fn turso_fts_prototype_smoke_test() {
        let db = Builder::new_local(":memory:")
            .experimental_index_method(true)
            .build()
            .await
            .unwrap();
        let conn = db.connect().unwrap();
        conn.execute_batch(
            "CREATE TABLE fts_probe (id TEXT PRIMARY KEY, title TEXT, body TEXT);\
                 CREATE INDEX fts_probe_idx ON fts_probe USING fts (title, body);\
                 INSERT INTO fts_probe VALUES\
                   ('a', 'CRISPR editing', 'A phrase search in full text'),\
                   ('b', 'Other paper', 'Different material');",
        )
        .await
        .unwrap();

        let mut rows = conn
            .query(
                "SELECT id, fts_score(title, body, 'phrase search') AS score,\
                        fts_highlight(body, '<mark>', '</mark>', 'phrase search')\
                 FROM fts_probe WHERE fts_match(title, body, 'phrase search')\
                 ORDER BY score DESC",
                turso::params![],
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().expect("FTS should find a row");
        assert_eq!(row.get::<String>(0).unwrap(), "a");
        assert!(row.get::<f64>(1).unwrap().is_finite());
        let highlight = row.get::<String>(2).unwrap();
        assert!(highlight.contains("<mark>phrase</mark>"));
        assert!(highlight.contains("<mark>search</mark>"));
        assert!(rows.next().await.unwrap().is_none());

        let mut rows = conn
            .query(
                "SELECT id FROM fts_probe \
                 WHERE fts_match(body, '\"phrase search\"')",
                turso::params![],
            )
            .await
            .unwrap();
        assert_eq!(
            rows.next()
                .await
                .unwrap()
                .unwrap()
                .get::<String>(0)
                .unwrap(),
            "a"
        );
        assert!(rows.next().await.unwrap().is_none());
    }

    /// Regression test for issue #143: two concurrent readers on one
    /// shared `Arc<BibBase>` used to race on the read connection and one
    /// failed with turso's `Misuse("concurrent use forbidden")` — a
    /// node-level failure that killed the whole ingest wave. The storage
    /// actor serializes access structurally, so every concurrent caller
    /// must now succeed.
    ///
    /// The multi-thread flavor matters: this exact storm shape failed
    /// 5/5 runs against the pre-actor implementation.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_callers_on_one_handle_never_hit_concurrent_use_guard() {
        let bib = Arc::new(BibBase::open_in_memory().await.unwrap());
        bib.upsert_article(&Article::new("doi:10.1/a", "Article A"))
            .await
            .unwrap();
        bib.upsert_article(&Article::new("doi:10.1/b", "Article B"))
            .await
            .unwrap();
        bib.upsert_collection(&Collection::new("col-1", "Research"))
            .await
            .unwrap();

        // The issue's minimal repro: two reads racing through one handle.
        let (collection, article) =
            tokio::join!(bib.get_collection("col-1"), bib.get_article("doi:10.1/a"),);
        collection.unwrap().unwrap();
        article.unwrap().unwrap();

        // A wider storm — concurrent tasks doing mixed reads and writes
        // through the same handle, mimicking a DAG wave with several
        // bibliography nodes dispatched at once.
        let mut tasks = Vec::new();
        for task_index in 0..8u64 {
            let bib = Arc::clone(&bib);
            tasks.push(tokio::spawn(async move {
                for round in 0..10u64 {
                    bib.article_count().await.unwrap();
                    bib.search_articles("article", 5, 0).await.unwrap();
                    bib.list_article_ids().await.unwrap();
                    bib.get_collection("col-1").await.unwrap();
                    bib.upsert_article(&Article::new(
                        format!("doi:10.1/storm-{task_index}-{round}"),
                        "Storm article",
                    ))
                    .await
                    .unwrap();
                    bib.list_collections(None).await.unwrap();
                    bib.get_fulltext("doi:10.1/a").await.unwrap();
                }
            }));
        }
        for task in tasks {
            task.await.unwrap();
        }

        assert_eq!(bib.article_count().await.unwrap(), 2 + 8 * 10);
    }

    /// Dropping every handle must shut the actor down without hanging
    /// callers that are still in flight — and a caller that arrives after
    /// the actor is gone gets a descriptive error instead of a hang.
    #[tokio::test]
    async fn actor_stops_cleanly_when_last_handle_drops() {
        let bib = BibBase::open_in_memory().await.unwrap();
        bib.upsert_article(&Article::new("doi:10.1/x", "Solo"))
            .await
            .unwrap();
        drop(bib);

        // The channel closes with the last sender; a stale clone would
        // observe `ActorStopped`. Here we simply verify the happy path:
        // open → use → drop completes without hanging.
    }
}
