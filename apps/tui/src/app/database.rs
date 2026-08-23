//! Local application database schema initialization.

use super::*;

impl App {
    pub(super) fn init_database(conn: &Connection) -> rusqlite::Result<()> {
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
}
