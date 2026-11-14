//! Engine-native scene edits used by agents and interactive clients.
//! Operations apply atomically to a clone; unrelated objects and image files survive.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

/// Native numeric fields must retain their JSON types in shallow updates.
pub const NUMERIC_UPDATE_FIELDS: [&str; 11] = [
    "x",
    "y",
    "width",
    "height",
    "angle",
    "fontSize",
    "fontFamily",
    "lineHeight",
    "strokeWidth",
    "roughness",
    "opacity",
];

// Crop uses native source-pixel units, not normalized SVG bounds. Validate
// only incoming writes so older documents can still be read and repaired.
fn validate_image_fields(fields: &serde_json::Map<String, Value>, id: &str) -> Result<(), SceneError> {
    if let Some(crop) = fields.get("crop").filter(|v| !v.is_null()) {
        let invalid = || SceneError::InvalidUpdate(format!("{id}.crop requires a complete native object with finite JSON numbers x/y/width/height/naturalWidth/naturalHeight in source pixels, not quoted numbers or normalized fractions without source dimensions; use null to clear crop"));
        let read = |key: &str| crop.get(key).and_then(Value::as_f64)
            .filter(|n| n.is_finite() && n.abs() <= 1e6).ok_or_else(invalid);
        let (x,y,w,h,nw,nh) = (read("x")?,read("y")?,read("width")?,read("height")?,read("naturalWidth")?,read("naturalHeight")?);
        if x<0. || y<0. || w<=0. || h<=0. || nw<=0. || nh<=0. || x+w>nw+1e-6 || y+h>nh+1e-6 {
            return Err(invalid());
        }
    }
    if let Some(scale) = fields.get("scale")
        && !scale.as_array().is_some_and(|a| a.len()==2 && a.iter().all(|v| matches!(v.as_f64(),Some(1.0)|Some(-1.0)))) {
        return Err(SceneError::InvalidUpdate(format!("{id}.scale requires two native numeric flips [1,1], [-1,1], [1,-1] or [-1,-1], not display size factors or quoted values")));
    }
    Ok(())
}

// Native linear dimensions describe the point bounds. Excalidraw recomputes
// them on restore, so an independent width edit cannot resize a connector.
fn update_path_dimensions(
    element: &mut Value,
    changes: &serde_json::Map<String, Value>,
) -> Result<(), SceneError> {
    if !matches!(
        element["type"].as_str(),
        Some("arrow" | "line" | "freedraw")
    ) || !["points", "width", "height"]
        .iter()
        .any(|key| changes.contains_key(*key))
    {
        return Ok(());
    }
    let id = element["id"].as_str().unwrap_or("unknown");
    let minimum = if element["type"] == "freedraw" { 1 } else { 2 };
    let invalid = || {
        SceneError::InvalidUpdate(format!(
            "{id}.points requires at least {minimum} finite numeric [x,y] pairs within ±1000000"
        ))
    };
    let points = element["points"]
        .as_array()
        .filter(|p| p.len() >= minimum)
        .ok_or_else(invalid)?;
    let mut min = [f64::INFINITY; 2];
    let mut max = [f64::NEG_INFINITY; 2];
    for point in points {
        let pair = point
            .as_array()
            .filter(|p| p.len() == 2)
            .ok_or_else(invalid)?;
        for axis in 0..2 {
            let value = pair[axis]
                .as_f64()
                .filter(|v| v.is_finite() && v.abs() <= 1e6)
                .ok_or_else(invalid)?;
            min[axis] = min[axis].min(value);
            max[axis] = max[axis].max(value);
        }
    }
    let dimensions = [max[0] - min[0], max[1] - min[1]];
    for (field, dimension) in ["width", "height"].into_iter().zip(dimensions) {
        if dimension > 1e6
            || changes
                .get(field)
                .is_some_and(|value| value.as_f64().is_none_or(|n| (n - dimension).abs() > 1e-6))
        {
            return Err(SceneError::InvalidUpdate(format!(
                "{id}.{field} is derived from the connector/stroke points (expected {dimension}); change the actual points to reroute/resize it, or reflow its label separately. Do not inflate path dimensions to widen a label: the GUI would reset them on restore"
            )));
        }
    }
    element["width"] = serde_json::json!(dimensions[0]);
    element["height"] = serde_json::json!(dimensions[1]);
    Ok(())
}

fn update_changes_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    let properties = NUMERIC_UPDATE_FIELDS
        .into_iter()
        .map(|field| {
            (
                field.to_owned(),
                serde_json::json!({"type":"number","minimum":-1000000,"maximum":1000000}),
            )
        })
        .collect();
    let mut object = serde_json::Map::new();
    object.insert("type".into(), serde_json::json!("object"));
    object.insert("properties".into(), Value::Object(properties));
    object.insert("additionalProperties".into(), Value::Bool(true));
    object.insert("description".into(),serde_json::json!("Shallow native edits. Numeric geometry and style fields require JSON numbers, never quoted numbers or null. Other native fields such as text and strokeColor remain available. IDs are immutable. Native reference collections boundElements/groupIds require JSON arrays (use [] for none), while containerId/frameId require null or nonempty IDs. customData illustration ownership, group, label, badge and fit fields must remain identical; use dedicated asset/illustration operations to change them, never generic update. Use set_wrapped_text for complete scientific text reflow."));
    object.into()
}

#[derive(schemars::JsonSchema, Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IllustrationMember {
    pub id: String,
    /// Explicit user/agent description, not an inferred biological identity.
    pub meaning: String,
}

