//! Explicit ownership of an existing accessory image, without inferring its biological meaning.
use crate::scene::{self, SceneError};
use serde_json::{Value, json};
fn fail(message: &str) -> SceneError {
    SceneError::InvalidUpdate(format!("illustration badge: {message}"))
}
fn index(elements: &[Value], id: &str) -> Result<usize, SceneError> {
    elements
        .iter()
        .position(|e| e["id"] == id && e["isDeleted"] != true)
        .ok_or_else(|| fail("live member is missing"))
}
fn groups(e: &Value) -> Result<Vec<Value>, SceneError> {
    match e.get("groupIds") {
        None | Some(Value::Null) => Ok(vec![]),
        Some(Value::Array(g)) if g.iter().all(|v| v.as_str().is_some_and(|s| !s.is_empty())) => {
            Ok(g.clone())
        }
        _ => Err(fail("invalid group membership")),
    }
}
fn owner_group<'a>(elements: &'a [Value], owner: &Value) -> Result<&'a str, SceneError> {
    let id = owner["customData"]["visualAssetElementId"]
        .as_str()
        .ok_or_else(|| fail("anchor has no primary illustration"))?;
    let primary = &elements[index(elements, id)?];
    if primary["type"] != "image" || primary["customData"]["semanticAnchorId"] != owner["id"] {
        return Err(fail("primary illustration ownership is inconsistent"));
    }
    let anchor = &elements[index(elements, owner["id"].as_str().unwrap())?];
    let group = anchor["customData"]["visualAssetGroupId"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| fail("anchor has no illustration group"))?;
    if !groups(anchor)?.contains(&json!(group)) || !groups(primary)?.contains(&json!(group)) {
        return Err(fail("primary group membership is inconsistent"));
    }
    Ok(group)
}
fn unrotated(e: &Value) -> bool {
    match e.get("angle") {
        None | Some(Value::Null) => true,
        Some(v) => v.as_f64() == Some(0.),
    }
}
fn bounds(e: &Value) -> Result<[f64; 4], SceneError> {
    let mut b = [0.; 4];
    for (i, key) in ["x", "y", "width", "height"].iter().enumerate() {
        b[i] = e[*key]
            .as_f64()
            .filter(|v| v.is_finite() && v.abs() <= 1e6)
            .ok_or_else(|| fail("invalid image geometry"))?;
    }
    if b[2] <= 0. || b[3] <= 0. {
        return Err(fail("image must have positive dimensions"));
    }
    Ok(b)
}
pub(crate) fn attach(
    elements: &mut [Value],
    id: &str,
    anchor_id: &str,
    meaning: &str,
) -> Result<(), SceneError> {
    if meaning.trim().is_empty() || meaning.len() > 512 {
        return Err(fail("meaning must be nonblank and at most 512 UTF-8 bytes"));
    }
    let (bi, ai) = (index(elements, id)?, index(elements, anchor_id)?);
    let (badge, anchor) = (&elements[bi], &elements[ai]);
    if bi == ai
        || badge["type"] != "image"
        || !matches!(
            anchor["type"].as_str(),
            Some("rectangle" | "ellipse" | "diamond")
        )
    {
        return Err(fail(
            "requires a standalone image and an existing illustrated shape anchor",
        ));
    }
    if badge["locked"] == true
        || anchor["locked"] == true
        || !unrotated(badge)
        || !unrotated(anchor)
    {
        return Err(fail(
            "unlock and unrotate the image and anchor before attaching",
        ));
    }
    if !badge["customData"]["semanticAnchorId"].is_null()
        || !badge["customData"]["semanticBadgeAnchorId"].is_null()
        || !groups(badge)?.is_empty()
        || !badge["customData"]["inset"].is_null()
        || !badge["customData"]["annotationKind"].is_null()
    {
        return Err(fail(
            "badge is already owned, grouped or a protected annotation",
        ));
    }
    if badge
        .get("customData")
        .is_some_and(|v| !v.is_null() && !v.is_object())
    {
        return Err(fail("invalid badge metadata"));
    }
    if elements.iter().any(|e| {
        e["isDeleted"] != true
            && (e["customData"]["visualAssetElementId"] == id
                || e["customData"]["visualBadgeIds"]
                    .as_array()
                    .is_some_and(|ids| ids.contains(&json!(id))))
    }) {
        return Err(fail("badge already has a component reference"));
    }
    if badge["frameId"] != anchor["frameId"] {
        return Err(fail(
            "badge and anchor must already belong to the same frame",
        ));
    }
    bounds(anchor)?;
    let b = bounds(badge)?;
    if let Some(frame) = anchor["frameId"].as_str() {
        let f = &elements[index(elements, frame)?];
        let fbox = bounds(f)?;
        if f["type"] != "frame"
            || !unrotated(f)
            || b[0] < fbox[0]
            || b[1] < fbox[1]
            || b[0] + b[2] > fbox[0] + fbox[2]
            || b[1] + b[3] > fbox[1] + fbox[3]
        {
            return Err(fail("complete badge must be inside its unrotated frame"));
        }
    }
    let group = owner_group(elements, anchor)?.to_owned();
    let mut ids = match anchor["customData"].get("visualBadgeIds") {
        None => vec![],
        Some(Value::Array(ids)) if ids.len() < 32 && ids.iter().all(Value::is_string) => {
            ids.clone()
        }
        _ => return Err(fail("invalid or full badge list (maximum 32)")),
    };
    let mut unique = std::collections::HashSet::new();
    for member in &ids {
        let member = member.as_str().unwrap();
        let peer = &elements[index(elements, member)?];
        if !unique.insert(member)
            || peer["type"] != "image"
            || peer["customData"]["semanticBadgeAnchorId"] != anchor_id
            || peer["customData"]["visualBadgeGroupId"] != group
            || !groups(peer)?.contains(&json!(group))
            || peer["frameId"] != anchor["frameId"]
        {
            return Err(fail("existing badge ownership is inconsistent"));
        }
    }
    ids.push(json!(id));
    elements[ai]["customData"]["visualBadgeIds"] = json!(ids);
    scene::touch(&mut elements[ai]);
    if elements[bi]["customData"].is_null() {
        elements[bi]["customData"] = json!({});
    }
    elements[bi]["customData"]["semanticBadgeAnchorId"] = json!(anchor_id);
    elements[bi]["customData"]["badgeMeaning"] = json!(meaning);
    elements[bi]["customData"]["visualBadgeGroupId"] = json!(group);
    elements[bi]["groupIds"] = json!([group]);
    scene::touch(&mut elements[bi]);
    Ok(())
}
pub(crate) fn detach(elements: &mut [Value], id: &str, anchor_id: &str) -> Result<(), SceneError> {
    let (bi, ai) = (index(elements, id)?, index(elements, anchor_id)?);
    let (badge, anchor) = (&elements[bi], &elements[ai]);
    if badge["locked"] == true || anchor["locked"] == true {
        return Err(fail("unlock the image and anchor before detaching"));
    }
    let group = anchor["customData"]["visualAssetGroupId"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| fail("anchor group is missing"))?
        .to_owned();
    if badge["type"] != "image"
        || badge["customData"]["semanticBadgeAnchorId"] != anchor_id
        || badge["customData"]["visualBadgeGroupId"] != group
        || anchor["customData"]["visualBadgeIds"]
            .as_array()
            .is_none_or(|v| v.iter().filter(|v| v.as_str() == Some(id)).count() != 1)
    {
        return Err(fail("badge attachment does not match the specified anchor"));
    }
    let mut badge_groups = groups(badge)?;
    badge_groups.retain(|g| g.as_str() != Some(&group));
    elements[ai]["customData"]["visualBadgeIds"]
        .as_array_mut()
        .unwrap()
        .retain(|v| v.as_str() != Some(id));
    scene::touch(&mut elements[ai]);
    let metadata = elements[bi]["customData"].as_object_mut().unwrap();
    metadata.remove("semanticBadgeAnchorId");
    metadata.remove("visualBadgeGroupId");
    elements[bi]["groupIds"] = json!(badge_groups);
    scene::touch(&mut elements[bi]);
    Ok(())
}

