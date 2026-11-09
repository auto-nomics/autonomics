//! Browser-rendered previews tied to an exact saved revision, never to a moving head.
use crate::{Error, Result, Store};
use base64::Engine;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preview {
    pub visualization_id: String,
    pub revision: i64,
    #[serde(rename = "dataURL")]
    pub data_url: String,
}
pub(crate) fn validate_png_preview(data_url: &str) -> Result<()> {
    let data = data_url
        .strip_prefix("data:image/png;base64,")
        .ok_or(Error::Invalid("preview must be a PNG data URL"))?;
    if data.len() > 6 * 1024 * 1024 {
        return Err(Error::Invalid("preview exceeds vision transfer limit"));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|_| Error::Invalid("preview base64"))?;
    if bytes.len() < 24 || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") || &bytes[12..16] != b"IHDR" {
        return Err(Error::Invalid("preview PNG header"));
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
    if width == 0 || height == 0 || width > 1280 || height > 1280 {
        return Err(Error::Invalid("preview dimensions (1..1280)"));
    }
    Ok(())
}
impl Store {
    pub fn save_preview(
        &self,
        project: &str,
        id: &str,
        revision: i64,
        data_url: &str,
    ) -> Result<()> {
        self.get(project, id)?;
        validate_png_preview(data_url)?;
        if self.connection.execute("INSERT INTO visualization_previews SELECT ?1,?2,?3 WHERE EXISTS(SELECT 1 FROM visualizations WHERE id=?1 AND project_id=?4 AND revision=?2) ON CONFLICT(visualization_id,revision) DO UPDATE SET data_url=excluded.data_url",params![id,revision,data_url,project])?==0 {return Err(Error::Conflict);}
        Ok(())
    }
    pub fn preview(&self, project: &str, id: &str, revision: i64) -> Result<Preview> {
        self.get(project, id)?;
        let data_url=self.connection.query_row("SELECT data_url FROM visualization_previews WHERE visualization_id=?1 AND revision=?2",params![id,revision],|row|row.get(0)).optional()?.ok_or(Error::NotFound)?;
        Ok(Preview {
            visualization_id: id.into(),
            revision,
            data_url,
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn previews_are_versioned_and_reject_stale_uploads() -> Result<()> {
        let mut store = Store::in_memory()?;
        store.create_project("p", "Study")?;
        store.create_project("other", "Other")?;
        let figure = store.create("p", "Figure", "excalidraw/v2", &json!({"elements":[]}))?;
        let png = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg==";
        store.save_preview("p", &figure.id, 1, png)?;
        store.save("p", &figure.id, 1, "Changed", &json!({"elements":[]}))?;
        assert!(matches!(
            store.save_preview("p", &figure.id, 1, png),
            Err(Error::Conflict)
        ));
        assert_eq!(store.preview("p", &figure.id, 1)?.data_url, png);
        assert!(matches!(
            store.preview("p", &figure.id, 2),
            Err(Error::NotFound)
        ));
        assert!(matches!(
            store.preview("other", &figure.id, 1),
            Err(Error::NotFound)
        ));
        assert!(
            store
                .save_preview("p", &figure.id, 2, "data:text/plain;base64,aGk=")
                .is_err()
        );
        Ok(())
    }
}
