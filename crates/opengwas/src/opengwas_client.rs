use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use reqwest::{Client, header, multipart};
use rusqlite::Connection;
use serde_json::Value;

use fs::OpendalFileStorage;

use crate::error::{OpengwasError, Result};
use crate::types::*;

const BASE_URL: &str = "https://api.opengwas.io/api";

/// Default file name for the persistent gwasinfo SQLite cache.
const CACHE_FILE_NAME: &str = "gwasinfo.sqlite";

/// Environment variable overriding the on-disk cache directory.
const CACHE_DIR_ENV: &str = "OPENGWAS_CACHE_DIR";

/// Resolve the default on-disk cache directory.
///
/// Priority:
/// 1. `OPENGWAS_CACHE_DIR` environment variable (if set and non-empty).
/// 2. `$HOME/.cache/opengwas` (XDG-style).
/// 3. `std::env::temp_dir()/opengwas` (last-resort fallback).
fn default_cache_dir() -> PathBuf {
    if let Ok(dir) = std::env::var(CACHE_DIR_ENV) {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir);
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".cache").join("opengwas");
    }
    std::env::temp_dir().join("opengwas")
}

// ---------------------------------------------------------------------------
// SQLite helper: Row → GwasInfo
// ---------------------------------------------------------------------------

fn row_to_gwasinfo(row: &rusqlite::Row<'_>) -> rusqlite::Result<GwasInfo> {
    Ok(GwasInfo {
        id: row.get(0)?,
        trait_: row.get(1)?,
        build: row.get(2)?,
        group_name: row.get(3)?,
        category: row.get(4)?,
        subcategory: row.get(5)?,
        population: row.get(6)?,
        sex: row.get(7)?,
        author: row.get(8)?,
        nsnp: row.get(9)?,
        sample_size: row.get(10)?,
        year: row.get(11)?,
        ontology: row.get(12)?,
        unit: row.get(13)?,
        ncase: row.get(14)?,
        ncontrol: row.get(15)?,
        study_design: row.get(16)?,
        covariates: row.get(17)?,
        coverage: row.get(18)?,
        qc_prior_to_upload: row.get(19)?,
        imputation_panel: row.get(20)?,
        beta_transformation: row.get(21)?,
        doi: row.get(22)?,
        consortium: row.get(23)?,
        pmid: row.get(24)?,
        sd: row.get(25)?,
        mr: row.get(26)?,
        priority: row.get(27)?,
        note: row.get(28)?,
        is_nc: row.get(29)?,
    })
}

const SELECT_COLUMNS: &str = "
    id, trait_, build, group_name, category, subcategory,
    population, sex, author, nsnp, sample_size, year,
    ontology, unit, ncase, ncontrol, study_design,
    covariates, coverage, qc_prior_to_upload,
    imputation_panel, beta_transformation, doi,
    consortium, pmid, sd, mr, priority, note, is_nc
";

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

pub struct OpengwasClient {
    client: Client,
    /// Directory used to persist the gwasinfo SQLite cache.
    cache_dir: PathBuf,
    /// On-disk SQLite cache handle for gwasinfo metadata. Initialised lazily
    /// on first access via [`ensure_db_loaded`](Self::ensure_db_loaded).
    /// Wrapped in a `Mutex<Option<…>>` so [`clear_disk_cache`](Self::clear_disk_cache)
    /// can drop the cached handle and force a refetch in this process too.
    db: Mutex<Option<Arc<Mutex<Connection>>>>,
}

impl OpengwasClient {
    /// Create a new client, reading the token from:
    /// 1. the provided `token` argument (if `Some`);
    /// 2. the `OPENGWAS_TOKEN` environment variable (if set).
    ///
    /// The token should **not** include the `Bearer` prefix — it is
    /// automatically prepended.
    ///
    /// The on-disk gwasinfo cache is stored under the directory chosen by
    /// [`default_cache_dir`] (typically `$HOME/.cache/opengwas`).
    /// Pass an explicit directory via [`with_cache_dir`](Self::with_cache_dir)
    /// if you want a different location.
    ///
    /// Returns [`OpengwasError::InvalidToken`] if no token can be resolved.
    pub fn new(token: Option<&str>) -> Result<Self> {
        Self::with_cache_dir(token, default_cache_dir())
    }

    /// Create a client without any authentication header.
    ///
    /// Only public endpoints (`/status`, `/batches`) will work. Uses the
    /// default on-disk cache directory — see [`with_cache_dir`](Self::with_cache_dir)
    /// to override.
    pub fn new_no_auth() -> Result<Self> {
        Self::with_cache_dir_no_auth(default_cache_dir())
    }

    /// Create a new client that persists the gwasinfo cache to `cache_dir`.
    ///
    /// The cache file lives at `<cache_dir>/gwasinfo.sqlite` and is reused
    /// across process restarts. The directory is created lazily on first
    /// access if it does not yet exist.
    ///
    /// See [`new`](Self::new) for token-resolution rules. Returns
    /// [`OpengwasError::InvalidToken`] if no token can be resolved.
    pub fn with_cache_dir(token: Option<&str>, cache_dir: impl Into<PathBuf>) -> Result<Self> {
        let token = match token {
            Some(t) => Some(t.to_string()),
            None => std::env::var("OPENGWAS_TOKEN").ok(),
        };
        let full = format!(
            "Bearer {}",
            token.ok_or_else(|| {
                OpengwasError::InvalidToken(
                    "no token provided and OPENGWAS_TOKEN env var not set".into(),
                )
            })?
        );
        let mut headers = header::HeaderMap::new();
        let mut auth = header::HeaderValue::from_str(&full)
            .map_err(|e| OpengwasError::InvalidToken(format!("invalid token: {e}")))?;
        auth.set_sensitive(true);
        headers.insert(header::AUTHORIZATION, auth);
        let client = Client::builder().default_headers(headers).build()?;
        Ok(Self {
            client,
            cache_dir: cache_dir.into(),
            db: Mutex::new(None),
        })
    }

