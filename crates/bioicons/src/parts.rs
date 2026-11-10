//! Address individual rendered paths in one immutable SVG snapshot.
use crate::{
    Error, Result,
    vector::{self, PreparedSvg},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use quick_xml::{
    Reader, Writer,
    events::{BytesStart, Event},
};
use resvg::usvg;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
pub(super) fn invalid(message: impl Into<String>) -> Error {
    Error::Invalid(message.into())
}
#[derive(Debug, Serialize)]
pub struct Part {
    pub part_id: String,
    pub index: usize,
    pub svg_id: Option<String>,
    pub fill: String,
    pub stroke: String,
    pub opacity: f64,
    pub bounds: [f64; 4],
    pub translation_supported: bool,
    pub point_count: usize,
    pub geometry_supported: bool,
}
#[derive(Debug, Serialize)]
pub struct Inspection {
    pub bounds_kind: &'static str,
    pub bounds_note: &'static str,
    pub sha256: String,
    pub width: u32,
    pub height: u32,
    pub parts: Vec<Part>,
    #[serde(rename = "previewDataURL")]
    pub preview_data_url: String,
    #[serde(rename = "annotatedDataURL")]
    pub annotated_data_url: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Edit {
    pub part_id: String,
    /// #RRGGBB or none; omitted means unchanged. Gradient replacement is explicit.
    pub fill: Option<String>,
    pub stroke: Option<String>,
    /// Local path opacity, multiplied by any ancestor group opacity.
    pub opacity: Option<f64>,
    /// Relative [dx,dy] in the normalized SVG root coordinate system, not canvas pixels.
    pub translate: Option<[f64; 2]>,
    /// Sparse absolute root-coordinate point edits from geometry, applied to this snapshot.
    pub points: Option<Vec<crate::geometry::PointEdit>>,
}
pub(super) fn source(bytes: &[u8]) -> Result<(PreparedSvg, Vec<u8>)> {
    let vector = vector::normalize(bytes)?;
    if vector.text_outlined {
        return Err(invalid("outline SVG text before editing paths"));
    }
    let canonical = STANDARD
        .decode(vector.data_url.split_once(',').unwrap().1)
        .map_err(|e| invalid(e.to_string()))?;
    Ok((vector, canonical))
}
// Resource paths are not editable scene components: changing one could alter
// many unrelated rendered paths sharing a mask, clip or pattern.
fn resource(name: &[u8]) -> bool {
    matches!(
        name,
        b"defs" | b"clipPath" | b"mask" | b"pattern" | b"marker"
    )
}
fn rewrite(
    bytes: &[u8],
    mut path: impl FnMut(&BytesStart<'_>, usize) -> Result<BytesStart<'static>>,
) -> Result<Vec<u8>> {
    let mut reader = Reader::from_reader(bytes);
    let mut writer = Writer::new(Vec::new());
    let mut excluded = 0usize;
    let mut index = 0;
    loop {
        let event = reader.read_event().map_err(|e| invalid(e.to_string()))?;
        match &event {
            Event::Start(e) | Event::Empty(e) => {
                let blocked = resource(e.local_name().as_ref());
                if matches!(&event, Event::Start(_)) && (blocked || excluded > 0) {
                    excluded += 1;
                }
                if excluded == 0 && !blocked && e.local_name().as_ref() == b"path" {
                    index += 1;
                    if index > 256 {
                        return Err(invalid("path editing supports at most 256 rendered paths"));
                    }
                    let next = path(e, index)?;
                    writer
                        .write_event(if matches!(&event, Event::Empty(_)) {
                            Event::Empty(next)
                        } else {
                            Event::Start(next)
                        })
                        .map_err(|e| invalid(e.to_string()))?;
                    continue;
                }
            }
            Event::End(_) if excluded > 0 => excluded -= 1,
            Event::Eof => break,
            _ => {}
        }
        writer
            .write_event(event)
            .map_err(|e| invalid(e.to_string()))?;
    }
    Ok(writer.into_inner())
}
fn attributes(e: &BytesStart<'_>) -> Result<BTreeMap<String, String>> {
    e.attributes()
        .map(|a| {
            let a = a.map_err(|e| invalid(e.to_string()))?;
            Ok((
                String::from_utf8_lossy(a.key.as_ref()).into_owned(),
                a.unescape_value()
                    .map_err(|e| invalid(e.to_string()))?
                    .into_owned(),
            ))
        })
        .collect()
}
fn element(attrs: BTreeMap<String, String>) -> BytesStart<'static> {
    let mut next = BytesStart::new("path");
    for (key, value) in attrs {
        next.push_attribute((key.as_str(), value.as_str()));
    }
    next
}
fn id(hash: &str, index: usize) -> String {
    format!("part-{hash}-{index}")
}
pub(super) fn path_tree(canonical: &[u8], hash: &str) -> Result<(usvg::Tree, Vec<Part>)> {
    let mut parts = Vec::new();
    let tagged = rewrite(canonical, |e, index| {
        let mut attrs = attributes(e)?;
        let part_id = id(hash, index);
        parts.push(Part {
            part_id: part_id.clone(),
            index,
            svg_id: attrs.get("id").cloned(),
            fill: attrs.get("fill").cloned().unwrap_or("#000000".into()),
            stroke: attrs.get("stroke").cloned().unwrap_or("none".into()),
            opacity: attrs
                .get("opacity")
                .and_then(|v| v.parse().ok())
                .unwrap_or(1.),
            bounds: [0.; 4],
            translation_supported: !attrs.contains_key("transform"),
            point_count: 0,
            geometry_supported: false,
        });
        attrs.insert("id".into(), part_id);
        Ok(element(attrs))
    })?;
    if parts.is_empty() {
        return Err(invalid("SVG has no editable rendered paths"));
    }
    let tree = usvg::Tree::from_data(&tagged, &usvg::Options::default())
        .map_err(|e| invalid(e.to_string()))?;
    for part in &mut parts {
        let node = tree
            .node_by_id(&part.part_id)
            .ok_or_else(|| invalid("rendered path cannot be located"))?;
        part.translation_supported &= node.abs_transform().invert().is_some();
        if let usvg::Node::Path(path) = node {
            part.point_count = path
                .data()
                .segments()
                .map(|segment| match segment {
                    resvg::tiny_skia::PathSegment::MoveTo(_)
                    | resvg::tiny_skia::PathSegment::LineTo(_) => 1,
                    resvg::tiny_skia::PathSegment::QuadTo(..) => 2,
                    resvg::tiny_skia::PathSegment::CubicTo(..) => 3,
                    resvg::tiny_skia::PathSegment::Close => 0,
                })
                .sum();
            part.geometry_supported = part.translation_supported && part.point_count <= 512;
        }
        let b = node.abs_stroke_bounding_box();
        part.bounds = [
            b.x() as f64,
            b.y() as f64,
            b.width() as f64,
            b.height() as f64,
        ];
    }
    Ok((tree, parts))
}
pub fn inspect(bytes: &[u8]) -> Result<Inspection> {
    let (vector, canonical) = source(bytes)?;
    let hash = vector.source_sha256.clone();
    let (_, parts) = path_tree(&canonical, &hash)?;
    // This numbered visual is review material only; never store it as artwork.
    let mut annotated = String::from_utf8(canonical).map_err(|e| invalid(e.to_string()))?;
    let end = annotated
        .rfind("</svg>")
        .ok_or_else(|| invalid("invalid SVG root"))?;
    let size = (f64::from(vector.width.max(vector.height)) / 50.).max(2.);
    let mut overlay = String::new();
    for p in &parts {
        let [x, y, w, h] = p.bounds;
        overlay.push_str(&format!("<rect x='{x}' y='{y}' width='{w}' height='{h}' fill='none' stroke='#ff6600' stroke-width='{}'/><text x='{x}' y='{}' font-size='{size}' fill='#000000' stroke='#ffffff' stroke-width='{}' paint-order='stroke'>{}</text>",size/12.,y+size,size/8.,p.index));
    }
    annotated.insert_str(end, &overlay);
    let annotated = vector::normalize(annotated.as_bytes())?;
    Ok(Inspection {
        bounds_kind: "svg_root_stroke_aabb",
        bounds_note: "Includes path strokes and ancestor transforms in normalized SVG root coordinates; not biological domains or an exact clipped-pixel mask.",
        sha256: hash,
        width: vector.width,
        height: vector.height,
        parts,
        preview_data_url: vector.preview_data_url,
        annotated_data_url: annotated.preview_data_url,
    })
}
fn paint(value: &str) -> Result<String> {
    if value == "none" {
        return Ok(value.into());
    }
    if value.len() != 7
        || !value.starts_with('#')
        || !value[1..].bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(invalid("path paint must be #RRGGBB or none"));
    }
    Ok(value.to_ascii_lowercase())
}
pub fn edit(bytes: &[u8], expected_sha256: &str, edits: &[Edit]) -> Result<PreparedSvg> {
    let (original, canonical) = source(bytes)?;
    if original.source_sha256 != expected_sha256 {
        return Err(invalid(
            "SVG snapshot hash has changed; inspect paths again",
        ));
    }
    if edits.is_empty() || edits.len() > 32 {
        return Err(invalid("requires 1..32 path edits"));
    }
    let mut pending = BTreeMap::new();
    for edit in edits {
        if edit.fill.is_none()
            && edit.stroke.is_none()
            && edit.opacity.is_none()
            && edit.translate.is_none()
            && edit.points.is_none()
        {
            return Err(invalid("path edit has no changes"));
        }
        if edit
            .opacity
            .is_some_and(|v| !v.is_finite() || !(0. ..=1.).contains(&v))
        {
            return Err(invalid("path opacity must be 0..1"));
        }
        if edit
            .translate
            .is_some_and(|d| d.iter().any(|v| !v.is_finite() || v.abs() > 100000.))
        {
            return Err(invalid(
                "path translation requires finite root-coordinate deltas within ±100000",
            ));
        }
        if edit.points.is_some() && edit.translate.is_some() {
            return Err(invalid("edit points and translate in separate proposals"));
        }
        if pending.insert(edit.part_id.clone(), edit).is_some() {
            return Err(invalid("duplicate path ID"));
        }
    }
    let mut point_paths = BTreeMap::new();
    let mut topology = BTreeMap::new();
    let mut original_part_count = None;
    if edits.iter().any(|e| e.points.is_some()) {
        let (tree, parts) = path_tree(&canonical, expected_sha256)?;
        original_part_count = Some(parts.len());
        for edit in edits {
            if let Some(points) = &edit.points {
                let part = parts
                    .iter()
                    .find(|p| p.part_id == edit.part_id)
                    .ok_or_else(|| invalid("unknown path ID"))?;
                if !part.geometry_supported {
                    return Err(invalid(
                        "path geometry is unsupported or exceeds 512 points",
                    ));
                }
                let usvg::Node::Path(path) = tree
                    .node_by_id(&edit.part_id)
                    .ok_or_else(|| invalid("path unavailable"))?
                else {
                    return Err(invalid("target is not a path"));
                };
                topology.insert(part.index, crate::geometry::topology(path));
                point_paths.insert(
                    edit.part_id.clone(),
                    crate::geometry::edit_path(path, points)?,
                );
            }
        }
    }
    let movement = if edits.iter().any(|e| e.translate.is_some()) {
        let (tree, parts) = path_tree(&canonical, expected_sha256)?;
        let mut movement = BTreeMap::new();
        for edit in edits {
            let Some([dx, dy]) = edit.translate else {
                continue;
            };
            let part = parts
                .iter()
                .find(|p| p.part_id == edit.part_id)
                .ok_or_else(|| invalid("unknown path ID"))?;
            if !part.translation_supported {
                return Err(invalid("path transform does not support translation"));
            }
            let t = tree
                .node_by_id(&edit.part_id)
                .ok_or_else(|| invalid("path unavailable"))?
                .abs_transform();
            let (sx, sy, kx, ky) = (
                f64::from(t.sx),
                f64::from(t.sy),
                f64::from(t.kx),
                f64::from(t.ky),
            );
            let determinant = sx * sy - kx * ky;
            let local = [
                (sy * dx - kx * dy) / determinant,
                (-ky * dx + sx * dy) / determinant,
            ];
            if local.iter().any(|v| !v.is_finite() || v.abs() > 1e9) {
                return Err(invalid(
                    "path transform is too ill-conditioned for this translation",
                ));
            }
            movement.insert(edit.part_id.clone(), local);
        }
        movement
    } else {
        BTreeMap::new()
    };
    let rewritten = rewrite(&canonical, |e, index| {
        let Some(edit) = pending.remove(&id(expected_sha256, index)) else {
            return Ok(e.clone().into_owned());
        };
        let mut attrs = attributes(e)?;
        if let Some(fill) = &edit.fill {
            attrs.insert("fill".into(), paint(fill)?);
        }
        if let Some(stroke) = &edit.stroke {
            attrs.insert("stroke".into(), paint(stroke)?);
        }
        if let Some(opacity) = edit.opacity {
            attrs.insert("opacity".into(), opacity.to_string());
        }
        if let Some(data) = point_paths.get(&edit.part_id) {
            attrs.insert("d".into(), data.clone());
        }
        if let Some([dx, dy]) = movement.get(&edit.part_id) {
            // In canonical render-tree SVG, ordinary paths have identity local
            // transforms. Translating the element (rather than d) also moves
            // its paint coordinate system; shared parent clips remain fixed.
            if attrs.contains_key("transform") {
                return Err(invalid("unexpected canonical path transform"));
            }
            attrs.insert("transform".into(), format!("translate({dx} {dy})"));
        }
        Ok(element(attrs))
    })?;
    if !pending.is_empty() {
        return Err(invalid("unknown path ID; use IDs from this SVG inspection"));
    }
    let result = vector::normalize(&rewritten)?;
    if let Some(count) = original_part_count {
        let bytes = STANDARD
            .decode(result.data_url.split_once(',').unwrap().1)
            .map_err(|e| invalid(e.to_string()))?;
        let (tree, parts) = path_tree(&bytes, &result.sha256)?;
        if parts.len() != count {
            return Err(invalid(
                "point edit would remove a path; preserve non-degenerate geometry",
            ));
        }
        for (index, expected) in topology {
            let usvg::Node::Path(path) = tree
                .node_by_id(&id(&result.sha256, index))
                .ok_or_else(|| invalid("edited path was lost"))?
            else {
                return Err(invalid("edited target is not a path"));
            };
            if crate::geometry::topology(path) != expected {
                return Err(invalid(
                    "point edit would change command topology; preserve distinct endpoints and controls",
                ));
            }
        }
    }
    if result.sha256 == original.sha256 {
        return Err(invalid("path edits do not change the vector"));
    }
    Ok(result)
}
