//! Model bootstrap — persistence and resolution for the app database
//! (`app.db`: `providers` / `models` / `settings`).
//!
//! This module is the single source of truth for "which model does this
//! installation run" and for the schema of the three app tables. It is
//! split into three layers:
//!
//! 1. **Free functions over [`rusqlite::Connection`]** — the SQL and row
//!    mappings. Synchronous; the storage impl below is their only
//!    production caller, tests call them directly.
//! 2. **[`ModelStorage`]** — the async storage abstraction the daemon's
//!    model actor depends on. Swapping the engine (e.g. to a remote
//!    backend) means adding an impl, not touching consumers.
//! 3. **Resolution** — [`resolve_from_parts`] builds a [`Model`] from
//!    storage-independent parts (credentials + optional catalogue row),
//!    so both the sync helper and remote callers share one construction
//!    path. Model metadata comes from the SDK's built-in catalogue,
//!    falling back to the `models` table for entries imported from a
//!    remote catalogue.
//!
//! The SQLite impl holds the connection behind a mutex rather than
//! assuming a single caller — but production wiring (the gateway's model
//! actor) serializes all access on one task anyway, so the lock is
//! uncontended and the sync-inside-async work is bounded to small
//! point queries.

use std::sync::Mutex;

use agentik_sdk::model::{Model, ModelInfo, ProviderConfig, ProviderType};
use async_trait::async_trait;
use rusqlite::Connection;
use uuid::Uuid;

// ─────────────────────────────────────────────────────────────────────
// Schema
// ─────────────────────────────────────────────────────────────────────

/// The row name of the ChatGPT subscription provider.
pub const OPENAI_PROVIDER_ROW: &str = "openai";

/// Create the `providers` / `models` / `settings` tables if they don't
/// exist yet, and run the light column migrations for pre-existing
/// databases. Idempotent — safe to call on every startup.
pub fn ensure_app_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS providers (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            name            TEXT    NOT NULL UNIQUE,
            provider_type   TEXT    NOT NULL,
            base_url        TEXT    NOT NULL,
            api_key         TEXT    NOT NULL,
            auth_method     TEXT    NOT NULL DEFAULT 'Anthropic'
        )",
        (),
    )?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS models (
            id                          INTEGER PRIMARY KEY AUTOINCREMENT,
            model_name                  TEXT    NOT NULL,
            provider_id                 INTEGER NOT NULL,
            context_length              INTEGER NOT NULL DEFAULT 0,
            max_output_tokens           INTEGER NOT NULL DEFAULT 0,
            vision_ability              INTEGER NOT NULL DEFAULT 0,
            supports_function_calling   INTEGER NOT NULL DEFAULT 1,
            supports_streaming          INTEGER NOT NULL DEFAULT 1,
            supports_thinking           INTEGER NOT NULL DEFAULT 0,
            thinking_enabled            INTEGER NOT NULL DEFAULT 0,
            input_token_price           REAL    NOT NULL DEFAULT 0,
            output_token_price          REAL    NOT NULL DEFAULT 0,
            is_custom                   INTEGER NOT NULL DEFAULT 1,
            UNIQUE(provider_id, model_name),
            FOREIGN KEY (provider_id) REFERENCES providers(id) ON DELETE CASCADE
        )",
        (),
    )?;

    // Migration: add `thinking_enabled` to pre-existing databases where
    // the column doesn't exist yet. SQLite doesn't support ADD COLUMN IF
    // NOT EXISTS, so we probe PRAGMA table_info and ignore "duplicate
    // column" errors.
    let has_thinking_enabled: bool = {
        let mut stmt = conn.prepare("PRAGMA table_info(models)")?;
        let cols: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(1))?
            .filter_map(|c| c.ok())
            .collect();
        cols.iter().any(|c| c == "thinking_enabled")
    };
    if !has_thinking_enabled {
        let _ = conn.execute(
            "ALTER TABLE models ADD COLUMN thinking_enabled INTEGER NOT NULL DEFAULT 0",
            (),
        );
    }

    migrate_models_unique_to_provider(conn)?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS settings (
            key             TEXT PRIMARY KEY,
            value           TEXT NOT NULL
        )",
        (),
    )?;

    Ok(())
}

/// Upgrade legacy model storage: add the built-in/custom origin and replace
/// the old globally-unique model name with a per-provider identity.
fn migrate_models_unique_to_provider(conn: &Connection) -> rusqlite::Result<()> {
    let has_is_custom: bool = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('models') WHERE name = 'is_custom'",
        [],
        |row| row.get::<_, i64>(0),
    )? != 0;
    if !has_is_custom {
        rebuild_models_table(conn, false)?;
        return Ok(());
    }
    if models_unique_by_provider(conn)? {
        return Ok(());
    }
    rebuild_models_table(conn, true)?;
    Ok(())
}

fn models_unique_by_provider(conn: &Connection) -> rusqlite::Result<bool> {
    let mut stmt = conn.prepare("PRAGMA index_list(models)")?;
    let indexes = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(stmt);

    for (index, unique, origin) in indexes {
        if unique == 0 || origin != "u" {
            continue;
        }
        let mut info = conn.prepare(&format!("PRAGMA index_info({index})"))?;
        let columns: Vec<String> = info
            .query_map([], |row| row.get::<_, String>(2))?
            .collect::<rusqlite::Result<_>>()?;
        if columns == ["provider_id", "model_name"] {
            return Ok(true);
        }
    }
    Ok(false)
}