    /// Create a client without authentication that persists the gwasinfo
    /// cache to `cache_dir`. See [`with_cache_dir`](Self::with_cache_dir).
    pub fn with_cache_dir_no_auth(cache_dir: impl Into<PathBuf>) -> Result<Self> {
        let client = Client::builder().build()?;
        Ok(Self {
            client,
            cache_dir: cache_dir.into(),
            db: Mutex::new(None),
        })
    }

    /// Return the directory used for the persistent gwasinfo cache.
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// Return the full path of the on-disk gwasinfo cache file.
    pub fn cache_file_path(&self) -> PathBuf {
        self.cache_dir.join(CACHE_FILE_NAME)
    }

    // =======================================================================
    // SQLite helpers (private)
    // =======================================================================

    /// Open (or create) the on-disk SQLite cache at [`Self::cache_file_path`]
    /// and ensure the `gwasinfo` table and its indexes exist.
    fn init_db(&self) -> Result<Arc<Mutex<Connection>>> {
        std::fs::create_dir_all(&self.cache_dir)?;
        let path = self.cache_file_path();
        let conn = Connection::open(&path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS gwasinfo (
                id                  TEXT PRIMARY KEY,
                trait_              TEXT,
                build               TEXT,
                group_name          TEXT,
                category            TEXT,
                subcategory         TEXT,
                population          TEXT,
                sex                 TEXT,
                author              TEXT,
                nsnp                INTEGER,
                sample_size         INTEGER,
                year                INTEGER,
                ontology            TEXT,
                unit                TEXT,
                ncase               INTEGER,
                ncontrol            INTEGER,
                study_design        TEXT,
                covariates          TEXT,
                coverage            TEXT,
                qc_prior_to_upload  TEXT,
                imputation_panel    TEXT,
                beta_transformation TEXT,
                doi                 TEXT,
                consortium          TEXT,
                pmid                INTEGER,
                sd                  REAL,
                mr                  INTEGER,
                priority            INTEGER,
                note                TEXT,
                is_nc               INTEGER
            );
            CREATE INDEX IF NOT EXISTS idx_gwasinfo_trait
                ON gwasinfo(trait_);
            CREATE INDEX IF NOT EXISTS idx_gwasinfo_author
                ON gwasinfo(author);
            CREATE INDEX IF NOT EXISTS idx_gwasinfo_population
                ON gwasinfo(population);",
        )?;
        Ok(Arc::new(Mutex::new(conn)))
    }

