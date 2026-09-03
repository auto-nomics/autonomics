//! Shared application configuration database (`config.db`): schema
//! initialization, provider/model row access, and `Model` construction from
//! the `active_model` setting.
//!
//! Extracted verbatim from `apps/tui` so the desktop shell (and any future
//! headless consumer) can resolve the model slot without depending on the
//! TUI's runtime stack. The TUI remains the only writer; behavior is
//! unchanged.

use agentik_sdk::model::{Model, ProviderConfig, ProviderType};
use agentik_sdk::provider::registry;
use rusqlite::Connection;
use uuid::Uuid;

/// Open (and initialize) the application configuration database.
///
/// Enables `foreign_keys` and creates the `providers` / `models` / `settings`
/// tables if missing — the exact sequence `App::new` used to inline.
pub fn open(path: &std::path::Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    init_database(&conn)?;
    Ok(conn)
}

/// Local application database schema initialization.
pub fn init_database(conn: &Connection) -> rusqlite::Result<()> {
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
            model_name                  TEXT    NOT NULL UNIQUE,
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

    conn.execute(
        "CREATE TABLE IF NOT EXISTS settings (
            key             TEXT PRIMARY KEY,
            value           TEXT NOT NULL
        )",
        (),
    )?;

    Ok(())
}

/// Build a `Model` from the DB settings + built-in catalog.
///
/// Reads `active_model` from the `settings` table (format:
/// `"provider_name:model_name"`), looks up the model in the SDK registry,
/// and joins it with provider credentials from the `providers` table.
/// Returns `None` if no model is configured, credentials are missing,
/// or any lookup fails — the caller treats that as "start without agent".
pub fn build_model(conn: &Connection) -> Option<Model> {
    // Read the active model setting.
    let active: String = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'active_model'",
            [],
            |row| row.get(0),
        )
        .ok()?;

    build_model_from_spec(conn, &active)
}

/// Build a `Model` from a `"provider_name:model_name"` spec, using
/// provider credentials from the DB.
///
/// Returns `None` if the spec is malformed, the provider is not
/// configured, or the model is not in the built-in catalog.
pub fn build_model_from_spec(conn: &Connection, spec: &str) -> Option<Model> {
    // Parse "provider_name:model_name"
    let (provider_name, model_name) = spec.split_once(':')?;

    // Look up provider credentials + selected base_url from the DB.
    let (api_key, db_base_url): (String, String) = conn
        .query_row(
            "SELECT api_key, base_url FROM providers WHERE name = ?1",
            [provider_name],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok()?;

    if api_key.is_empty() {
        return None;
    }

    // Look up model info from the built-in catalog.
    let provider_type = ProviderType::from(provider_name);
    let base_url = if db_base_url.is_empty() {
        registry::default_base_url(&provider_type)
            .unwrap_or("")
            .to_string()
    } else {
        db_base_url
    };
    let auth_method = registry::default_auth_method(&provider_type);
    // Look up model info from the built-in catalog, falling back to the
    // local `models` table for entries imported from a remote catalogue.
    // `unwrap_or_default` (rather than `?`) so Custom providers whose
    // models only exist in the DB also resolve.
    let preset_models = registry::preset_models(&provider_type).unwrap_or_default();
    let mut model_info = preset_models
        .into_iter()
        .find(|m| m.model_name == model_name)
        .or_else(|| {
            ModelRow::find(conn, provider_name, model_name).map(|row| row.to_model_info())
        })?;

    let provider_config = ProviderConfig {
        id: Uuid::nil(),
        name: provider_name.to_string(),
        provider_type,
        base_url,
        api_key,
        auth_method,
    };
    model_info.provider_id = provider_config.id;

    Model::new(model_info, &provider_config).ok()
}

/// A row from the `providers` table.
#[derive(Debug, Clone)]
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

/// A row from the `models` table — a model imported from a provider's remote
/// catalogue (e.g. OpenRouter's `/v1/models`). Mirrors the
/// [`ModelInfo`](agentik_sdk::model::ModelInfo) columns that survive a DB
/// round-trip.
#[derive(Debug, Clone)]
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
}

