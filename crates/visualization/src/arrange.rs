//! Deterministic arrangement of existing editable components. No relationships are inferred.
use crate::scene;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
#[derive(Debug, Clone, Copy, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Left,
    Center,
    Right,
    Top,
    Middle,
    Bottom,
    DistributeHorizontal,
    DistributeVertical,
}
#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    pub element_ids: Vec<String>,
    pub mode: Mode,
}
#[derive(Debug, thiserror::Error)]
#[error("cannot arrange selection: {0}")]
pub struct InvalidArrangement(pub &'static str);
type Result<T> = std::result::Result<T, InvalidArrangement>;
fn number(e: &Value, key: &str) -> Result<f64> {
    e[key]
        .as_f64()
        .filter(|v| v.is_finite() && v.abs() <= 1e6)
        .ok_or(InvalidArrangement("invalid or out-of-range geometry"))
}
fn delta(moves: &HashMap<String, [f64; 2]>, id: Option<&str>) -> [f64; 2] {
    id.and_then(|id| moves.get(id)).copied().unwrap_or([0., 0.])
}
fn points(e: &Value) -> Result<Vec<[f64; 2]>> {
    e["points"]
        .as_array()
        .filter(|points| !points.is_empty())
        .and_then(|points| {
            points
                .iter()
                .map(|p| {
                    let p = p.as_array().filter(|p| p.len() == 2)?;
                    Some([p[0].as_f64()?, p[1].as_f64()?])
                })
                .collect::<Option<Vec<_>>>()
        })
        .filter(|points| {
            points
                .iter()
                .flatten()
                .all(|n| n.is_finite() && n.abs() <= 1e6)
        })
        .ok_or(InvalidArrangement("invalid component point geometry"))
}
fn linear_points(e: &Value) -> Result<Vec<[f64; 2]>> {
    let points = points(e)?;
    if points.len() < 2
        || e["roughness"].as_f64() != Some(0.)
        || !e["roundness"].is_null()
        || e["elbowed"] == true
    {
        return Err(InvalidArrangement(
            "curved, rough or elbowed connector bounds need explicit native geometry",
        ));
    }
    Ok(points)
}
fn point_bounds(e: &Value, points: &[[f64; 2]]) -> Result<[f64; 4]> {
    let (x, y, a) = (number(e, "x")?, number(e, "y")?, number(e, "angle")?);
    let min = [
        points.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min),
        points.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min),
    ];
    let max = [
        points
            .iter()
            .map(|p| p[0])
            .fold(f64::NEG_INFINITY, f64::max),
        points
            .iter()
            .map(|p| p[1])
            .fold(f64::NEG_INFINITY, f64::max),
    ];
    let center = [(min[0] + max[0]) / 2., (min[1] + max[1]) / 2.];
    let (s, c) = a.sin_cos();
    let mut result = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    for p in points {
        let d = [p[0] - center[0], p[1] - center[1]];
        let world = [
            x + center[0] + c * d[0] - s * d[1],
            y + center[1] + s * d[0] + c * d[1],
        ];
        result[0] = result[0].min(world[0]);
        result[1] = result[1].min(world[1]);
        result[2] = result[2].max(world[0]);
        result[3] = result[3].max(world[1]);
    }
    Ok(result)
}
fn member_bounds(e: &Value, index: &HashMap<&str, &Value>) -> Result<[f64; 4]> {
    if e["type"] == "text" {
        if let Some(arrow) = e["containerId"]
            .as_str()
            .and_then(|id| index.get(id))
            .filter(|e| e["type"] == "arrow")
        {
            if number(arrow, "angle")? != 0. || number(e, "angle")? != 0. {
                return Err(InvalidArrangement(
                    "rotated arrow-bound text needs native geometry bounds",
                ));
            }
            let p = linear_points(arrow)?;
            let midpoint = center(&p, number(arrow, "x")?, number(arrow, "y")?);
            let (w, h) = (number(e, "width")?, number(e, "height")?);
            return Ok([
                midpoint[0] - w / 2.,
                midpoint[1] - h / 2.,
                midpoint[0] + w / 2.,
                midpoint[1] + h / 2.,
            ]);
        }
    }
    bounds(e)
}
fn bounds(e: &Value) -> Result<[f64; 4]> {
    match e["type"].as_str() {
        Some("freedraw") => return point_bounds(e, &points(e)?),
        Some("arrow" | "line") => return point_bounds(e, &linear_points(e)?),
        _ => {}
    }
    let (x, y, w, h, a) = (
        number(e, "x")?,
        number(e, "y")?,
        number(e, "width")?,
        number(e, "height")?,
        number(e, "angle")?,
    );
    if w < 0. || h < 0. {
        return Err(InvalidArrangement("negative component dimensions"));
    }
    let (s, c) = a.sin_cos();
    let center = [x + w / 2., y + h / 2.];
    let extent = match e["type"].as_str() {
        Some("ellipse") => [
            (w * c / 2.).hypot(h * s / 2.),
            (h * c / 2.).hypot(w * s / 2.),
        ],
        Some("diamond") => [
            (w * c / 2.).abs().max((h * s / 2.).abs()),
            (h * c / 2.).abs().max((w * s / 2.).abs()),
        ],
        Some("rectangle" | "image" | "text" | "frame" | "magicframe") => [
            (w * c).abs() / 2. + (h * s).abs() / 2.,
            (h * c).abs() / 2. + (w * s).abs() / 2.,
        ],
        _ => {
            return Err(InvalidArrangement(
                "unsupported component geometry; select native shapes, images, text, ink, straight connectors or frames",
            ));
        }
    };
    Ok([
        center[0] - extent[0],
        center[1] - extent[1],
        center[0] + extent[0],
        center[1] + extent[1],
    ])
}
struct Ownership {
    children: HashMap<String, Vec<String>>,
    owners: HashMap<String, Vec<String>>,
    groups: HashMap<String, Vec<String>>,
}
impl Ownership {
    fn new(elements: &[Value]) -> Self {
        let mut result = Self {
            children: HashMap::new(),
            owners: HashMap::new(),
            groups: HashMap::new(),
        };
        let index = elements
            .iter()
            .map(|e| (e["id"].as_str().unwrap(), e))
            .collect::<HashMap<_, _>>();
        let mut link = |parent: &str, child: &str, up: bool| {
            result
                .children
                .entry(parent.into())
                .or_default()
                .push(child.into());
            if up {
                result
                    .owners
                    .entry(child.into())
                    .or_default()
                    .push(parent.into());
            }
        };
        for e in elements {
            let id = e["id"].as_str().unwrap();
            if let Some(frame) = e["frameId"].as_str().filter(|id| {
                index.get(id).is_some_and(|parent| {
                    matches!(parent["type"].as_str(), Some("frame" | "magicframe"))
                })
            }) {
                link(frame, id, false);
            }
            if e["type"] == "text" {
                if let Some(parent) = e["containerId"]
                    .as_str()
                    .filter(|id| index.contains_key(id))
                {
                    link(parent, id, true);
                }
            }
            if e["type"] == "image" {
                if let Some(parent) = e["customData"]["semanticAnchorId"].as_str().filter(|id| {
                    index
                        .get(id)
                        .is_some_and(|p| p["customData"]["visualAssetElementId"] == e["id"])
                }) {
                    link(parent, id, true);
                }
            }
            if e["type"] == "image" {
                if let Some(parent) =
                    e["customData"]["semanticBadgeAnchorId"]
                        .as_str()
                        .filter(|id| {
                            index.get(id).is_some_and(|p| {
                                p["customData"]["visualBadgeIds"]
                                    .as_array()
                                    .is_some_and(|ids| ids.contains(&e["id"]))
                            })
                        })
                {
                    link(parent, id, true);
                }
            }
            if let Some(labels) = e["customData"]["visualLabelIds"].as_array() {
                for label in labels.iter().filter_map(Value::as_str) {
                    if index.get(label).is_some_and(|child| {
                        child["type"] == "text"
                            && e["customData"]["visualAssetGroupId"]
                                .as_str()
                                .is_some_and(|group| {
                                    child["groupIds"].as_array().is_some_and(|g| {
                                        g.iter().any(|id| id.as_str() == Some(group))
                                    })
                                })
                    }) {
                        link(id, label, true);
                    }
                }
            }
        }
        for e in elements {
            for group in e["groupIds"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                result
                    .groups
                    .entry(group.into())
                    .or_default()
                    .push(e["id"].as_str().unwrap().into());
            }
        }
        result
    }
    fn expand(&self, index: &HashMap<&str, &Value>, id: &str) -> HashSet<String> {
        let mut set = HashSet::new();
        let mut pending = vec![id.to_owned()];
        let mut groups = HashSet::new();
        while let Some(id) = pending.pop() {
            if !set.insert(id.clone()) {
                continue;
            }
            if let Some(children) = self.children.get(&id) {
                pending.extend(children.iter().cloned());
            }
            if let Some(parents) = self.owners.get(&id) {
                pending.extend(parents.iter().cloned());
            }
            if let Some(group) = index[id.as_str()]["groupIds"]
                .as_array()
                .and_then(|g| g.last())
                .and_then(Value::as_str)
                .filter(|group| !group.is_empty())
            {
                if groups.insert(group) {
                    pending.extend(self.groups.get(group).into_iter().flatten().cloned());
                }
            }
        }
        set
    }
}