    /// Insert a batch of [`GwasInfo`] rows into the in-memory SQLite cache.
    fn bulk_insert(conn: &Connection, infos: &[GwasInfo]) -> Result<()> {
        let tx = conn.unchecked_transaction()?;
        tx.execute("DELETE FROM gwasinfo", [])?;
        {
            let mut stmt = tx.prepare(
                "INSERT OR REPLACE INTO gwasinfo (
                    id, trait_, build, group_name, category, subcategory,
                    population, sex, author, nsnp, sample_size, year,
                    ontology, unit, ncase, ncontrol, study_design,
                    covariates, coverage, qc_prior_to_upload,
                    imputation_panel, beta_transformation, doi,
                    consortium, pmid, sd, mr, priority, note, is_nc
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                    ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22,
                    ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30
                 )",
            )?;
            for info in infos {
                stmt.execute(rusqlite::params![
                    info.id,
                    info.trait_,
                    info.build,
                    info.group_name,
                    info.category,
                    info.subcategory,
                    info.population,
                    info.sex,
                    info.author,
                    info.nsnp,
                    info.sample_size,
                    info.year,
                    info.ontology,
                    info.unit,
                    info.ncase,
                    info.ncontrol,
                    info.study_design,
                    info.covariates,
                    info.coverage,
                    info.qc_prior_to_upload,
                    info.imputation_panel,
                    info.beta_transformation,
                    info.doi,
                    info.consortium,
                    info.pmid,
                    info.sd,
                    info.mr,
                    info.priority,
                    info.note,
                    info.is_nc,
                ])?;
            }
        } // stmt dropped here, releasing borrow on tx
        tx.commit()?;
        Ok(())
    }

    /// Ensure the on-disk SQLite cache is populated.
    ///
    /// On the first call this fetches all gwasinfo metadata from the remote
    /// API and inserts it into the cache file at
    /// [`Self::cache_file_path`]. Subsequent calls return immediately.
    /// On future process restarts the cache is reused from disk so no
    /// network round-trip is needed until [`refresh_disk_cache`](Self::refresh_disk_cache)
    /// is called.
    async fn ensure_db_loaded(&self) -> Result<()> {
        // Fast path: already initialised in this process.
        if self.db.lock().unwrap().is_some() {
            return Ok(());
        }

        let path = self.cache_file_path();
        let cache_already_present = path.exists();

        // Open the on-disk DB up front. If the cache file is empty (or
        // missing), we have to fetch from the remote API.
        let db = self.init_db()?;

        if cache_already_present && Self::is_populated(&db)? {
            // Already populated from a previous run — reuse as-is.
            *self.db.lock().unwrap() = Some(db);
            return Ok(());
        }

        // Fetch from remote API.
        let raw: Value = self.get("/gwasinfo").await?;
        // API returns a map keyed by GWAS ID: { "ieu-a-2": { ... }, ... }
        let infos: Vec<GwasInfo> = raw
            .as_object()
            .map(|m| {
                m.values()
                    .filter_map(|v| serde_json::from_value(v.clone()).ok())
                    .collect()
            })
            .ok_or_else(|| {
                OpengwasError::UnexpectedResponse(
                    "failed to deserialize gwasinfo response: expected a map".into(),
                )
            })?;

        // Bulk insert inside spawn_blocking.
        {
            let db_clone = Arc::clone(&db);
            tokio::task::spawn_blocking(move || {
                let guard = db_clone.lock().unwrap();
                Self::bulk_insert(&guard, &infos)
            })
            .await
            // JoinError (panic in blocking task) → OpengwasError::TaskJoin;
            // the inner rusqlite::Error → OpengwasError::Sqlite (both via From).
            .map_err(OpengwasError::from)??;
        }

        // Cache the open handle. Another concurrent caller may have raced
        // us and stored its own copy — that's fine, both point to
        // equivalent on-disk state.
        *self.db.lock().unwrap() = Some(db);
        Ok(())
    }

    /// Returns `true` if the gwasinfo table has at least one row.
    fn is_populated(db: &Arc<Mutex<Connection>>) -> Result<bool> {
        let guard = db.lock().unwrap();
        let n: i64 = guard.query_row("SELECT COUNT(*) FROM gwasinfo", [], |row| row.get(0))?;
        Ok(n > 0)
    }

    // =======================================================================
    // HTTP helpers (private)
    // =======================================================================

    async fn get(&self, path: &str) -> Result<Value> {
        let url = format!("{BASE_URL}{path}");
        let resp = self.client.get(&url).send().await?;
        resp.error_for_status_ref()?;
        Ok(resp.json().await?)
    }

    async fn get_raw(&self, path: &str) -> Result<String> {
        let url = format!("{BASE_URL}{path}");
        let resp = self.client.get(&url).send().await?;
        resp.error_for_status_ref()?;
        Ok(resp.text().await?)
    }

    async fn post_json<T: serde::Serialize + ?Sized>(&self, path: &str, body: &T) -> Result<Value> {
        let url = format!("{BASE_URL}{path}");
        let resp = self.client.post(&url).json(body).send().await?;
        resp.error_for_status_ref()?;
        Ok(resp.json().await?)
    }

    async fn delete(&self, path: &str) -> Result<Value> {
        let url = format!("{BASE_URL}{path}");
        let resp = self.client.delete(&url).send().await?;
        resp.error_for_status_ref()?;
        Ok(resp.json().await?)
    }

    // =======================================================================
    // Status
    // =======================================================================

    /// `GET /status` — check that API services are running (no auth required).
    pub async fn status(&self) -> Result<Value> {
        self.get("/status").await
    }

    // =======================================================================
    // Batches
    // =======================================================================

    /// `GET /batches` — list existing data batches (no auth required).
    pub async fn batches(&self) -> Result<Value> {
        self.get("/batches").await
    }

    // =======================================================================
    // User
    // =======================================================================

    /// `GET /user` — get information about the authenticated user.
    pub async fn user(&self) -> Result<Value> {
        self.get("/user").await
    }

    // =======================================================================
    // GwasInfo (SQLite-cached)
    // =======================================================================

    /// Get metadata for **all** GWAS datasets accessible to you.
    ///
    /// Results are cached in an on-disk SQLite database. On the first call
    /// the catalog is fetched from the remote API and persisted to
    /// [`Self::cache_file_path`]. Subsequent calls — including those from
    /// future process invocations — read from the local cache without
    /// hitting the network.
    pub async fn gwasinfo_all(&self) -> Result<Vec<GwasInfo>> {
        self.ensure_db_loaded().await?;

        let db = Arc::clone(
            self.db
                .lock()
                .unwrap()
                .as_ref()
                .expect("db initialised by ensure_db_loaded"),
        );
        let db_clone = Arc::clone(&db);
        tokio::task::spawn_blocking(move || {
            let guard = db_clone.lock().unwrap();
            let mut stmt = guard.prepare(&format!("SELECT {SELECT_COLUMNS} FROM gwasinfo"))?;
            let rows = stmt.query_map([], row_to_gwasinfo)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        })
        .await
        // JoinError (panic in blocking task) → OpengwasError::TaskJoin;
        // the inner rusqlite::Error is converted by the trailing map_err.
        .map_err(OpengwasError::from)?
        .map_err(Into::into)
    }

    /// Get metadata for specific GWAS datasets by ID.
    ///
    /// Results are served from the on-disk cache (populated on first use).
    /// If any requested IDs are not in the cache, they are simply absent
    /// from the returned vector.
    pub async fn gwasinfo(&self, req: &GwasInfoRequest) -> Result<Vec<GwasInfo>> {
        if req.id.is_empty() {
            return Ok(vec![]);
        }

        self.ensure_db_loaded().await?;

        let db = Arc::clone(
            self.db
                .lock()
                .unwrap()
                .as_ref()
                .expect("db initialised by ensure_db_loaded"),
        );
        let ids = req.id.clone();
        let db_clone = Arc::clone(&db);
        tokio::task::spawn_blocking(move || {
            let guard = db_clone.lock().unwrap();
            let placeholders: String = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let sql = format!("SELECT {SELECT_COLUMNS} FROM gwasinfo WHERE id IN ({placeholders})");
            let mut stmt = guard.prepare(&sql)?;
            let params: Vec<&dyn rusqlite::types::ToSql> = ids
                .iter()
                .map(|s| s as &dyn rusqlite::types::ToSql)
                .collect();
            let rows = stmt.query_map(params.as_slice(), row_to_gwasinfo)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        })
        .await
        // JoinError (panic in blocking task) → OpengwasError::TaskJoin;
        // the inner rusqlite::Error is converted by the trailing map_err.
        .map_err(OpengwasError::from)?
        .map_err(Into::into)
    }

    /// Force a refresh of the gwasinfo cache by re-fetching all metadata
    /// from the remote API. The fresh data replaces the contents of the
    /// on-disk SQLite cache at [`Self::cache_file_path`] so the new
    /// snapshot survives process restarts.
    ///
    /// Equivalent to [`refresh_disk_cache`](Self::refresh_disk_cache); that
    /// name documents the disk persistence semantics more explicitly.
    pub async fn gwasinfo_refresh(&self) -> Result<Vec<GwasInfo>> {
        self.refresh_disk_cache().await
    }

    /// Force a refresh of the **on-disk** gwasinfo cache by re-fetching
    /// all metadata from the remote API. The new snapshot replaces the
    /// contents of the SQLite file at [`Self::cache_file_path`] so future
    /// process restarts pick it up without re-fetching.
    pub async fn refresh_disk_cache(&self) -> Result<Vec<GwasInfo>> {
        self.ensure_db_loaded().await?;

        let db = Arc::clone(
            self.db
                .lock()
                .unwrap()
                .as_ref()
                .expect("db initialised by ensure_db_loaded"),
        );

        // Fetch fresh data from remote.
        let raw: Value = self.get("/gwasinfo").await?;
        let infos: Vec<GwasInfo> = raw
            .as_object()
            .map(|m| {
                m.values()
                    .filter_map(|v| serde_json::from_value(v.clone()).ok())
                    .collect()
            })
            .ok_or_else(|| {
                OpengwasError::UnexpectedResponse(
                    "failed to deserialize gwasinfo response: expected a map".into(),
                )
            })?;

        // Replace table contents inside spawn_blocking. SQLite commits the
        // replacement transaction to disk automatically, so the new snapshot
        // is durable across restarts.
        let db_clone = Arc::clone(&db);
        tokio::task::spawn_blocking(move || {
            let guard = db_clone.lock().unwrap();
            Self::bulk_insert(&guard, &infos)?;
            Ok::<_, OpengwasError>(infos)
        })
        .await
        .expect("spawn_blocking task panicked")
    }

    /// Delete the on-disk gwasinfo cache file at
    /// [`Self::cache_file_path`]. The cache directory itself is preserved.
    ///
    /// The in-process cached connection handle is also dropped, so the
    /// next query will trigger a fresh fetch from the remote API even
    /// within the same process.
    ///
    /// Returns `Ok` even if the cache file does not exist — this is
    /// treated as an idempotent "ensure no stale cache is on disk".
    pub async fn clear_disk_cache(&self) -> Result<()> {
        // Drop the cached connection handle first so any subsequent
        // `ensure_db_loaded` re-opens from a clean slate.
        *self.db.lock().unwrap() = None;

        let path = self.cache_file_path();
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// Return the number of cached GWAS datasets.
    pub async fn gwasinfo_count(&self) -> Result<i64> {
        self.ensure_db_loaded().await?;

        let db = Arc::clone(
            self.db
                .lock()
                .unwrap()
                .as_ref()
                .expect("db initialised by ensure_db_loaded"),
        );
        let db_clone = Arc::clone(&db);
        tokio::task::spawn_blocking(move || {
            let guard = db_clone.lock().unwrap();
            guard.query_row("SELECT COUNT(*) FROM gwasinfo", [], |row| row.get(0))
        })
        .await
        // JoinError (panic in blocking task) → OpengwasError::TaskJoin;
        // the inner rusqlite::Error is converted by the trailing map_err.
        .map_err(OpengwasError::from)?
        .map_err(Into::into)
    }

    /// Search cached GWAS datasets by keyword using a SQL LIKE query on the
    /// given indexed column (`trait_`, `author`, or `population`).
    ///
    /// Returns at most `limit` results.
    pub async fn gwasinfo_search(
        &self,
        keyword: &str,
        field: &str,
        limit: i64,
        sort_by: Option<&str>,
        sort_order: Option<&str>,
    ) -> Result<Vec<GwasInfo>> {
        const ALLOWED: &[&str] = &["trait_", "author", "population"];
        const ALLOWED_SORT: &[&str] = &[
            "nsnp",
            "sample_size",
            "year",
            "ncase",
            "ncontrol",
            "pmid",
            "mr",
            "priority",
            "sd",
            "author",
            "trait_",
        ];
        const ALLOWED_ORDER: &[&str] = &["asc", "desc"];

        if !ALLOWED.contains(&field) {
            return Err(OpengwasError::Param(format!(
                "invalid search field: {field} (allowed: trait, author, population)"
            )));
        }

        // Validate sort parameters if provided.
        if let Some(col) = sort_by {
            if !ALLOWED_SORT.contains(&col) {
                return Err(OpengwasError::Param(format!(
                    "invalid sort column: {col} (allowed: nsnp, sample_size, year, ncase, ncontrol, pmid, mr, priority, sd, author, trait)"
                )));
            }
        }
        if let Some(order) = sort_order {
            if !ALLOWED_ORDER.contains(&order) {
                return Err(OpengwasError::Param(format!(
                    "invalid sort order: {order} (allowed: asc, desc)"
                )));
            }
        }

        self.ensure_db_loaded().await?;

        let db = Arc::clone(
            self.db
                .lock()
                .unwrap()
                .as_ref()
                .expect("db initialised by ensure_db_loaded"),
        );
        let pattern = format!("%{}%", keyword.to_lowercase());
        let field = field.to_string();
        let sort_by = sort_by.map(|s| s.to_string());
        let sort_order = sort_order.map(|s| s.to_string());
        let db_clone = Arc::clone(&db);
        tokio::task::spawn_blocking(move || {
            let guard = db_clone.lock().unwrap();
            let sql = match (&sort_by, &sort_order) {
                (Some(col), Some(order)) => format!(
                    "SELECT {SELECT_COLUMNS} FROM gwasinfo WHERE LOWER(\"{field}\") LIKE ?1 \
                     ORDER BY CASE WHEN \"{col}\" IS NULL THEN 1 ELSE 0 END, \"{col}\" {order} LIMIT ?2"
                ),
                (Some(col), None) => format!(
                    "SELECT {SELECT_COLUMNS} FROM gwasinfo WHERE LOWER(\"{field}\") LIKE ?1 \
                     ORDER BY CASE WHEN \"{col}\" IS NULL THEN 1 ELSE 0 END, \"{col}\" DESC LIMIT ?2"
                ),
                _ => format!(
                    "SELECT {SELECT_COLUMNS} FROM gwasinfo WHERE LOWER(\"{field}\") LIKE ?1 LIMIT ?2"
                ),
            };
            let mut stmt = guard.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params![pattern, limit], row_to_gwasinfo)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        })
        .await
        // JoinError (panic in blocking task) → OpengwasError::TaskJoin;
        // the inner rusqlite::Error is converted by the trailing map_err.
        .map_err(OpengwasError::from)?
        .map_err(Into::into)
    }

    /// `POST /gwasinfo/files` — get download URLs for dataset files
    /// (`.vcf.gz`, `.vcf.gz.tbi`, `_report.html`). URLs expire in 2 hours.
    ///
    /// This method always hits the remote API; it is **not** cached.
    pub async fn gwasinfo_files(&self, req: &GwasInfoFilesRequest) -> Result<Value> {
        self.post_json("/gwasinfo/files", req).await
    }

    // =======================================================================
    // Associations
    // =======================================================================

    /// `POST /associations` — get specific variant associations from specific
    /// GWAS datasets.
    pub async fn associations(&self, req: &AssociationsRequest) -> Result<Value> {
        self.post_json("/associations", req).await
    }

    // =======================================================================
    // Tophits
    // =======================================================================

    /// `POST /tophits` — extract top hits based on a p-value threshold
    /// from a GWAS dataset.
    pub async fn tophits(&self, req: &TophitsRequest) -> Result<Value> {
        self.post_json("/tophits", req).await
    }

    // =======================================================================
    // PheWAS
    // =======================================================================

    /// `POST /phewas` — perform PheWAS of specified variants across all
    /// available GWAS datasets (only p ≤ 0.01).
    pub async fn phewas(&self, req: &PhewasRequest) -> Result<Value> {
        self.post_json("/phewas", req).await
    }

    // =======================================================================
    // Variants
    // =======================================================================

    /// `POST /variants/rsid` — obtain variant information by rs IDs.
    pub async fn variants_rsid(&self, req: &VariantsRsidRequest) -> Result<Value> {
        self.post_json("/variants/rsid", req).await
    }

    /// `POST /variants/chrpos` — obtain variant information by chr:pos.
    pub async fn variants_chrpos(&self, req: &VariantsChrposRequest) -> Result<Value> {
        self.post_json("/variants/chrpos", req).await
    }

    /// `GET /variants/gene/{gene}` — obtain variant information for a gene
    /// (Ensembl or Entrez ID).
    pub async fn variants_gene(&self, gene: &str, radius: Option<i32>) -> Result<Value> {
        let mut path = format!("/variants/gene/{gene}");
        if let Some(r) = radius {
            path = format!("{path}?radius={r}");
        }
        self.get(&path).await
    }

    /// `POST /variants/afl2` — obtain allele frequency and LD scores for
    /// variants.
    pub async fn variants_afl2(&self, req: &VariantsAfl2Request) -> Result<Value> {
        self.post_json("/variants/afl2", req).await
    }

    /// `GET /variants/afl2/snplist` — get list of rsids variable across
    /// populations for ancestry analyses.
    pub async fn variants_afl2_snplist(&self) -> Result<Value> {
        self.get("/variants/afl2/snplist").await
    }

    // =======================================================================
    // LD
    // =======================================================================

    /// `POST /ld/clump` — perform LD clumping on a set of rs IDs using 1000
    /// Genomes reference data.
    pub async fn ld_clump(&self, req: &LdClumpRequest) -> Result<Value> {
        self.post_json("/ld/clump", req).await
    }

    /// `POST /ld/matrix` — for a list of SNPs get the LD R values.
    pub async fn ld_matrix(&self, req: &LdMatrixRequest) -> Result<Value> {
        self.post_json("/ld/matrix", req).await
    }

    /// `POST /ld/reflookup` — lookup whether rsids are present in the LD
    /// reference panel.
    pub async fn ld_reflookup(&self, req: &LdReflookupRequest) -> Result<Value> {
        self.post_json("/ld/reflookup", req).await
    }

    // =======================================================================
    // Edit — metadata
    // =======================================================================

    /// `POST /edit/add` — add new GWAS metadata.
    pub async fn edit_add(&self, req: &EditAddRequest) -> Result<Value> {
        self.post_json("/edit/add", req).await
    }

    /// `POST /edit/edit` — edit existing GWAS metadata.
    pub async fn edit_edit(&self, req: &EditEditRequest) -> Result<Value> {
        self.post_json("/edit/edit", req).await
    }

    /// `GET /edit/list` — list datasets added by the user. Supports filtering
    /// by state (`draft` / `released`) and pagination for released datasets.
    pub async fn edit_list(&self, query: &EditListQuery) -> Result<Value> {
        let q = serde_urlencoded::to_string(query)?;
        self.get(&format!("/edit/list?{q}")).await
    }

    /// `GET /edit/check/{gwas_id}` — get metadata about a specific GWAS
    /// dataset.
    pub async fn edit_check(&self, gwas_id: &str) -> Result<Value> {
        self.get(&format!("/edit/check/{gwas_id}")).await
    }

    /// `GET /edit/state/{gwas_id}` — check DAG runs and task instances
    /// related to the dataset.
    pub async fn edit_state(&self, gwas_id: &str) -> Result<Value> {
        self.get(&format!("/edit/state/{gwas_id}")).await
    }

    /// `DELETE /edit/delete/draft/{gwas_id}` — force the QC pipeline to
    /// fail, delete uploaded files and QC products, and optionally delete
    /// metadata. Available until the dataset is submitted for approval.
    pub async fn edit_delete_draft(&self, gwas_id: &str) -> Result<Value> {
        self.delete(&format!("/edit/delete/draft/{gwas_id}")).await
    }

    // =======================================================================
    // Edit — upload
    // =======================================================================

    /// `POST /edit/upload` — upload a GWAS summary stats file.
    ///
    /// `file_path` is the local path to the text file (can be gzipped).
    /// The `id` must be the identifier the summary stats belong to.
    pub async fn edit_upload(
        &self,
        id: &str,
        file_path: &Path,
        opts: &EditUploadOptions,
    ) -> Result<Value> {
        let file_name = file_path
            .file_name()
            .expect("file has no name")
            .to_string_lossy()
            .to_string();

        let path = file_path.to_owned();
        let file_bytes = tokio::task::spawn_blocking(move || std::fs::read(path)).await??;
        let file_part = multipart::Part::bytes(file_bytes)
            .file_name(file_name)
            .mime_str("application/octet-stream")?;

        let mut form = multipart::Form::new()
            .text("id", id.to_string())
            .text("delimiter", opts.delimiter.clone())
            .text("header", opts.header.clone())
            .text("gzipped", opts.gzipped.clone())
            .text("chr_col", opts.chr_col.to_string())
            .text("pos_col", opts.pos_col.to_string())
            .text("ea_col", opts.ea_col.to_string())
            .text("oa_col", opts.oa_col.to_string())
            .text("beta_col", opts.beta_col.to_string())
            .text("se_col", opts.se_col.to_string())
            .text("pval_col", opts.pval_col.to_string())
            .part("gwas_file", file_part);

        if let Some(v) = opts.snp_col {
            form = form.text("snp_col", v.to_string());
        }
        if let Some(v) = opts.eaf_col {
            form = form.text("eaf_col", v.to_string());
        }
        if let Some(v) = opts.oaf_col {
            form = form.text("oaf_col", v.to_string());
        }
        if let Some(v) = opts.imp_z_col {
            form = form.text("imp_z_col", v.to_string());
        }
        if let Some(v) = opts.imp_info_col {
            form = form.text("imp_info_col", v.to_string());
        }
        if let Some(v) = opts.ncase_col {
            form = form.text("ncase_col", v.to_string());
        }
        if let Some(v) = opts.ncontrol_col {
            form = form.text("ncontrol_col", v.to_string());
        }
        if let Some(ref v) = opts.md5 {
            form = form.text("md5", v.clone());
        }
        if let Some(v) = opts.nsnp {
            form = form.text("nsnp", v.to_string());
        }

        let url = format!("{BASE_URL}/edit/upload");
        let resp = self.client.post(&url).multipart(form).send().await?;
        resp.error_for_status_ref()?;
        Ok(resp.json().await?)
    }

    // =======================================================================
    // Quality Control
    // =======================================================================

    /// `GET /quality_control/list` — return all GWAS datasets requiring QC.
    pub async fn qc_list(&self) -> Result<Value> {
        self.get("/quality_control/list").await
    }

    /// `GET /quality_control/check/{id}` — view files generated for a dataset.
    pub async fn qc_check(&self, id: &str) -> Result<Value> {
        self.get(&format!("/quality_control/check/{id}")).await
    }

    /// `GET /quality_control/report/{gwas_id}` — view the HTML QC report.
    pub async fn qc_report(&self, gwas_id: &str) -> Result<String> {
        self.get_raw(&format!("/quality_control/report/{gwas_id}"))
            .await
    }

    /// `GET /quality_control/submit/{gwas_id}` — submit a dataset for
    /// approval.
    pub async fn qc_submit(&self, gwas_id: &str) -> Result<Value> {
        self.get(&format!("/quality_control/submit/{gwas_id}"))
            .await
    }

    /// `POST /quality_control/release` — release data from the QC process.
    pub async fn qc_release(&self, req: &QcReleaseRequest) -> Result<Value> {
        self.post_json("/quality_control/release", req).await
    }

    /// `DELETE /quality_control/delete/{id}` — delete a QC relationship
    /// (does not delete metadata or data files).
    pub async fn qc_delete(&self, id: &str) -> Result<Value> {
        self.delete(&format!("/quality_control/delete/{id}")).await
    }

    // =======================================================================
    // Download
    // =======================================================================

    /// Return a reference to the inner reqwest [`Client`] (carrying auth
    /// headers), so callers can issue streaming requests without exposing
    /// other internals.
    pub fn http_client(&self) -> &Client {
        &self.client
    }

    /// Download a file from a URL and store it in [`OpendalFileStorage`].
    ///
    /// Uses the existing `Client` (with auth headers) to fetch the file,
    /// then writes the entire body to OpenDAL at the given path.
    pub async fn download_file_to_storage(
        &self,
        url: &str,
        storage: &OpendalFileStorage,
        path: &str,
    ) -> Result<u64> {
        let resp = self.client.get(url).send().await?;
        resp.error_for_status_ref()?;

        let bytes = resp.bytes().await?;
        let size = bytes.len() as u64;

        storage.op.write(path, bytes.to_vec()).await?;

        Ok(size)
    }

    /// Stream-download a file, calling `on_progress` after every chunk with
    /// `(bytes_downloaded, total_bytes)`.
    ///
    /// Unlike [`download_file_to_storage`](Self::download_file_to_storage),
    /// which buffers the entire body in memory, this method streams chunks
    /// through a bounded `Vec` so the peak memory is the chunk size (≈64 KiB)
    /// rather than the full file. `on_progress` is invoked on every chunk,
    /// giving the caller real-time download progress.
    ///
    /// `total_bytes` is `Some` when the server sends a `Content-Length` header;
    /// `None` for chunked-transfer or compressed responses without it.
    pub async fn download_stream_to_storage<F>(
        &self,
        url: &str,
        storage: &OpendalFileStorage,
        path: &str,
        mut on_progress: F,
    ) -> Result<u64>
    where
        F: FnMut(u64, Option<u64>),
    {
        use futures::StreamExt;

        let resp = self.client.get(url).send().await?;
        resp.error_for_status_ref()?;

        let total = resp.content_length();
        let mut stream = resp.bytes_stream();

        // Accumulate chunks into a buffer that grows by the real total (when
        // known) so we avoid reallocations. When the size is unknown we start
        // small and let Vec amortize.
        let mut buf: Vec<u8> = Vec::with_capacity(total.unwrap_or(0) as usize);
        let mut downloaded: u64 = 0;

        while let Some(chunk) = stream.next().await {
            let bytes = chunk?;
            buf.extend_from_slice(&bytes);
            downloaded += bytes.len() as u64;
            on_progress(downloaded, total);
        }

        storage.op.write(path, buf).await?;

        Ok(downloaded)
    }
}

