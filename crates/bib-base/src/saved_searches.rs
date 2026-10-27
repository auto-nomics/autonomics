//! Versioned dynamic search rules, independent of static collections/results.
use crate::{BibBase, Error, Result, actor::BibCommand, bib_base::BibCore};
use bib_types::{SaveSearchInput, SavedSearch};
use turso::Connection;

pub(crate) async fn ensure(conn: &Connection) -> Result<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS savedSearches(\
        id TEXT PRIMARY KEY,name TEXT NOT NULL,schema_version INTEGER NOT NULL,\
        version INTEGER NOT NULL,request_json TEXT NOT NULL,created_at TEXT NOT NULL,updated_at TEXT NOT NULL)").await?;
    Ok(())
}
fn validate_version(version: u64) -> Result<()> {
    if version == 0 || version > i64::MAX as u64 {
        return Err(Error::InvalidSearchQuery(
            "expected_version must be a positive signed 64-bit integer".into(),
        ));
    }
    Ok(())
}
fn decode(row: &turso::Row) -> Result<SavedSearch> {
    let schema_version = row.get::<i64>(2)?;
    if schema_version != 1 {
        return Err(Error::InvalidSearchQuery(format!(
            "unsupported saved search schema version {schema_version}"
        )));
    }
    Ok(SavedSearch {
        id: row.get(0)?,
        name: row.get(1)?,
        schema_version: 1,
        version: row.get::<i64>(3)? as u64,
        request: serde_json::from_str(&row.get::<String>(4)?)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}
const SELECT: &str =
    "SELECT id,name,schema_version,version,request_json,created_at,updated_at FROM savedSearches";
impl BibBase {
    pub async fn list_saved_searches(&self) -> Result<Vec<SavedSearch>> {
        self.call(|reply| BibCommand::ListSavedSearches { reply })
            .await
    }
    pub async fn save_search(
        &self,
        id: Option<&str>,
        input: SaveSearchInput,
    ) -> Result<SavedSearch> {
        self.call(|reply| BibCommand::SaveSearch {
            id: id.map(str::to_owned),
            input,
            reply,
        })
        .await
    }
    pub async fn delete_saved_search(&self, id: &str, expected_version: u64) -> Result<()> {
        self.call(|reply| BibCommand::DeleteSavedSearch {
            id: id.into(),
            expected_version,
            reply,
        })
        .await
    }
}
impl BibCore {
    pub(crate) async fn list_saved_searches(&self) -> Result<Vec<SavedSearch>> {
        let conn = self.conn();
        let mut rows = conn.query(format!("{SELECT} ORDER BY name,id"), ()).await?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await? {
            out.push(decode(&row)?);
        }
        Ok(out)
    }
    pub(crate) async fn save_search(
        &self,
        id: Option<&str>,
        mut input: SaveSearchInput,
    ) -> Result<SavedSearch> {
        input.name = input.name.trim().into();
        if input.name.is_empty()
            || input.name.chars().count() > 120
            || input.name.chars().any(char::is_control)
        {
            return Err(Error::InvalidSearchQuery(
                "saved search name must contain 1–120 printable characters".into(),
            ));
        }
        // Validate the executable AST too, while retaining the original query text.
        crate::search::validate_request(input.request.clone())?;
        input.request.cursor = None;
        let request_json = serde_json::to_string(&input.request)?;
        let now = chrono::Utc::now().to_rfc3339();
        let conn = self.conn();
        let id = if let Some(id) = id {
            let expected = input.expected_version.ok_or_else(|| {
                Error::InvalidSearchQuery("expected_version is required for updates".into())
            })?;
            validate_version(expected)?;
            let changed=conn.execute("UPDATE savedSearches SET name=?1,request_json=?2,version=version+1,updated_at=?3 WHERE id=?4 AND version=?5",turso::params![input.name,request_json,now,id,expected as i64]).await?;
            if changed == 0 {
                return Err(self.saved_search_conflict(id).await?);
            }
            id.to_owned()
        } else {
            if input.expected_version.is_some() {
                return Err(Error::InvalidSearchQuery(
                    "new saved searches must not specify expected_version".into(),
                ));
            }
            let id = uuid::Uuid::new_v4().to_string();
            conn.execute("INSERT INTO savedSearches(id,name,schema_version,version,request_json,created_at,updated_at) VALUES(?1,?2,1,1,?3,?4,?4)",turso::params![id.as_str(),input.name,request_json,now]).await?;
            id
        };
        let mut rows = conn
            .query(format!("{SELECT} WHERE id=?1"), turso::params![id.as_str()])
            .await?;
        decode(
            &rows
                .next()
                .await?
                .ok_or_else(|| Error::NotFound(format!("saved search {id}")))?,
        )
    }
    async fn saved_search_conflict(&self, id: &str) -> Result<Error> {
        let conn = self.conn();
        let mut rows = conn
            .query(
                "SELECT 1 FROM savedSearches WHERE id=?1",
                turso::params![id],
            )
            .await?;
        Ok(if rows.next().await?.is_some() {
            Error::SearchConflict("rule changed in another window; reload before editing".into())
        } else {
            Error::NotFound(format!("saved search {id}"))
        })
    }
    pub(crate) async fn delete_saved_search(&self, id: &str, expected_version: u64) -> Result<()> {
        validate_version(expected_version)?;
        let changed = self
            .conn()
            .execute(
                "DELETE FROM savedSearches WHERE id=?1 AND version=?2",
                turso::params![id, expected_version as i64],
            )
            .await?;
        if changed == 0 {
            return Err(self.saved_search_conflict(id).await?);
        }
        Ok(())
    }
}
