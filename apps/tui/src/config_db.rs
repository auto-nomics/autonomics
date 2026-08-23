//! CRUD access for the `providers` table.
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