// ---------------------------------------------------------------------------
// Upload options
// ---------------------------------------------------------------------------

/// Options for [`OpengwasClient::edit_upload`].
#[derive(Debug, Clone)]
pub struct EditUploadOptions {
    pub chr_col: i32,
    pub pos_col: i32,
    pub ea_col: i32,
    pub oa_col: i32,
    pub beta_col: i32,
    pub se_col: i32,
    pub pval_col: i32,
    pub delimiter: String,
    pub header: String,
    pub gzipped: String,
    pub snp_col: Option<i32>,
    pub eaf_col: Option<i32>,
    pub oaf_col: Option<i32>,
    pub imp_z_col: Option<i32>,
    pub imp_info_col: Option<i32>,
    pub ncase_col: Option<i32>,
    pub ncontrol_col: Option<i32>,
    pub md5: Option<String>,
    pub nsnp: Option<i32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn get_client() -> OpengwasClient {
        OpengwasClient::new(None).expect("opengwas client")
    }

    /// Build a client pointing at a temporary cache directory so tests
    /// never touch the real `$HOME/.cache/opengwas` file.
    fn client_with_temp_cache() -> (TempDir, OpengwasClient) {
        let tmp = TempDir::new().expect("tempdir");
        let client = OpengwasClient::with_cache_dir_no_auth(tmp.path().to_path_buf())
            .expect("opengwas client");
        (tmp, client)
    }