#[derive(schemars::JsonSchema, Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IllustrationSample {
    pub id: String,
    pub file_id: String,
    pub normalized_bounds: [f64; 4],
}

#[derive(schemars::JsonSchema, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorPort { Left, Right, Top, Bottom }
impl ConnectorPort { pub(crate) fn index(&self)->usize {match self {Self::Left=>0,Self::Right=>1,Self::Top=>2,Self::Bottom=>3}} }

#[derive(schemars::JsonSchema, Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    /// Move/resize one native node with bound text reflow and coherent centered
    /// sharp connectors. Preserves frame, IDs, fonts, evidence and other nodes.
    SetNodeGeometry {
        id: String,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    },
    /// Append an explicit native graph at its top-left x/y, preserving existing
    /// objects/files/settings. Use a fresh prefix for collision-free identities.
    /// This does not replace the scene.
    InsertSketch {
        plan: crate::sketch::Plan,
        id_prefix: String,
        x: f64,
        y: f64,
    },
    /// Place/resize a standalone image without altering pixels, crop, flips or provenance.
    SetImageGeometry {
        id: String,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    },
    /// Uniformly resize around the image center, reusing its current reviewed fit.
    ScaleIllustration {
        id: String,
        file_id: String,
        factor: f64,
    },
    /// Resize/reposition artwork with explicit source bounds; labels keep their geometry.
    SetIllustrationGeometry {
        id: String,
        file_id: String,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        normalized_bounds: [f64; 4],
    },
    /// Fit a semantic anchor to explicitly reviewed bounds inside its illustration.
    FitIllustrationAnchor {
        id: String,
        file_id: String,
        normalized_bounds: [f64; 4],
    },
    /// Add complete native scientific-font text with validated geometry and wrapping.
    AddWrappedText {
        id: String,
        text: String,
        x: f64,
        y: f64,
        max_width: f64,
        /// Shrink a new left-aligned text box to its measured wrapped lines.
        /// Defaults false; preserves the requested width when omitted.
        #[serde(default)]
        fit_width: bool,
        #[serde(default)]
        font_size: Option<f64>,
        #[serde(default)]
        frame_id: Option<String>,
    },
    /// Reposition region/callout around frozen pixels, preserving the detail snapshot.
    SyncInsetRegion {
        id: String,
    },
    /// Reposition one bound caption by fractional length along the existing path.
    PositionArrowCaption { id: String, position: f64 },
    /// Fit a component to the union of exact current samples for every primary/accessory image.
    FitIllustrationGroup { anchor_id: String, samples: Vec<IllustrationSample> },
    /// Bind already placed images and detached labels to one component, preserving
    /// geometry/files. Explicit members use accessory ownership and meanings;
    /// old fit metadata is removed and the primary must be reviewed/refitted.
    BindIllustration {
        anchor_id: String,
        primary_id: String,
        label_ids: Vec<String>,
        #[serde(default)]
        members: Vec<IllustrationMember>,
    },
    /// Attach one explicitly selected text label to an existing illustration group.
    AttachIllustrationLabel { id: String, anchor_id: String },
    /// Crop an image in source fractions without stretching or moving retained pixels.
    /// Read actual natural dimensions from image_info; invalidates its owner's fit snapshot.
    CropImage { id: String, file_id: String, natural_width: f64, natural_height: f64, normalized_crop: [f64;4] },
    /// Link an existing standalone accessory image to an illustrated component.
    AttachIllustrationBadge { id: String, anchor_id: String, meaning: String },
    /// Remove accessory ownership without changing geometry, file or recorded meaning.
    DetachIllustrationBadge { id: String, anchor_id: String },
    /// Translate complete existing components rigidly, preserving IDs and metadata.
    Translate {
        element_ids: Vec<String>,
        dx: f64,
        dy: f64,
    },
    /// Atomically exchange two top-level frames and reroute generated sharp cross-panel arrows.
    SwapFrames { first_id: String, second_id: String },
    /// Pack selected top-level frames in existing spatial order with an explicit 8..512 gap.
    /// Move complete contents and reroute supported cross-panel connectors; no scientific reordering.
    PackFrames { element_ids: Vec<String>, vertical: bool, gap: f64 },
    /// Duplicate complete native components with new IDs and rigid offsets.
    Duplicate {
        element_ids: Vec<String>,
        id_prefix: String,
        dx: f64,
        dy: f64,
    },
    /// Shrink a top-level frame around its live direct children without moving any content.
    /// Keep the frame origin; padding 8..128 is added only where the old size permits.
    FitFrame { id: String, padding: f64 },
    /// Shrink existing unbound scientific text to its current wrapped lines.
    /// Preserve wording, line breaks and visible alignment; never reflow or grow.
    FitTextWidth {
        id: String,
    },
    /// Wrap text using pinned font metrics while keeping containers and bindings.
    SetWrappedText {
        id: String,
        text: String,
        /// Optional width for unbound text or an explicit arrow-caption reflow.
        /// Omit for node labels: their width comes from the shape container.
        #[serde(default)]
        max_width: Option<f64>,
        /// Optional explicit scientific font size (4..200); otherwise preserve it.
        #[serde(default)]
        font_size: Option<f64>,
    },
    /// Move one free text label without expanding its illustration/group.
    SetTextPosition {
        id: String,
        x: f64,
        y: f64,
    },
    /// Replace only recorded relation evidence after reading its exact existing value.
    SetRelationEvidence {
        id: String,
        expected_evidence: Value,
        evidence: Vec<String>,
    },
    /// Set native relation endpoints and explicit direction metadata together.
    /// Geometry, bindings, labels, evidence and historical plan snapshots survive.
    SetRelationDirection {
        id: String,
        direction: crate::sketch::Direction,
        /// Explicit state, never inferred from the requested direction.
        inhibitory: bool,
    },
    /// Reroute a generated cross-panel connector, or a sharp local connector within its frame.
    /// Local routes preserve endpoint coordinates and caption geometry.
    /// Preserves IDs, scientific metadata, text, styling and unrelated objects.
    /// Explicitly choose centered ports and rebuild a sharp local path, retaining
    /// bindings, gaps, direction, caption geometry and scientific metadata.
    RebindConnectorPorts { id: String, start_side: ConnectorPort, end_side: ConnectorPort },
    /// Recompute the path without changing scientific endpoint identities.
    Reroute {
        id: String,
    },
    Add {
        element: Value,
    },
    Update {
        id: String,
        #[schemars(schema_with = "update_changes_schema")]
        changes: serde_json::Map<String, Value>,
    },
    /// Remove one object. Illustrated anchors cascade to owned images/labels.
    /// Inset anchors also remove their reciprocal region/callout decorations.
    /// Other bound text and connected arrows survive detached; remove their IDs
    /// explicitly when requested. Do not repeat IDs already removed by a cascade.
    Remove {
        id: String,
    },
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum SceneError {
    #[error("cannot set native node geometry: {0}")]
    InvalidNodeGeometry(String),
    #[error("cannot insert native sketch: {0}")]
    InvalidSketch(String),
    #[error("scene must contain an elements array of objects with unique nonempty ids")]
    InvalidScene,
    #[error("element already exists: {0}")]
    Duplicate(String),
    #[error("element not found: {0}")]
    NotFound(String),
    #[error("cannot reroute connector: {0}")]
    InvalidRoute(String),
    #[error("cannot reflow text: {0}")]
    InvalidText(String),
    #[error("cannot set relation direction: {0}")]
    InvalidRelation(String),
    #[error("cannot translate components: {0}")]
    InvalidTranslation(String),
    #[error("cannot duplicate components: {0}")]
    InvalidCopy(String),
    #[error("cannot sync inset region: {0}")]
    InvalidInset(String),
    #[error("cannot fit illustration anchor: {0}")]
    InvalidIllustration(String),
    #[error("invalid image geometry: {0}")]
    InvalidImageGeometry(String),
    #[error("invalid update: {0}")]
    InvalidUpdate(String),
    #[error("element id cannot be changed")]
    ImmutableId,
}