fn rebuild_models_table(conn: &Connection, preserve_is_custom: bool) -> rusqlite::Result<()> {
    let foreign_keys_were_on: i64 = conn.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
    if foreign_keys_were_on != 0 {
        conn.execute_batch("PRAGMA foreign_keys=OFF")?;
    }
    let copy_is_custom = if preserve_is_custom {
        ", is_custom"
    } else {
        ""
    };
    let result = conn.execute_batch(&format!(
        "BEGIN IMMEDIATE;
         DROP TABLE IF EXISTS models_new;
         CREATE TABLE models_new (
             id                          INTEGER PRIMARY KEY AUTOINCREMENT,
             model_name                  TEXT    NOT NULL,
             provider_id                 INTEGER NOT NULL,
             context_length              INTEGER NOT NULL DEFAULT 0,
             max_output_tokens           INTEGER NOT NULL DEFAULT 0,
             vision_ability              INTEGER NOT NULL DEFAULT 0,
             supports_function_calling   INTEGER NOT NULL DEFAULT 1,
             supports_streaming          INTEGER NOT NULL DEFAULT 1,
             supports_thinking           INTEGER NOT NULL DEFAULT 0,
             thinking_enabled            INTEGER NOT NULL DEFAULT 0,
             input_token_price           REAL    NOT NULL DEFAULT 0,
             output_token_price          REAL    NOT NULL DEFAULT 0,
             is_custom                   INTEGER NOT NULL DEFAULT 1,
             UNIQUE(provider_id, model_name),
             FOREIGN KEY (provider_id) REFERENCES providers(id) ON DELETE CASCADE
         );
         INSERT INTO models_new (
             id, model_name, provider_id, context_length, max_output_tokens,
             vision_ability, supports_function_calling, supports_streaming,
             supports_thinking, thinking_enabled, input_token_price, output_token_price
         )
         SELECT id, model_name, provider_id, context_length, max_output_tokens,
                vision_ability, supports_function_calling, supports_streaming,
                supports_thinking, thinking_enabled, input_token_price, output_token_price
                {copy_is_custom}
         FROM models;
         DROP TABLE models;
         ALTER TABLE models_new RENAME TO models;
         COMMIT;",
    ));
    if foreign_keys_were_on != 0 {
        conn.execute_batch("PRAGMA foreign_keys=ON")?;
    }
    result
}

// ─────────────────────────────────────────────────────────────────────
// Row mappings
// ─────────────────────────────────────────────────────────────────────

/// A row from the `providers` table.
// Mirrors the `providers` table schema; not every column is consumed yet.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[allow(dead_code)]
pub struct ProviderRow {
    pub id: i64,
    pub name: String,
    pub provider_type: String,
    pub base_url: String,
    pub api_key: String,
    pub auth_method: String,
}

impl ProviderRow {
    pub fn all(conn: &Connection) -> rusqlite::Result<Vec<ProviderRow>> {
        let mut stmt = conn.prepare(
            "SELECT id, name, provider_type, base_url, api_key, auth_method FROM providers ORDER BY id",
        )?;
        stmt.query_map([], |row| {
            Ok(ProviderRow {
                id: row.get(0)?,
                name: row.get(1)?,
                provider_type: row.get(2)?,
                base_url: row.get(3)?,
                api_key: row.get(4)?,
                auth_method: row.get(5)?,
            })
        })?
        .collect()
    }
}

/// A row from the `models` table — a user-added model or a provider's
/// remote-catalogue import (e.g. OpenRouter's `/v1/models`). Mirrors the
/// [`ModelInfo`](agentik_sdk::model::ModelInfo) columns that survive a DB
/// round-trip.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ModelRow {
    pub model_name: String,
    /// Owning provider's `name` (== its provider-type string).
    pub provider_name: String,
    pub context_length: u64,
    pub max_output_tokens: u64,
    pub vision_ability: bool,
    pub supports_function_calling: bool,
    pub supports_streaming: bool,
    pub supports_thinking: bool,
    pub thinking_enabled: bool,
    pub input_token_price: f64,
    pub output_token_price: f64,
    /// True when the entry is user-added, edited, or catalogue-imported.
    /// SDK presets are `false` and remain read-only in the TUI.
    #[serde(default)]
    pub custom: bool,
}

impl ModelRow {
    /// All model rows joined with their provider name, ordered by model name.
    pub fn all(conn: &Connection) -> rusqlite::Result<Vec<ModelRow>> {
        let mut stmt = conn.prepare(
            "SELECT m.model_name, p.name, m.context_length, m.max_output_tokens,
                    m.vision_ability, m.supports_function_calling, m.supports_streaming,
                    m.supports_thinking, m.thinking_enabled,
                    m.input_token_price, m.output_token_price
                    , m.is_custom
             FROM models m JOIN providers p ON p.id = m.provider_id
             ORDER BY m.model_name",
        )?;
        stmt.query_map([], row_to_model)?.collect()
    }

    /// Look up a single model by (provider name, model name).
    pub fn find(conn: &Connection, provider_name: &str, model_name: &str) -> Option<ModelRow> {
        conn.query_row(
            "SELECT m.model_name, p.name, m.context_length, m.max_output_tokens,
                    m.vision_ability, m.supports_function_calling, m.supports_streaming,
                    m.supports_thinking, m.thinking_enabled,
                    m.input_token_price, m.output_token_price
                    , m.is_custom
             FROM models m JOIN providers p ON p.id = m.provider_id
             WHERE p.name = ?1 AND m.model_name = ?2",
            rusqlite::params![provider_name, model_name],
            row_to_model,
        )
        .ok()
    }

    /// Convert to a metadata-only [`ModelInfo`](agentik_sdk::model::ModelInfo)
    /// — `provider_id` is left nil; the caller binds it when constructing
    /// the provider config.
    pub fn to_model_info(&self) -> agentik_sdk::model::ModelInfo {
        agentik_sdk::model::ModelInfo {
            model_name: self.model_name.clone(),
            provider_id: Uuid::nil(),
            context_length: self.context_length,
            max_output_tokens: self.max_output_tokens,
            vision_ability: self.vision_ability,
            supports_function_calling: self.supports_function_calling,
            supports_streaming: self.supports_streaming,
            supports_thinking: self.supports_thinking,
            thinking_enabled: self.thinking_enabled,
            input_token_price: self.input_token_price,
            output_token_price: self.output_token_price,
            custom: self.custom,
            ..Default::default()
        }
    }
}

