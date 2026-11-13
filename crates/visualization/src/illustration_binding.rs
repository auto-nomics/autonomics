//! Bind explicit, already placed illustration members without changing pixels or geometry.
use crate::scene::{self, IllustrationMember, SceneError};
use serde_json::{Value, json};
use std::collections::HashSet;
fn fail(s: &str) -> SceneError {
    SceneError::InvalidUpdate(format!("bind_illustration: {s}"))
}
fn index(elements: &[Value], id: &str) -> Result<usize, SceneError> {
    let found = elements
        .iter()
        .enumerate()
        .filter(|(_, e)| e["id"] == id && e["isDeleted"] != true)
        .map(|(i, _)| i)
        .collect::<Vec<_>>();
    if found.len() != 1 {
        return Err(fail("each member must have one unique live identity"));
    }
    Ok(found[0])
}
fn groups(e: &Value) -> Result<Vec<String>, SceneError> {
    match e.get("groupIds") {
        None | Some(Value::Null) => Ok(vec![]),
        Some(Value::Array(a)) => a
            .iter()
            .map(|v| {
                v.as_str()
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .ok_or_else(|| fail("invalid group"))
            })
            .collect(),
        _ => Err(fail("invalid group")),
    }
}
fn bounds(e: &Value) -> Result<[f64; 4], SceneError> {
    let mut b = [0.; 4];
    for (i, k) in ["x", "y", "width", "height"].iter().enumerate() {
        b[i] = e[*k]
            .as_f64()
            .filter(|v| v.is_finite() && v.abs() <= 1e6)
            .ok_or_else(|| fail("invalid geometry"))?;
    }
    if b[2] <= 0. || b[3] <= 0. || e["locked"] == true || e["angle"].as_f64().unwrap_or(0.) != 0. {
        return Err(fail(
            "members must have positive unlocked unrotated geometry",
        ));
    }
    Ok(b)
}
pub(crate) fn bind(
    elements: &mut [Value],
    anchor_id: &str,
    primary_id: &str,
    label_ids: &[String],
    members: &[IllustrationMember],
) -> Result<(), SceneError> {
    if label_ids.len() > 32 || members.len() > 32 {
        return Err(fail("at most 32 labels and 32 accessory images"));
    }
    let mut selected = HashSet::new();
    for id in std::iter::once(anchor_id)
        .chain(std::iter::once(primary_id))
        .chain(label_ids.iter().map(String::as_str))
        .chain(members.iter().map(|m| m.id.as_str()))
    {
        if id.is_empty() || !selected.insert(id.to_owned()) {
            return Err(fail("member identities must be nonempty and distinct"));
        }
    }
    let ai = index(elements, anchor_id)?;
    let pi = index(elements, primary_id)?;
    let anchor = &elements[ai];
    if !matches!(
        anchor["type"].as_str(),
        Some("rectangle" | "ellipse" | "diamond")
    ) {
        return Err(fail("anchor must be a native component shape"));
    }
    if anchor["customData"]
        .get("visualBadgeIds")
        .is_some_and(|v| v.as_array().is_none_or(|a| !a.is_empty()))
    {
        return Err(fail("detach existing accessories before rebinding"));
    }
    if anchor["customData"]["visualAssetElementId"]
        .as_str()
        .is_some_and(|id| id != primary_id)
    {
        return Err(fail("use asset replace to change an existing primary"));
    }
    let frame = anchor["frameId"].clone();
    let frame_bounds = if let Some(id) = frame.as_str() {
        let f = &elements[index(elements, id)?];
        if f["type"] != "frame" {
            return Err(fail("invalid frame"));
        }
        Some(bounds(f)?)
    } else {
        None
    };
    let mut old_groups = HashSet::new();
    for id in &selected {
        let e = &elements[index(elements, id)?];
        let b = bounds(e)?;
        if e["frameId"] != frame {
            return Err(fail("all members must already share the anchor frame"));
        }
        if let Some(f) = frame_bounds {
            if b[0] < f[0] || b[1] < f[1] || b[0] + b[2] > f[0] + f[2] || b[1] + b[3] > f[1] + f[3]
            {
                return Err(fail("every member must fit completely inside its frame"));
            }
        }
        if e.get("customData")
            .is_some_and(|v| !v.is_null() && !v.is_object())
        {
            return Err(fail("invalid member metadata"));
        }
        for key in ["inset", "annotationKind", "semanticBadgeAnchorId"] {
            if !e["customData"][key].is_null() {
                return Err(fail("protected or already-owned member"));
            }
        }
        if e["customData"]["semanticAnchorId"]
            .as_str()
            .is_some_and(|owner| id != primary_id || owner != anchor_id)
        {
            return Err(fail("member already belongs to another component"));
        }
        old_groups.extend(groups(e)?);
    }
    for id in std::iter::once(primary_id).chain(members.iter().map(|m| m.id.as_str())) {
        let e = &elements[index(elements, id)?];
        if e["type"] != "image" || e["fileId"].as_str().is_none() {
            return Err(fail("primary and accessories must be saved images"));
        }
    }
    for id in label_ids {
        let e = &elements[index(elements, id)?];
        if e["type"] != "text" {
            return Err(fail("labels must be text"));
        }
        if !e["boundElements"].is_null() && !e["boundElements"].is_array() && e["boundElements"]!="" {return Err(fail("invalid native label connector references"));}

        if let Some(container) = e["containerId"].as_str() {
            if container != anchor_id
                && elements
                    .iter()
                    .any(|v| v["id"] == container && v["isDeleted"] != true)
            {
                return Err(fail("label is bound to another live container"));
            }
        }
    }
    for e in elements
        .iter()
        .filter(|e| e["isDeleted"] != true && !selected.contains(e["id"].as_str().unwrap_or("")))
    {
        if groups(e)?.iter().any(|g| old_groups.contains(g)) {
            return Err(fail("existing group contains an unlisted member"));
        }
        if e["type"] == "text" && e["containerId"] == anchor_id {
            return Err(fail("include every label currently bound to the anchor"));
        }
        let data = &e["customData"];
        if data["semanticAnchorId"] == anchor_id
            || data["semanticBadgeAnchorId"] == anchor_id
            || std::iter::once(&data["visualAssetElementId"])
                .chain(data["visualLabelIds"].as_array().into_iter().flatten())
                .chain(data["visualBadgeIds"].as_array().into_iter().flatten())
                .any(|v| v.as_str().is_some_and(|id| selected.contains(id)))
        {
            return Err(fail("an unlisted object has conflicting ownership"));
        }
    }
    for m in members {
        if m.meaning.trim().is_empty() || m.meaning.len() > 512 {
            return Err(fail(
                "accessory meaning must be nonblank and at most 512 bytes",
            ));
        }
    }
    // scene::apply provides atomicity, including the accessory validation below.
    let group = uuid::Uuid::new_v4().to_string();
    for id in &selected {
        let e = &mut elements[index(elements, id)?];
        e["groupIds"] = json!([group]);
        if e["type"] != "text" && e["customData"].is_null() {
            e["customData"] = json!({});
        }
        scene::touch(e);
    }
    let anchor = &mut elements[ai];
    anchor["strokeColor"] = json!("transparent");
    anchor["backgroundColor"] = json!("transparent");
    anchor["customData"]["visualAssetElementId"] = json!(primary_id);
    anchor["customData"]["visualAssetGroupId"] = json!(group);
    anchor["customData"]["visualLabelIds"] = json!(label_ids);
    anchor["customData"]
        .as_object_mut()
        .unwrap()
        .remove("visualAnchorFit");
    if let Some(bound) = anchor["boundElements"].as_array_mut() {
        bound.retain(|b| !label_ids.iter().any(|id| b["id"] == *id));
    }
    elements[pi]["customData"]["semanticAnchorId"] = json!(anchor_id);
    for id in label_ids {
        let i = index(elements, id)?;
        elements[i]["containerId"] = Value::Null;
        if elements[i]["boundElements"]=="" {elements[i]["boundElements"]=json!([]);}

    }
    for m in members {
        let i = index(elements, &m.id)?;
        elements[i]["groupIds"] = json!([]);
        crate::illustration_badge::attach(elements, &m.id, anchor_id, &m.meaning)?;
    }
    Ok(())
}