pub(crate) fn owned_elements(elements: &[Value], id: &str) -> HashSet<String> {
    let Some(parent) = elements
        .iter()
        .find(|element| element["id"].as_str() == Some(id))
    else {
        return HashSet::new();
    };
    elements
        .iter()
        .filter(|element| {
            let native_label =
                element["type"] == "text" && element["containerId"].as_str() == Some(id);
            let visual = element["type"] == "image"
                && element["customData"]["semanticAnchorId"].as_str() == Some(id)
                && parent["customData"]["visualAssetElementId"] == element["id"];
            let detached_label = element["type"] == "text"
                && parent["customData"]["visualLabelIds"]
                    .as_array()
                    .is_some_and(|ids| ids.contains(&element["id"]))
                && parent["customData"]["visualAssetGroupId"]
                    .as_str()
                    .is_some_and(|group| {
                        element["groupIds"].as_array().is_some_and(|groups| {
                            groups.iter().any(|value| value.as_str() == Some(group))
                        })
                    });
            let frame_child = parent["type"] == "frame" && element["frameId"].as_str() == Some(id);
            let badge=element["type"]=="image"&&element["customData"]["semanticBadgeAnchorId"].as_str()==Some(id)&&parent["customData"]["visualBadgeIds"].as_array().is_some_and(|ids|ids.contains(&element["id"]));
            native_label || visual || badge || detached_label || frame_child
        })
        .filter_map(|element| element["id"].as_str().map(str::to_owned))
        .collect()
}
pub(crate) fn touch(element: &mut Value) {
    element["version"] =
        serde_json::json!(element["version"].as_u64().unwrap_or(0).saturating_add(1));
    element["versionNonce"] =
        serde_json::json!(uuid::Uuid::new_v4().as_u128() as u32 & 0x7fff_ffff);
    element["updated"] = serde_json::json!(chrono::Utc::now().timestamp_millis());
}
fn shift(element: &mut Value, dx: f64, dy: f64) {
    element["x"] = serde_json::json!(element["x"].as_f64().unwrap_or(0.0) + dx);
    element["y"] = serde_json::json!(element["y"].as_f64().unwrap_or(0.0) + dy);
    touch(element);
}
pub(crate) fn path_label_center(points: &[[f64; 2]], x: f64, y: f64) -> [f64; 2] {
    let middle = points.len() / 2;
    let point = if points.len().is_multiple_of(2) {
        [
            (points[middle - 1][0] + points[middle][0]) / 2.,
            (points[middle - 1][1] + points[middle][1]) / 2.,
        ]
    } else {
        points[middle]
    };
    [x + point[0], y + point[1]]
}

