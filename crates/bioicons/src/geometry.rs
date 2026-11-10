//! Topology-preserving endpoint/control-point editing in SVG root coordinates.
use crate::{
    Result,
    parts::{invalid, path_tree, source},
};
use resvg::{
    tiny_skia::{PathSegment, Point},
    usvg,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PointEdit {
    pub index: usize,
    pub position: [f64; 2],
}
#[derive(Debug, Serialize)]
pub struct Handle {
    pub index: usize,
    pub kind: &'static str,
    pub position: [f64; 2],
}
#[derive(Debug, Serialize)]
pub struct Command {
    pub command: &'static str,
    pub points: Vec<usize>,
}
#[derive(Debug, Serialize)]
pub struct Geometry {
    pub sha256: String,
    pub part_id: String,
    pub width: u32,
    pub height: u32,
    pub points: Vec<Handle>,
    pub commands: Vec<Command>,
    #[serde(rename = "previewDataURL")]
    pub preview_data_url: String,
    #[serde(rename = "annotatedDataURL")]
    pub annotated_data_url: String,
}
fn segments(path: &usvg::Path) -> Vec<(&'static str, Vec<(Point, &'static str)>)> {
    path.data()
        .segments()
        .map(|s| match s {
            PathSegment::MoveTo(a) => ("M", vec![(a, "anchor")]),
            PathSegment::LineTo(a) => ("L", vec![(a, "anchor")]),
            PathSegment::QuadTo(a, b) => ("Q", vec![(a, "control"), (b, "anchor")]),
            PathSegment::CubicTo(a, b, c) => {
                ("C", vec![(a, "control"), (b, "control"), (c, "anchor")])
            }
            PathSegment::Close => ("Z", vec![]),
        })
        .collect()
}
fn root(t: usvg::Transform, p: Point) -> [f64; 2] {
    [
        f64::from(t.sx) * f64::from(p.x) + f64::from(t.kx) * f64::from(p.y) + f64::from(t.tx),
        f64::from(t.ky) * f64::from(p.x) + f64::from(t.sy) * f64::from(p.y) + f64::from(t.ty),
    ]
}
pub fn inspect(bytes: &[u8], sha256: &str, part_id: &str) -> Result<Geometry> {
    let (vector, canonical) = source(bytes)?;
    if vector.source_sha256 != sha256 {
        return Err(invalid(
            "SVG snapshot hash has changed; inspect paths again",
        ));
    }
    let (tree, parts) = path_tree(&canonical, sha256)?;
    let part = parts
        .iter()
        .find(|p| p.part_id == part_id)
        .ok_or_else(|| invalid("unknown path ID"))?;
    if !part.geometry_supported {
        return Err(invalid(
            "path geometry is unsupported or exceeds 512 points",
        ));
    }
    let usvg::Node::Path(path) = tree
        .node_by_id(part_id)
        .ok_or_else(|| invalid("path unavailable"))?
    else {
        return Err(invalid("target is not a path"));
    };
    let mut points = Vec::new();
    let mut commands = Vec::new();
    for (command, values) in segments(path) {
        let mut indices = Vec::new();
        for (p, kind) in values {
            let index = points.len();
            indices.push(index);
            points.push(Handle {
                index,
                kind,
                position: root(path.abs_transform(), p),
            });
        }
        commands.push(Command {
            command,
            points: indices,
        });
    }
    let mut annotated = String::from_utf8(canonical).map_err(|e| invalid(e.to_string()))?;
    let end = annotated
        .rfind("</svg>")
        .ok_or_else(|| invalid("invalid SVG root"))?;
    let size = (f64::from(vector.width.max(vector.height)) / 60.).max(2.);
    let mut overlay = String::new();
    let mut anchor = None;
    for command in &commands {
        if matches!(command.command, "Q" | "C") {
            let positions = anchor
                .into_iter()
                .chain(command.points.iter().map(|&i| points[i].position))
                .map(|p| format!("{},{}", p[0], p[1]))
                .collect::<Vec<_>>()
                .join(" ");
            overlay.push_str(&format!(
                "<polyline points='{positions}' fill='none' stroke='#8b5cf6' stroke-width='{}'/>",
                size / 12.
            ));
        }
        if let Some(&last) = command.points.last() {
            anchor = Some(points[last].position);
        }
    }
    for handle in &points {
        let [x, y] = handle.position;
        let color = if handle.kind == "control" {
            "#8b5cf6"
        } else {
            "#f97316"
        };
        overlay.push_str(&format!("<circle cx='{x}' cy='{y}' r='{}' fill='{color}'/><text x='{}' y='{}' font-size='{size}' fill='#000000' stroke='#ffffff' stroke-width='{}' paint-order='stroke'>{}</text>",size/5.,x+size/4.,y-size/4.,size/8.,handle.index));
    }
    annotated.insert_str(end, &overlay);
    let preview = crate::vector::normalize(annotated.as_bytes())?;
    Ok(Geometry {
        sha256: sha256.into(),
        part_id: part_id.into(),
        width: vector.width,
        height: vector.height,
        points,
        commands,
        preview_data_url: vector.preview_data_url,
        annotated_data_url: preview.preview_data_url,
    })
}
pub(super) fn topology(path: &usvg::Path) -> Vec<&'static str> {
    segments(path)
        .into_iter()
        .map(|(command, _)| command)
        .collect()
}
pub(super) fn edit_path(path: &usvg::Path, edits: &[PointEdit]) -> Result<String> {
    if edits.is_empty() || edits.len() > 64 {
        return Err(invalid("requires 1..64 distinct point edits"));
    }
    let t = path.abs_transform();
    if t.invert().is_none() {
        return Err(invalid("path transform is not invertible"));
    }
    let (sx, sy, kx, ky, tx, ty) = (
        f64::from(t.sx),
        f64::from(t.sy),
        f64::from(t.kx),
        f64::from(t.ky),
        f64::from(t.tx),
        f64::from(t.ty),
    );
    let mut changes = BTreeMap::new();
    for edit in edits {
        if edit
            .position
            .iter()
            .any(|v| !v.is_finite() || v.abs() > 1e6)
        {
            return Err(invalid(
                "point positions must be finite and within ±1000000",
            ));
        }
        if changes.insert(edit.index, edit.position).is_some() {
            return Err(invalid("duplicate point index"));
        }
    }
    let mut data = String::new();
    let mut index = 0;
    for (command, points) in segments(path) {
        data.push_str(command);
        for (point, _) in points {
            let local = if let Some([x, y]) = changes.remove(&index) {
                let (dx, dy) = (x - tx, y - ty);
                let det = sx * sy - kx * ky;
                [(sy * dx - kx * dy) / det, (-ky * dx + sx * dy) / det]
            } else {
                [f64::from(point.x), f64::from(point.y)]
            };
            if local.iter().any(|v| !v.is_finite() || v.abs() > 1e9) {
                return Err(invalid(
                    "point edit is ill-conditioned in local coordinates",
                ));
            }
            data.push_str(&format!(" {} {}", local[0], local[1]));
            index += 1;
        }
        data.push(' ');
    }
    if !changes.is_empty() {
        return Err(invalid("unknown point index; read geometry first"));
    }
    Ok(data)
}