impl ModelRow {
    /// All model rows joined with their provider name, ordered by model name.
    pub fn all(conn: &Connection) -> rusqlite::Result<Vec<ModelRow>> {
        let mut stmt = conn.prepare(
            "SELECT m.model_name, p.name, m.context_length, m.max_output_tokens,
                    m.vision_ability, m.supports_function_calling, m.supports_streaming,
                    m.supports_thinking, m.thinking_enabled,
                    m.input_token_price, m.output_token_price
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
             FROM models m JOIN providers p ON p.id = m.provider_id
             WHERE p.name = ?1 AND m.model_name = ?2",
            rusqlite::params![provider_name, model_name],
            row_to_model,
        )
        .ok()
    }

    /// Convert to a metadata-only [`ModelInfo`] — `provider_id` is left nil;
    /// the caller binds it when constructing the provider config.
    pub fn to_model_info(&self) -> agentik_sdk::model::ModelInfo {
        agentik_sdk::model::ModelInfo {
            model_name: self.model_name.clone(),
            provider_id: uuid::Uuid::nil(),
            context_length: self.context_length,
            max_output_tokens: self.max_output_tokens,
            vision_ability: self.vision_ability,
            supports_function_calling: self.supports_function_calling,
            supports_streaming: self.supports_streaming,
            supports_thinking: self.supports_thinking,
            thinking_enabled: self.thinking_enabled,
            input_token_price: self.input_token_price,
            output_token_price: self.output_token_price,
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        open(std::path::Path::new(":memory:")).expect("in-memory db with schema")
    }

    #[test]
    fn empty_db_yields_no_model() {
        let conn = conn();
        assert!(build_model(&conn).is_none());
        assert!(build_model_from_spec(&conn, "Anthropic:claude-x").is_none());
    }

    #[test]
    fn provider_and_active_model_build_a_model() {
        let conn = conn();

        conn.execute(
            "INSERT INTO providers (name, provider_type, base_url, api_key, auth_method)
             VALUES ('deepseek', 'deepseek', '', 'sk-test', 'deepseek')",
            (),
        )
        .unwrap();

        // Resolve via the local `models` table (the remote-catalogue path):
        // a model name that exists in no preset catalog.
        conn.execute(
            "INSERT INTO models (model_name, provider_id)
             VALUES ('deepseek-test-model',
                     (SELECT id FROM providers WHERE name = 'deepseek'))",
            (),
        )
        .unwrap();
        conn.execute(
            "INSERT INTO settings (key, value)
             VALUES ('active_model', 'deepseek:deepseek-test-model')",
            (),
        )
        .unwrap();

        let model = build_model(&conn)
            .expect("model should build from provider + active_model");
        assert_eq!(model.model_info.model_name, "deepseek-test-model");

        // Empty api_key must refuse to build.
        conn.execute("UPDATE providers SET api_key = ''", ()).unwrap();
        assert!(build_model(&conn).is_none());
    }

    #[test]
    fn preset_catalog_model_resolves() {
        let conn = conn();

        // Pick a real preset model name from the registry so this follows
        // the built-in-catalog branch (not the DB fallback).
        let preset_name = registry::preset_models(&ProviderType::from("deepseek"))
            .expect("deepseek presets")
            .into_iter()
            .map(|m| m.model_name)
            .next();
        let Some(preset_name) = preset_name else {
            // Catalog empty — nothing to assert on this branch.
            return;
        };

        conn.execute(
            "INSERT INTO providers (name, provider_type, base_url, api_key, auth_method)
             VALUES ('deepseek', 'deepseek', '', 'sk-test', 'deepseek')",
            (),
        )
        .unwrap();
        conn.execute(
            &format!(
                "INSERT INTO settings (key, value)
                 VALUES ('active_model', 'deepseek:{preset_name}')"
            ),
            (),
        )
        .unwrap();

        let model =
            build_model(&conn).expect("preset model should build via the registry catalog");
        assert_eq!(model.model_info.model_name, preset_name);
    }
}