fn center(points: &[[f64; 2]], x: f64, y: f64) -> [f64; 2] {
    let n = points.len() / 2;
    let p = if points.len().is_multiple_of(2) {
        [
            (points[n - 1][0] + points[n][0]) / 2.,
            (points[n - 1][1] + points[n][1]) / 2.,
        ]
    } else {
        points[n]
    };
    [x + p[0], y + p[1]]
}
fn moved(e: &Value, d: [f64; 2]) -> Result<Value> {
    let (x, y) = (number(e, "x")? + d[0], number(e, "y")? + d[1]);
    if !x.is_finite() || !y.is_finite() || x.abs() > 1e6 || y.abs() > 1e6 {
        return Err(InvalidArrangement("destination outside canvas range"));
    }
    let mut result = e.clone();
    result["x"] = json!(x);
    result["y"] = json!(y);
    // This is derived canvas geometry, unlike the scientific metadata retained above.
    if let Some(bounds) = result
        .get_mut("customData")
        .and_then(|v| v.get_mut("visualAnchorFit"))
        .and_then(|v| v.get_mut("imageBounds"))
        .and_then(Value::as_array_mut)
    {
        if bounds.len() == 4
            && bounds
                .iter()
                .all(|v| v.as_f64().is_some_and(f64::is_finite))
        {
            bounds[0] = json!(bounds[0].as_f64().unwrap() + d[0]);
            bounds[1] = json!(bounds[1].as_f64().unwrap() + d[1]);
        }
    }
    if let Some(fit) = result
        .get_mut("customData")
        .and_then(|d| d.get_mut("visualAnchorFit"))
    {
        crate::illustration_fit::translate_group_record(fit, d[0], d[1]);
    }
    scene::touch(&mut result);
    Ok(result)
}
#[derive(Clone, Copy)]
enum Transform {
    Arrange(Mode),
    PackFrames { axis: usize, gap: f64 },
    Translate([f64; 2]),
    SwapFrames([[f64; 2]; 2]),
}
pub fn compile(document: &Value, spec: &Spec) -> Result<Value> {
    compile_transform(document, &spec.element_ids, Transform::Arrange(spec.mode))
}
/// Pack explicitly selected top-level frames in their current spatial order.
/// The first frame stays fixed; the orthogonal coordinate is preserved.
pub fn pack_frames(document: &Value, ids: &[String], vertical: bool, gap: f64) -> Result<Value> {
    if !gap.is_finite() || !(8. ..=512.).contains(&gap) {
        return Err(InvalidArrangement("frame gap must be finite within 8..512"));
    }
    let elements = document["elements"]
        .as_array()
        .ok_or(InvalidArrangement("invalid scene"))?;
    for id in ids {
        let frame = elements
            .iter()
            .find(|e| e["id"] == *id && e["isDeleted"] != true)
            .ok_or(InvalidArrangement("selected frame is missing"))?;
        if frame["type"] != "frame"
            || frame["locked"] == true
            || frame["angle"].as_f64().unwrap_or(0.) != 0.
            || !frame["frameId"].is_null()
        {
            return Err(InvalidArrangement(
                "packing requires unlocked unrotated top-level frames",
            ));
        }
    }
    compile_transform(
        document,
        ids,
        Transform::PackFrames {
            axis: usize::from(vertical),
            gap,
        },
    )
}
/// Move existing complete components once, without copying or changing IDs.
pub fn translate(document: &Value, element_ids: &[String], dx: f64, dy: f64) -> Result<Value> {
    if [dx, dy].iter().any(|v| !v.is_finite() || v.abs() > 1e6) {
        return Err(InvalidArrangement(
            "translation requires finite dx/dy within canvas range",
        ));
    }
    compile_transform(document, element_ids, Transform::Translate([dx, dy]))
}
/// Atomically exchange two top-level frames and rigidly move their contents.
/// Generated sharp cross-panel connectors are rerouted after both moves.
pub fn swap_frames(document: &Value, first: &str, second: &str) -> Result<Value> {
    if first == second {
        return Err(InvalidArrangement("select two distinct frames"));
    }
    let elements = document["elements"]
        .as_array()
        .ok_or(InvalidArrangement("invalid scene"))?;
    let mut origins = [[0.; 2]; 2];
    for (position, id) in [first, second].into_iter().enumerate() {
        let frame = elements
            .iter()
            .find(|e| e["id"] == id && e["isDeleted"] != true)
            .ok_or(InvalidArrangement("selected frame is missing"))?;
        if frame["type"] != "frame"
            || frame["locked"] == true
            || frame["angle"].as_f64().unwrap_or(0.) != 0.
            || !frame["frameId"].is_null()
        {
            return Err(InvalidArrangement(
                "swap_frames requires unlocked unrotated top-level frames",
            ));
        }
        origins[position] = [number(frame, "x")?, number(frame, "y")?];
    }
    compile_transform(
        document,
        &[first.into(), second.into()],
        Transform::SwapFrames(origins),
    )
}
fn compile_transform(
    document: &Value,
    element_ids: &[String],
    transform: Transform,
) -> Result<Value> {
    scene::apply(document, &[]).map_err(|_| InvalidArrangement("invalid scene"))?;
    if !(if matches!(transform, Transform::Translate(_)) {
        1..=100
    } else {
        2..=100
    })
    .contains(&element_ids.len())
        || element_ids.iter().collect::<HashSet<_>>().len() != element_ids.len()
    {
        return Err(InvalidArrangement(
            "select distinct existing IDs (1..100 for translation, 2..100 for arrangement)",
        ));
    }
    let elements = document["elements"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["isDeleted"] != true)
        .cloned()
        .collect::<Vec<_>>();
    let index = elements
        .iter()
        .map(|e| (e["id"].as_str().unwrap(), e))
        .collect::<HashMap<_, _>>();
    let ownership = Ownership::new(&elements);
    let mut units: Vec<HashSet<String>> = Vec::new();
    for id in element_ids {
        if !index.contains_key(id.as_str()) {
            return Err(InvalidArrangement("selected object is missing"));
        }
        let mut unit = ownership.expand(&index, id);
        let mut i = 0;
        while i < units.len() {
            if !unit.is_disjoint(&units[i]) {
                unit.extend(units.remove(i));
                i = 0;
            } else {
                i += 1;
            }
        }
        units.push(unit);
    }
    if matches!(transform, Transform::Arrange(_)) && units.len() < 2 {
        return Err(InvalidArrangement(
            "selection collapses to fewer than two independent components",
        ));
    }
    let mut boxes = Vec::new();
    for unit in &units {
        let mut bbox = [
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ];
        for id in unit {
            let e = index[id.as_str()];
            if e["locked"] == true {
                return Err(InvalidArrangement(
                    "selection contains a locked component or owned member",
                ));
            }
            if matches!(
                e["customData"]["annotationKind"].as_str(),
                Some("inset_region" | "inset_callout")
            ) {
                return Err(InvalidArrangement(
                    "inset region/callout annotations must not be arranged as scientific components",
                ));
            }
            if e["frameId"].as_str().is_some_and(|frame| {
                unit.contains(frame)
                    && index.get(frame).is_some_and(|parent| {
                        matches!(parent["type"].as_str(), Some("frame" | "magicframe"))
                    })
            }) {
                continue;
            }
            let b = if matches!(
                transform,
                Transform::Translate(_) | Transform::SwapFrames(_) | Transform::PackFrames { .. }
            ) {
                // Rigid motion needs no linear connector bounds or arrow-label
                // midpoint reflow. Preserve rough/curved native point geometry.
                if matches!(e["type"].as_str(), Some("arrow" | "line" | "freedraw")) {
                    point_bounds(e, &points(e)?)?
                } else {
                    bounds(e)?
                }
            } else {
                member_bounds(e, &index)?
            };
            bbox[0] = bbox[0].min(b[0]);
            bbox[1] = bbox[1].min(b[1]);
            bbox[2] = bbox[2].max(b[2]);
            bbox[3] = bbox[3].max(b[3]);
        }
        if bbox.iter().any(|v| !v.is_finite()) {
            return Err(InvalidArrangement("component has no usable bounds"));
        }
        boxes.push(bbox);
    }
    let offsets = match transform {
        Transform::Translate(offset) => vec![offset; units.len()],
        Transform::Arrange(mode) => arrangement_offsets(&boxes, mode)?,
        Transform::PackFrames { axis, gap } => {
            if units.len() != element_ids.len() {
                return Err(InvalidArrangement(
                    "frames must be independent rigid components",
                ));
            }
            let mut order = (0..boxes.len()).collect::<Vec<_>>();
            order.sort_by(|a, b| boxes[*a][axis].total_cmp(&boxes[*b][axis]).then(a.cmp(b)));
            let mut offsets = vec![[0.; 2]; boxes.len()];
            let mut cursor = boxes[order[0]][axis];
            for i in order {
                offsets[i][axis] = cursor - boxes[i][axis];
                cursor += boxes[i][axis + 2] - boxes[i][axis] + gap;
            }
            offsets
        }
        Transform::SwapFrames(origins) => {
            if units.len() != 2 {
                return Err(InvalidArrangement(
                    "frames must be independent rigid components",
                ));
            }
            let d = [origins[1][0] - origins[0][0], origins[1][1] - origins[0][1]];
            vec![d, [-d[0], -d[1]]]
        }
    };
    let mut moves = HashMap::new();
    for (unit, d) in units.iter().zip(offsets) {
        for id in unit {
            moves.insert(id.clone(), d);
        }
    }
    // A selected component is translated rigidly. Its own bound connectors must
    // not stretch to independently moved objects, which would invalidate its bounds.
    for arrow in elements
        .iter()
        .filter(|e| e["type"] == "arrow" && moves.contains_key(e["id"].as_str().unwrap()))
    {
        let own = delta(&moves, arrow["id"].as_str());
        for field in ["startBinding", "endBinding"] {
            if let Some(id) = arrow[field]["elementId"].as_str() {
                if delta(&moves, Some(id)) != own {
                    return Err(InvalidArrangement(
                        "a connector inside the selected component would deform; keep its endpoints in the same rigid component or exclude the connector",
                    ));
                }
            }
        }
    }
    let mut replacements = HashMap::new();
    for e in &elements {
        let d = delta(&moves, e["id"].as_str());
        if d != [0., 0.] {
            replacements.insert(e["id"].as_str().unwrap().to_owned(), moved(e, d)?);
        }
    }
    let mut reroutes = Vec::new();
    for arrow in elements.iter().filter(|e| e["type"] == "arrow") {
        let own = delta(&moves, arrow["id"].as_str());
        let start = arrow["startBinding"]["elementId"]
            .as_str()
            .map(|id| delta(&moves, Some(id)))
            .unwrap_or(own);
        let end = arrow["endBinding"]["elementId"]
            .as_str()
            .map(|id| delta(&moves, Some(id)))
            .unwrap_or(own);
        if own == [0., 0.] && start == own && end == own {
            continue;
        }
        if arrow["locked"] == true {
            return Err(InvalidArrangement("a locked connector would need to move"));
        }
        let mut points = arrow["points"]
            .as_array()
            .and_then(|points| {
                points
                    .iter()
                    .map(|p| Some([p[0].as_f64()?, p[1].as_f64()?]))
                    .collect::<Option<Vec<_>>>()
            })
            .filter(|p| p.len() >= 2 && p.iter().flatten().all(|n| n.is_finite() && n.abs() <= 1e6))
            .ok_or(InvalidArrangement("invalid connected arrow points"))?;
        if matches!(
            transform,
            Transform::SwapFrames(_)
                | Transform::Arrange(_)
                | Transform::Translate(_)
                | Transform::PackFrames { .. }
        ) && start != end
            && arrow["customData"]["crossPanel"] == true
            && number(arrow, "angle")? == 0.
            && arrow["elbowed"] != true
            && arrow["roundness"].is_null()
            && arrow["roughness"].as_f64() == Some(0.)
        {
            if elements.iter().any(|e| {
                e["type"] == "text" && e["containerId"] == arrow["id"] && e["locked"] == true
            }) {
                return Err(InvalidArrangement(
                    "a locked connector label would need to move",
                ));
            }
            reroutes.push(arrow["id"].as_str().unwrap().to_owned());
            continue;
        }
        // Fitting an illustration can turn a two-point arrow into a sharp
        // orthogonal route with short endpoint stubs. Reuse the same guarded
        // port rebinding when just one component (or two different deltas) moves.
        if own == [0., 0.]
            && start != end
            && points.len() > 2
            && arrow["customData"]["crossPanel"] != true
            && number(arrow, "angle")? == 0.
            && arrow["elbowed"] != true
            && arrow["roundness"].is_null()
            && arrow["roughness"].as_f64() == Some(0.)
            && points
                .windows(2)
                .all(|p| (p[0][0] - p[1][0]).abs() <= 1e-6 || (p[0][1] - p[1][1]).abs() <= 1e-6)
        {
            let mut connected = elements
                .iter()
                .filter(|e| {
                    e["id"] == arrow["id"]
                        || e["type"] == "frame"
                        || (e["type"] == "text" && e["containerId"] == arrow["id"])
                })
                .cloned()
                .collect::<Vec<_>>();
            for (field, movement) in [("startBinding", start), ("endBinding", end)] {
                if movement == [0., 0.] {
                    continue;
                }
                let anchor_id = arrow[field]["elementId"]
                    .as_str()
                    .ok_or(InvalidArrangement("missing moved connector anchor"))?;
                let anchor = elements
                    .iter()
                    .find(|e| e["id"] == anchor_id)
                    .ok_or(InvalidArrangement("missing moved connector anchor"))?;
                let old = [
                    number(anchor, "x")?,
                    number(anchor, "y")?,
                    number(anchor, "width")?,
                    number(anchor, "height")?,
                ];
                let next = [old[0] + movement[0], old[1] + movement[1], old[2], old[3]];
                crate::illustration_fit::adjust_connected(&mut connected,anchor_id,old,next).map_err(|_|InvalidArrangement("sharp orthogonal connector cannot follow these moves; check centered ports, endpoint gaps, locks and frame bounds"))?;
            }
            for changed in connected.into_iter().filter(|e| e["type"] != "frame") {
                let id = changed["id"]
                    .as_str()
                    .ok_or(InvalidArrangement("missing connector member identity"))?
                    .to_owned();
                let original = elements.iter().find(|e| e["id"] == id).unwrap();
                if &changed == original {
                    replacements.remove(&id);
                } else {
                    replacements.insert(id, changed);
                }
            }
            continue;
        }
        let complex = number(arrow, "angle")? != 0.
            || arrow["elbowed"] == true
            || !arrow["roundness"].is_null()
            || points.len() != 2;
        let next_arrow;
        let label_delta;
        if complex {
            if start != end || (moves.contains_key(arrow["id"].as_str().unwrap()) && own != start) {
                return Err(InvalidArrangement(
                    "a curved, rotated or elbowed connector requires explicit native geometry before arranging these components",
                ));
            }
            next_arrow = moved(arrow, start)?;
            label_delta = start;
        } else {
            let before = center(&points, number(arrow, "x")?, number(arrow, "y")?);
            points[0][0] += start[0] - own[0];
            points[0][1] += start[1] - own[1];
            let last = points.len() - 1;
            points[last][0] += end[0] - own[0];
            points[last][1] += end[1] - own[1];
            let first = points[0];
            let x = number(arrow, "x")? + own[0] + first[0];
            let y = number(arrow, "y")? + own[1] + first[1];
            for p in &mut points {
                p[0] -= first[0];
                p[1] -= first[1];
            }
            if [x, y].iter().any(|v| !v.is_finite() || v.abs() > 1e6)
                || points
                    .iter()
                    .flatten()
                    .any(|v| !v.is_finite() || v.abs() > 1e6)
            {
                return Err(InvalidArrangement(
                    "connected arrow destination outside canvas range",
                ));
            }
            let after = center(&points, x, y);
            label_delta = [after[0] - before[0], after[1] - before[1]];
            let mut next = arrow.clone();
            next["x"] = json!(x);
            next["y"] = json!(y);
            next["width"] = json!((points[last][0] - points[0][0]).abs());
            next["height"] = json!((points[last][1] - points[0][1]).abs());
            next["points"] = json!(points);
            scene::touch(&mut next);
            next_arrow = next;
        }
        replacements.insert(arrow["id"].as_str().unwrap().to_owned(), next_arrow);
        for label in elements
            .iter()
            .filter(|e| e["type"] == "text" && e["containerId"] == arrow["id"])
        {
            if label_delta != [0., 0.] && label["locked"] == true {
                return Err(InvalidArrangement(
                    "a locked connector label would need to move",
                ));
            }
            if label_delta == [0., 0.] {
                replacements.remove(label["id"].as_str().unwrap());
            } else {
                replacements.insert(
                    label["id"].as_str().unwrap().to_owned(),
                    moved(label, label_delta)?,
                );
            }
        }
    }
    let mut next = document.clone();
    for e in next["elements"].as_array_mut().unwrap() {
        if let Some(value) = replacements.remove(e["id"].as_str().unwrap()) {
            *e = value;
        }
    }
    if !reroutes.is_empty() {
        // Port occupancy is order-dependent. Compare two bounded deterministic
        // orders on independent clones, retaining only a fully valid result.
        let mut best = None;
        let mut best_cost = f64::INFINITY;
        for reverse in [false, true] {
            let mut candidate = next.clone();
            let mut order = reroutes.clone();
            if reverse {
                order.reverse();
            }
            if order.iter().any(|id| {
                crate::panel_routes::reroute(candidate["elements"].as_array_mut().unwrap(), id)
                    .is_err()
            }) {
                continue;
            }
            let cost: f64 = candidate["elements"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|e| reroutes.iter().any(|id| e["id"] == *id))
                .map(|e| {
                    e["points"]
                        .as_array()
                        .unwrap()
                        .windows(2)
                        .map(|p| {
                            (p[1][0].as_f64().unwrap() - p[0][0].as_f64().unwrap()).abs()
                                + (p[1][1].as_f64().unwrap() - p[0][1].as_f64().unwrap()).abs()
                        })
                        .sum::<f64>()
                })
                .sum();
            if cost < best_cost {
                best_cost = cost;
                best = Some(candidate);
            }
        }
        return best.ok_or(InvalidArrangement(
            "cannot reroute cross-panel connectors after layout; nothing changed",
        ));
    }
    Ok(next)
}

