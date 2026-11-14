//! Revision-checked SVG inspection and direct editing.
use crate::Result;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use visualization::Store;
pub enum Inspection<'a> {
    Palette,
    Parts,
    Geometry { sha256: &'a str, part_id: &'a str },
}
pub fn inspect_svg(
    store: &Store,
    project: &str,
    figure: &str,
    revision: i64,
    target: &str,
    kind: Inspection<'_>,
) -> Result<Value> {
    let document = crate::current(store, project, figure, revision)?;
    let mut value = match kind {
        Inspection::Palette => crate::palette_document(&document, target)?,
        Inspection::Parts => crate::parts_document(&document, target)?,
        Inspection::Geometry { sha256, part_id } => {
            crate::geometry_document(&document, target, sha256, part_id)?
        }
    };
    value["revision"] = json!(revision);
    value["scope"] = json!("saved_revision");
    Ok(value)
}
pub enum SvgEdit<'a> {
    Recolor(&'a BTreeMap<String, String>),
    Parts {
        sha256: &'a str,
        edits: &'a [bioicons::parts::Edit],
    },
}
pub struct SvgRefinement<'a> {
    pub target: &'a str,
    pub edit: SvgEdit<'a>,
}
pub fn refine_svg(
    store: &Store,
    project: &str,
    figure: &str,
    revision: i64,
    input: SvgRefinement<'_>,
) -> Result<visualization::Visualization> {
    let document = crate::current(store, project, figure, revision)?;
    let next = match input.edit {
        SvgEdit::Recolor(colors) => crate::recolor_document(&document, input.target, colors)?,
        SvgEdit::Parts { sha256, edits } => {
            crate::edit_parts_document(&document, input.target, sha256, edits)?
        }
    };
    Ok(store.save(
        project,
        figure,
        revision,
        &store.get(project, figure)?.title,
        &next,
    )?)
}
