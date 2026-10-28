//! Resource policy is separate from embedding identity. Disk accounting is a
//! cooperative budget, not a filesystem hard quota: it is checked at batch
//! boundaries, while vector payload reservations are checked in the write TX.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use turso::{
    Connection,
    transaction::{Transaction, TransactionBehavior},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SemanticLimits {
    pub max_chunks: usize,
    pub max_input_bytes: usize,
    /// Across all profiles in one physical vector store, including cached batches.
    pub max_vector_bytes: u64,
    /// All managed semantic store files, WAL and shared files for this bibliography.
    pub max_disk_bytes: u64,
}
impl Default for SemanticLimits {
    fn default() -> Self {
        Self {
            max_chunks: 1_000_000,
            max_input_bytes: 262_144,
            max_vector_bytes: 2 * 1024 * 1024 * 1024,
            max_disk_bytes: 8 * 1024 * 1024 * 1024,
        }
    }
}
impl SemanticLimits {
    pub fn validate(&self) -> Result<()> {
        if !(1..=10_000_000).contains(&self.max_chunks)
            || !(1..=16_777_216).contains(&self.max_input_bytes)
            || !(1..=1_099_511_627_776).contains(&self.max_vector_bytes)
            || !(1..=1_099_511_627_776).contains(&self.max_disk_bytes)
        {
            return Err(Error::InvalidSearchQuery(
                "invalid semantic resource limits".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticResourceUsage {
    pub vector_bytes: u64,
    pub disk_bytes: Option<u64>,
}

pub(crate) fn check(resource: &str, used: u64, limit: u64) -> Result<()> {
    if used > limit {
        Err(Error::SemanticResourceLimit {
            resource: resource.into(),
            used,
            maximum: limit,
        })
    } else {
        Ok(())
    }
}

// Triggers keep accounting correct for concurrent connections, ignored INSERTs,
// rollback, garbage collection and externally repaired vector rows. Initialization
// and trigger installation share one writer transaction for legacy stores.
const TRIGGERS: &str = r#"
CREATE TRIGGER IF NOT EXISTS semanticVectorUsageInsert AFTER INSERT ON semanticVectors BEGIN
 UPDATE semanticCacheUsage SET vectorBytes=vectorBytes+length(NEW.vector) WHERE id=1;
END;
CREATE TRIGGER IF NOT EXISTS semanticVectorUsageDelete AFTER DELETE ON semanticVectors BEGIN
 UPDATE semanticCacheUsage SET vectorBytes=vectorBytes-length(OLD.vector) WHERE id=1;
END;
CREATE TRIGGER IF NOT EXISTS semanticVectorUsageUpdate AFTER UPDATE OF vector ON semanticVectors BEGIN
 UPDATE semanticCacheUsage SET vectorBytes=vectorBytes-length(OLD.vector)+length(NEW.vector) WHERE id=1;
END;
"#;
pub(crate) async fn initialize_store(store: &Connection) -> Result<()> {
    store.execute_batch("CREATE TABLE IF NOT EXISTS semanticCacheUsage(id INTEGER PRIMARY KEY CHECK(id=1),vectorBytes INTEGER NOT NULL CHECK(vectorBytes>=0));").await?;
    let tx = Transaction::new_unchecked(store, TransactionBehavior::Immediate).await?;
    let result=async {
        let mut rows=store.query("SELECT vectorBytes FROM semanticCacheUsage WHERE id=1",()).await?;
        let exists=rows.next().await?.is_some();drop(rows);
        if !exists {store.execute("INSERT INTO semanticCacheUsage SELECT 1,COALESCE(SUM(length(vector)),0) FROM semanticVectors",()).await?;}
        store.execute_batch(TRIGGERS).await?;
        Ok::<_,Error>(())
    }.await;
    match result {
        Ok(()) => tx.commit().await.map_err(Into::into),
        Err(error) => {
            let _ = tx.rollback().await;
            Err(error)
        }
    }
}

pub(crate) async fn vector_bytes(store: &Connection) -> Result<u64> {
    let mut rows = store
        .query("SELECT vectorBytes FROM semanticCacheUsage WHERE id=1", ())
        .await?;
    let bytes = rows
        .next()
        .await?
        .ok_or_else(|| Error::Unknown("semantic cache accounting missing".into()))?
        .get::<i64>(0)?;
    u64::try_from(bytes).map_err(|_| Error::Unknown("invalid semantic cache accounting".into()))
}

fn managed_file(name: &str, base: &str) -> bool {
    if ["", "-wal", "-tshm"]
        .iter()
        .any(|suffix| name == format!("{base}{suffix}"))
    {
        return true;
    }
    let Some(id) = name.strip_prefix(&format!("{base}.repair-")) else {
        return false;
    };
    let id = id
        .strip_suffix("-wal")
        .or_else(|| id.strip_suffix("-tshm"))
        .unwrap_or(id);
    uuid::Uuid::parse_str(id).is_ok_and(|uuid| uuid.to_string() == id)
}

pub(crate) async fn disk_bytes(family: Option<&Path>) -> Result<Option<u64>> {
    let Some(family) = family else {
        return Ok(None);
    };
    let family: PathBuf = family.into();
    tokio::task::spawn_blocking(move || {
        let base = family
            .file_name()
            .ok_or_else(|| Error::Unknown("semantic cache filename missing".into()))?
            .to_string_lossy();
        let parent = family
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let mut bytes = 0u64;
        for entry in std::fs::read_dir(parent)? {
            let entry = entry?;
            if !managed_file(&entry.file_name().to_string_lossy(), &base) {
                continue;
            }
            let metadata = match std::fs::metadata(entry.path()) {
                Ok(value) => value,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            if metadata.is_file() {
                bytes = bytes
                    .checked_add(metadata.len())
                    .ok_or_else(|| Error::Unknown("semantic disk accounting overflow".into()))?;
            }
        }
        Ok(Some(bytes))
    })
    .await
    .map_err(|_| Error::Unknown("semantic disk accounting task failed".into()))?
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("semantic-limits-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn legacy_usage_and_triggers_follow_insert_ignore_update_delete_and_rollback() {
        let db = turso::Builder::new_local(":memory:").build().await.unwrap();
        let store = db.connect().unwrap();
        store.execute_batch("CREATE TABLE semanticVectors(signature TEXT,inputHash TEXT,vector BLOB NOT NULL,PRIMARY KEY(signature,inputHash)); INSERT INTO semanticVectors VALUES('a','one',x'01020304');").await.unwrap();
        initialize_store(&store).await.unwrap();
        assert_eq!(vector_bytes(&store).await.unwrap(), 4);
        initialize_store(&store).await.unwrap();
        store
            .execute(
                "INSERT OR IGNORE INTO semanticVectors VALUES('a','one',x'00')",
                (),
            )
            .await
            .unwrap();
        assert_eq!(vector_bytes(&store).await.unwrap(), 4);
        store.execute("INSERT INTO semanticVectors VALUES('a','one',x'00') ON CONFLICT(signature,inputHash) DO NOTHING",()).await.unwrap();
        assert_eq!(vector_bytes(&store).await.unwrap(), 4);
        let tx = Transaction::new_unchecked(&store, TransactionBehavior::Immediate)
            .await
            .unwrap();
        store
            .execute("INSERT INTO semanticVectors VALUES('b','two',x'0102')", ())
            .await
            .unwrap();
        assert_eq!(vector_bytes(&store).await.unwrap(), 6);
        tx.rollback().await.unwrap();
        assert_eq!(vector_bytes(&store).await.unwrap(), 4);
        store
            .execute("UPDATE semanticVectors SET vector=x'00'", ())
            .await
            .unwrap();
        assert_eq!(vector_bytes(&store).await.unwrap(), 1);
        store
            .execute("DELETE FROM semanticVectors", ())
            .await
            .unwrap();
        assert_eq!(vector_bytes(&store).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn disk_accounting_includes_private_stores_and_excludes_other_files() {
        let root = Directory::new();
        let base = "library.db.semantic-v1";
        let repair = format!("{base}.repair-{}", uuid::Uuid::new_v4());
        for (name, size) in [
            (base.to_owned(), 11),
            (format!("{base}-wal"), 7),
            (format!("{base}-tshm"), 3),
            (repair.clone(), 5),
            (format!("{repair}-wal"), 13),
            (format!("{repair}-tshm"), 17),
            (format!("{base}.repair-not-a-uuid"), 99),
            ("other.db.semantic-v1".into(), 101),
        ] {
            std::fs::write(root.path().join(name), vec![0; size]).unwrap();
        }
        assert_eq!(
            disk_bytes(Some(&root.path().join(base))).await.unwrap(),
            Some(56)
        );
        assert_eq!(disk_bytes(None).await.unwrap(), None);
    }

    #[test]
    fn resource_limits_are_bounded_and_unknown_fields_rejected() {
        assert!(SemanticLimits::default().validate().is_ok());
        for json in [
            r#"{"max_chunks":0}"#,
            r#"{"max_input_bytes":0}"#,
            r#"{"max_vector_bytes":0}"#,
            r#"{"max_disk_bytes":1099511627777}"#,
        ] {
            assert!(
                serde_json::from_str::<SemanticLimits>(json)
                    .unwrap()
                    .validate()
                    .is_err()
            );
        }
        assert!(serde_json::from_str::<SemanticLimits>(r#"{"unknown":1}"#).is_err());
    }
}