fn arrangement_offsets(boxes: &[[f64; 4]], mode: Mode) -> Result<Vec<[f64; 2]>> {
    let axis = if matches!(
        mode,
        Mode::Top | Mode::Middle | Mode::Bottom | Mode::DistributeVertical
    ) {
        1
    } else {
        0
    };
    let start = boxes.iter().map(|b| b[axis]).fold(f64::INFINITY, f64::min);
    let end = boxes
        .iter()
        .map(|b| b[axis + 2])
        .fold(f64::NEG_INFINITY, f64::max);
    let mut offsets = vec![[0., 0.]; boxes.len()];
    if matches!(mode, Mode::DistributeHorizontal | Mode::DistributeVertical) {
        if boxes.len() < 3 {
            return Err(InvalidArrangement(
                "distribution needs at least three independent components",
            ));
        }
        let mut order = (0..boxes.len()).collect::<Vec<_>>();
        order.sort_by(|a, b| {
            ((boxes[*a][axis] + boxes[*a][axis + 2]) / 2.)
                .total_cmp(&((boxes[*b][axis] + boxes[*b][axis + 2]) / 2.))
        });
        let total = boxes.iter().map(|b| b[axis + 2] - b[axis]).sum::<f64>();
        let gap = (end - start - total) / (boxes.len() - 1) as f64;
        if gap < 0. {
            // Match the editor's fallback: anchor the outer-boundary components,
            // distribute remaining centers, rather than invent negative edge gaps.
            let first = order.iter().position(|i| boxes[*i][axis] == start).unwrap();
            let last = order
                .iter()
                .position(|i| boxes[*i][axis + 2] == end)
                .unwrap();
            let middle = |i: usize| (boxes[order[i]][axis] + boxes[order[i]][axis + 2]) / 2.;
            let step = (middle(last) - middle(first)) / (boxes.len() - 1) as f64;
            let mut cursor = middle(first);
            for (position, i) in order.into_iter().enumerate() {
                if position != first && position != last {
                    cursor += step;
                    offsets[i][axis] = cursor - (boxes[i][axis] + boxes[i][axis + 2]) / 2.;
                }
            }
        } else {
            let mut cursor = start;
            for i in order {
                offsets[i][axis] = cursor - boxes[i][axis];
                cursor += boxes[i][axis + 2] - boxes[i][axis] + gap;
            }
        }
    } else {
        for (i, b) in boxes.iter().enumerate() {
            offsets[i][axis] = match mode {
                Mode::Left | Mode::Top => start - b[axis],
                Mode::Right | Mode::Bottom => end - b[axis + 2],
                _ => (start + end - b[axis] - b[axis + 2]) / 2.,
            };
        }
    }
    Ok(offsets)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn shape(id: &str, x: f64, y: f64, w: f64, h: f64) -> Value {
        json!({"id":id,"type":"rectangle","x":x,"y":y,"width":w,"height":h,"angle":0,"groupIds":[],"locked":false,"customData":{"note":"keep"}})
    }
    fn find<'a>(doc: &'a Value, id: &str) -> &'a Value {
        doc["elements"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["id"] == id)
            .unwrap()
    }
    fn spec(mode: Mode, ids: &[&str]) -> Spec {
        Spec {
            mode,
            element_ids: ids.iter().map(|id| (*id).into()).collect(),
        }
    }
    fn fixture() -> Value {
        let mut a = shape("a", 0., 100., 50., 30.);
        a["groupIds"] = json!(["g"]);
        a["customData"]["visualAssetElementId"] = json!("image");
        a["customData"]["visualAssetGroupId"] = json!("g");
        a["customData"]["visualLabelIds"] = json!(["title"]);
        let mut image = shape("image", 5., 105., 40., 20.);
        image["type"] = json!("image");
        image["fileId"] = json!("shared");
        image["groupIds"] = json!(["g"]);
        image["customData"] = json!({"semanticAnchorId":"a","asset":{"license":"cc0"}});
        let mut title = shape("title", 0., 135., 50., 20.);
        title["type"] = json!("text");
        title["text"] = json!("A");
        title["groupIds"] = json!(["g"]);
        let mut arrow = shape("arrow", 58., 115., 134., 0.);
        arrow["type"] = json!("arrow");
        arrow["points"] = json!([[0., 0.], [134., 0.]]);
        arrow["startBinding"] = json!({"elementId":"a","focus":0,"gap":8});
        arrow["endBinding"] = json!({"elementId":"b","focus":0,"gap":8});
        arrow["elbowed"] = json!(false);
        arrow["roundness"] = Value::Null;
        let mut label = shape("arrow-label", 110., 105., 30., 20.);
        label["type"] = json!("text");
        label["containerId"] = json!("arrow");
        label["text"] = json!("explicit");
        json!({"elements":[a,image,title,shape("b",200.,100.,50.,30.),shape("c",500.,0.,50.,30.),arrow,label,shape("unrelated",900.,900.,50.,50.)],"files":{"shared":{"dataURL":"immutable original","mimeType":"image/svg+xml"}}})
    }
    #[test]
    fn top_alignment_moves_groups_owned_assets_and_connections_exactly_once() {
        let original = fixture();
        let next = compile(&original, &spec(Mode::Top, &["a", "image", "b", "c"])).unwrap();
        for id in ["a", "b", "c"] {
            assert_eq!(find(&next, id)["y"], 0.);
        }
        assert_eq!(find(&next, "image")["y"], 5.);
        assert_eq!(find(&next, "title")["y"], 35.);
        assert_eq!(find(&next, "arrow")["y"], 15.);
        assert_eq!(
            find(&next, "arrow")["points"],
            find(&original, "arrow")["points"]
        );
        assert_eq!(find(&next, "arrow-label")["y"], 5.);
        assert_eq!(
            find(&next, "image")["customData"],
            find(&original, "image")["customData"]
        );
        assert_eq!(find(&next, "unrelated"), find(&original, "unrelated"));
        assert_eq!(next["files"], original["files"]);
    }
    #[test]
    fn distribution_preserves_outer_edges_and_updates_only_bound_endpoint() {
        let original = fixture();
        let next = compile(
            &original,
            &spec(Mode::DistributeHorizontal, &["a", "b", "c"]),
        )
        .unwrap();
        assert_eq!(find(&next, "a"), find(&original, "a"));
        assert_eq!(find(&next, "c"), find(&original, "c"));
        assert_eq!(find(&next, "b")["x"], 250.);
        assert_eq!(
            find(&next, "arrow")["points"],
            json!([[0., 0.], [184., 0.]])
        );
        assert_eq!(find(&next, "arrow-label")["x"], 135.);
        let crowded = json!({"elements":[shape("wide",0.,0.,1000.,20.),shape("small-a",20.,0.,10.,20.),shape("small-b",40.,0.,10.,20.)]});
        let distributed = compile(
            &crowded,
            &spec(Mode::DistributeHorizontal, &["wide", "small-a", "small-b"]),
        )
        .unwrap();
        assert_eq!(find(&distributed, "wide"), find(&crowded, "wide"));
        assert_eq!(find(&distributed, "small-a")["x"], 495.);
        assert_eq!(find(&distributed, "small-b")["x"], 495.);
    }
    #[test]
    fn rotated_ellipse_diamond_and_frame_bounds_follow_native_geometry() {
        let mut ellipse = shape("ellipse", 100., 100., 80., 40.);
        ellipse["type"] = json!("ellipse");
        ellipse["angle"] = json!(0.7);
        let b = bounds(&ellipse).unwrap();
        let expected = (40. * 0.7_f64.cos()).hypot(20. * 0.7_f64.sin());
        assert!((b[0] - (140. - expected)).abs() < 1e-9);
        let mut diamond = ellipse.clone();
        diamond["type"] = json!("diamond");
        assert!(
            (bounds(&diamond).unwrap()[0]
                - (140. - (40. * 0.7_f64.cos()).abs().max((20. * 0.7_f64.sin()).abs())))
            .abs()
                < 1e-9
        );
        let mut frame = shape("frame", 0., 100., 300., 200.);
        frame["type"] = json!("frame");
        let mut child = shape("child", 20., 120., 50., 40.);
        child["frameId"] = json!("frame");
        let doc = json!({"elements":[frame,child,shape("other",500.,0.,100.,100.)]});
        let next = compile(&doc, &spec(Mode::Top, &["frame", "child", "other"])).unwrap();
        assert_eq!(find(&next, "frame")["y"], 0.);
        assert_eq!(find(&next, "child")["y"], 20.);
    }
    #[test]
    fn locked_ambiguous_or_unsupported_geometry_fails_without_mutation() {
        let original = fixture();
        assert!(compile(&original, &spec(Mode::Top, &["a", "image"])).is_err());
        assert!(compile(&original, &spec(Mode::Top, &["a", "missing"])).is_err());
        assert!(compile(&original, &spec(Mode::Top, &["a", "a"])).is_err());
        let mut locked = original.clone();
        locked["elements"][1]["locked"] = json!(true);
        assert!(compile(&locked, &spec(Mode::Top, &["a", "c"])).is_err());
        let mut complex = original.clone();
        complex["elements"][5]["elbowed"] = json!(true);
        assert!(compile(&complex, &spec(Mode::Top, &["a", "c"])).is_err());
        // Both endpoints of a complex connector can translate rigidly together.
        assert!(compile(&complex, &spec(Mode::Top, &["a", "b", "c"])).is_ok());
        let mut annotation = original.clone();
        annotation["elements"][4]["customData"]["annotationKind"] = json!("inset_region");
        assert!(compile(&annotation, &spec(Mode::Top, &["a", "c"])).is_err());
        assert_eq!(original, fixture());
    }
    #[test]
    fn all_alignment_modes_and_vertical_distribution_match_their_axes() {
        let doc = json!({"elements":[shape("a",10.,70.,20.,30.),shape("b",150.,200.,70.,10.),shape("c",330.,0.,30.,50.)]});
        for (mode, axis, edge) in [
            (Mode::Left, 0, 0),
            (Mode::Center, 0, 1),
            (Mode::Right, 0, 2),
            (Mode::Top, 1, 0),
            (Mode::Middle, 1, 1),
            (Mode::Bottom, 1, 2),
        ] {
            let next = compile(&doc, &spec(mode, &["a", "b", "c"])).unwrap();
            let b = next["elements"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| bounds(e).unwrap())
                .collect::<Vec<_>>();
            let coordinate = |r: [f64; 4]| match edge {
                0 => r[axis],
                2 => r[axis + 2],
                _ => (r[axis] + r[axis + 2]) / 2.,
            };
            assert!(
                b.iter()
                    .all(|r| (coordinate(*r) - coordinate(b[0])).abs() < 1e-9),
                "{mode:?}"
            );
        }
        let next = compile(&doc, &spec(Mode::DistributeVertical, &["a", "b", "c"])).unwrap();
        assert_eq!(find(&next, "a")["y"], 110.);
        assert_eq!(find(&next, "b"), find(&doc, "b"));
        assert_eq!(find(&next, "c"), find(&doc, "c"));
        let mut empty_groups = doc.clone();
        for e in empty_groups["elements"].as_array_mut().unwrap() {
            e["groupIds"] = json!([""]);
        }
        let next = compile(&empty_groups, &spec(Mode::Left, &["a", "b"])).unwrap();
        assert_eq!(find(&next, "c"), find(&empty_groups, "c"));
    }
    #[test]
    fn compound_connectors_labels_and_rotated_ink_translate_without_deformation() {
        let mut a = shape("a", 20., 40., 40., 40.);
        a["groupIds"] = json!(["compound"]);
        let mut b = shape("b", 140., 40., 40., 40.);
        b["groupIds"] = json!(["compound"]);
        let mut arrow = shape("edge", 68., 60., 64., 25.);
        arrow["type"] = json!("arrow");
        arrow["points"] = json!([[0., 0.], [32., -25.], [64., 0.]]);
        arrow["roughness"] = json!(0);
        arrow["roundness"] = Value::Null;
        arrow["elbowed"] = json!(false);
        arrow["groupIds"] = json!(["compound"]);
        arrow["startBinding"] = json!({"elementId":"a"});
        arrow["endBinding"] = json!({"elementId":"b"});
        let mut label = shape("label", 1000., 1000., 200., 20.);
        label["type"] = json!("text");
        label["containerId"] = json!("edge");
        label["groupIds"] = json!(["compound"]);
        let mut ink = shape("ink", 80., 100., 70., 40.);
        ink["type"] = json!("freedraw");
        ink["points"] = json!([[-10., 0.], [0., 20.], [40., -10.], [60., 30.]]);
        ink["pressures"] = json!([0.5, 0.7, 0.6, 0.5]);
        ink["angle"] = json!(0.4);
        ink["groupIds"] = json!(["compound"]);
        let doc = json!({"elements":[a,b,arrow,label,ink,shape("other",400.,0.,100.,100.)]});
        let next = compile(&doc, &spec(Mode::Top, &["a", "ink", "other"])).unwrap();
        // Native bound-label position, not its stale imported x/y, defines the top.
        assert_eq!(find(&next, "a")["y"], 15.);
        assert_eq!(find(&next, "b")["y"], 15.);
        assert_eq!(find(&next, "edge")["y"], 35.);
        assert_eq!(find(&next, "ink")["y"], 75.);
        for id in ["edge", "ink"] {
            assert_eq!(find(&next, id)["points"], find(&doc, id)["points"]);
            assert_eq!(find(&next, id)["angle"], find(&doc, id)["angle"]);
            assert_eq!(find(&next, id)["customData"], find(&doc, id)["customData"]);
        }
        assert_eq!(
            find(&next, "ink")["pressures"],
            find(&doc, "ink")["pressures"]
        );
        let elements = next["elements"].as_array().unwrap();
        let index = elements
            .iter()
            .map(|e| (e["id"].as_str().unwrap(), e))
            .collect::<HashMap<_, _>>();
        assert_eq!(
            member_bounds(find(&next, "label"), &index).unwrap(),
            [0., 0., 200., 20.]
        );
        assert_eq!(find(&next, "other"), find(&doc, "other"));
        let mut rotated_label = doc.clone();
        rotated_label["elements"][2]["angle"] = json!(0.3);
        assert!(compile(&rotated_label, &spec(Mode::Top, &["a", "other"])).is_err());
        let mut rough = doc.clone();
        rough["elements"][2]["roughness"] = json!(1);
        assert!(compile(&rough, &spec(Mode::Top, &["a", "other"])).is_err());
        let mut independent = doc.clone();
        for e in independent["elements"].as_array_mut().unwrap() {
            e["groupIds"] = json!([]);
        }
        assert!(
            compile(&independent, &spec(Mode::Top, &["edge", "other"])).is_err(),
            "a selected bound connector cannot deform to stationary nodes"
        );
    }
    #[test]
    fn point_bounds_use_actual_points_and_reject_unavailable_native_geometry() {
        let mut ink = shape("ink", 100., 200., 1., 1.);
        ink["type"] = json!("freedraw");
        ink["points"] = json!([[-20., -10.], [40., 30.]]);
        ink["angle"] = json!(std::f64::consts::FRAC_PI_2);
        let b = bounds(&ink).unwrap();
        for (value, expected) in b.into_iter().zip([90., 180., 130., 240.]) {
            assert!((value - expected).abs() < 1e-8);
        }
        let mut line = ink.clone();
        line["type"] = json!("line");
        line["roughness"] = json!(0);
        line["roundness"] = Value::Null;
        assert_eq!(bounds(&line).unwrap(), bounds(&ink).unwrap());
        line["roughness"] = json!(1);
        assert!(bounds(&line).is_err());
        line["roughness"] = json!(0);
        line["roundness"] = json!({"type":2});
        assert!(bounds(&line).is_err());
        ink["points"] = json!([]);
        assert!(bounds(&ink).is_err());
        ink["points"] = json!([[0, null]]);
        assert!(bounds(&ink).is_err());
    }
}
