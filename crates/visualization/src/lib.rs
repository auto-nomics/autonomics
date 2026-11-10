//! Project-scoped visualization persistence, independent of GUI and rendering engine.
pub mod annotation;
pub mod arrange;
mod duplicate;
pub mod error;
mod font_metrics;
pub mod image;
pub mod inset;
pub mod inset_refresh;
mod layout;
pub mod markup;
mod panel_routes;
mod panels;
pub mod planning;
pub mod preview;
pub mod proposal;
pub mod proposal_preview;
pub mod scene;
pub mod service;
pub mod sketch;
pub mod storage;
pub use crate::error::{Error, Result};
pub use crate::storage::Store;

use serde::{Deserialize, Serialize};
use serde_json::Value;

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
pub struct RevisionSummary {
    pub revision: i64,
    pub title: String,
    pub created_at: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Revision {
    pub visualization_id: String,
    pub revision: i64,
    pub title: String,
    pub document: Value,
    pub created_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn history_pages_and_restore_preserve_prior_revisions() -> Result<()> {
        let mut store = Store::in_memory()?;
        store.create_project("p", "Study")?;
        let old = serde_json::json!({"elements":[],"files":{"reference":"bytes"}});
        let figure = store.create("p", "Original", "excalidraw/v2", &old)?;
        store.save(
            "p",
            &figure.id,
            1,
            "Edited",
            &serde_json::json!({"elements":[{"id":"cell"}]}),
        )?;
        assert_eq!(store.revisions("p", &figure.id, None, 1)?[0].revision, 2);
        assert_eq!(
            store.revisions("p", &figure.id, Some(2), 20)?[0].title,
            "Original"
        );
        assert!(store.revisions("p", &figure.id, Some(1), 20)?.is_empty());
        assert!(matches!(
            store.save("p", &figure.id, 1, "Original", &old),
            Err(Error::Conflict)
        ));
        assert_eq!(
            store.save("p", &figure.id, 2, "Original", &old)?.revision,
            3
        );
        assert_eq!(store.revision("p", &figure.id, 1)?.document, old);
        assert_eq!(store.revision("p", &figure.id, 2)?.title, "Edited");
        assert_eq!(store.revision("p", &figure.id, 3)?.document, old);
        Ok(())
    }
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

mod text_edit;
