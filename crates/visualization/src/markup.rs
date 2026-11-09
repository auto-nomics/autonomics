//! Version-bound review ink and its composite preview, separate from publication artwork.
use crate::{Error, Result, Store};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Draw,
    Cross,
    Emphasis,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Stroke {
    pub kind: Kind,
    pub points: Vec<[f64; 2]>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Summary {
    pub id: String,
    pub visualization_id: String,
    pub at_revision: i64,
    pub note: String,
    pub stroke_count: usize,
    pub status: String,
    pub created_at: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Markup {
    #[serde(flatten)]
    pub summary: Summary,
    pub strokes: Vec<Stroke>,
    #[serde(rename = "dataURL")]
    pub data_url: String,
}
fn validate(strokes: &[Stroke]) -> Result<()> {
    if strokes.is_empty()
        || strokes.len() > 100
        || strokes.iter().map(|s| s.points.len()).sum::<usize>() > 10000
    {
        return Err(Error::Invalid(
            "markup strokes (1..100, at most 10000 points)",
        ));
    }
    for stroke in strokes {
        if stroke.points.len() < 2
            || stroke.points.len() > 2000
            || (stroke.kind != Kind::Draw && stroke.points.len() != 2)
            || stroke
                .points
                .iter()
                .flatten()
                .any(|n| !n.is_finite() || n.abs() > 1e6)
            || !stroke.points.iter().any(|p| p != &stroke.points[0])
            || (stroke.kind != Kind::Draw
                && (stroke.points[0][0] == stroke.points[1][0]
                    || stroke.points[0][1] == stroke.points[1][1]))
        {
            return Err(Error::Invalid("markup stroke geometry"));
        }
    }
    Ok(())
}
impl Store {
    pub fn create_markup(
        &self,
        project: &str,
        id: &str,
        revision: i64,
        note: &str,
        strokes: &[Stroke],
        data_url: &str,
    ) -> Result<Summary> {
        if note.trim().is_empty() || note.len() > 16000 {
            return Err(Error::Invalid("markup note (1..16000 bytes)"));
        }
        validate(strokes)?;
        crate::preview::validate_png_preview(data_url)?;
        if self.get(project, id)?.revision != revision {
            return Err(Error::Conflict);
        }
        let summary = Summary {
            id: uuid::Uuid::new_v4().to_string(),
            visualization_id: id.into(),
            at_revision: revision,
            note: note.trim().into(),
            stroke_count: strokes.len(),
            status: "open".into(),
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        if self.connection.execute("INSERT INTO visualization_markups SELECT ?1,?2,?3,?4,?5,?6,?7,?8 WHERE EXISTS(SELECT 1 FROM visualizations WHERE id=?2 AND project_id=?9 AND revision=?3)", params![summary.id,id,revision,summary.note,serde_json::to_string(strokes)?,data_url,summary.status,summary.created_at,project])? == 0 { return Err(Error::Conflict); }
        Ok(summary)
    }
    /// Latest 50 review records, without preview bytes or detailed geometry.
    pub fn markups(&self, project: &str, id: &str) -> Result<Vec<Summary>> {
        self.get(project, id)?;
        let mut statement = self.connection.prepare("SELECT id,at_revision,note,strokes,status,created_at FROM visualization_markups WHERE visualization_id=?1 ORDER BY created_at DESC,id DESC LIMIT 50")?;
        let rows = statement.query_map([id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?;
        rows.map(|row| {
            let (markup_id, at_revision, note, strokes, status, created_at) = row?;
            Ok(Summary {
                id: markup_id,
                visualization_id: id.into(),
                at_revision,
                note,
                stroke_count: serde_json::from_str::<Vec<Stroke>>(&strokes)?.len(),
                status,
                created_at,
            })
        })
        .collect()
    }
    pub fn markup(&self, project: &str, id: &str, markup_id: &str) -> Result<Markup> {
        self.get(project, id)?;
        let (at_revision,note,strokes,data_url,status,created_at) = self.connection.query_row("SELECT at_revision,note,strokes,preview,status,created_at FROM visualization_markups WHERE visualization_id=?1 AND id=?2",params![id,markup_id],|row| Ok((row.get::<_,i64>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?,row.get::<_,String>(3)?,row.get::<_,String>(4)?,row.get::<_,String>(5)?))).optional()?.ok_or(Error::NotFound)?;
        let strokes: Vec<Stroke> = serde_json::from_str(&strokes)?;
        Ok(Markup {
            summary: Summary {
                id: markup_id.into(),
                visualization_id: id.into(),
                at_revision,
                note,
                stroke_count: strokes.len(),
                status,
                created_at,
            },
            strokes,
            data_url,
        })
    }
    pub fn resolve_markup(&self, project: &str, id: &str, markup_id: &str) -> Result<()> {
        self.get(project, id)?;
        if self.connection.execute("UPDATE visualization_markups SET status='resolved' WHERE visualization_id=?1 AND id=?2",params![id,markup_id])? == 0 { return Err(Error::NotFound); }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    pub const PNG: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg==";
    #[test]
    fn markup_survives_revisions_without_changing_artwork_or_formal_preview() -> Result<()> {
        let mut store = Store::in_memory()?;
        store.create_project("p", "Study")?;
        store.create_project("other", "Other")?;
        let document = json!({"elements":[{"id":"cell"}],"files":{}});
        let figure = store.create("p", "Figure", "excalidraw/v2", &document)?;
        let strokes = vec![Stroke {
            kind: Kind::Cross,
            points: vec![[1., 2.], [30., 40.]],
        }];
        let summary =
            store.create_markup("p", &figure.id, 1, "Remove this component", &strokes, PNG)?;
        assert_eq!(store.get("p", &figure.id)?.revision, 1);
        assert_eq!(store.revision("p", &figure.id, 1)?.document, document);
        assert!(matches!(
            store.preview("p", &figure.id, 1),
            Err(Error::NotFound)
        ));
        assert!(matches!(
            store.markup("other", &figure.id, &summary.id),
            Err(Error::NotFound)
        ));
        store.save("p", &figure.id, 1, "Edited", &json!({"elements":[]}))?;
        assert!(matches!(
            store.create_markup("p", &figure.id, 1, "Stale", &strokes, PNG),
            Err(Error::Conflict)
        ));
        let historical = store.markup("p", &figure.id, &summary.id)?;
        assert_eq!(historical.summary.at_revision, 1);
        assert_eq!(historical.strokes, strokes);
        assert_eq!(historical.data_url, PNG);
        assert!(
            serde_json::to_value(store.markups("p", &figure.id)?).unwrap()[0]
                .get("dataURL")
                .is_none()
        );
        store.resolve_markup("p", &figure.id, &summary.id)?;
        assert_eq!(store.markups("p", &figure.id)?[0].status, "resolved");
        store.delete("p", &figure.id, 2)?;
        assert_eq!(
            store.connection.query_row(
                "SELECT count(*) FROM visualization_markups",
                [],
                |row| row.get::<_, i64>(0)
            )?,
            0
        );
        Ok(())
    }
    #[test]
    fn invalid_ink_and_images_are_rejected() -> Result<()> {
        let mut store = Store::in_memory()?;
        store.create_project("p", "Study")?;
        let figure = store.create("p", "Figure", "excalidraw/v2", &json!({"elements":[]}))?;
        let stroke = Stroke {
            kind: Kind::Draw,
            points: vec![[0., 0.], [20., 30.]],
        };
        for strokes in [
            vec![],
            vec![Stroke {
                kind: Kind::Draw,
                points: vec![[0., 0.], [f64::NAN, 0.]],
            }],
            vec![Stroke {
                kind: Kind::Emphasis,
                points: vec![[1., 1.], [1., 1.]],
            }],
            vec![stroke.clone(); 101],
        ] {
            assert!(
                store
                    .create_markup("p", &figure.id, 1, "Review", &strokes, PNG)
                    .is_err()
            );
        }
        assert!(
            store
                .create_markup("p", &figure.id, 1, "", std::slice::from_ref(&stroke), PNG)
                .is_err()
        );
        assert!(
            store
                .create_markup("p", &figure.id, 1, "Review", &[stroke], "not a PNG")
                .is_err()
        );
        assert!(store.markups("p", &figure.id)?.is_empty());
        Ok(())
    }
}