/// Map one SELECT row (column order as in [`ModelRow::all`]) to a `ModelRow`.
/// SQLite integers are read as `i64` (rusqlite has no `u64` SQL impl) and
/// losslessly widened.
fn row_to_model(row: &rusqlite::Row<'_>) -> rusqlite::Result<ModelRow> {
    Ok(ModelRow {
        model_name: row.get(0)?,
        provider_name: row.get(1)?,
        context_length: row.get::<_, i64>(2)?.max(0) as u64,
        max_output_tokens: row.get::<_, i64>(3)?.max(0) as u64,
        vision_ability: row.get::<_, i64>(4)? != 0,
        supports_function_calling: row.get::<_, i64>(5)? != 0,
        supports_streaming: row.get::<_, i64>(6)? != 0,
        supports_thinking: row.get::<_, i64>(7)? != 0,
        thinking_enabled: row.get::<_, i64>(8)? != 0,
        input_token_price: row.get(9)?,
        output_token_price: row.get(10)?,
        custom: row.get::<_, i64>(11)? != 0,
    })
}

// ─────────────────────────────────────────────────────────────────────
// Free SQL helpers (single-writer style, over `&Connection`)
// ─────────────────────────────────────────────────────────────────────

/// Provider credentials as needed to build a [`Model`].
#[derive(Debug, Clone)]
pub struct ProviderAuth {
    pub api_key: String,
    pub base_url: String,
}

/// Look up a provider's credentials by row name.
fn provider_auth(conn: &Connection, name: &str) -> Option<ProviderAuth> {
    conn.query_row(
        "SELECT api_key, base_url FROM providers WHERE name = ?1",
        [name],
        |row| {
            Ok(ProviderAuth {
                api_key: row.get(0)?,
                base_url: row.get(1)?,
            })
        },
    )
    .ok()
}

/// Insert or update a provider row (credentials + base URL). The stored
/// `auth_method` is derived from the provider type's default — the column
/// is not user-editable.
pub fn save_provider(conn: &Connection, params: &SaveProviderParams) -> rusqlite::Result<()> {
    let auth_method = agentik_sdk::provider::registry::default_auth_method(&ProviderType::from(
        params.provider_type.as_str(),
    ))
    .storage_tag()
    .to_string();
    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM providers WHERE name = ?1",
            [&params.name],
            |row| row.get(0),
        )
        .ok();
    let result = if let Some(id) = existing {
        conn.execute(
            "UPDATE providers SET api_key = ?1, base_url = ?2, auth_method = ?3 WHERE id = ?4",
            rusqlite::params![params.api_key, params.base_url, auth_method, id],
        )
    } else {
        conn.execute(
            "INSERT INTO providers (name, provider_type, base_url, api_key, auth_method)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                params.name,
                params.provider_type,
                params.base_url,
                params.api_key,
                auth_method
            ],
        )
    };
    result.map(|_| ())
}

/// New-provider parameters (mirrors the HTTP `SaveProviderRequest` without
/// tying this module to the wire type).
#[derive(Debug, Clone)]
pub struct SaveProviderParams {
    pub name: String,
    pub provider_type: String,
    pub base_url: String,
    pub api_key: String,
}

/// Insert or update a user-owned model row. `original_model_name` identifies
/// the row being renamed; `None` creates a new custom model.
#[derive(Debug, Clone)]
pub struct SaveModelParams {
    pub provider_name: String,
    pub original_model_name: Option<String>,
    pub model: ModelRow,
}

/// Parameters for removing a custom model row. Built-in SDK presets are
/// protected at this layer; see [`delete_model`].
#[derive(Debug, Clone)]
pub struct DeleteModelParams {
    pub provider_name: String,
    pub model_name: String,
}

pub fn save_model(conn: &Connection, params: &SaveModelParams) -> Result<(), ModelStorageError> {
    let model_name = params.model.model_name.trim();
    if model_name.is_empty() || model_name.contains(':') {
        return Err(ModelStorageError::Message(
            "model name must be non-empty and must not contain ':'".into(),
        ));
    }
    if params.model.context_length == 0 || params.model.max_output_tokens == 0 {
        return Err(ModelStorageError::Message(
            "context length and max output must be greater than zero".into(),
        ));
    }
    if params.model.thinking_enabled && !params.model.supports_thinking {
        return Err(ModelStorageError::Message(
            "thinking enabled requires thinking support".into(),
        ));
    }

    let provider_id: i64 = conn
        .query_row(
            "SELECT id FROM providers WHERE name = ?1",
            [&params.provider_name],
            |row| row.get(0),
        )
        .map_err(|_| {
            ModelStorageError::Message(format!(
                "provider `{}` is not configured",
                params.provider_name
            ))
        })?;

    let original = params
        .original_model_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty());
    if let Some(original) = original {
        if ModelRow::find(conn, &params.provider_name, original).is_none() {
            return Err(ModelStorageError::Message(format!(
                "custom model `{original}` was not found"
            )));
        }
    }

    let duplicate: i64 = conn.query_row(
        "SELECT COUNT(*) FROM models m JOIN providers p ON p.id = m.provider_id
         WHERE p.name = ?1 AND m.model_name = ?2 AND (?3 IS NULL OR m.model_name <> ?3)",
        rusqlite::params![params.provider_name, model_name, original],
        |row| row.get(0),
    )?;
    if duplicate > 0 {
        return Err(ModelStorageError::Message(format!(
            "model `{model_name}` already exists for `{}`",
            params.provider_name
        )));
    }

    if let Some(original) = original {
        conn.execute(
            "UPDATE models SET
                 model_name = ?1, context_length = ?2, max_output_tokens = ?3,
                 vision_ability = ?4, supports_function_calling = ?5,
                 supports_streaming = ?6, supports_thinking = ?7,
                 thinking_enabled = ?8, input_token_price = ?9,
                 output_token_price = ?10, is_custom = 1
             WHERE provider_id = ?11 AND model_name = ?12",
            rusqlite::params![
                model_name,
                params.model.context_length as i64,
                params.model.max_output_tokens as i64,
                params.model.vision_ability,
                params.model.supports_function_calling,
                params.model.supports_streaming,
                params.model.supports_thinking,
                params.model.thinking_enabled,
                params.model.input_token_price,
                params.model.output_token_price,
                provider_id,
                original
            ],
        )?;
        if original != model_name {
            let old_spec = format!("{}:{original}", params.provider_name);
            let new_spec = format!("{}:{model_name}", params.provider_name);
            conn.execute(
                "UPDATE settings SET value = ?1 WHERE key = 'active_model' AND value = ?2",
                rusqlite::params![new_spec, old_spec],
            )?;
        }
    } else {
        conn.execute(
            "INSERT INTO models (
                 model_name, provider_id, context_length, max_output_tokens,
                 vision_ability, supports_function_calling, supports_streaming,
                 supports_thinking, thinking_enabled, input_token_price,
                 output_token_price, is_custom
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 1)",
            rusqlite::params![
                model_name,
                provider_id,
                params.model.context_length as i64,
                params.model.max_output_tokens as i64,
                params.model.vision_ability,
                params.model.supports_function_calling,
                params.model.supports_streaming,
                params.model.supports_thinking,
                params.model.thinking_enabled,
                params.model.input_token_price,
                params.model.output_token_price
            ],
        )?;
    }
    Ok(())
}

