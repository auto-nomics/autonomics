//! Local PubMed DuckDB mirror as a [`LiteratureSource`].
//!
//! Reads a pre-built DuckDB database (default `/mnt/data/pubmed_local/pubmed.duckdb`,
//! overridable via `AUTONOMICS_PUBMED_LOCAL_DB`) holding the full PubMed
//! baseline + updatefiles:
//!
//! - `articles` — one row per citation (`pmid`, `doi`, `title`,
//!   `abstract_text`, `authors`, `journal`, `issn`, `volume`, `issue`,
//!   `pages`, `pub_date`, `language`, `publication_types`,
//!   `publication_status`)
//! - `article_mesh` — MeSH descriptors (`pmid`, `ui`, `descriptor_name`,
//!   `major_topic_yn`)
//! - `article_keywords` — author keywords (`pmid`, `keyword`)
//!
//! The database is opened **read-only**; queries run on `spawn_blocking`
//! under one process-wide handle. A writer holding the file lock makes the
//! open fail — callers degrade to the remote eutils source instead of
//! failing the whole search.
//!
//! Keyword queries use the DuckDB `fts` extension index (`fts_main_articles`)
//! when present in the database; without it, keyword queries fail fast
//! rather than falling back to a multi-minute full-column scan of 40M
//! abstracts. Structured-only queries (MeSH, journal, author, year,
//! publication type) never need FTS.
//!
//! SQL is compiled with **inlined literals** (values escaped by
//! [`sql_text`]) rather than bound parameters, so the identical compiled
//! SELECT can serve both row queries and DuckDB's `COPY (…) TO … (FORMAT
//! PARQUET)` export, which does not accept bind parameters.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use bib_types::query::{BoolOp, StructuredSearch, YearRange};
use bib_types::search::{SearchExpr, SearchField};
use bib_types::types::{Article, ArticleSource, Author, IdKind, Identifier};
use bib_types::unified::UnifiedSearchRequest;

use crate::error::{Error, Result};
use crate::query::{LiteratureSource, SourceBatch};

/// Default mirror location on this host.
pub const DEFAULT_DB_PATH: &str = "/mnt/data/pubmed_local/pubmed.duckdb";

/// Environment variable overriding the mirror location.
pub const DB_PATH_ENV: &str = "AUTONOMICS_PUBMED_LOCAL_DB";

/// Hard cap on rows exported through the Parquet path.
pub const MAX_EXPORT_ROWS: u64 = 1_000_000;

/// Resolve the mirror path from the environment, else the default.
pub fn db_path_from_env() -> PathBuf {
    std::env::var(DB_PATH_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_DB_PATH))
}

