//! Rigid native component copies. Scientific claims are preserved, never inferred.
use crate::scene::SceneError;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
fn fail(message: &str) -> SceneError {
    SceneError::InvalidCopy(message.into())
}
fn groups(e: &Value) -> Result<Vec<&str>, SceneError> {
    match e.get("groupIds") {
        None | Some(Value::Null) => Ok(vec![]),
        Some(Value::Array(values)) => values
            .iter()
            .map(|v| {
                v.as_str()
                    .filter(|s| !s.is_empty() && s.len() <= 512)
                    .ok_or_else(|| fail("invalid group identity"))
            })
            .collect(),
        _ => Err(fail("invalid group membership")),
    }
}
pub(crate) fn apply(
    elements: &mut Vec<Value>,
    files: &Value,
    requested: &[String],
    prefix: &str,
    dx: f64,
    dy: f64,
) -> Result<Vec<String>, SceneError> {
    if requested.is_empty()
        || requested.len() > 100
        || prefix.is_empty()
        || prefix.len() > 64
        || !prefix
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        || ![dx, dy].iter().all(|v| v.is_finite() && v.abs() <= 1e6)
    {
        return Err(fail(
            "requires 1..100 distinct targets, a 1..64 ASCII identifier prefix and finite offsets",
        ));
    }
    let mut selected: HashSet<String> = requested.iter().cloned().collect();
    if selected.len() != requested.len() {
        return Err(fail("duplicate target identities"));
    }
    let live: HashMap<&str, &Value> = elements
        .iter()
        .filter(|e| e["isDeleted"] != true)
        .filter_map(|e| e["id"].as_str().map(|id| (id, e)))
        .collect();
    if selected.iter().any(|id| !live.contains_key(id.as_str())) {
        return Err(fail("target is missing or deleted"));
    }
    let mut processed = HashSet::new();
    let mut selected_groups = HashSet::new();
    loop {
        if selected.len() > 1000 {
            return Err(fail("expanded component exceeds 1000 objects"));
        }
        let mut next = selected.clone();
        for id in selected.difference(&processed) {
            let e = live[id.as_str()];
            selected_groups.extend(groups(e)?);
            // A bound label/image is copied with its owning component, never with stale ownership.
            if let Some(owner) = e["containerId"]
                .as_str()
                .or(e["customData"]["semanticAnchorId"].as_str())
            {
                if !live.contains_key(owner) {
                    return Err(fail("component owner is missing"));
                }
                next.insert(owner.into());
            }
            next.extend(
                crate::scene::owned_elements(elements, id)
                    .into_iter()
                    .filter(|id| live.contains_key(id.as_str())),
            );
        }
        for (id, e) in &live {
            if groups(e)?.iter().any(|g| selected_groups.contains(g)) {
                next.insert((*id).into());
            }
        }
        if next == selected {
            break;
        }
        processed = selected;
        selected = next;
    }
    let mapping: HashMap<String, String> = selected
        .iter()
        .map(|id| (id.clone(), format!("{prefix}:e:{id}")))
        .collect();
    let existing: HashSet<&str> = elements.iter().filter_map(|e| e["id"].as_str()).collect();
    if mapping
        .values()
        .any(|id| id.len() > 512 || existing.contains(id.as_str()))
    {
        return Err(fail("copy identity collides or exceeds 512 bytes"));
    }
    let mut group_map = HashMap::new();
    for id in &selected {
        for group in groups(live[id.as_str()])? {
            group_map.insert(group.to_owned(), format!("{prefix}:g:{group}"));
        }
    }
    let all_groups: HashSet<&str> = elements
        .iter()
        .flat_map(|e| {
            e["groupIds"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
        })
        .collect();
    if group_map
        .values()
        .any(|group| group.len() > 512 || all_groups.contains(group.as_str()))
    {
        return Err(fail("copy group identity collides or exceeds 512 bytes"));
    }
    let mut copies = Vec::new();
    for e in elements
        .iter()
        .filter(|e| e["id"].as_str().is_some_and(|id| selected.contains(id)))
    {
        let data = &e["customData"];
        if e["locked"] == true
            || data["inset"] != Value::Null
            || data["annotationKind"] != Value::Null
            || (!data.is_null() && !data.is_object())
            || !matches!(
                e["type"].as_str(),
                Some(
                    "rectangle"
                        | "ellipse"
                        | "diamond"
                        | "text"
                        | "image"
                        | "arrow"
                        | "line"
                        | "freedraw"
                        | "frame"
                )
            )
        {
            return Err(fail(
                "requires unlocked native objects; annotations, insets and opaque metadata are unsupported",
            ));
        }
        let mut copy = e.clone();
        let source = e["id"].as_str().unwrap();
        copy["id"] = json!(mapping[source]);
        for (key, offset) in [
            ("x", dx),
            ("y", dy),
            ("width", 0.),
            ("height", 0.),
            ("angle", 0.),
        ] {
            let value = e[key]
                .as_f64()
                .filter(|v| v.is_finite() && v.abs() <= 1e6)
                .ok_or_else(|| fail("invalid source geometry"))?
                + offset;
            if !value.is_finite() || value.abs() > 1e6 {
                return Err(fail("copied geometry exceeds range"));
            }
            copy[key] = json!(value);
        }
        if e["type"] == "image"
            && e["fileId"]
                .as_str()
                .is_none_or(|id| files.get(id).is_none())
        {
            return Err(fail("image file is missing"));
        }
        for key in ["containerId", "frameId"] {
            if !e[key].is_null() {
                copy[key] = e[key]
                    .as_str()
                    .and_then(|id| mapping.get(id))
                    .map_or(Value::Null, |id| json!(id));
            }
        }
        for key in ["startBinding", "endBinding"] {
            if !e[key].is_null() {
                if let Some(id) = e[key]["elementId"].as_str().and_then(|id| mapping.get(id)) {
                    copy[key]["elementId"] = json!(id);
                } else {
                    copy[key] = Value::Null;
                }
            }
        }
        if let Some(bound) = e["boundElements"].as_array() {
            copy["boundElements"] = Value::Array(
                bound
                    .iter()
                    .filter_map(|binding| {
                        mapping.get(binding["id"].as_str()?).map(|id| {
                            let mut binding = binding.clone();
                            binding["id"] = json!(id);
                            binding
                        })
                    })
                    .collect(),
            );
        }
        copy["groupIds"] = json!(
            groups(e)?
                .iter()
                .map(|g| &group_map[*g])
                .collect::<Vec<_>>()
        );
        for key in ["semanticAnchorId", "visualAssetElementId"] {
            if !data[key].is_null() {
                let id = data[key]
                    .as_str()
                    .and_then(|id| mapping.get(id))
                    .ok_or_else(|| fail("semantic member is missing from component"))?;
                copy["customData"][key] = json!(id);
            }
        }
        if !data["visualAssetGroupId"].is_null() {
            let group = data["visualAssetGroupId"]
                .as_str()
                .and_then(|g| group_map.get(g))
                .ok_or_else(|| fail("semantic group is missing"))?;
            copy["customData"]["visualAssetGroupId"] = json!(group);
        }
        if !data["visualLabelIds"].is_null() {
            let labels = data["visualLabelIds"]
                .as_array()
                .ok_or_else(|| fail("invalid semantic label identities"))?
                .iter()
                .map(|v| {
                    v.as_str()
                        .and_then(|id| mapping.get(id))
                        .cloned()
                        .ok_or_else(|| fail("semantic label is missing"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            copy["customData"]["visualLabelIds"] = json!(labels);
        }
        copy["index"] = Value::Null;
        copy["version"] = json!(0);
        crate::scene::touch(&mut copy);
        copies.push(copy);
    }
    let ids = copies
        .iter()
        .map(|e| e["id"].as_str().unwrap().to_owned())
        .collect();
    elements.extend(copies);
    Ok(ids)
}
