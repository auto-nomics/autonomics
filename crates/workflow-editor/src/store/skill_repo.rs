//! Skill persistence — versioned DAG subgraphs.
//!
//! Saving the same `name` twice produces two `skill_versions` rows; the
//! `skills` row's `current_version` always points at the latest.

use crate::error::{Result, StorageError};
use crate::model::{PortSpec, Skill, SkillInfo};
use crate::store::pool::DbPool;
use chrono::{DateTime, Utc};
use rusqlite::{params, OptionalExtension};
use uuid::Uuid;

/// Skill repository.
#[derive(Clone)]
pub struct SkillRepo {
    pool: DbPool,
}

impl SkillRepo {
    /// Wrap an existing pool.
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// Borrow the underlying pool.
    pub fn pool(&self) -> DbPool {
        self.pool.clone()
    }

    /// Save a new version of a skill. Creates the `skills` row on first save
    /// and bumps `current_version`. Returns the saved `Skill` (with id + version).
    pub fn save(&self, skill: &Skill) -> Result<Skill> {
        skill.validate()?;
        let tool_refs_json = serde_json::to_string(&skill.tool_refs)?;
        let manifest_json = serde_json::to_string(&skill.manifest)?;
        let inputs_json = serde_json::to_string(&skill.surface_inputs)?;
        let outputs_json = serde_json::to_string(&skill.surface_outputs)?;
        let now = Utc::now().to_rfc3339();

        let mut conn = self.pool.lock();
        let tx = conn.transaction()?;

        let existing: Option<(String, i64)> = tx
            .query_row(
                "SELECT id, current_version FROM skills WHERE name = ?1",
                params![skill.name],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()?;

        let (skill_id, version) = match existing {
            Some((id, version)) => {
                let version = (version + 1) as u32;
                tx.execute(
                    "UPDATE skills SET current_version = ?1, updated_at = ?2 WHERE id = ?3",
                    params![version as i64, now, id],
                )?;
                (
                    Uuid::parse_str(&id).map_err(|e| StorageError::Decode(e.to_string()))?,
                    version,
                )
            }
            None => {
                let id = Uuid::new_v4();
                tx.execute(
                    "INSERT INTO skills (id, name, current_version, created_at, updated_at)
                     VALUES (?1, ?2, 1, ?3, ?3)",
                    params![id.to_string(), skill.name, now],
                )?;
                (id, 1)
            }
        };

        tx.execute(
            "INSERT INTO skill_versions
                (id, skill_id, version, parent_id, description, sop_text,
                 tool_refs_json, manifest_json, surface_inputs_json, surface_outputs_json, created_at)
             VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                Uuid::new_v4().to_string(),
                skill_id.to_string(),
                version as i64,
                skill.description,
                skill.sop_text,
                tool_refs_json,
                manifest_json,
                inputs_json,
                outputs_json,
                now,
            ],
        )?;
        tx.commit()?;

        Ok(Skill {
            id: skill_id,
            version,
            ..skill.clone()
        })
    }

    /// Fetch the latest version of a skill by id.
    pub fn get(&self, id: Uuid) -> Result<Skill> {
        match self.latest_by_id(id)? {
            Some(s) => Ok(s),
            None => Err(StorageError::NotFound {
                kind: "skill",
                id: id.to_string(),
            }
            .into()),
        }
    }

    /// Fetch the latest version of a skill by id, returning `None` if missing.
    pub fn latest_by_id(&self, id: Uuid) -> Result<Option<Skill>> {
        let conn = self.pool.lock();
        let row = conn
            .query_row(
                "SELECT sv.description, sv.sop_text, sv.tool_refs_json,
                        sv.manifest_json, sv.surface_inputs_json, sv.surface_outputs_json,
                        s.name, sv.version
                 FROM skill_versions sv
                 JOIN skills s ON s.id = sv.skill_id
                 WHERE s.id = ?1
                 ORDER BY sv.version DESC LIMIT 1",
                params![id.to_string()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, i64>(7)?,
                    ))
                },
            )
            .optional()?;
        match row {
            Some(r) => Ok(Some(row_to_skill(id, &r)?)),
            None => Ok(None),
        }
    }

    /// Fetch a specific version of a skill.
    pub fn get_version(&self, id: Uuid, version: u32) -> Result<Skill> {
        let conn = self.pool.lock();
        let row = conn
            .query_row(
                "SELECT sv.description, sv.sop_text, sv.tool_refs_json,
                        sv.manifest_json, sv.surface_inputs_json, sv.surface_outputs_json,
                        s.name, sv.version
                 FROM skill_versions sv
                 JOIN skills s ON s.id = sv.skill_id
                 WHERE s.id = ?1 AND sv.version = ?2",
                params![id.to_string(), version as i64],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, i64>(7)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| StorageError::NotFound {
                kind: "skill_version",
                id: format!("{}@v{}", id, version),
            })?;
        row_to_skill(id, &row)
    }

    /// List all skills, latest version of each.
    pub fn list(&self) -> Result<Vec<SkillInfo>> {
        let conn = self.pool.lock();
        let mut stmt = conn.prepare(
            "SELECT s.id, s.name, s.current_version, sv.description,
                    json_array_length(sv.manifest_json, '$.nodes') AS node_count,
                    s.updated_at
             FROM skills s
             JOIN skill_versions sv
               ON sv.skill_id = s.id AND sv.version = s.current_version
             ORDER BY s.updated_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            let name: String = row.get(1)?;
            let version: i64 = row.get(2)?;
            let description: String = row.get(3)?;
            let node_count: i64 = row.get(4)?;
            let updated: String = row.get(5)?;
            Ok((id, name, version, description, node_count, updated))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (id, name, version, description, node_count, updated) = r?;
            let updated = DateTime::parse_from_rfc3339(&updated)
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());
            out.push(SkillInfo {
                id: Uuid::parse_str(&id).map_err(|e| StorageError::Decode(e.to_string()))?,
                name,
                version: version as u32,
                description,
                node_count: node_count.max(0) as u32,
                updated_at: updated,
            });
        }
        Ok(out)
    }
}

/// Convenience: turn a raw sqlite row into a `Skill`.
fn row_to_skill(
    id: Uuid,
    row: &(String, String, String, String, String, String, String, i64),
) -> Result<Skill> {
    let (description, sop_text, tool_refs_json, manifest_json, inputs_json, outputs_json, name, version) = row;

    let tool_refs: Vec<String> = serde_json::from_str(tool_refs_json)?;
    let manifest: crate::model::WorkflowManifest = serde_json::from_str(manifest_json)?;
    let surface_inputs: Vec<PortSpec> = serde_json::from_str(inputs_json)?;
    let surface_outputs: Vec<PortSpec> = serde_json::from_str(outputs_json)?;

    Ok(Skill {
        id,
        name: name.clone(),
        version: *version as u32,
        description: description.clone(),
        sop_text: sop_text.clone(),
        tool_refs,
        manifest,
        surface_inputs,
        surface_outputs,
    })
}