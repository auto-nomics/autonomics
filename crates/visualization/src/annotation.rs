//! Review notes linked to scene objects, kept outside publication artwork.
use crate::{Error, Result, Store};
use rusqlite::params;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Annotation {
    pub id: String,
    pub visualization_id: String,
    pub at_revision: i64,
    pub element_ids: Vec<String>,
    pub note: String,
    pub status: String,
    pub created_at: String,
}
impl Store {
    pub fn annotate(
        &self,
        project: &str,
        id: &str,
        expected_revision: i64,
        element_ids: &[String],
        note: &str,
    ) -> Result<Annotation> {
        if note.trim().is_empty() || note.len() > 16000 {
            return Err(Error::Invalid("note (1..16000 bytes)"));
        }
        if element_ids.is_empty() || element_ids.len() > 100 {
            return Err(Error::Invalid("annotation targets (1..100)"));
        }
        if self.get(project, id)?.revision != expected_revision {
            return Err(Error::Conflict);
        }
        let scene = self.revision(project, id, expected_revision)?;
        let elements = scene
            .document
            .get("elements")
            .and_then(serde_json::Value::as_array)
            .ok_or(Error::Invalid("scene elements"))?;
        let mut targets = std::collections::HashSet::new();
        for target in element_ids {
            if !targets.insert(target)
                || !elements.iter().any(|element| {
                    element["id"].as_str() == Some(target) && element["isDeleted"] != true
                })
            {
                return Err(Error::Invalid("annotation target"));
            }
        }
        let annotation = Annotation {
            id: uuid::Uuid::new_v4().to_string(),
            visualization_id: id.into(),
            at_revision: expected_revision,
            element_ids: element_ids.into(),
            note: note.trim().into(),
            status: "open".into(),
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        if self.connection.execute("INSERT INTO visualization_annotations SELECT ?1,?2,?3,?4,?5,?6,?7 WHERE EXISTS (SELECT 1 FROM visualizations WHERE id=?2 AND project_id=?8 AND revision=?3)",params![annotation.id,id,expected_revision,serde_json::to_string(element_ids)?,annotation.note,annotation.status,annotation.created_at,project])?==0 {return Err(Error::Conflict);}
        Ok(annotation)
    }
    pub fn annotations(&self, project: &str, id: &str) -> Result<Vec<Annotation>> {
        self.get(project, id)?;
        let mut statement=self.connection.prepare("SELECT id,at_revision,element_ids,note,status,created_at FROM visualization_annotations WHERE visualization_id=?1 ORDER BY created_at,id")?;
        statement
            .query_map([id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })?
            .map(|row| {
                let (annotation_id, at_revision, element_ids, note, status, created_at) = row?;
                Ok(Annotation {
                    id: annotation_id,
                    visualization_id: id.into(),
                    at_revision,
                    element_ids: serde_json::from_str(&element_ids)?,
                    note,
                    status,
                    created_at,
                })
            })
            .collect()
    }
    pub fn resolve_annotation(&self, project: &str, id: &str, annotation_id: &str) -> Result<()> {
        self.get(project, id)?;
        if self.connection.execute("UPDATE visualization_annotations SET status='resolved' WHERE id=?1 AND visualization_id=?2",params![annotation_id,id])?==0{return Err(Error::NotFound);}
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn notes_are_scoped_and_do_not_change_artwork() -> Result<()> {
        let mut store = Store::in_memory()?;
        store.create_project("p", "Study")?;
        store.create_project("other", "Other")?;
        let document = json!({"elements":[{"id":"cell"}]});
        let figure = store.create("p", "Figure", "excalidraw/v2", &document)?;
        let note = store.annotate("p", &figure.id, 1, &["cell".into()], "Emphasize this cell")?;
        assert_eq!(store.revision("p", &figure.id, 1)?.document, document);
        assert!(matches!(
            store.annotate("p", &figure.id, 0, &["cell".into()], "stale"),
            Err(Error::Conflict)
        ));
        assert!(
            store
                .annotate("p", &figure.id, 1, &["missing".into()], "invalid")
                .is_err()
        );
        assert!(matches!(
            store.annotations("other", &figure.id),
            Err(Error::NotFound)
        ));
        store.resolve_annotation("p", &figure.id, &note.id)?;
        assert_eq!(store.annotations("p", &figure.id)?[0].status, "resolved");
        Ok(())
    }
}
