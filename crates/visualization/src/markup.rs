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
pub struct Page {
    pub items: Vec<Summary>,
    pub next_before: Option<String>,
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
    /// Compatibility view; use markups_page to traverse the complete history.
    pub fn markups(&self, project: &str, id: &str) -> Result<Vec<Summary>> {
        Ok(self.markups_page(project, id, None, None, 50)?.items)
    }
    /// Stable descending (created_at, id) cursor, scoped to the figure.
    /// Resolved records can be filtered before pagination so open reviews are not hidden.
    pub fn markups_page(
        &self,
        project: &str,
        id: &str,
        before: Option<&str>,
        status: Option<&str>,
        limit: u32,
    ) -> Result<Page> {
        self.get(project, id)?;
        if !(1..=100).contains(&limit) || status.is_some_and(|s| s != "open" && s != "resolved") {
            return Err(Error::Invalid(
                "markup page limit (1..100) or status (open/resolved)",
            ));
        }
        let cursor: Option<String> = before.map(|cursor| self.connection.query_row(
            "SELECT created_at FROM visualization_markups WHERE visualization_id=?1 AND id=?2", params![id,cursor], |row| row.get(0)
        ).optional()?.ok_or(Error::NotFound)).transpose()?;
        let mut statement = self.connection.prepare("SELECT id,at_revision,note,strokes,status,created_at FROM visualization_markups WHERE visualization_id=?1 AND (?2 IS NULL OR status=?2) AND (?3 IS NULL OR created_at < ?3 OR (created_at=?3 AND id < ?4)) ORDER BY created_at DESC,id DESC LIMIT ?5")?;
        let rows = statement.query_map(params![id, status, cursor, before, limit + 1], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?;
        let mut items = rows
            .map(|row| {
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
            .collect::<Result<Vec<_>>>()?;
        let next_before = if items.len() > limit as usize {
            items.truncate(limit as usize);
            items.last().map(|item| item.id.clone())
        } else {
            None
        };
        Ok(Page { items, next_before })
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
    fn pages_reach_old_open_reviews_without_duplicates_or_foreign_cursors() -> Result<()> {
        let mut store = Store::in_memory()?;
        store.create_project("p", "Study")?;
        let figure = store.create("p", "Figure", "excalidraw/v2", &json!({"elements":[]}))?;
        let other = store.create("p", "Other", "excalidraw/v2", &json!({"elements":[]}))?;
        let strokes = [Stroke {
            kind: Kind::Cross,
            points: vec![[0., 0.], [10., 10.]],
        }];
        let oldest = store.create_markup("p", &figure.id, 1, "Still outstanding", &strokes, PNG)?;
        for _ in 0..54 {
            let record = store.create_markup("p", &figure.id, 1, "Done", &strokes, PNG)?;
            store.resolve_markup("p", &figure.id, &record.id)?;
        }
        // Force equal timestamps to prove the UUID tie breaker works across pages.
        store.connection.execute(
            "UPDATE visualization_markups SET created_at='2026-01-01T00:00:00Z'",
            [],
        )?;
        let mut before = None;
        let mut ids = std::collections::BTreeSet::new();
        loop {
            let page = store.markups_page("p", &figure.id, before.as_deref(), None, 7)?;
            for item in page.items {
                assert!(ids.insert(item.id));
            }
            before = page.next_before;
            if before.is_none() {
                break;
            }
        }
        assert_eq!(ids.len(), 55);
        assert!(ids.contains(&oldest.id));
        let open = store.markups_page("p", &figure.id, None, Some("open"), 50)?;
        assert_eq!(open.items.len(), 1);
        assert_eq!(open.items[0].id, oldest.id);
        assert!(open.next_before.is_none());
        assert!(matches!(
            store.markups_page("p", &other.id, Some(&oldest.id), None, 7),
            Err(Error::NotFound)
        ));
        for limit in [0, 101] {
            assert!(
                store
                    .markups_page("p", &figure.id, None, None, limit)
                    .is_err()
            );
        }
        assert!(
            store
                .markups_page("p", &figure.id, None, Some("unknown"), 7)
                .is_err()
        );
        // A cursor record remains usable when its status changes between requests.
        let page = store.markups_page("p", &figure.id, None, Some("resolved"), 1)?;
        let cursor = page.next_before.unwrap();
        store.connection.execute(
            "UPDATE visualization_markups SET status='open' WHERE id=?1",
            [&cursor],
        )?;
        assert!(
            !store
                .markups_page("p", &figure.id, Some(&cursor), Some("resolved"), 1)?
                .items
                .is_empty()
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
