//! SQLite-backed project visualization store.
use crate::error::{Error, Result};
use crate::{Project, Revision, Visualization};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use std::path::Path;

pub struct Store {
    pub(crate) connection: Connection,
    pub(crate) linear_candidate_revisions: bool,
}
fn nonblank(value: &str, field: &'static str) -> Result<()> {
    if value.trim().is_empty() {
        Err(Error::Invalid(field))
    } else {
        Ok(())
    }
}
fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}
pub(crate) fn metadata(row: &rusqlite::Row<'_>) -> rusqlite::Result<Visualization> {
    Ok(Visualization {
        id: row.get(0)?,
        project_id: row.get(1)?,
        title: row.get(2)?,
        format: row.get(3)?,
        revision: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}
impl Store {
    /// Require pending leaf parents for revisions. Explicit alternative workflows
    /// can leave this disabled; validation happens inside the write transaction.
    pub fn with_linear_candidate_revisions(mut self, enabled: bool) -> Self {
        self.linear_candidate_revisions = enabled;
        self
    }
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::initialize(Connection::open(path)?)
    }
    pub fn in_memory() -> Result<Self> {
        Self::initialize(Connection::open_in_memory()?)
    }
    fn initialize(connection: Connection) -> Result<Self> {
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.execute_batch("PRAGMA foreign_keys=ON;
            CREATE TABLE IF NOT EXISTS visualization_projects (
              id TEXT PRIMARY KEY, title TEXT NOT NULL, created_at TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS visualizations (
              id TEXT PRIMARY KEY, project_id TEXT NOT NULL REFERENCES visualization_projects(id) ON DELETE CASCADE,
              title TEXT NOT NULL, format TEXT NOT NULL, revision INTEGER NOT NULL,
              created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS visualizations_project ON visualizations(project_id, updated_at);
            CREATE TABLE IF NOT EXISTS visualization_revisions (
              visualization_id TEXT NOT NULL REFERENCES visualizations(id) ON DELETE CASCADE,
              revision INTEGER NOT NULL, title TEXT NOT NULL, document TEXT NOT NULL, created_at TEXT NOT NULL,
              PRIMARY KEY(visualization_id, revision));
            CREATE TABLE IF NOT EXISTS visualization_proposals (
              id TEXT PRIMARY KEY, visualization_id TEXT NOT NULL REFERENCES visualizations(id) ON DELETE CASCADE,
              base_revision INTEGER NOT NULL, summary TEXT NOT NULL, document TEXT NOT NULL,
              status TEXT NOT NULL, created_at TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS visualization_proposal_lineage (
              proposal_id TEXT PRIMARY KEY REFERENCES visualization_proposals(id) ON DELETE CASCADE,
              parent_id TEXT NOT NULL REFERENCES visualization_proposals(id) ON DELETE CASCADE,
              parent_document_sha256 TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS visualization_proposals_figure ON visualization_proposals(visualization_id,created_at,id);
            CREATE TABLE IF NOT EXISTS visualization_proposal_previews (
              proposal_id TEXT PRIMARY KEY REFERENCES visualization_proposals(id) ON DELETE CASCADE,
              document_sha256 TEXT NOT NULL, render_status TEXT NOT NULL,
              data_url TEXT, error TEXT, renderer TEXT, requested_at TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS visualization_proposal_preview_queue ON visualization_proposal_previews(render_status,requested_at,proposal_id);
            CREATE TABLE IF NOT EXISTS visualization_annotations (
              id TEXT PRIMARY KEY, visualization_id TEXT NOT NULL REFERENCES visualizations(id) ON DELETE CASCADE,
              at_revision INTEGER NOT NULL, element_ids TEXT NOT NULL, note TEXT NOT NULL,
              status TEXT NOT NULL, created_at TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS visualization_markups (
              id TEXT PRIMARY KEY, visualization_id TEXT NOT NULL, at_revision INTEGER NOT NULL,
              note TEXT NOT NULL, strokes TEXT NOT NULL, preview TEXT NOT NULL,
              status TEXT NOT NULL, created_at TEXT NOT NULL,
              FOREIGN KEY(visualization_id, at_revision) REFERENCES visualization_revisions(visualization_id, revision) ON DELETE CASCADE);
            CREATE INDEX IF NOT EXISTS visualization_markups_figure ON visualization_markups(visualization_id, created_at);
            CREATE TABLE IF NOT EXISTS visualization_proposal_markups (
              proposal_id TEXT NOT NULL REFERENCES visualization_proposals(id) ON DELETE CASCADE,
              markup_id TEXT NOT NULL REFERENCES visualization_markups(id) ON DELETE CASCADE,
              PRIMARY KEY(proposal_id,markup_id));
            CREATE TABLE IF NOT EXISTS visualization_previews (
              visualization_id TEXT NOT NULL, revision INTEGER NOT NULL, data_url TEXT NOT NULL,
              PRIMARY KEY(visualization_id, revision),
              FOREIGN KEY(visualization_id, revision) REFERENCES visualization_revisions(visualization_id, revision) ON DELETE CASCADE);")?;
        Ok(Self {
            connection,
            linear_candidate_revisions: false,
        })
    }
    /// Caller supplies a stable project ID, allowing association with the host project system.
    pub fn create_project(&self, id: &str, title: &str) -> Result<Project> {
        nonblank(id, "project id")?;
        nonblank(title, "title")?;
        let project = Project {
            id: id.into(),
            title: title.trim().into(),
            created_at: now(),
        };
        self.connection.execute(
            "INSERT INTO visualization_projects VALUES (?1,?2,?3)",
            params![project.id, project.title, project.created_at],
        )?;
        Ok(project)
    }
    pub fn projects(&self) -> Result<Vec<Project>> {
        let mut query = self.connection.prepare(
            "SELECT id,title,created_at FROM visualization_projects ORDER BY created_at,id",
        )?;
        Ok(query
            .query_map([], |r| {
                Ok(Project {
                    id: r.get(0)?,
                    title: r.get(1)?,
                    created_at: r.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }
    pub fn create(
        &mut self,
        project_id: &str,
        title: &str,
        format: &str,
        document: &Value,
    ) -> Result<Visualization> {
        nonblank(title, "title")?;
        nonblank(format, "format")?;
        let transaction = self.connection.transaction()?;
        if !transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM visualization_projects WHERE id=?1)",
            [project_id],
            |r| r.get::<_, bool>(0),
        )? {
            return Err(Error::NotFound);
        }
        let record = Visualization {
            id: uuid::Uuid::new_v4().to_string(),
            project_id: project_id.into(),
            title: title.trim().into(),
            format: format.into(),
            revision: 1,
            created_at: now(),
            updated_at: now(),
        };
        transaction.execute(
            "INSERT INTO visualizations VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![
                record.id,
                record.project_id,
                record.title,
                record.format,
                record.revision,
                record.created_at,
                record.updated_at
            ],
        )?;
        transaction.execute(
            "INSERT INTO visualization_revisions VALUES (?1,1,?2,?3,?4)",
            params![
                record.id,
                record.title,
                serde_json::to_string(document)?,
                record.updated_at
            ],
        )?;
        transaction.commit()?;
        Ok(record)
    }
    pub fn list(&self, project_id: &str) -> Result<Vec<Visualization>> {
        let mut query = self.connection.prepare("SELECT id,project_id,title,format,revision,created_at,updated_at FROM visualizations WHERE project_id=?1 ORDER BY updated_at DESC,id")?;
        Ok(query
            .query_map([project_id], metadata)?
            .collect::<rusqlite::Result<_>>()?)
    }
    pub fn get(&self, project_id: &str, id: &str) -> Result<Visualization> {
        self.connection.query_row("SELECT id,project_id,title,format,revision,created_at,updated_at FROM visualizations WHERE project_id=?1 AND id=?2",params![project_id,id],metadata).optional()?.ok_or(Error::NotFound)
    }
    /// Saves a new immutable revision. Stale writers fail without modifying history.
    pub fn save(
        &mut self,
        project_id: &str,
        id: &str,
        expected_revision: i64,
        title: &str,
        document: &Value,
    ) -> Result<Visualization> {
        nonblank(title, "title")?;
        let mut record = self.get(project_id, id)?;
        let transaction = self.connection.transaction()?;
        let timestamp = now();
        let changed = transaction.execute("UPDATE visualizations SET title=?1,revision=revision+1,updated_at=?2 WHERE project_id=?3 AND id=?4 AND revision=?5",params![title.trim(),timestamp,project_id,id,expected_revision])?;
        if changed == 0 {
            return Err(Error::Conflict);
        }
        record.title = title.trim().into();
        record.revision = expected_revision + 1;
        record.updated_at = timestamp;
        transaction.execute(
            "INSERT INTO visualization_revisions VALUES (?1,?2,?3,?4,?5)",
            params![
                id,
                record.revision,
                record.title,
                serde_json::to_string(document)?,
                record.updated_at
            ],
        )?;
        transaction.commit()?;
        Ok(record)
    }
    /// Page revision metadata without loading embedded scenes or images.
    pub fn revisions(
        &self,
        project_id: &str,
        id: &str,
        before: Option<i64>,
        limit: u32,
    ) -> Result<Vec<crate::RevisionSummary>> {
        self.get(project_id, id)?;
        if before.is_some_and(|revision| revision <= 0) {
            return Err(Error::Invalid("before revision"));
        }
        let mut query = self.connection.prepare("SELECT revision,title,created_at FROM visualization_revisions WHERE visualization_id=?1 AND (?2 IS NULL OR revision < ?2) ORDER BY revision DESC LIMIT ?3")?;
        Ok(query
            .query_map(params![id, before, limit.clamp(1, 100)], |row| {
                Ok(crate::RevisionSummary {
                    revision: row.get(0)?,
                    title: row.get(1)?,
                    created_at: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }
    pub fn revision(&self, project_id: &str, id: &str, revision: i64) -> Result<Revision> {
        let row: Option<(String,String,String)> = self.connection.query_row("SELECT r.title,r.document,r.created_at FROM visualization_revisions r JOIN visualizations v ON v.id=r.visualization_id WHERE v.project_id=?1 AND v.id=?2 AND r.revision=?3",params![project_id,id,revision],|r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        let (title, document, created_at) = row.ok_or(Error::NotFound)?;
        Ok(Revision {
            visualization_id: id.into(),
            revision,
            title,
            document: serde_json::from_str(&document)?,
            created_at,
        })
    }
    pub fn delete(&mut self, project_id: &str, id: &str, expected_revision: i64) -> Result<()> {
        self.get(project_id, id)?;
        if self.connection.execute(
            "DELETE FROM visualizations WHERE project_id=?1 AND id=?2 AND revision=?3",
            params![project_id, id, expected_revision],
        )? == 0
        {
            return Err(Error::Conflict);
        }
        Ok(())
    }
}
