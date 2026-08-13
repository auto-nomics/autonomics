//! `node_kinds` cache — mirrors the [`crate::registry::NodeRegistry`] into
//! SQLite so the TUI can list / search node kinds without a registry.
//!
//! The registry itself lives in memory and is the source of truth at runtime;
//! this table is purely a durable cache populated on app start.

use crate::error::Result;
use crate::model::{NodeKindInfo, PortSpec};
use crate::store::pool::DbPool;
use chrono::Utc;
use rusqlite::{OptionalExtension, params};

#[derive(Clone)]
#[allow(missing_docs)]
pub struct NodeKindRepo {
    pool: DbPool,
}

impl NodeKindRepo {
    /// Wrap a pool.
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// Upsert one node kind.
    pub fn upsert(
        &self,
        kind: &str,
        label: &str,
        description: &str,
        spec_schema: &serde_json::Value,
        ports: &[PortSpec],
        doc: &str,
        category: &str,
    ) -> Result<()> {
        let ports_json = serde_json::to_string(ports)?;
        let now = Utc::now().to_rfc3339();
        let conn = self.pool.lock();
        conn.execute(
            "INSERT INTO node_kinds
                (kind, label, description, spec_schema, ports_json, doc, category, registered_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(kind) DO UPDATE SET
                label = excluded.label,
                description = excluded.description,
                spec_schema = excluded.spec_schema,
                ports_json = excluded.ports_json,
                doc = excluded.doc,
                category = excluded.category,
                registered_at = excluded.registered_at",
            params![
                kind,
                label,
                description,
                spec_schema.to_string(),
                ports_json,
                doc,
                category,
                now
            ],
        )?;
        Ok(())
    }

    /// List all known node kinds.
    pub fn list(&self) -> Result<Vec<NodeKindInfo>> {
        let conn = self.pool.lock();
        let mut stmt = conn.prepare(
            "SELECT kind, label, description, category FROM node_kinds ORDER BY category, label",
        )?;
        let rows = stmt.query_map([], |row| {
            let kind: String = row.get(0)?;
            let label: String = row.get(1)?;
            let description: String = row.get(2)?;
            let category: String = row.get(3)?;
            Ok((kind, label, description, category))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (kind, label, description, category) = r?;
            out.push(NodeKindInfo {
                kind,
                label,
                description,
                category,
                in_use: 0,
            });
        }
        Ok(out)
    }

    /// Fetch the spec schema JSON for one kind.
    pub fn spec_schema(&self, kind: &str) -> Result<Option<serde_json::Value>> {
        let conn = self.pool.lock();
        let row: Option<String> = conn
            .query_row(
                "SELECT spec_schema FROM node_kinds WHERE kind = ?1",
                params![kind],
                |row| row.get(0),
            )
            .optional()?;
        Ok(row.and_then(|s| serde_json::from_str(&s).ok()))
    }
}