fn translate_bindings(
    elements: &mut [Value],
    moved: &HashSet<String>,
    shifted: &HashSet<String>,
    dx: f64,
    dy: f64,
) {
    let mut labels = Vec::new();
    for arrow in elements
        .iter_mut()
        .filter(|element| element["type"] == "arrow")
    {
        if arrow["id"].as_str().is_some_and(|id| shifted.contains(id)) {
            continue;
        }
        let start = arrow["startBinding"]["elementId"]
            .as_str()
            .is_some_and(|id| moved.contains(id));
        let end = arrow["endBinding"]["elementId"]
            .as_str()
            .is_some_and(|id| moved.contains(id));
        if !(start || end)
            || arrow["angle"].as_f64().unwrap_or(0.0) != 0.0
            || arrow["elbowed"] == true
        {
            continue;
        }
        let Some(mut points) = arrow["points"].as_array().and_then(|points| {
            points
                .iter()
                .map(|point| Some([point[0].as_f64()?, point[1].as_f64()?]))
                .collect::<Option<Vec<_>>>()
        }) else {
            continue;
        };
        if points.len() < 2 {
            continue;
        }
        let before = path_label_center(
            &points,
            arrow["x"].as_f64().unwrap_or(0.),
            arrow["y"].as_f64().unwrap_or(0.),
        );
        if start && end {
            shift(arrow, dx, dy);
        } else {
            if start {
                shift(arrow, dx, dy);
                for point in points.iter_mut().skip(1) {
                    point[0] -= dx;
                    point[1] -= dy;
                }
            }
            if end {
                let point = points.last_mut().unwrap();
                point[0] += dx;
                point[1] += dy;
                touch(arrow);
            }
            arrow["width"] = serde_json::json!(
                points
                    .iter()
                    .map(|p| p[0])
                    .fold(f64::NEG_INFINITY, f64::max)
                    - points.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min)
            );
            arrow["height"] = serde_json::json!(
                points
                    .iter()
                    .map(|p| p[1])
                    .fold(f64::NEG_INFINITY, f64::max)
                    - points.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min)
            );
            arrow["points"] = serde_json::json!(points);
        }
        if let Some(id) = arrow["id"].as_str() {
            let after = path_label_center(
                &points,
                arrow["x"].as_f64().unwrap_or(0.),
                arrow["y"].as_f64().unwrap_or(0.),
            );
            labels.push((id.to_owned(), after[0] - before[0], after[1] - before[1]));
        }
    }
    for (id, dx, dy) in labels {
        for label in elements.iter_mut().filter(|element| {
            element["type"] == "text" && element["containerId"].as_str() == Some(&id)
        }) {
            if dx != 0. || dy != 0. {
                shift(label, dx, dy);
            }
        }
    }
}