/// Escape a string as a single-quoted SQL literal.
fn sql_text(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn sql_int(i: i64) -> String {
    i.to_string()
}

/// A read-only handle to the local PubMed DuckDB mirror.
pub struct LocalPubmedDb {
    conn: Mutex<duckdb::Connection>,
    /// Whether `fts_main_articles` is usable (extension loadable + index
    /// present in this database file).
    fts_available: bool,
}

/// Shared process-wide mirror handle.
pub type SharedLocalPubmed = Arc<LocalPubmedDb>;

impl LocalPubmedDb {
    /// Open the mirror read-only. Fails if the file is missing or locked by
    /// a writer — callers should treat that as "mirror unavailable".
    pub fn open(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Err(Error::Unknown(format!(
                "local pubmed mirror not found at {}",
                path.display()
            )));
        }
        let config = duckdb::Config::default()
            .access_mode(duckdb::AccessMode::ReadOnly)
            .map_err(|e| Error::Unknown(format!("pubmed mirror config: {e}")))?;
        let conn = duckdb::Connection::open_with_flags(path, config)
            .map_err(|e| Error::Unknown(format!("open pubmed mirror {}: {e}", path.display())))?;

        // Keep heavy analytical queries from eating the whole host. Threads
        // default to the DuckDB engine default (all cores); a low thread
        // count or memory cap makes the FTS hash joins 5–10× slower.
        let _ = conn.execute_batch("SET memory_limit='16GB';");

        // The FTS index lives inside the DB file (schema `fts_main_articles`);
        // loading the extension makes the schema's macros callable. The
        // extension is per-engine-version under ~/.duckdb/extensions — install
        // it for this build's engine version when a maintenance CLI (which
        // runs a different engine) has it only for its own.
        let fts_available = (|| -> bool {
            if conn.execute_batch("LOAD fts;").is_ok() {
                return true;
            }
            conn.execute_batch("INSTALL fts; LOAD fts;").is_ok()
        })() && conn
            .query_row(
                "SELECT count(*) FROM information_schema.tables \
                     WHERE table_schema = 'fts_main_articles'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n > 0)
            .unwrap_or(false);

        Ok(Self {
            conn: Mutex::new(conn),
            fts_available,
        })
    }

    pub fn fts_available(&self) -> bool {
        self.fts_available
    }

    /// Run a SQL query, mapping each row through `f`.
    pub(crate) fn query_rows<T>(
        &self,
        sql: &str,
        mut f: impl FnMut(&duckdb::Row<'_>) -> Result<T>,
    ) -> Result<Vec<T>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| Error::Unknown("pubmed mirror connection poisoned".into()))?;
        let mut stmt = conn
            .prepare(sql)
            .map_err(|e| Error::Unknown(format!("prepare: {e}")))?;
        let mut rows = stmt
            .query(duckdb::params![])
            .map_err(|e| Error::Unknown(format!("query: {e}")))?;
        let mut out = Vec::new();
        while let Some(row) = rows
            .next()
            .map_err(|e| Error::Unknown(format!("step: {e}")))?
        {
            out.push(f(row)?);
        }
        Ok(out)
    }

    /// Execute a parameter-free statement (e.g. a `COPY … TO` export whose
    /// SELECT was compiled with inlined literals).
    pub(crate) fn execute(&self, sql: &str) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| Error::Unknown("pubmed mirror connection poisoned".into()))?;
        conn.execute_batch(sql)
            .map_err(|e| Error::Unknown(format!("execute: {e}")))
    }

    /// Run a `COPY (…) TO '<path>' (FORMAT PARQUET)` export on the blocking
    /// pool.
    pub async fn query_export(self: Arc<Self>, copy_sql: &str, _tmp_path: &Path) -> Result<()> {
        let sql = copy_sql.to_owned();
        tokio::task::spawn_blocking(move || self.execute(&sql))
            .await
            .map_err(|e| Error::Unknown(format!("pubmed mirror worker join: {e}")))?
    }
}

/// Open the mirror from the environment path if it exists.
pub fn open_from_env() -> Result<SharedLocalPubmed> {
    Ok(Arc::new(LocalPubmedDb::open(&db_path_from_env())?))
}

