//! Project-scoped visualization persistence, independent of GUI and rendering engine.
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Database(#[from] rusqlite::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("{0} must not be blank")]
    Invalid(&'static str),
    #[error("project or visualization not found")]
    NotFound,
    #[error("document changed; reload before saving")]
    Conflict,
}
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Project {
    pub id: String,
    pub title: String,
    pub created_at: String,
}
/// Metadata only: listing documents does not load scenes or embedded images.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Visualization {
    pub id: String,
    pub project_id: String,
    pub title: String,
    /// Versioned format identifier, for example `excalidraw/v2`.
    pub format: String,
    pub revision: i64,
    pub created_at: String,
    pub updated_at: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Revision {
    pub visualization_id: String,
    pub revision: i64,
    pub title: String,
    pub document: Value,
    pub created_at: String,
}

pub struct Store {
    connection: Connection,
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
fn metadata(row: &rusqlite::Row<'_>) -> rusqlite::Result<Visualization> {
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
              PRIMARY KEY(visualization_id, revision));")?;
        Ok(Self { connection })
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn project_isolation_history_and_conflicts() -> Result<()> {
        let mut store = Store::in_memory()?;
        store.create_project("a", "Study A")?;
        store.create_project("b", "Study B")?;
        let first = store.create(
            "a",
            "Figure 1",
            "excalidraw/v2",
            &json!({"elements":[],"files":{}}),
        )?;
        assert!(store.list("b")?.is_empty());
        assert!(matches!(store.get("b", &first.id), Err(Error::NotFound)));
        assert!(matches!(
            store.revision("b", &first.id, 1),
            Err(Error::NotFound)
        ));
        let second = store.save(
            "a",
            &first.id,
            1,
            "Figure 2",
            &json!({"elements":[{"id":"cell"}]}),
        )?;
        assert_eq!(second.revision, 2);
        assert!(matches!(
            store.save("a", &first.id, 1, "Stale", &Value::Null),
            Err(Error::Conflict)
        ));
        assert_eq!(store.revision("a", &first.id, 1)?.title, "Figure 1");
        assert_eq!(
            store.revision("a", &first.id, 2)?.document["elements"][0]["id"],
            "cell"
        );
        assert!(matches!(
            store.delete("a", &first.id, 1),
            Err(Error::Conflict)
        ));
        store.delete("a", &first.id, 2)?;
        assert!(matches!(
            store.revision("a", &first.id, 1),
            Err(Error::NotFound)
        ));
        assert!(matches!(
            store.create("missing", "x", "svg/v1", &Value::Null),
            Err(Error::NotFound)
        ));
        assert!(matches!(
            store.create_project(" ", "x"),
            Err(Error::Invalid(_))
        ));
        Ok(())
    }
    #[test]
    fn reopens_database() -> Result<()> {
        let path =
            std::env::temp_dir().join(format!("visualization-{}.sqlite", uuid::Uuid::new_v4()));
        let id;
        {
            let mut store = Store::open(&path)?;
            store.create_project("p", "Project")?;
            id = store
                .create(
                    "p",
                    "图一",
                    "excalidraw/v2",
                    &json!({"files":{"asset":"data:image/png;base64,abc"}}),
                )?
                .id;
        }
        {
            let store = Store::open(&path)?;
            assert_eq!(store.list("p")?.len(), 1);
            assert_eq!(
                store.revision("p", &id, 1)?.document["files"]["asset"],
                "data:image/png;base64,abc"
            );
        }
        std::fs::remove_file(path).expect("remove test database");
        Ok(())
    }
}