/// Field updates are shallow: compound properties must be supplied in full.
/// Excalidraw runtime normalization is still required before rendering new elements.
pub fn apply(document: &Value, operations: &[Operation]) -> Result<Value, SceneError> {
    let mut result = document.clone();
    let elements = result
        .get_mut("elements")
        .and_then(Value::as_array_mut)
        .ok_or(SceneError::InvalidScene)?;
    let mut ids = HashSet::new();
    for element in elements.iter() {
        let id = element
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or(SceneError::InvalidScene)?;
        if !element.is_object() || !ids.insert(id.to_owned()) {
            return Err(SceneError::InvalidScene);
        }
    }
    for operation in operations {
        match operation {
            Operation::SetNodeGeometry {
                id,
                x,
                y,
                width,
                height,
            } => {
                crate::node_geometry::apply(elements, id, [*x, *y, *width, *height])?;
            }
            Operation::InsertSketch {
                plan,
                id_prefix,
                x,
                y,
            } => {
                ids.extend(crate::insert_sketch::apply(
                    elements, plan, id_prefix, *x, *y,
                )?);
            }
            Operation::ScaleIllustration {
                id,
                file_id,
                factor,
            } => {
                crate::illustration_fit::scale(elements, id, file_id, *factor)?;
            }
            Operation::SetIllustrationGeometry {
                id,
                file_id,
                x,
                y,
                width,
                height,
                normalized_bounds,
            } => {
                crate::illustration_fit::set_geometry(
                    elements,
                    id,
                    file_id,
                    [*x, *y, *width, *height],
                    *normalized_bounds,
                )?;
            }
            Operation::FitIllustrationAnchor {
                id,
                file_id,
                normalized_bounds,
            } => {
                crate::illustration_fit::apply(elements, id, file_id, *normalized_bounds)?;
            }
            Operation::AddWrappedText {
                id,
                text,
                x,
                y,
                max_width,
                fit_width,
                font_size,
                frame_id,
            } => {
                if ids.contains(id) {
                    return Err(SceneError::Duplicate(id.clone()));
                }
                crate::text_edit::add(
                    elements,
                    id,
                    text,
                    [*x, *y],
                    *max_width,
                    *font_size,
                    frame_id.as_deref(),
                )?;
                if *fit_width {
                    crate::text_edit::fit_width(elements, id)?;
                }
                ids.insert(id.clone());
            }
            Operation::SyncInsetRegion { id } => {
                crate::inset_sync::apply(elements, &document["files"], id)?
            }
            Operation::PositionArrowCaption { id, position } => crate::connector_path::position_caption(elements,id,*position)?,
            Operation::FitIllustrationGroup { anchor_id, samples } => crate::illustration_fit::apply_group(elements, anchor_id, samples)?,
            Operation::BindIllustration { anchor_id, primary_id, label_ids, members } => crate::illustration_binding::bind(elements, anchor_id, primary_id, label_ids, members)?,
            Operation::AttachIllustrationLabel { id, anchor_id } => crate::illustration_badge::attach_label(elements,id,anchor_id)?,
            Operation::AttachIllustrationBadge { id, anchor_id, meaning } => crate::illustration_badge::attach(elements,id,anchor_id,meaning)?,
            Operation::DetachIllustrationBadge { id, anchor_id } => crate::illustration_badge::detach(elements,id,anchor_id)?,
            Operation::Translate {
                element_ids,
                dx,
                dy,
            } => {
                let mut snapshot = document.clone();
                snapshot["elements"] = Value::Array(elements.clone());
                let translated = crate::arrange::translate(&snapshot, element_ids, *dx, *dy)
                    .map_err(|error| SceneError::InvalidTranslation(error.to_string()))?;
                *elements = translated["elements"].as_array().unwrap().clone();
            }
            Operation::PackFrames { element_ids, vertical, gap } => {
                let mut snapshot = document.clone();
                snapshot["elements"] = Value::Array(elements.clone());
                let packed = crate::arrange::pack_frames(&snapshot, element_ids, *vertical, *gap)
                    .map_err(|error| SceneError::InvalidTranslation(error.to_string()))?;
                *elements = packed["elements"].as_array().unwrap().clone();
            }
            Operation::SwapFrames { first_id, second_id } => {
                let mut snapshot = document.clone();
                snapshot["elements"] = Value::Array(elements.clone());
                let swapped = crate::arrange::swap_frames(&snapshot, first_id, second_id)
                    .map_err(|error| SceneError::InvalidTranslation(error.to_string()))?;
                *elements = swapped["elements"].as_array().unwrap().clone();
            }
            Operation::Duplicate {
                element_ids,
                id_prefix,
                dx,
                dy,
            } => {
                ids.extend(crate::duplicate::apply(
                    elements,
                    &document["files"],
                    element_ids,
                    id_prefix,
                    *dx,
                    *dy,
                )?);
            }
            Operation::FitTextWidth { id } => crate::text_edit::fit_width(elements, id)?,
            Operation::SetWrappedText {
                id,
                text,
                max_width,
                font_size,
            } => crate::text_edit::apply(elements, id, text, *max_width, *font_size)?,
            Operation::SetTextPosition { id, x, y } => {
                crate::text_edit::set_position(elements, id, *x, *y)?
            }
            Operation::SetRelationEvidence {
                id,
                expected_evidence,
                evidence,
            } => {
                if evidence.len() > 100
                    || evidence
                        .iter()
                        .any(|entry| entry.trim().is_empty() || entry.chars().count() > 2000)
                {
                    return Err(SceneError::InvalidRelation("evidence requires at most 100 nonblank references of at most 2000 characters each".into()));
                }
                let element = editable_relation(elements, id)?;
                if element["customData"]["evidence"] != *expected_evidence {
                    return Err(SceneError::InvalidRelation("recorded evidence changed; read the exact current value before replacing it".into()));
                }
                if element["customData"].is_null() {
                    element["customData"] = serde_json::json!({});
                }
                element["customData"]["evidence"] = serde_json::json!(evidence);
                touch(element);
            }
            Operation::SetRelationDirection {
                id,
                direction,
                inhibitory,
            } => {
                if *inhibitory && *direction != crate::sketch::Direction::Forward {
                    return Err(SceneError::InvalidRelation(
                        "inhibitory is supported only for forward relations".into(),
                    ));
                }
                let element = editable_relation(elements, id)?;
                let metadata = element.get("customData").cloned().unwrap_or(Value::Null);
                let (start, end) = direction.arrowheads(*inhibitory);
                element["startArrowhead"] = serde_json::json!(start);
                element["endArrowhead"] = serde_json::json!(end);
                if metadata.is_null() {
                    element["customData"] = serde_json::json!({});
                }
                element["customData"]["direction"] = serde_json::json!(direction);
                element["customData"]["inhibitory"] = serde_json::json!(inhibitory);
                touch(element);
            }
            Operation::FitFrame { id, padding } => {
                crate::frame_fit::fit(elements, id, *padding)?;
            }
            Operation::RebindConnectorPorts { id, start_side, end_side } => crate::panel_routes::rebind_local_ports(elements,id,[start_side.index(),end_side.index()]).map_err(|e|SceneError::InvalidRoute(e.0))?,
            Operation::Reroute { id } => {
                if !ids.contains(id) {
                    return Err(SceneError::NotFound(id.clone()));
                }
                crate::panel_routes::reroute(elements, id)
                    .map_err(|error| SceneError::InvalidRoute(error.0))?;
            }
            Operation::CropImage { id,file_id,natural_width,natural_height,normalized_crop } => {
                crate::image_crop::apply(elements,id,file_id,[*natural_width,*natural_height],*normalized_crop)?;
            }
            Operation::Add { element } => {
                let id = element
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                    .ok_or(SceneError::InvalidScene)?;
                if !element.is_object() {
                    return Err(SceneError::InvalidScene);
                }
                let fields = element.as_object().unwrap();
                for field in NUMERIC_UPDATE_FIELDS {
                    if fields.get(field).is_some_and(|v| !v.as_f64().is_some_and(|n| n.is_finite() && n.abs()<=1e6)) {
                        return Err(SceneError::InvalidUpdate(format!("{id}.{field} requires a finite JSON number within ±1000000 when adding native objects; quoted numbers, null and containers are invalid")));
                    }
                }
                validate_image_fields(fields,id)?;
                if !ids.insert(id.to_owned()) {
                    return Err(SceneError::Duplicate(id.into()));
                }
                elements.push(element.clone());
            }
            Operation::SetImageGeometry {
                id,
                x,
                y,
                width,
                height,
            } => {
                if ![x, y, width, height].iter().all(|n| n.is_finite())
                    || x.abs() > 1e6
                    || y.abs() > 1e6
                    || *width <= 0.
                    || *height <= 0.
                    || *width > 1e6
                    || *height > 1e6
                {
                    return Err(SceneError::InvalidImageGeometry("x/y require finite scene coordinates within ±1000000; width/height require positive finite numbers <=1000000".into()));
                }
                if elements
                    .iter()
                    .any(|e| e["customData"]["visualAssetElementId"].as_str() == Some(id))
                {
                    return Err(SceneError::InvalidImageGeometry("owned illustrations require their component/anchor operation, not standalone image placement".into()));
                }
                if elements.iter().any(|e| {
                    ["startBinding", "endBinding"]
                        .iter()
                        .any(|key| e[key]["elementId"].as_str() == Some(id))
                        || e["containerId"].as_str() == Some(id)
                }) {
                    return Err(SceneError::InvalidImageGeometry(
                        "image has bound objects; use its component-specific/native operation"
                            .into(),
                    ));
                }
                let element = elements
                    .iter_mut()
                    .find(|e| e["id"].as_str() == Some(id))
                    .ok_or_else(|| SceneError::NotFound(id.clone()))?;
                if element["type"] != "image"
                    || element["isDeleted"] == true
                    || element["locked"] == true
                    || !element["customData"]["semanticAnchorId"].is_null()
                    || !element["customData"]["inset"].is_null()
                    || element["boundElements"]
                        .as_array()
                        .is_some_and(|v| !v.is_empty())
                {
                    return Err(SceneError::InvalidImageGeometry("requires a live unlocked standalone image without semantic ownership, inset membership or bound objects".into()));
                }
                element["x"] = serde_json::json!(x);
                element["y"] = serde_json::json!(y);
                element["width"] = serde_json::json!(width);
                element["height"] = serde_json::json!(height);
                touch(element);
            }
            Operation::Update { id, changes } => {
                if changes.contains_key("id") {
                    return Err(SceneError::ImmutableId);
                }
                for field in ["boundElements", "groupIds"] {
                    if let Some(value) = changes.get(field).filter(|v| !v.is_null()) {
                        let Some(items) = value.as_array() else {
                            return Err(SceneError::InvalidUpdate(format!(
                                "{id}.{field} requires a JSON array; use [] for no references, never an empty string"
                            )));
                        };
                        let mut seen = std::collections::HashSet::new();
                        for item in items {
                            let identity = if field == "groupIds" {
                                item.as_str()
                            } else {
                                if !matches!(item["type"].as_str(), Some("arrow" | "text")) {
                                    return Err(SceneError::InvalidUpdate(
                                        "boundElements entries require native {id,type:arrow|text} objects"
                                            .into(),
                                    ));
                                }
                                item["id"].as_str()
                            };
                            if !identity.is_some_and(|id| !id.is_empty() && seen.insert(id)) {
                                return Err(SceneError::InvalidUpdate(format!(
                                    "{id}.{field} requires nonempty, distinct reference IDs"
                                )));
                            }
                        }
                    }
                }
                for field in ["containerId", "frameId"] {
                    if changes
                        .get(field)
                        .is_some_and(|v| !v.is_null() && v.as_str().is_none_or(|s| s.is_empty()))
                    {
                        return Err(SceneError::InvalidUpdate(format!(
                            "{id}.{field} requires null or a nonempty native object ID, never an empty string"
                        )));
                    }
                }
                for field in NUMERIC_UPDATE_FIELDS {
                    if changes.get(field).is_some_and(|value| {
                        !value
                            .as_f64()
                            .is_some_and(|number| number.is_finite() && number.abs() <= 1e6)
                    }) {
                        return Err(SceneError::InvalidUpdate(format!(
                            "{id}.{field} requires a finite JSON number within ±1000000, not a quoted string, null or container; read the native fields and retry without changing unrelated objects"
                        )));
                    }
                }
                validate_image_fields(changes,id)?;
                let owned = owned_elements(elements, id);
                let element = elements
                    .iter_mut()
                    .find(|element| element["id"].as_str() == Some(id))
                    .ok_or_else(|| SceneError::NotFound(id.clone()))?;
                if let Some(metadata) = changes.get("customData") {
                    for key in ["visualAssetElementId", "semanticAnchorId", "visualAssetGroupId", "visualLabelIds", "visualBadgeIds", "semanticBadgeAnchorId", "visualBadgeGroupId", "badgeMeaning", "visualAnchorFit", "visualAssetSlot"] {
                        if metadata[key] != element["customData"][key] {
                            return Err(SceneError::InvalidUpdate(format!(
                                "{id}.customData.{key} is maintained by illustration operations; use asset replace for a primary illustration, attach_illustration_badge/detach_illustration_badge for accessories, or the dedicated fit/geometry operations. Generic update cannot create or erase ownership and fit records"
                            )));
                        }
                    }
                }
                let old_x = element["x"].as_f64().unwrap_or(0.0);
                let old_y = element["y"].as_f64().unwrap_or(0.0);
                let version = element
                    .get("version")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    .saturating_add(1);
                element
                    .as_object_mut()
                    .ok_or(SceneError::InvalidScene)?
                    .extend(changes.clone());
                update_path_dimensions(element, changes)?;
                if element["type"] == "text" {
                    if let Some(text) = changes.get("text") {
                        if !text.is_string() {
                            return Err(SceneError::InvalidScene);
                        }
                        element["originalText"] = text.clone();
                    }
                }
                element["version"] = serde_json::json!(version);
                element["versionNonce"] =
                    serde_json::json!(uuid::Uuid::new_v4().as_u128() as u32 & 0x7fff_ffff);
                element["updated"] = serde_json::json!(chrono::Utc::now().timestamp_millis());
                let dx = element["x"].as_f64().unwrap_or(old_x) - old_x;
                let dy = element["y"].as_f64().unwrap_or(old_y) - old_y;
                if dx != 0.0 || dy != 0.0 {
                    for child in elements.iter_mut().filter(|element| {
                        element["id"].as_str().is_some_and(|id| owned.contains(id))
                    }) {
                        shift(child, dx, dy);
                    }
                    let mut moved = owned.clone();
                    moved.insert(id.clone());
                    translate_bindings(elements, &moved, &owned, dx, dy);
                }
            }
            Operation::Remove { id } => {
                let position = elements
                    .iter()
                    .position(|element| element["id"].as_str() == Some(id))
                    .ok_or_else(|| SceneError::NotFound(id.clone()))?;
                let mut removed = HashSet::from([id.clone()]);
                if elements[position]["customData"]["visualAssetGroupId"].is_string() {
                    removed.extend(owned_elements(elements, id));
                    removed.extend(crate::inset::annotations(elements, id));
                }
                elements.retain(|element| {
                    !element["id"]
                        .as_str()
                        .is_some_and(|id| removed.contains(id))
                });
                for id in &removed {
                    ids.remove(id);
                }
                // Remove stale Excalidraw links while leaving scene geometry intact.
                for element in elements.iter_mut() {
                    if let Some(badges)=element.get_mut("customData").and_then(|d|d.get_mut("visualBadgeIds")).and_then(Value::as_array_mut){badges.retain(|v|!v.as_str().is_some_and(|id|removed.contains(id)));}
                    if let Some(bindings) = element
                        .get_mut("boundElements")
                        .and_then(Value::as_array_mut)
                    {
                        bindings.retain(|binding| {
                            !binding["id"]
                                .as_str()
                                .is_some_and(|id| removed.contains(id))
                        });
                    }
                    for field in ["containerId", "frameId"] {
                        if element
                            .get(field)
                            .and_then(Value::as_str)
                            .is_some_and(|id| removed.contains(id))
                        {
                            element[field] = Value::Null;
                        }
                    }
                    for field in ["startBinding", "endBinding"] {
                        if element
                            .get(field)
                            .and_then(|binding| binding.get("elementId"))
                            .and_then(Value::as_str)
                            .is_some_and(|id| removed.contains(id))
                        {
                            element[field] = Value::Null;
                        }
                    }
                }
            }
        }
    }
    Ok(result)
}