/// Open the mirror from the environment path, logging (not propagating)
/// failures. A missing file or a writer-held lock just means the mirror is
/// unavailable — callers continue with the remote sources.
pub fn try_open_from_env() -> Option<SharedLocalPubmed> {
    let path = db_path_from_env();
    match LocalPubmedDb::open(&path) {
        Ok(db) => {
            tracing::info!(
                path = %path.display(),
                fts = db.fts_available(),
                "local pubmed mirror attached"
            );
            Some(Arc::new(db))
        }
        Err(e) => {
            tracing::info!(
                path = %path.display(),
                error = %e,
                "local pubmed mirror unavailable; continuing with remote sources"
            );
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Query translation (literal-inlined SQL, usable inside COPY)
// ---------------------------------------------------------------------------

/// Render the keyword string handed to `fts_main_articles.tokenize`.
///
/// The tokenizer stems each whitespace-separated word; boolean operators
/// are not part of the query language (AND semantics come from the
/// conjunctive `HAVING` clause instead), so terms are joined by spaces.
fn fts_query_string(terms: &[String]) -> String {
    terms.join(" ")
}

/// BM25 scoring CTE over the FTS extension's inverted-index tables,
/// evaluated once per query. Returns `(pmid, score)` rows.
///
/// This mirrors the `fts_main_articles.match_bm25` macro's math (k=1.2,
/// b=0.75, porter stemming, corpus df from `dict`) but as a single set-based
/// query — the per-row macro form costs 8–35 s at 40M documents while this
/// form runs in seconds for typical term selectivities.
fn ranked_cte(terms: &[String], conjunctive: bool) -> String {
    let conj = if conjunctive {
        " HAVING count(DISTINCT t.termid) = (SELECT count(*) FROM tokens)"
    } else {
        ""
    };
    format!(
        "ranked AS ( \
           WITH tokens AS (SELECT DISTINCT stem(unnest(fts_main_articles.tokenize({q})), 'porter') AS t), \
           qterms AS (SELECT dict.termid, dict.df, terms.docid \
                      FROM fts_main_articles.dict dict JOIN tokens ON dict.term = tokens.t \
                      JOIN fts_main_articles.terms terms ON terms.termid = dict.termid), \
           qdf AS (SELECT termid, any_value(df) df FROM qterms GROUP BY termid), \
           tf AS (SELECT docid, termid, count(*) tf FROM qterms GROUP BY docid, termid), \
           stats AS (SELECT num_docs, avgdl FROM fts_main_articles.stats) \
           SELECT d.name AS pmid, sum(log(((s.num_docs - q.df) + 0.5) / (q.df + 0.5) + 1) \
             * ((t.tf * 2.2) / (t.tf + 1.2 * (0.25 + 0.75 * (d.len / s.avgdl))))) AS score \
           FROM tf t JOIN qdf q USING (termid) \
           CROSS JOIN stats s JOIN fts_main_articles.docs d ON d.docid = t.docid \
           GROUP BY t.docid, d.name{conj} \
         )",
        q = sql_text(&fts_query_string(terms)),
        conj = conj
    )
}

/// Columns every compiled SELECT returns (prefixed `a.`).
const ARTICLE_COLUMNS: &str = "a.pmid, a.doi, a.title, a.abstract_text, a.authors, a.journal, \
     a.issn, a.volume, a.issue, a.pages, a.pub_date, a.language, a.publication_types";

/// StructuredSearch → SQL `WHERE` clause over the `articles a` alias for all
/// **non-text** filters. Full-text terms are handled by [`compile_search`]
/// via the `ranked` CTE join.
///
/// Field semantics mirror the workspace contract: fields AND together,
/// values within a field OR. `keywords` and `title` both go through the FTS
/// index over `title` + `abstract_text` (there is no title-only index).
pub(crate) fn compile_where(fts_available: bool, query: &StructuredSearch) -> Result<String> {
    let mut clauses: Vec<String> = Vec::new();

    // ---- full-text exclusion (keywords_op = Not) --------------------------
    let text_terms: Vec<String> = query
        .keywords
        .iter()
        .flatten()
        .chain(query.title.iter().flatten())
        .cloned()
        .collect();
    if !text_terms.is_empty() {
        if !fts_available {
            return Err(Error::InvalidSearchQuery(
                "keyword search requires the FTS index; rebuild the mirror with \
                 `PRAGMA create_fts_index('articles','pmid','title','abstract_text')`"
                    .into(),
            ));
        }
        if query.keywords_op.unwrap_or_default() == BoolOp::Not {
            // FTS has no negation-only query: exclude via substring filters.
            for term in &text_terms {
                clauses.push(format!(
                    "NOT contains(lower(coalesce(a.title,'') || ' ' || \
                     coalesce(a.abstract_text,'')), {})",
                    sql_text(&term.to_lowercase())
                ));
            }
        }
    }

    // ---- authors (case-insensitive substring over the ';'-joined list) ----
    if let Some(authors) = query.authors.as_ref().filter(|a| !a.is_empty()) {
        let clause = authors
            .iter()
            .map(|name| {
                format!(
                    "contains(lower(coalesce(a.authors,'')), {})",
                    sql_text(&name.to_lowercase())
                )
            })
            .collect::<Vec<_>>()
            .join(" OR ");
        clauses.push(format!("({clause})"));
    }

    // ---- MeSH descriptors (exact IN match) --------------------------------
    if let Some(mesh) = query.mesh.as_ref().filter(|m| !m.is_empty()) {
        let list = mesh
            .iter()
            .map(|t| sql_text(t))
            .collect::<Vec<_>>()
            .join(",");
        clauses.push(format!(
            "a.pmid IN (SELECT pmid FROM article_mesh WHERE descriptor_name IN ({list}))"
        ));
    }

    // ---- journals (exact IN match) ----------------------------------------
    if let Some(journals) = query.journal.as_ref().filter(|j| !j.is_empty()) {
        let list = journals
            .iter()
            .map(|t| sql_text(t))
            .collect::<Vec<_>>()
            .join(",");
        clauses.push(format!("a.journal IN ({list})"));
    }

    // ---- publication types (substring over the comma-joined list) --------
    if let Some(pub_types) = query.publication_types.as_ref().filter(|p| !p.is_empty()) {
        let clause = pub_types
            .iter()
            .map(|t| {
                format!(
                    "contains(coalesce(a.publication_types,''), {})",
                    sql_text(t)
                )
            })
            .collect::<Vec<_>>()
            .join(" OR ");
        clauses.push(format!("({clause})"));
    }

    // ---- publication year --------------------------------------------------
    if let Some(YearRange { from, to }) = query.year_range.as_ref() {
        clauses.push(format!(
            "try_cast(substr(a.pub_date, 1, 4) AS INTEGER) BETWEEN {} AND {}",
            sql_int(*from as i64),
            sql_int(*to as i64)
        ));
    }

    Ok(if clauses.is_empty() {
        "TRUE".to_string()
    } else {
        clauses.join(" AND ")
    })
}

/// Compile the full ranked SELECT over `articles a`, LIMIT included.
///
/// Usable both as a plain query (rows streamed through [`read_article_row`])
/// and inside `COPY (…) TO …` — all values are inlined literals.
/// Whether a compiled query's text half runs through the ranked FTS CTE.
fn uses_ranked_fts(fts_available: bool, query: &StructuredSearch) -> bool {
    let has_text = query.keywords.as_ref().is_some_and(|k| !k.is_empty())
        || query.title.as_ref().is_some_and(|t| !t.is_empty());
    has_text && fts_available && query.keywords_op.unwrap_or_default() != BoolOp::Not
}

pub(crate) fn compile_search(
    fts_available: bool,
    query: &StructuredSearch,
    limit: u64,
) -> Result<String> {
    let where_sql = compile_where(fts_available, query)?;

    if uses_ranked_fts(fts_available, query) {
        let text_terms: Vec<String> = query
            .keywords
            .iter()
            .flatten()
            .chain(query.title.iter().flatten())
            .cloned()
            .collect();
        let conjunctive = query.keywords_op == Some(BoolOp::And);
        // Two-stage plan: score first (cheap inverted-index CTE), trim to
        // the top rows, and only then fetch full article rows. Joining all
        // scored rows to `articles` before LIMIT costs 30+s at 40M docs.
        let has_filters = where_sql != "TRUE";
        let filter_stage = if has_filters {
            format!(
                "filtered AS ( \
                   SELECT r.pmid, r.score FROM ranked r \
                   JOIN articles a ON a.pmid = r.pmid WHERE {where_sql} \
                 ), top AS (SELECT pmid, score FROM filtered \
                            ORDER BY score DESC, pmid LIMIT {limit})"
            )
        } else {
            format!(
                "top AS (SELECT pmid, score FROM ranked \
                        ORDER BY score DESC, pmid LIMIT {limit})"
            )
        };
        Ok(format!(
            "WITH {ranked}, {filter_stage} \
             SELECT {ARTICLE_COLUMNS}, t.score AS bm25_score \
             FROM articles a JOIN top t ON a.pmid = t.pmid \
             ORDER BY t.score DESC, a.pmid LIMIT {limit}",
            ranked = ranked_cte(&text_terms, conjunctive),
        ))
    } else {
        Ok(format!(
            "SELECT {ARTICLE_COLUMNS} FROM articles a WHERE {where_sql} \
             ORDER BY a.pmid LIMIT {limit}"
        ))
    }
}

// ---------------------------------------------------------------------------
// Row → Article
// ---------------------------------------------------------------------------

pub(crate) fn read_article_row(row: &duckdb::Row<'_>) -> Result<Article> {
    let opt = |idx: usize| -> Result<String> {
        row.get::<_, Option<String>>(idx)
            .map(|v| v.unwrap_or_default())
            .map_err(|e| Error::Unknown(format!("column {idx}: {e}")))
    };
    let pmid = opt(0)?;
    let doi = opt(1)?;
    let title = opt(2)?;
    let abstract_text = opt(3)?;
    let authors = opt(4)?;
    let journal = opt(5)?;
    let issn = opt(6)?;
    let volume = opt(7)?;
    let issue = opt(8)?;
    let pages = opt(9)?;
    let pub_date = opt(10)?;
    let language = opt(11)?;
    let publication_types = opt(12)?;

    let mut article = Article::new(pmid.trim(), title.trim());
    article.identifiers = vec![Identifier::new(IdKind::Pmid, pmid.trim())];
    if !doi.trim().is_empty() {
        article.identifiers.push(Identifier::doi(doi.trim()));
    }
    article.abstract_text = if abstract_text.is_empty() {
        None
    } else {
        Some(abstract_text)
    };
    article.authors = parse_authors(&authors);
    article.journal = non_empty(journal);
    article.issn = non_empty(issn);
    article.volume = non_empty(volume);
    article.issue = non_empty(issue);
    article.pages = non_empty(pages);
    article.language = non_empty(language);
    article.pub_types = publication_types
        .split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .collect();
    let (year, month) = parse_pub_date(&pub_date);
    article.year = year;
    article.month = month;
    article.source = ArticleSource::Pubmed;
    Ok(article)
}

/// Parse the mirror's `pub_date` ("2023-Jan", "2023 Jan 15", "2023 Feb", "2023").
fn parse_pub_date(s: &str) -> (Option<u16>, Option<u8>) {
    let mut parts = s.split(|c: char| c.is_whitespace() || c == '-' || c == '/');
    let year = parts
        .next()
        .and_then(|p| p.get(0..4))
        .and_then(|y| y.parse().ok());
    let month = parts.next().and_then(month_number);
    (year, month)
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

fn month_number(name: &str) -> Option<u8> {
    MONTHS
        .iter()
        .position(|m| m.eq_ignore_ascii_case(name))
        .map(|i| i as u8 + 1)
}

/// Split the mirror's authors string ("Alice B; Carol D; ...") into authors.
fn parse_authors(s: &str) -> Vec<Author> {
    s.split(';')
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(|full| {
            // Full names are "Given Family" ("Beate Peter"); single-token
            // names (consortia, mononyms) stay whole in last_name.
            match full.rsplit_once(' ') {
                Some((given, family)) if !family.is_empty() => Author {
                    last_name: family.to_owned(),
                    fore_name: Some(given.to_owned()),
                    initials: {
                        let i: String = given
                            .split_whitespace()
                            .filter_map(|w| w.chars().next())
                            .collect();
                        if i.is_empty() { None } else { Some(i) }
                    },
                    affiliation: None,
                    orcid: None,
                    corresponding: false,
                },
                _ => Author {
                    last_name: full.to_owned(),
                    fore_name: None,
                    initials: None,
                    affiliation: None,
                    orcid: None,
                    corresponding: false,
                },
            }
        })
        .collect()
}

fn non_empty(s: String) -> Option<String> {
    if s.trim().is_empty() { None } else { Some(s) }
}

// ---------------------------------------------------------------------------
// LiteratureSource impl
// ---------------------------------------------------------------------------

/// [`LiteratureSource`] over the local mirror.
pub struct LocalPubmedSource {
    db: SharedLocalPubmed,
}

impl LocalPubmedSource {
    pub fn new(db: SharedLocalPubmed) -> Self {
        Self { db }
    }
}

#[async_trait]
impl LiteratureSource for LocalPubmedSource {
    fn name(&self) -> &'static str {
        "pubmed_local"
    }

    fn supports(&self, kind: IdKind) -> bool {
        matches!(kind, IdKind::Pmid | IdKind::Doi)
    }

    async fn search(&self, query: &StructuredSearch, limit: usize) -> Result<SourceBatch> {
        let sql = compile_search(self.db.fts_available(), query, limit as u64)?;
        let db = self.db.clone();
        let articles = spawn_query(db, move |db| db.query_rows(&sql, read_article_row)).await?;
        let total = articles.len();
        Ok(SourceBatch {
            source: self.name().into(),
            total,
            articles,
            error: None,
        })
    }

    async fn fetch(&self, id: &Identifier) -> Result<Option<Article>> {
        let column = match id.kind {
            IdKind::Pmid => "pmid",
            IdKind::Doi => "doi",
            _ => return Ok(None),
        };
        let sql = format!(
            "SELECT {ARTICLE_COLUMNS} FROM articles a WHERE a.{column} = {}",
            sql_text(id.value.trim())
        );
        let db = self.db.clone();
        let rows = spawn_query(db, move |db| db.query_rows(&sql, read_article_row)).await?;
        Ok(rows.into_iter().next())
    }
}

/// Run a blocking DuckDB query on the blocking thread pool.
async fn spawn_query<T: Send + 'static>(
    db: SharedLocalPubmed,
    f: impl FnOnce(&LocalPubmedDb) -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(move || f(&db))
        .await
        .map_err(|e| Error::Unknown(format!("pubmed mirror worker join: {e}")))?
}

// ---------------------------------------------------------------------------
// Unified-search translation (SearchExpr → StructuredSearch)
// ---------------------------------------------------------------------------

/// Source name the unified search surface uses for the mirror.
pub const UNIFIED_SOURCE: &str = "pubmed_local";

/// Translate a unified search request (query string or prebuilt expression
/// plus filters) into a [`StructuredSearch`] for the mirror.
///
/// The mirror cannot express every query shape; unsupported constructs
/// return [`Error::InvalidSearchQuery`] so callers report the source as
/// failed rather than silently approximating.
pub(crate) fn structured_from_request(request: &UnifiedSearchRequest) -> Result<StructuredSearch> {
    let expr: Option<SearchExpr> = match request.expression.clone() {
        Some(e) => Some(e),
        None => bib_types::search_parser::parse_search_query(&request.query)
            .map_err(|e| Error::InvalidSearchQuery(format!("pubmed_local: {e}")))?
            .filter(|_| !request.query.trim().is_empty()),
    };
    let mut search = StructuredSearch::default();
    if let Some(expr) = expr {
        translate_expr(&expr, &mut search)?;
    }

    // Filters apply on top of the expression.
    if search.year_range.is_none() {
        let f = &request.filters;
        if f.year_from.is_some() || f.year_to.is_some() {
            search.year_range = Some(YearRange {
                from: f.year_from.unwrap_or(1),
                to: f.year_to.unwrap_or(9999),
            });
        }
    }
    for t in &request.filters.publication_types {
        search
            .publication_types
            .get_or_insert_with(Vec::new)
            .push(t.clone());
    }
    Ok(search)
}

/// Fold one expression node into `search`. Supported: term/phrase per
/// field, top-level OR, AND of plain terms, year ranges. Everything else
/// (nested booleans, NOT, identifier lookups) is rejected.
fn translate_expr(expr: &SearchExpr, search: &mut StructuredSearch) -> Result<()> {
    let unsupported = |what: &str| {
        Error::InvalidSearchQuery(format!(
            "pubmed_local cannot express {what}; simplify the query or query the source directly"
        ))
    };
    match expr {
        SearchExpr::Term { value, field } | SearchExpr::Phrase { value, field } => {
            let field = field.unwrap_or(SearchField::Any);
            match field {
                SearchField::Any | SearchField::Abstract | SearchField::Keyword => {
                    search
                        .keywords
                        .get_or_insert_with(Vec::new)
                        .push(value.clone());
                }
                SearchField::Title => search
                    .title
                    .get_or_insert_with(Vec::new)
                    .push(value.clone()),
                SearchField::Creator => search
                    .authors
                    .get_or_insert_with(Vec::new)
                    .push(value.clone()),
                SearchField::Publication => search
                    .journal
                    .get_or_insert_with(Vec::new)
                    .push(value.clone()),
                SearchField::Year => {
                    let year: u16 = value.parse().map_err(|_| {
                        Error::InvalidSearchQuery(format!(
                            "pubmed_local: year `{value}` is not a number"
                        ))
                    })?;
                    search.year_range = Some(YearRange {
                        from: year,
                        to: year,
                    });
                }
                _ => return Err(unsupported(&format!("field `{field:?}`"))),
            }
            Ok(())
        }
        SearchExpr::Or { clauses } => {
            for clause in clauses {
                translate_expr(clause, search)?;
            }
            Ok(())
        }
        SearchExpr::And { clauses } => {
            let mut any_keywords = false;
            for clause in clauses {
                let had_keywords = search.keywords.as_ref().is_some_and(|k| !k.is_empty());
                translate_expr(clause, search)?;
                let has_keywords = search.keywords.as_ref().is_some_and(|k| !k.is_empty());
                if !had_keywords && has_keywords {
                    any_keywords = true;
                }
            }
            // AND between two keyword contributions narrows the match.
            let keyword_contributions = search.keywords.as_ref().map(|k| k.len()).unwrap_or(0);
            if any_keywords && keyword_contributions > 1 {
                search.keywords_op = Some(BoolOp::And);
            }
            Ok(())
        }
        SearchExpr::Not { .. } => Err(unsupported("NOT")),
        SearchExpr::Range { field, gte, lte } => match field {
            SearchField::Year => {
                search.year_range = Some(YearRange {
                    from: gte.as_deref().and_then(|v| v.parse().ok()).unwrap_or(1),
                    to: lte.as_deref().and_then(|v| v.parse().ok()).unwrap_or(9999),
                });
                Ok(())
            }
            _ => Err(unsupported(&format!("range over `{field:?}`"))),
        },
    }
}

// ---------------------------------------------------------------------------
// Tests (tiny on-disk fixture; FTS paths need the real mirror)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_db() -> LocalPubmedDb {
        let dir = std::env::temp_dir().join(format!("pubmed-local-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pubmed.duckdb");
        let conn = duckdb::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE articles (pmid VARCHAR, doi VARCHAR, title VARCHAR, \
                abstract_text VARCHAR, authors VARCHAR, journal VARCHAR, issn VARCHAR, \
                volume VARCHAR, issue VARCHAR, pages VARCHAR, pub_date VARCHAR, \
                language VARCHAR, publication_types VARCHAR, publication_status VARCHAR); \
             INSERT INTO articles VALUES \
               ('1','10.1/a','CRISPR base editing review','A review of base editing.', \
                'Alice B; Carol D','Nature','1234-5678','21','2','1-9','2023-Jan','eng', \
                'Review,Journal Article','ppublish'), \
               ('2','10.1/b','Telomere shortening','Telomeres shorten.', \
                'Eve F','Cell','0987-6543','5','1','e1','2022 Feb 3','eng', \
                'Journal Article','ppublish'), \
               ('3', NULL, 'Quote''s edge case', 'Abstract with ''quotes''.', \
                'Ann O''Brien', 'Nature', NULL, NULL, NULL, NULL, '2021', 'eng', NULL, 'ppublish'); \
             CREATE TABLE article_mesh (pmid VARCHAR, ui VARCHAR, descriptor_name VARCHAR, \
                major_topic_yn VARCHAR); \
             INSERT INTO article_mesh VALUES ('1','D0001','CRISPR','Y'), \
                ('2','D0002','Telomere Shortening','N'); \
             CREATE TABLE article_keywords (pmid VARCHAR, keyword VARCHAR); \
             INSERT INTO article_keywords VALUES ('1','base editing');",
        )
        .unwrap();
        drop(conn);
        // Fixture has no FTS index — mirrors an un-indexed database.
        let db = LocalPubmedDb::open(&path).unwrap();
        assert!(!db.fts_available());
        db
    }

    #[test]
    fn sql_text_escapes_single_quotes() {
        assert_eq!(sql_text("O'Brien"), "'O''Brien'");
    }

    #[test]
    fn parse_pub_date_variants() {
        assert_eq!(parse_pub_date("2023-Jan"), (Some(2023), Some(1)));
        assert_eq!(parse_pub_date("2022 Feb 3"), (Some(2022), Some(2)));
        assert_eq!(parse_pub_date("2021"), (Some(2021), None));
        assert_eq!(parse_pub_date(""), (None, None));
    }

    #[test]
    fn parse_authors_semicolon_list() {
        let authors = parse_authors("Alice B; Carol D");
        assert_eq!(authors.len(), 2);
        assert_eq!(authors[0].last_name, "B");
        assert_eq!(authors[0].fore_name.as_deref(), Some("Alice"));
        let single = parse_authors("Plato");
        assert_eq!(single.len(), 1);
        assert_eq!(single[0].last_name, "Plato");
    }

    #[tokio::test]
    async fn fetch_by_pmid_and_doi() {
        let source = LocalPubmedSource::new(Arc::new(fixture_db()));
        let by_pmid = source
            .fetch(&Identifier::new(IdKind::Pmid, "1"))
            .await
            .unwrap()
            .expect("row 1");
        assert_eq!(by_pmid.title, "CRISPR base editing review");
        assert_eq!(by_pmid.year, Some(2023));
        assert_eq!(by_pmid.month, Some(1));
        assert_eq!(by_pmid.pub_types, vec!["Review", "Journal Article"]);
        assert_eq!(by_pmid.authors.len(), 2);

        let by_doi = source
            .fetch(&Identifier::doi("10.1/b"))
            .await
            .unwrap()
            .expect("row 2");
        assert_eq!(by_doi.title, "Telomere shortening");
        assert!(
            source
                .fetch(&Identifier::new(IdKind::Pmid, "999"))
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn quoted_values_round_trip() {
        // Author name with a quote must round-trip through the inlined SQL.
        let source = LocalPubmedSource::new(Arc::new(fixture_db()));
        let query = StructuredSearch {
            authors: Some(vec!["O'Brien".into()]),
            ..Default::default()
        };
        let batch = source.search(&query, 10).await.unwrap();
        assert_eq!(batch.articles.len(), 1);
        assert_eq!(batch.articles[0].title, "Quote's edge case");
    }

    #[tokio::test]
    async fn search_mesh_year_journal_filters() {
        let source = LocalPubmedSource::new(Arc::new(fixture_db()));
        let query = StructuredSearch {
            mesh: Some(vec!["Telomere Shortening".into()]),
            year_range: Some(YearRange {
                from: 2020,
                to: 2024,
            }),
            ..Default::default()
        };
        let batch = source.search(&query, 10).await.unwrap();
        assert_eq!(batch.articles.len(), 1);
        assert_eq!(batch.articles[0].title, "Telomere shortening");

        // Year filter excludes.
        let query = StructuredSearch {
            year_range: Some(YearRange {
                from: 2030,
                to: 2031,
            }),
            ..Default::default()
        };
        let batch = source.search(&query, 10).await.unwrap();
        assert!(batch.articles.is_empty());

        // Journal exact IN match.
        let query = StructuredSearch {
            journal: Some(vec!["Nature".into()]),
            ..Default::default()
        };
        let batch = source.search(&query, 10).await.unwrap();
        assert_eq!(batch.articles.len(), 2);

        // Publication-type substring.
        let query = StructuredSearch {
            publication_types: Some(vec!["Review".into()]),
            ..Default::default()
        };
        let batch = source.search(&query, 10).await.unwrap();
        assert_eq!(batch.articles.len(), 1);
    }

    #[tokio::test]
    async fn keyword_search_without_fts_fails_fast() {
        let source = LocalPubmedSource::new(Arc::new(fixture_db()));
        let query = StructuredSearch {
            keywords: Some(vec!["base editing".into()]),
            ..Default::default()
        };
        let err = source.search(&query, 10).await.unwrap_err().to_string();
        assert!(err.contains("FTS"), "{err}");
    }

    #[test]
    fn export_sql_compiles_with_limit() {
        let sql = compile_search(false, &StructuredSearch::default(), MAX_EXPORT_ROWS).unwrap();
        assert!(sql.contains("ORDER BY a.pmid"), "{sql}");
        assert!(sql.ends_with("LIMIT 1000000"), "{sql}");
    }

    /// Live integration against the real mirror (feature-gated code path +
    /// FTS index). Run manually:
    /// `cargo test -p bib-base --features pubmed_local live_mirror -- --ignored`
    #[tokio::test]
    #[ignore = "requires the real mirror at /mnt/data/pubmed_local/pubmed.duckdb"]
    async fn live_mirror_keyword_and_structured_search() {
        let db = open_from_env().unwrap();
        assert!(db.fts_available(), "FTS index missing on the live mirror");
        let source = LocalPubmedSource::new(db);

        // Conjunctive BM25 keyword search.
        let t0 = std::time::Instant::now();
        let q = StructuredSearch {
            keywords: Some(vec!["APOE".into(), "lipoprotein".into()]),
            keywords_op: Some(BoolOp::And),
            year_range: Some(YearRange {
                from: 2020,
                to: 2026,
            }),
            ..Default::default()
        };
        let batch = source.search(&q, 5).await.unwrap();
        eprintln!("live-mirror keyword search: {:?}", t0.elapsed());
        assert!(!batch.articles.is_empty(), "{batch:?}");
        assert!(batch.articles.iter().all(|a| a.year.unwrap_or(0) >= 2020));

        // MeSH filter path.
        let t1 = std::time::Instant::now();
        let q = StructuredSearch {
            mesh: Some(vec!["Telomere Shortening".into()]),
            year_range: Some(YearRange {
                from: 2023,
                to: 2024,
            }),
            ..Default::default()
        };
        let batch = source.search(&q, 3).await.unwrap();
        eprintln!("live-mirror mesh search: {:?}", t1.elapsed());
        assert!(!batch.articles.is_empty());

        // PMID fetch.
        let t2 = std::time::Instant::now();
        let fetched = source
            .fetch(&Identifier::new(IdKind::Pmid, "36271830"))
            .await
            .unwrap()
            .expect("live pmid");
        eprintln!("live-mirror fetch: {:?}", t2.elapsed());
        assert!(fetched.title.len() > 10);
    }
}