    fn sample_gwasinfo(id: &str) -> GwasInfo {
        GwasInfo {
            id: Some(id.to_string()),
            trait_: Some(format!("trait-{id}")),
            ..Default::default()
        }
    }

    #[test]
    fn init_db_creates_file_and_schema_on_disk() {
        let (tmp, client) = client_with_temp_cache();
        let path = client.cache_file_path();
        assert!(!path.exists(), "cache file should not exist before init");

        let _db = client.init_db().expect("init_db");

        assert!(path.exists(), "cache file must be created on disk");
        // Open with a fresh Connection to verify schema persistence.
        let verify = Connection::open(&path).expect("reopen cache");
        let n: i64 = verify
            .query_row("SELECT COUNT(*) FROM gwasinfo", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "fresh cache must be empty");
        drop(verify);
        drop(_db);
        drop(client);
        drop(tmp);
    }

    #[test]
    fn gwasinfo_persists_across_clients() {
        let (tmp, client) = client_with_temp_cache();

        // First "session": initialise schema and insert rows.
        {
            let db = client.init_db().expect("init_db");
            let infos = vec![
                sample_gwasinfo("ieu-a-2"),
                sample_gwasinfo("ieu-b-3"),
                sample_gwasinfo("ieu-c-9"),
            ];
            let db_clone = Arc::clone(&db);
            tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(async {
                    tokio::task::spawn_blocking(move || {
                        let guard = db_clone.lock().unwrap();
                        OpengwasClient::bulk_insert(&guard, &infos)
                    })
                    .await
                    .unwrap()
                })
                .expect("bulk_insert");
            // Drop the in-process handle before "restart".
            *client.db.lock().unwrap() = None;
        }

        // Second "session": brand-new client instance, same cache dir.
        let client2 = OpengwasClient::with_cache_dir_no_auth(tmp.path().to_path_buf())
            .expect("opengwas client");
        let db = client2.init_db().expect("init_db second session");
        let guard = db.lock().unwrap();
        let count: i64 = guard
            .query_row("SELECT COUNT(*) FROM gwasinfo", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 3, "rows must persist across process restarts");
        let trait_values: Vec<String> = {
            let mut stmt = guard
                .prepare("SELECT trait_ FROM gwasinfo ORDER BY id")
                .unwrap();
            stmt.query_map([], |r| r.get::<_, String>(0))
                .unwrap()
                .map(|r| r.unwrap())
                .collect()
        };
        assert_eq!(
            trait_values,
            vec![
                "trait-ieu-a-2".to_string(),
                "trait-ieu-b-3".to_string(),
                "trait-ieu-c-9".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn clear_disk_cache_removes_file_and_drops_handle() {
        let (tmp, client) = client_with_temp_cache();
        let path = client.cache_file_path();

        // Populate cache. We mirror what `ensure_db_loaded` does: open
        // the on-disk DB and cache the handle inside the client.
        let db = client.init_db().expect("init_db");
        let infos = vec![sample_gwasinfo("ieu-a-2")];
        let db_clone = Arc::clone(&db);
        tokio::task::spawn_blocking(move || {
            let guard = db_clone.lock().unwrap();
            OpengwasClient::bulk_insert(&guard, &infos)
        })
        .await
        .unwrap()
        .expect("bulk_insert");
        *client.db.lock().unwrap() = Some(db);
        assert!(path.exists(), "cache file must exist after population");
        assert!(
            client.db.lock().unwrap().is_some(),
            "in-process handle must be present"
        );

        client.clear_disk_cache().await.expect("clear_disk_cache");
        assert!(!path.exists(), "cache file must be deleted by clear");
        assert!(
            client.db.lock().unwrap().is_none(),
            "in-process handle must be dropped by clear"
        );

        // Re-opening the client should still work and see an empty cache.
        let client2 = OpengwasClient::with_cache_dir_no_auth(tmp.path().to_path_buf())
            .expect("opengwas client");
        let db2 = client2.init_db().expect("init_db after clear");
        let n: i64 = db2
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM gwasinfo", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "post-clear cache must be empty");
    }

    #[tokio::test]
    async fn clear_disk_cache_is_idempotent_on_missing_file() {
        let (tmp, client) = client_with_temp_cache();
        let path = client.cache_file_path();
        assert!(!path.exists());
        // First call removes a non-existent file — must succeed.
        client.clear_disk_cache().await.expect("first clear");
        // Second call also must succeed (idempotent).
        client.clear_disk_cache().await.expect("second clear");
        drop(tmp);
    }

    #[test]
    fn cache_file_path_lives_under_cache_dir() {
        let (tmp, client) = client_with_temp_cache();
        let path = client.cache_file_path();
        assert!(path.starts_with(tmp.path()));
        assert_eq!(path.file_name().unwrap(), "gwasinfo.sqlite");
    }

    #[ignore]
    #[tokio::test]
    async fn test_search_by_trait() {
        let client = get_client();

        let result = client
            .gwasinfo_search("tension", "trait_", 10, None, None)
            .await
            .unwrap();

        dbg!(result);
    }

    #[tokio::test]
    #[ignore]
    async fn test_gwasinfo_files_response() {
        let client = get_client();

        let result = client
            .gwasinfo_files(&GwasInfoFilesRequest {
                id: vec!["ieu-a-2".to_string()],
                commercial_approval_received: None,
            })
            .await
            .unwrap();

        dbg!(&result);
    }
}