fn editable_relation<'a>(elements: &'a mut [Value], id: &str) -> Result<&'a mut Value, SceneError> {
    let element = elements
        .iter_mut()
        .find(|element| element["id"].as_str() == Some(id))
        .ok_or_else(|| SceneError::NotFound(id.into()))?;
    let metadata = &element["customData"];
    if element["type"] != "arrow"
        || element["isDeleted"] == true
        || element["locked"] == true
        || (!metadata.is_null() && !metadata.is_object())
        || metadata.get("annotationKind").is_some_and(|v| !v.is_null())
        || metadata.get("inset").is_some_and(|v| !v.is_null())
    {
        return Err(SceneError::InvalidRelation("requires a live unlocked relation arrow; annotation/inset arrows and opaque metadata are not supported".into()));
    }
    Ok(element)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn moving_and_removing_an_illustrated_node_keeps_related_elements_consistent() {
        let plan=serde_json::from_value(json!({"message":"Layout test","audience":"researchers","nodes":[{"id":"a","label":"Cell","row":0,"column":0},{"id":"b","label":"Response","row":0,"column":1}],"edges":[{"from":"a","to":"b","label":"signal"}]})).unwrap();
        let source = crate::sketch::compile(&plan).unwrap();
        let (scene, visual) = crate::image::illustrate(
            &source,
            "node-0",
            crate::image::Image {
                file_id: "fixture",
                data_url: "data:image/png;base64,abc",
                width: 100,
                height: 100,
                provenance: json!({"source":"fixture"}),
            },
        )
        .unwrap();
        let moved = apply(
            &scene,
            &[Operation::Update {
                id: "node-0".into(),
                changes: serde_json::from_value(json!({"x":60,"y":40})).unwrap(),
            }],
        )
        .unwrap();
        let find = |scene: &Value, id: &str| {
            scene["elements"]
                .as_array()
                .unwrap()
                .iter()
                .find(|element| element["id"] == id)
                .unwrap()
                .clone()
        };
        let before = find(&scene, &visual);
        let after = find(&moved, &visual);
        assert_eq!(
            after["x"].as_f64().unwrap(),
            before["x"].as_f64().unwrap() + 60.0
        );
        assert_eq!(
            after["y"].as_f64().unwrap(),
            before["y"].as_f64().unwrap() + 40.0
        );
        assert_eq!(find(&moved, "node-1"), find(&scene, "node-1"));
        let old_arrow = find(&scene, "edge-0");
        let new_arrow = find(&moved, "edge-0");
        assert_eq!(
            new_arrow["x"].as_f64().unwrap(),
            old_arrow["x"].as_f64().unwrap() + 60.0
        );
        assert_eq!(
            new_arrow["x"].as_f64().unwrap() + new_arrow["points"][1][0].as_f64().unwrap(),
            old_arrow["x"].as_f64().unwrap() + old_arrow["points"][1][0].as_f64().unwrap()
        );
        let removed = apply(
            &moved,
            &[Operation::Remove {
                id: "node-0".into(),
            }],
        )
        .unwrap();
        assert!(
            !removed["elements"]
                .as_array()
                .unwrap()
                .iter()
                .any(|element| element["id"] == "node-0"
                    || element["id"] == "label-0"
                    || element["id"] == visual)
        );
        assert!(find(&removed, "edge-0")["startBinding"].is_null());
        assert_eq!(find(&removed, "node-1"), find(&scene, "node-1"));
    }
    #[test]
    fn removing_a_component_and_its_relationships_preserves_other_nodes() {
        let plan = serde_json::from_value(json!({
            "message":"Conceptual pathway", "audience":"researchers", "layout":"horizontal",
            "nodes":[{"id":"l","label":"L"},{"id":"r","label":"R"},{"id":"x","label":"X"}],
            "edges":[{"from":"l","to":"r","label":"binding"},{"from":"r","to":"x","label":"activation"}]
        })).unwrap();
        let source = crate::sketch::compile(&plan).unwrap();
        let (scene, _) = crate::image::illustrate(
            &source,
            "node-1",
            crate::image::Image {
                file_id: "fixture",
                data_url: "data:image/png;base64,abc",
                width: 40,
                height: 100,
                provenance: json!({"source":"fixture"}),
            },
        )
        .unwrap();
        let mut ids = vec!["node-1".to_string()];
        for element in scene["elements"].as_array().unwrap() {
            if element["type"] == "arrow"
                || (element["type"] == "text"
                    && element["containerId"]
                        .as_str()
                        .is_some_and(|id| id.starts_with("edge-")))
            {
                ids.push(element["id"].as_str().unwrap().to_string());
            }
        }
        let operations = ids
            .into_iter()
            .map(|id| Operation::Remove { id })
            .collect::<Vec<_>>();
        let edited = apply(&scene, &operations).unwrap();
        let remaining = edited["elements"].as_array().unwrap();
        assert_eq!(remaining.len(), 4);
        for element in remaining {
            assert!(
                ["node-0", "label-0", "node-2", "label-2"]
                    .contains(&element["id"].as_str().unwrap())
            );
            let mut expected = scene["elements"]
                .as_array()
                .unwrap()
                .iter()
                .find(|old| old["id"] == element["id"])
                .unwrap()
                .clone();
            if let Some(bindings) = expected["boundElements"].as_array_mut() {
                bindings.retain(|binding| binding["type"] != "arrow");
            }
            assert_eq!(&expected, element);
        }
        assert_eq!(edited["files"], scene["files"]);
    }

    #[test]
    fn text_updates_keep_engine_source_and_version_in_sync() {
        let original = json!({"elements":[{"id":"label","type":"text","text":"Old","originalText":"Old","version":4}]});
        let updated = apply(
            &original,
            &[Operation::Update {
                id: "label".into(),
                changes: serde_json::from_value(json!({"text":"New"})).unwrap(),
            }],
        )
        .unwrap();
        assert_eq!(updated["elements"][0]["originalText"], "New");
        assert_eq!(updated["elements"][0]["version"], 5);
        assert_eq!(original["elements"][0]["text"], "Old");
    }
    #[test]
    fn edits_are_atomic_and_preserve_unrelated_content() {
        let original = json!({"elements":[{"id":"cell","x":1},{"id":"arrow","startBinding":{"elementId":"cell"}},{"id":"label","containerId":"cell"}],"files":{"image":"bytes"},"appState":{"zoom":1}});
        let edited = apply(
            &original,
            &[Operation::Update {
                id: "cell".into(),
                changes: serde_json::from_value(json!({"x":42})).unwrap(),
            }],
        )
        .unwrap();
        assert_eq!(edited["elements"][0]["x"], 42);
        assert_eq!(edited["elements"][1], original["elements"][1]);
        assert_eq!(edited["files"], original["files"]);
        assert!(
            apply(
                &original,
                &[
                    Operation::Remove { id: "cell".into() },
                    Operation::Remove {
                        id: "missing".into()
                    }
                ]
            )
            .is_err()
        );
        assert_eq!(original["elements"][0]["x"], 1);
        let removed = apply(&original, &[Operation::Remove { id: "cell".into() }]).unwrap();
        assert!(removed["elements"][0]["startBinding"].is_null());
        assert!(removed["elements"][1]["containerId"].is_null());
    }
    #[test]
    fn rejects_ambiguous_identity() {
        assert_eq!(
            apply(&json!({"elements":[{"id":"a"},{"id":"a"}]}), &[]),
            Err(SceneError::InvalidScene)
        );
        let scene = json!({"elements":[{"id":"a"}]});
        assert_eq!(
            apply(
                &scene,
                &[Operation::Add {
                    element: json!({"id":"a"})
                }]
            ),
            Err(SceneError::Duplicate("a".into()))
        );
        assert_eq!(
            apply(
                &scene,
                &[Operation::Update {
                    id: "a".into(),
                    changes: serde_json::from_value(json!({"id":"b"})).unwrap()
                }]
            ),
            Err(SceneError::ImmutableId)
        );
    }
}
