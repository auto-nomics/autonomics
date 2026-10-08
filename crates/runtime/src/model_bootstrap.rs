//! Model bootstrap — resolve the active [`Model`] from the app database.
//!
//! Owns the `providers` / `models` / `settings` tables end to end (schema
//! DDL, row mappings, resolution logic) so every entry point — the TUI,
//! the headless runner, future adapters — shares one source of truth for
//! "which model does this installation run". The TUI's model-config
//! widgets keep only UI glue (catalog rendering, persistence prompts) and
//! delegate resolution here.

use agentik_sdk::model::{Model, ProviderConfig, ProviderType};
use rusqlite::Connection;
use uuid::Uuid;

// ─────────────────────────────────────────────────────────────────────
// Schema
// ─────────────────────────────────────────────────────────────────────

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

/// A row from the `models` table — a model imported from a provider's remote
/// catalogue (e.g. OpenRouter's `/v1/models`). Mirrors the
/// [`ModelInfo`](agentik_sdk::model::ModelInfo) columns that survive a DB
/// round-trip.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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

/// The `active_model` setting's raw `"provider_name:model_name"` spec,
/// when one is configured.
pub fn active_model_spec(conn: &Connection) -> Option<String> {
    conn.query_row(
        "SELECT value FROM settings WHERE key = 'active_model'",
        [],
        |row| row.get(0),
    )
    .ok()
}

/// Build a [`Model`] from a `"provider_name:model_name"` spec, using
/// provider credentials from the `providers` table.
///
/// Model metadata comes from the SDK's built-in catalogue, falling back
/// to the `models` table for entries imported from a remote catalogue —
/// so custom providers whose models only exist in the DB also resolve.
/// Returns `None` if the spec is malformed, the provider is not
/// configured (or has an empty API key), or the model is unknown.
pub fn resolve_model_spec(conn: &Connection, spec: &str) -> Option<Model> {
    use agentik_sdk::provider::registry;

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

    // Look up model info from the built-in catalog, falling back to the
    // local `models` table for entries imported from a remote catalogue.
    // `unwrap_or_default` (rather than `?`) so Custom providers whose
    // models only exist in the DB also resolve.
    let provider_type = ProviderType::from(provider_name);
    let base_url = if db_base_url.is_empty() {
        registry::default_base_url(&provider_type)
            .unwrap_or("")
            .to_string()
    } else {
        db_base_url
    };
    let auth_method = registry::default_auth_method(&provider_type);
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
}
