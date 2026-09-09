//! CRUD access for the `providers` and `models` tables.
//!
//! The schema is created in `app::database::init_database`. Both tables use an
//! auto-incrementing integer primary key (`id`).

use rusqlite::Connection;

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

/// Flattened, stringly-typed provider values used by the edit form.
pub struct ProviderInput {
    pub name: String,
    pub provider_type: String,
    pub base_url: String,
    pub api_key: String,
    pub auth_method: String,
}

impl ProviderInput {
    pub fn empty() -> Self {
        Self {
            name: String::new(),
            provider_type: "Anthropic".to_string(),
            base_url: String::new(),
            api_key: String::new(),
            auth_method: "Anthropic".to_string(),
        }
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