/// Remove a custom model row. Built-in SDK presets (`is_custom = 0`) are
/// refused at this layer so the storage backend never silently deletes a
/// read-only row, regardless of which caller triggered the request.
///
/// The active-model spec (if pointing at this row) is intentionally left
/// dangling: `resolve_model_spec` treats an unknown spec as None and the
/// next invocation falls back to the default model. A best-effort rewrite
/// here would race against concurrent UI updates and bring no real
/// recovery benefit.
pub fn delete_model(
    conn: &Connection,
    params: &DeleteModelParams,
) -> Result<(), ModelStorageError> {
    let model_name = params.model_name.trim();
    if model_name.is_empty() {
        return Err(ModelStorageError::Message(
            "model name must not be empty".into(),
        ));
    }
    if model_name.contains(':') {
        return Err(ModelStorageError::Message(
            "model name must not contain ':'".into(),
        ));
    }

    let row = ModelRow::find(conn, &params.provider_name, model_name).ok_or_else(|| {
        ModelStorageError::Message(format!(
            "custom model `{model_name}` was not found for provider `{}`",
            params.provider_name
        ))
    })?;
    if !row.custom {
        return Err(ModelStorageError::Message(format!(
            "model `{model_name}` is a built-in preset and cannot be deleted"
        )));
    }

    let affected = conn.execute(
        "DELETE FROM models
         WHERE provider_id = (SELECT id FROM providers WHERE name = ?1)
           AND model_name = ?2",
        rusqlite::params![params.provider_name, model_name],
    )?;
    if affected == 0 {
        return Err(ModelStorageError::Message(format!(
            "custom model `{model_name}` was not found for provider `{}`",
            params.provider_name
        )));
    }
    Ok(())
}

/// Read the ChatGPT token blob JSON stored in a provider row's `api_key`.
pub fn chatgpt_blob_json(conn: &Connection, provider: &str) -> Option<String> {
    conn.query_row(
        "SELECT api_key FROM providers WHERE name = ?1",
        [provider],
        |row| row.get::<_, String>(0),
    )
    .ok()
}

/// Persist a ChatGPT token blob JSON to the openai provider row.
pub fn save_chatgpt_blob_json(conn: &Connection, blob_json: &str) -> rusqlite::Result<()> {
    let base_url = agentik_sdk::provider::registry::default_base_url(&ProviderType::Openai)
        .unwrap_or_default()
        .to_string();
    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM providers WHERE name = ?1",
            [OPENAI_PROVIDER_ROW],
            |row| row.get(0),
        )
        .ok();
    let result = if let Some(id) = existing {
        conn.execute(
            "UPDATE providers SET api_key = ?1, base_url = ?2, auth_method = 'chatgpt' WHERE id = ?3",
            rusqlite::params![blob_json, base_url, id],
        )
    } else {
        conn.execute(
            "INSERT INTO providers (name, provider_type, base_url, api_key, auth_method)
             VALUES ('openai', 'openai', ?1, ?2, 'chatgpt')",
            rusqlite::params![base_url, blob_json],
        )
    };
    result.map(|_| ())
}

/// Upsert a freshly fetched catalogue without deleting user-owned custom
/// models. This is atomic, so a mid-loop failure cannot leave a partial
/// refresh.
pub fn replace_provider_models(
    conn: &mut Connection,
    provider_name: &str,
    models: &[ModelInfo],
) -> rusqlite::Result<usize> {
    let tx = conn.transaction()?;
    let provider_id: i64 = tx.query_row(
        "SELECT id FROM providers WHERE name = ?1",
        [provider_name],
        |r| r.get(0),
    )?;
    for m in models {
        tx.execute(
            "INSERT OR REPLACE INTO models (model_name, provider_id, context_length,
             max_output_tokens, vision_ability, supports_function_calling, supports_streaming,
             supports_thinking, thinking_enabled, input_token_price, output_token_price,
             is_custom)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)
             ON CONFLICT(provider_id, model_name) DO UPDATE SET
                 context_length = excluded.context_length,
                 max_output_tokens = excluded.max_output_tokens,
                 vision_ability = excluded.vision_ability,
                 supports_function_calling = excluded.supports_function_calling,
                 supports_streaming = excluded.supports_streaming,
                 supports_thinking = excluded.supports_thinking,
                 thinking_enabled = excluded.thinking_enabled,
                 input_token_price = excluded.input_token_price,
                 output_token_price = excluded.output_token_price,
                 is_custom = excluded.is_custom",
            rusqlite::params![
                m.model_name,
                provider_id,
                m.context_length as i64,
                m.max_output_tokens as i64,
                m.vision_ability as i64,
                m.supports_function_calling as i64,
                m.supports_streaming as i64,
                m.supports_thinking as i64,
                m.thinking_enabled as i64,
                m.input_token_price,
                m.output_token_price,
                1,
            ],
        )?;
    }
    tx.commit()?;
    Ok(models.len())
}

/// The `active_model` setting's raw `"provider_name:model_name"` spec,
/// when one is configured.
pub fn active_model_spec(conn: &Connection) -> Option<String> {
    get_setting(conn, "active_model")
}

