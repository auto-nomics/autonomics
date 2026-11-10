//! Engine-native scene edits used by agents and interactive clients.
//! Operations apply atomically to a clone; unrelated objects and image files survive.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

#[derive(schemars::JsonSchema, Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    /// Recompute an existing generated cross-panel connector from current geometry.
    /// Preserves IDs, scientific metadata, text, styling and unrelated objects.
    Reroute {
        id: String,
    },
    Add {
        element: Value,
    },
    Update {
        id: String,
        changes: serde_json::Map<String, Value>,
    },
    /// Remove one object. Illustrated anchors cascade to owned images/labels.
    /// Other bound text and connected arrows survive detached; remove their IDs
    /// explicitly when requested. Do not repeat IDs already removed by a cascade.
    Remove {
        id: String,
    },
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum SceneError {
    #[error("scene must contain an elements array of objects with unique nonempty ids")]
    InvalidScene,
    #[error("element already exists: {0}")]
    Duplicate(String),
    #[error("element not found: {0}")]
    NotFound(String),
    #[error("cannot reroute connector: {0}")]
    InvalidRoute(String),
    #[error("element id cannot be changed")]
    ImmutableId,
}

fn owned_elements(elements: &[Value], id: &str) -> HashSet<String> {
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
            native_label || visual || detached_label || frame_child
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
fn path_label_center(points: &[[f64; 2]], x: f64, y: f64) -> [f64; 2] {
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
            Operation::Reroute { id } => {
                if !ids.contains(id) {
                    return Err(SceneError::NotFound(id.clone()));
                }
                crate::panel_routes::reroute(elements, id)
                    .map_err(|error| SceneError::InvalidRoute(error.0))?;
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
                if !ids.insert(id.to_owned()) {
                    return Err(SceneError::Duplicate(id.into()));
                }
                elements.push(element.clone());
            }
            Operation::Update { id, changes } => {
                if changes.contains_key("id") {
                    return Err(SceneError::ImmutableId);
                }
                for field in ["x", "y"] {
                    if changes.get(field).is_some_and(|value| {
                        !value
                            .as_f64()
                            .is_some_and(|number| number.is_finite() && number.abs() <= 1e6)
                    }) {
                        return Err(SceneError::InvalidScene);
                    }
                }
                let owned = owned_elements(elements, id);
                let element = elements
                    .iter_mut()
                    .find(|element| element["id"].as_str() == Some(id))
                    .ok_or_else(|| SceneError::NotFound(id.clone()))?;
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
