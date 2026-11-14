//! Read-only projections; candidate hashes always identify the complete document.
use serde_json::{Value, json};
use std::collections::HashSet;

pub fn validate(ids: &[String]) -> Result<(), String> {
    if ids.is_empty() || ids.len() > 20 {
        return Err("element_ids requires 1..20 IDs".into());
    }
    let mut seen = HashSet::new();
    for id in ids {
        if id.trim().is_empty() || id.len() > 512 || !seen.insert(id) {
            return Err(
                "element_ids must be distinct nonblank verbatim IDs of at most 512 bytes".into(),
            );
        }
    }
    Ok(())
}

pub fn project_saved(mut full: Value, ids: &[String]) -> Result<Value, String> {
    let mut projection = select(&full["scene"]["document"], ids)?;
    projection["scope"]["revision_scope"] = json!("complete_immutable_saved_revision");
    full["scene"]
        .as_object_mut()
        .ok_or("Missing saved scene")?
        .remove("document");
    full.as_object_mut()
        .ok_or("Invalid figure read")?
        .extend(projection.as_object().ok_or("Invalid projection")?.clone());
    Ok(full)
}

fn select(document: &Value, ids: &[String]) -> Result<Value, String> {
    validate(ids)?;
    let elements = document["elements"]
        .as_array()
        .ok_or("Missing scene elements")?;
    let mut selected = Vec::new();
    for id in ids {
        let matches: Vec<_> = elements
            .iter()
            .filter(|e| e["id"].as_str() == Some(id))
            .collect();
        if matches.len() != 1 || matches[0]["isDeleted"] == true {
            return Err(format!(
                "Requested object {id:?} is missing, deleted or ambiguous"
            ));
        }
        selected.push(matches[0].clone());
    }
    let file_ids: HashSet<_> = selected
        .iter()
        .filter_map(|e| e["fileId"].as_str())
        .collect();
    let mut files = serde_json::Map::new();
    if let Some(all) = document["files"].as_object() {
        for id in file_ids {
            if let Some(file) = all.get(id) {
                let mut file = file.clone();
                if let Some(map) = file.as_object_mut() {
                    map.remove("dataURL");
                }
                files.insert(id.to_owned(), file);
            }
        }
    }
    if serde_json::to_vec(&(&selected, &files))
        .map_err(|e| e.to_string())?
        .len()
        > 256 * 1024
    {
        return Err("Selected payload exceeds 256 KiB; request fewer IDs or omit element_ids for a full read".into());
    }
    let live: Vec<_> = elements.iter().filter(|e| e["isDeleted"] != true).collect();
    let mut context = Vec::new();
    let mut bytes = 0;
    let mut points_omitted = 0;
    for e in &live {
        if e["id"]
            .as_str()
            .is_some_and(|id| ids.iter().any(|wanted| wanted == id))
        {
            continue;
        }
        let mut item = serde_json::Map::new();
        for key in [
            "id",
            "type",
            "x",
            "y",
            "width",
            "height",
            "angle",
            "frameId",
            "containerId",
            "groupIds",
            "locked",
            "opacity",
        ] {
            if let Some(value) = e.get(key) {
                item.insert(key.into(), value.clone());
            }
        }
        let mut omitted_points = 0;
        if let Some(points) = e["points"].as_array() {
            item.insert(
                "points".into(),
                json!(points.iter().take(64).collect::<Vec<_>>()),
            );
            omitted_points = points.len().saturating_sub(64);
        }
        let size = serde_json::to_vec(&item).map_err(|e| e.to_string())?.len();
        if context.len() == 200 || bytes + size > 64 * 1024 {
            continue;
        }
        points_omitted += omitted_points;
        bytes += size;
        context.push(Value::Object(item));
    }
    let review = visualization::geometry_review::review(document);
    let live_count = live.len();
    let omitted = live_count.saturating_sub(selected.len() + context.len());
    Ok(json!({
        "selected_elements": selected, "selected_files": files,
        "scope": {"mode":"selected_elements", "requested_ids":ids, "returned_count":ids.len(),
            "live_element_count":live_count, "document_omitted":true},
        "geometry_context": {"elements":context, "omitted_count":omitted,
            "points_omitted_count":points_omitted, "non_geometry_fields_omitted":true, "semantics":"raw_native_fields_not_validated_pixel_bounds"},
        "scene_geometry_review":review, "image_data_omitted":true
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_is_bounded_and_discloses_omissions() {
        let mut elements = vec![json!({"id":"selected","type":"rectangle"})];
        elements.extend((0..205).map(|i| json!({"id":format!("other-{i}"),"type":"line","x":"malformed-preserved","points":vec![[0,0];70],"customData":{"large_plan":"omitted"}})));
        let full = json!({"scene":{"document":{"elements":elements}}});
        let output = project_saved(full, &["selected".into()]).unwrap();
        let context = &output["geometry_context"];
        let count = context["elements"].as_array().unwrap().len();
        assert!(count <= 200);
        assert_eq!(context["omitted_count"], 205 - count);
        assert_eq!(context["points_omitted_count"], count * 6);
        assert_eq!(context["elements"][0]["x"], "malformed-preserved");
        assert!(context["elements"][0].get("customData").is_none());
        assert!(serde_json::to_vec(&context["elements"]).unwrap().len() < 66 * 1024);
    }

    #[test]
    fn ambiguous_and_oversized_selections_fail_without_truncation() {
        let duplicate = json!({"document":{"elements":[{"id":"a"},{"id":"a","isDeleted":true}]}});
        assert!(
            select(&duplicate["document"], &["a".into()])
                .unwrap_err()
                .contains("ambiguous")
        );
        let large = json!({"document":{"elements":[{"id":"a","text":"x".repeat(256*1024)}]}});
        assert!(
            select(&large["document"], &["a".into()])
                .unwrap_err()
                .contains("256 KiB")
        );
        assert!(validate(&vec!["a".into(); 21]).is_err());
        assert!(validate(&["x".repeat(513)]).is_err());
    }
}