/// Explicit label ownership; no text, typography, pixels or scientific fields change.
pub(crate) fn attach_label(
    elements: &mut [Value],
    id: &str,
    anchor_id: &str,
) -> Result<(), SceneError> {
    let li = index(elements, id)?;
    let ai = index(elements, anchor_id)?;
    let (label, anchor) = (&elements[li], &elements[ai]);
    if li == ai
        || label["type"] != "text"
        || !matches!(
            anchor["type"].as_str(),
            Some("rectangle" | "ellipse" | "diamond")
        )
    {
        return Err(fail(
            "label attachment requires text and an illustrated shape",
        ));
    }
    if label["locked"] == true
        || anchor["locked"] == true
        || !unrotated(label)
        || !unrotated(anchor)
    {
        return Err(fail("unlock and unrotate the label and component"));
    }
    if !label["customData"]["inset"].is_null() || !label["customData"]["annotationKind"].is_null() {
        return Err(fail(
            "protected annotation labels require their dedicated editor",
        ));
    }
    let group = owner_group(elements, anchor)?.to_owned();
    if label["frameId"] != anchor["frameId"] {
        return Err(fail("label and component must already share a frame"));
    }
    let b = bounds(label)?;
    bounds(anchor)?;
    if let Some(frame) = anchor["frameId"].as_str() {
        let f = &elements[index(elements, frame)?];
        let r = bounds(f)?;
        if f["type"] != "frame"
            || !unrotated(f)
            || b[0] < r[0]
            || b[1] < r[1]
            || b[0] + b[2] > r[0] + r[2]
            || b[1] + b[3] > r[1] + r[3]
        {
            return Err(fail(
                "the complete label must remain in its unrotated frame",
            ));
        }
    }
    if let Some(container) = label["containerId"].as_str() {
        if !container.is_empty()
            && container != anchor_id
            && elements
                .iter()
                .any(|e| e["id"] == container && e["isDeleted"] != true)
        {
            return Err(fail("label belongs to another live native container"));
        }
    }
    if groups(label)?.iter().any(|id| id != &group) {
        return Err(fail("label belongs to another native group"));
    }
    if !label["boundElements"].is_null()
        && !label["boundElements"].is_array()
        && label["boundElements"] != ""
    {
        return Err(fail("invalid label connector references"));
    }
    if !anchor["boundElements"].is_null() && !anchor["boundElements"].is_array() {
        return Err(fail("invalid component connector references"));
    }
    let mut labels = match anchor["customData"].get("visualLabelIds") {
        None | Some(Value::Null) => vec![],
        Some(Value::Array(ids)) if ids.len() < 32 && ids.iter().all(Value::is_string) => {
            ids.clone()
        }
        _ => return Err(fail("invalid or full label list")),
    };
    let mut unique = std::collections::HashSet::new();
    for member in &labels {
        let member = member.as_str().unwrap();
        let e = &elements[index(elements, member)?];
        if !unique.insert(member)
            || e["type"] != "text"
            || !groups(e)?.contains(&json!(group))
            || e["frameId"] != anchor["frameId"]
        {
            return Err(fail("existing label ownership is inconsistent"));
        }
    }
    if labels.contains(&json!(id)) {
        return Err(fail("label already belongs to this component"));
    }
    if elements.iter().any(|e| {
        e["isDeleted"] != true
            && e["id"] != anchor_id
            && e["customData"]["visualLabelIds"]
                .as_array()
                .is_some_and(|ids| ids.contains(&json!(id)))
    }) {
        return Err(fail("label already belongs to another illustration"));
    }
    labels.push(json!(id));
    elements[ai]["customData"]["visualLabelIds"] = json!(labels);
    if let Some(bound) = elements[ai]["boundElements"].as_array_mut() {
        bound.retain(|entry| !(entry["id"] == id && entry["type"] == "text"));
    }
    elements[li]["containerId"] = Value::Null;
    elements[li]["groupIds"] = json!([group]);
    if elements[li]["boundElements"] == "" {
        elements[li]["boundElements"] = json!([]);
    }
    scene::touch(&mut elements[li]);
    scene::touch(&mut elements[ai]);
    Ok(())
}