/// Read one `settings` row. Missing key → `None`; a real DB error also
/// degrades to `None` (settings are best-effort display state).
pub fn get_setting(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
        row.get::<_, String>(0)
    })
    .ok()
}

/// Write one `settings` row.
pub fn put_setting(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
        rusqlite::params![key, value],
    )
    .map(|_| ())
}

// ─────────────────────────────────────────────────────────────────────
// Storage abstraction
// ─────────────────────────────────────────────────────────────────────

/// Errors surfaced by a [`ModelStorage`] backend.
#[derive(Debug, thiserror::Error)]
pub enum ModelStorageError {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("{0}")]
    Message(String),
}

pub type StorageResult<T> = std::result::Result<T, ModelStorageError>;

/// Async storage facade over the app database — the seam the daemon's
/// model actor depends on. Methods map one-to-one onto the free SQL
/// helpers above; the default SQLite impl wraps a mutex-guarded
/// connection.
#[async_trait]
pub trait ModelStorage: Send + Sync {
    async fn providers(&self) -> StorageResult<Vec<ProviderRow>>;
    async fn models(&self) -> StorageResult<Vec<ModelRow>>;
    async fn find_model(
        &self,
        provider_name: &str,
        model_name: &str,
    ) -> StorageResult<Option<ModelRow>>;
    async fn provider_auth(&self, name: &str) -> StorageResult<Option<ProviderAuth>>;
    async fn active_model_spec(&self) -> StorageResult<Option<String>>;
    async fn set_active_model(&self, spec: &str) -> StorageResult<()>;
    async fn save_provider(&self, params: &SaveProviderParams) -> StorageResult<()>;
    async fn save_model(&self, params: &SaveModelParams) -> StorageResult<()>;
    async fn delete_model(&self, params: &DeleteModelParams) -> StorageResult<()>;
    async fn replace_provider_models(
        &self,
        provider_name: &str,
        models: &[ModelInfo],
    ) -> StorageResult<usize>;
    async fn chatgpt_blob_json(&self) -> StorageResult<Option<String>>;
    async fn save_chatgpt_blob_json(&self, blob_json: &str) -> StorageResult<()>;
    async fn get_setting(&self, key: &str) -> StorageResult<Option<String>>;
    async fn put_setting(&self, key: &str, value: &str) -> StorageResult<()>;
}

/// The default [`ModelStorage`]: a local SQLite file (rusqlite), opened
/// with foreign keys on and the schema ensured. The daemon must hold the
/// instance lock before opening it.
pub struct SqliteModelStorage {
    conn: Mutex<Connection>,
}

impl SqliteModelStorage {
    pub fn open(path: &std::path::Path) -> StorageResult<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                ModelStorageError::Message(format!("create {}: {e}", parent.display()))
            })?;
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        ensure_app_schema(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn with_conn<T>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> StorageResult<T> {
        let conn = self.conn.lock().expect("model store mutex poisoned");
        f(&conn).map_err(ModelStorageError::Db)
    }
}

#[async_trait]
impl ModelStorage for SqliteModelStorage {
    async fn providers(&self) -> StorageResult<Vec<ProviderRow>> {
        self.with_conn(ProviderRow::all)
    }

    async fn models(&self) -> StorageResult<Vec<ModelRow>> {
        self.with_conn(ModelRow::all)
    }

    async fn find_model(
        &self,
        provider_name: &str,
        model_name: &str,
    ) -> StorageResult<Option<ModelRow>> {
        self.with_conn(|conn| Ok(ModelRow::find(conn, provider_name, model_name)))
    }

    async fn provider_auth(&self, name: &str) -> StorageResult<Option<ProviderAuth>> {
        self.with_conn(|conn| Ok(provider_auth(conn, name)))
    }

    async fn active_model_spec(&self) -> StorageResult<Option<String>> {
        self.with_conn(|conn| Ok(active_model_spec(conn)))
    }

    async fn set_active_model(&self, spec: &str) -> StorageResult<()> {
        self.with_conn(|conn| put_setting(conn, "active_model", spec))
    }

    async fn save_provider(&self, params: &SaveProviderParams) -> StorageResult<()> {
        self.with_conn(|conn| save_provider(conn, params))
    }

    async fn save_model(&self, params: &SaveModelParams) -> StorageResult<()> {
        let conn = self.conn.lock().expect("model store mutex poisoned");
        save_model(&conn, params)
    }

    async fn delete_model(&self, params: &DeleteModelParams) -> StorageResult<()> {
        let conn = self.conn.lock().expect("model store mutex poisoned");
        delete_model(&conn, params)
    }

    async fn replace_provider_models(
        &self,
        provider_name: &str,
        models: &[ModelInfo],
    ) -> StorageResult<usize> {
        let mut conn = self.conn.lock().expect("model store mutex poisoned");
        replace_provider_models(&mut conn, provider_name, models).map_err(ModelStorageError::Db)
    }

    async fn chatgpt_blob_json(&self) -> StorageResult<Option<String>> {
        self.with_conn(|conn| Ok(chatgpt_blob_json(conn, OPENAI_PROVIDER_ROW)))
    }

    async fn save_chatgpt_blob_json(&self, blob_json: &str) -> StorageResult<()> {
        self.with_conn(|conn| save_chatgpt_blob_json(conn, blob_json))
    }

    async fn get_setting(&self, key: &str) -> StorageResult<Option<String>> {
        self.with_conn(|conn| Ok(get_setting(conn, key)))
    }

    async fn put_setting(&self, key: &str, value: &str) -> StorageResult<()> {
        self.with_conn(|conn| put_setting(conn, key, value))
    }
}

// ─────────────────────────────────────────────────────────────────────
// Resolution
// ─────────────────────────────────────────────────────────────────────

/// Resolve the installation's active model.
///
/// Reads `active_model` from the `settings` table (format:
/// `"provider_name:model_name"`) and delegates to
/// [`resolve_model_spec`]. Returns `None` if no model is configured,
/// credentials are missing, or any lookup fails — callers treat that as
/// "run without a model".
pub fn resolve_active_model(conn: &Connection) -> Option<Model> {
    let active = active_model_spec(conn)?;
    resolve_model_spec(conn, &active)
}

/// Build a [`Model`] from a `"provider_name:model_name"` spec, using
/// provider credentials from the `providers` table.
///
/// Delegates to [`resolve_from_parts`]; returns `None` if the spec is
/// malformed, the provider is not configured (or has an empty API key),
/// or the model is unknown.
pub fn resolve_model_spec(conn: &Connection, spec: &str) -> Option<Model> {
    // Parse "provider_name:model_name" — model names may themselves
    // contain colons, so split on the LAST colon.
    let (provider_name, model_name) = spec.rsplit_once(':')?;
    let auth = provider_auth(conn, provider_name)?;
    let db_model = ModelRow::find(conn, provider_name, model_name);
    resolve_from_parts(provider_name, model_name, &auth, db_model)
}

/// Storage-independent model construction. Persisted custom metadata wins
/// over the SDK preset, then build the provider config + client stack.
///
/// Returns `None` for an empty API key (except Ollama, which is local by
/// default) or an unknown model.
pub fn resolve_from_parts(
    provider_name: &str,
    model_name: &str,
    auth: &ProviderAuth,
    db_model: Option<ModelRow>,
) -> Option<Model> {
    use agentik_sdk::provider::registry;

    if auth.api_key.is_empty() && provider_name != "ollama" {
        return None;
    }

    let provider_type = ProviderType::from(provider_name);
    let base_url = if auth.base_url.is_empty() {
        registry::default_base_url(&provider_type)
            .unwrap_or("")
            .to_string()
    } else {
        auth.base_url.clone()
    };
    let auth_method = registry::default_auth_method(&provider_type);
    let preset_models = registry::preset_models(&provider_type).unwrap_or_default();
    let mut model_info = db_model.map(|row| row.to_model_info()).or_else(|| {
        preset_models
            .into_iter()
            .find(|m| m.model_name == model_name)
    })?;

    let provider_config = ProviderConfig {
        id: Uuid::nil(),
        name: provider_name.to_string(),
        provider_type,
        base_url,
        api_key: auth.api_key.clone(),
        auth_method,
    };
    model_info.provider_id = provider_config.id;

    Model::new(model_info, &provider_config).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        ensure_app_schema(&conn).unwrap();
        conn
    }

    fn seed_custom_provider(conn: &Connection) {
        conn.execute(
            "INSERT INTO providers (name, provider_type, base_url, api_key, auth_method)
             VALUES ('Custom', 'Custom', 'http://localhost:9', 'test-key', 'Anthropic')",
            (),
        )
        .unwrap();
        conn.execute(
            "INSERT INTO models (model_name, provider_id, context_length, max_output_tokens)
             VALUES ('db-only-model', (SELECT id FROM providers WHERE name = 'Custom'), 12345, 999)",
            (),
        )
        .unwrap();
    }

    #[test]
    fn schema_init_is_idempotent() {
        let conn = conn();
        ensure_app_schema(&conn).unwrap();
        ensure_app_schema(&conn).unwrap();
    }

    #[test]
    fn migrates_legacy_model_schema_to_custom_rows() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE providers (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                provider_type TEXT NOT NULL,
                base_url TEXT NOT NULL,
                api_key TEXT NOT NULL,
                auth_method TEXT NOT NULL DEFAULT 'Anthropic'
            )",
            (),
        )
        .unwrap();
        conn.execute(
            "CREATE TABLE models (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                model_name TEXT NOT NULL UNIQUE,
                provider_id INTEGER NOT NULL,
                context_length INTEGER NOT NULL DEFAULT 0,
                max_output_tokens INTEGER NOT NULL DEFAULT 0,
                vision_ability INTEGER NOT NULL DEFAULT 0,
                supports_function_calling INTEGER NOT NULL DEFAULT 1,
                supports_streaming INTEGER NOT NULL DEFAULT 1,
                supports_thinking INTEGER NOT NULL DEFAULT 0,
                input_token_price REAL NOT NULL DEFAULT 0,
                output_token_price REAL NOT NULL DEFAULT 0,
                FOREIGN KEY (provider_id) REFERENCES providers(id) ON DELETE CASCADE
            )",
            (),
        )
        .unwrap();
        conn.execute(
            "INSERT INTO providers (name, provider_type, base_url, api_key)
             VALUES ('ollama', 'ollama', 'http://localhost:11434', '')",
            (),
        )
        .unwrap();
        conn.execute(
            "INSERT INTO models (model_name, provider_id, context_length)
             VALUES ('legacy-model', (SELECT id FROM providers WHERE name = 'ollama'), 4096)",
            (),
        )
        .unwrap();

        ensure_app_schema(&conn).unwrap();
        assert!(models_unique_by_provider(&conn).unwrap());
        let row = ModelRow::find(&conn, "ollama", "legacy-model")
            .expect("legacy model should survive migration");
        assert!(row.custom);
        assert_eq!(row.context_length, 4096);
    }

    #[test]
    fn resolves_db_imported_model_for_custom_provider() {
        let conn = conn();
        seed_custom_provider(&conn);
        let model = resolve_model_spec(&conn, "Custom:db-only-model")
            .expect("DB-imported model should resolve");
        assert_eq!(model.context_length(), 12345);
    }

    #[test]
    fn resolve_active_model_follows_settings_key() {
        let conn = conn();
        assert!(resolve_active_model(&conn).is_none(), "no settings yet");
        seed_custom_provider(&conn);
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('active_model', 'Custom:db-only-model')",
            (),
        )
        .unwrap();
        assert_eq!(
            active_model_spec(&conn).as_deref(),
            Some("Custom:db-only-model")
        );
        assert!(resolve_active_model(&conn).is_some());
    }

    #[test]
    fn empty_api_key_or_unknown_provider_returns_none() {
        let conn = conn();
        assert!(resolve_model_spec(&conn, "missing:any").is_none());
        assert!(resolve_model_spec(&conn, "no-colon-spec").is_none());
        seed_custom_provider(&conn);
        conn.execute(
            "UPDATE providers SET api_key = '' WHERE name = 'Custom'",
            (),
        )
        .unwrap();
        assert!(resolve_model_spec(&conn, "Custom:db-only-model").is_none());
    }

    #[test]
    fn ollama_resolves_without_an_api_key() {
        let conn = conn();
        conn.execute(
            "INSERT INTO providers (name, provider_type, base_url, api_key, auth_method)
             VALUES ('ollama', 'ollama', 'http://localhost:11434', '', 'None')",
            (),
        )
        .unwrap();
        conn.execute(
            "INSERT INTO models (model_name, provider_id, context_length, max_output_tokens)
             VALUES ('llama3.2', (SELECT id FROM providers WHERE name = 'ollama'), 8192, 4096)",
            (),
        )
        .unwrap();

        let model = resolve_model_spec(&conn, "ollama:llama3.2")
            .expect("local Ollama provider should resolve without credentials");
        assert_eq!(model.provider_name(), "ollama");
    }

    #[test]
    fn custom_models_save_edit_and_resolve() {
        let mut conn = conn();
        conn.execute(
            "INSERT INTO providers (name, provider_type, base_url, api_key, auth_method)
             VALUES ('ollama', 'ollama', 'http://localhost:11434', '', 'None')",
            (),
        )
        .unwrap();
        let model = ModelRow {
            provider_name: "ollama".into(),
            model_name: "local-agent".into(),
            context_length: 77_000,
            max_output_tokens: 8_000,
            vision_ability: false,
            supports_function_calling: true,
            supports_streaming: true,
            supports_thinking: false,
            thinking_enabled: false,
            input_token_price: 0.0,
            output_token_price: 0.0,
            custom: true,
        };
        save_model(
            &conn,
            &SaveModelParams {
                provider_name: "ollama".into(),
                original_model_name: None,
                model: model.clone(),
            },
        )
        .expect("custom model should save");

        let resolved =
            resolve_model_spec(&conn, "ollama:local-agent").expect("custom model should resolve");
        assert_eq!(resolved.context_length(), 77_000);

        conn.execute(
            "INSERT INTO models (model_name, provider_id, context_length, max_output_tokens)
             VALUES ('preserve-me', (SELECT id FROM providers WHERE name = 'ollama'), 4096, 1024)",
            (),
        )
        .unwrap();
        replace_provider_models(
            &mut conn,
            "ollama",
            &[agentik_sdk::model::ModelInfo {
                model_name: "local-agent".into(),
                context_length: 64_000,
                max_output_tokens: 8_000,
                ..Default::default()
            }],
        )
        .expect("remote catalogue upsert should succeed");
        let preserved = ModelRow::find(&conn, "ollama", "preserve-me")
            .expect("user-owned custom model must survive refresh");
        assert!(preserved.custom);
        let refreshed = ModelRow::find(&conn, "ollama", "local-agent").unwrap();
        assert!(refreshed.custom);
        assert_eq!(refreshed.context_length, 64_000);

        let renamed = ModelRow {
            model_name: "local-agent-v2".into(),
            context_length: 88_000,
            ..model
        };
        save_model(
            &conn,
            &SaveModelParams {
                provider_name: "ollama".into(),
                original_model_name: Some("local-agent".into()),
                model: renamed,
            },
        )
        .expect("custom model rename should save");
        let row = ModelRow::find(&conn, "ollama", "local-agent-v2").unwrap();
        assert!(row.custom);
        assert_eq!(row.context_length, 88_000);
        assert!(ModelRow::find(&conn, "ollama", "local-agent").is_none());
    }

    #[test]
    fn custom_storage_overrides_builtin_metadata_and_blocks_builtin_edits() {
        let conn = conn();
        conn.execute(
            "INSERT INTO providers (name, provider_type, base_url, api_key, auth_method)
             VALUES ('deepseek', 'deepseek', 'https://api.deepseek.com', 'key', 'Bearer')",
            (),
        )
        .unwrap();
        let preset = agentik_sdk::provider::registry::preset_models(&ProviderType::Deepseek)
            .unwrap()
            .into_iter()
            .next()
            .expect("deepseek preset");
        let custom = ModelRow {
            provider_name: "deepseek".into(),
            model_name: preset.model_name.clone(),
            context_length: 123_456,
            max_output_tokens: 8_192,
            custom: true,
            ..ModelRow::default()
        };
        save_model(
            &conn,
            &SaveModelParams {
                provider_name: "deepseek".into(),
                original_model_name: None,
                model: custom,
            },
        )
        .expect("custom override may shadow a preset name");

        let model = resolve_model_spec(&conn, &format!("deepseek:{}", preset.model_name))
            .expect("custom override should resolve");
        assert_eq!(model.context_length(), 123_456);

        let error = save_model(
            &conn,
            &SaveModelParams {
                provider_name: "deepseek".into(),
                original_model_name: Some("never-saved".into()),
                model: ModelRow {
                    provider_name: "deepseek".into(),
                    model_name: "never-saved".into(),
                    context_length: 8_192,
                    max_output_tokens: 1_024,
                    custom: true,
                    ..ModelRow::default()
                },
            },
        )
        .expect_err("editing a missing row must not synthesize a custom model");
        assert!(error.to_string().contains("was not found"));
    }

    #[test]
    fn delete_model_removes_custom_row_and_refuses_builtins() {
        let conn = conn();
        // Seed a built-in (custom=0) and a custom row under the same provider.
        conn.execute(
            "INSERT INTO providers (name, provider_type, base_url, api_key, auth_method)
             VALUES ('ollama', 'ollama', 'http://localhost:11434', '', 'None')",
            (),
        )
        .unwrap();
        conn.execute(
            "INSERT INTO models (model_name, provider_id, is_custom,
                                  context_length, max_output_tokens)
             VALUES ('preset-llama', (SELECT id FROM providers WHERE name='ollama'),
                     0, 8192, 2048)",
            (),
        )
        .unwrap();
        save_model(
            &conn,
            &SaveModelParams {
                provider_name: "ollama".into(),
                original_model_name: None,
                model: ModelRow {
                    provider_name: "ollama".into(),
                    model_name: "local-agent".into(),
                    context_length: 32_768,
                    max_output_tokens: 4_096,
                    custom: true,
                    ..ModelRow::default()
                },
            },
        )
        .expect("seed custom row");

        // Built-in preset: refused even if the caller asks to remove it.
        let err = delete_model(
            &conn,
            &DeleteModelParams {
                provider_name: "ollama".into(),
                model_name: "preset-llama".into(),
            },
        )
        .expect_err("built-in preset must be protected");
        assert!(err.to_string().contains("built-in preset"));
        assert!(ModelRow::find(&conn, "ollama", "preset-llama").is_some());

        // Custom row: removed.
        delete_model(
            &conn,
            &DeleteModelParams {
                provider_name: "ollama".into(),
                model_name: "local-agent".into(),
            },
        )
        .expect("custom row should delete");
        assert!(ModelRow::find(&conn, "ollama", "local-agent").is_none());

        // Second delete: "not found" error, no panic, no row mutation.
        let err = delete_model(
            &conn,
            &DeleteModelParams {
                provider_name: "ollama".into(),
                model_name: "local-agent".into(),
            },
        )
        .expect_err("already-deleted row must report not found");
        assert!(err.to_string().contains("was not found"));
    }

    #[test]
    fn delete_model_rejects_empty_name_and_colons() {
        let conn = conn();
        let err = delete_model(
            &conn,
            &DeleteModelParams {
                provider_name: "ollama".into(),
                model_name: "  ".into(),
            },
        )
        .expect_err("blank model name rejected");
        assert!(err.to_string().contains("must not be empty"));

        let err = delete_model(
            &conn,
            &DeleteModelParams {
                provider_name: "ollama".into(),
                model_name: "a:b".into(),
            },
        )
        .expect_err("':' in model name rejected");
        assert!(err.to_string().contains("must not contain ':'"));
    }

    /// 两个 provider 的 preset 含同名模型（zai / bailian 都有 glm-5.3）：
    /// spec 里的 provider 必须决定解析结果，绝不许"先注册的 provider 赢"。
    #[test]
    fn resolve_model_spec_is_provider_precise_for_same_named_models() {
        let conn = conn();
        for (name, url) in [("zai", "https://api.z.ai"), ("bailian", "https://b.test")] {
            conn.execute(
                "INSERT INTO providers (name, provider_type, base_url, api_key, auth_method)
                 VALUES (?1, ?1, ?2, 'key', 'Bearer')",
                rusqlite::params![name, url],
            )
            .unwrap();
        }
        // 两家 preset 确实共享该模型名，测试才有意义。
        let shared = registry_preset_names("zai")
            .intersection(&registry_preset_names("bailian"))
            .next()
            .expect("zai and bailian share a preset model name")
            .clone();

        let model = resolve_model_spec(&conn, &format!("bailian:{shared}"))
            .expect("second provider resolves");
        assert_eq!(model.provider_name(), "bailian");
        assert_eq!(model.model_spec(), format!("bailian:{shared}"));

        let model = resolve_model_spec(&conn, &format!("zai:{shared}")).expect("first resolves");
        assert_eq!(model.provider_name(), "zai");
    }

    /// Preset model names of a registry-known provider type.
    fn registry_preset_names(provider: &str) -> std::collections::HashSet<String> {
        let provider_type = agentik_sdk::model::ProviderType::from(provider);
        agentik_sdk::provider::registry::preset_models(&provider_type)
            .unwrap_or_default()
            .into_iter()
            .map(|m| m.model_name)
            .collect()
    }

    // ── SqliteModelStorage round-trips ──────────────────────────────

    #[tokio::test]
    async fn storage_round_trips_provider_settings_and_catalogue() {
        let dir = tempfile::tempdir().unwrap();
        let storage = SqliteModelStorage::open(&dir.path().join("app.db")).unwrap();

        storage
            .save_provider(&SaveProviderParams {
                name: "Custom".into(),
                provider_type: "Custom".into(),
                base_url: "http://localhost:9".into(),
                api_key: "test-key".into(),
            })
            .await
            .unwrap();

        let auth = storage.provider_auth("Custom").await.unwrap().unwrap();
        assert_eq!(auth.api_key, "test-key");
        assert_eq!(auth.base_url, "http://localhost:9");
        assert!(storage.provider_auth("missing").await.unwrap().is_none());

        storage
            .put_setting("active_model", "Custom:db-only-model")
            .await
            .unwrap();
        assert_eq!(
            storage.active_model_spec().await.unwrap().as_deref(),
            Some("Custom:db-only-model")
        );
        assert_eq!(
            storage
                .get_setting("active_model")
                .await
                .unwrap()
                .as_deref(),
            Some("Custom:db-only-model")
        );
        assert_eq!(storage.get_setting("missing").await.unwrap(), None);

        let n = storage
            .replace_provider_models(
                "Custom",
                &[agentik_sdk::model::ModelInfo {
                    model_name: "db-only-model".into(),
                    context_length: 12345,
                    ..agentik_sdk::model::ModelInfo::default()
                }],
            )
            .await
            .unwrap();
        assert_eq!(n, 1);
        let row = storage
            .find_model("Custom", "db-only-model")
            .await
            .unwrap()
            .expect("imported row visible");
        assert_eq!(row.context_length, 12345);
        assert_eq!(storage.models().await.unwrap().len(), 1);
        assert_eq!(storage.providers().await.unwrap().len(), 1);

        storage
            .save_chatgpt_blob_json("{\"access_token\":\"t\"}")
            .await
            .unwrap();
        assert_eq!(
            storage.chatgpt_blob_json().await.unwrap().as_deref(),
            Some("{\"access_token\":\"t\"}")
        );
        storage
            .set_active_model("Custom:db-only-model")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn replace_provider_models_is_atomic_on_unknown_provider() {
        let dir = tempfile::tempdir().unwrap();
        let storage = SqliteModelStorage::open(&dir.path().join("app.db")).unwrap();
        assert!(
            storage
                .replace_provider_models("ghost", &[agentik_sdk::model::ModelInfo::default()])
                .await
                .is_err()
        );
    }
}
