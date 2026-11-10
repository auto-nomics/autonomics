//! Append an embedded image while preserving all other scene content.
use serde_json::{Value, json};

#[derive(Debug, thiserror::Error)]
#[error("invalid image insertion: {0}")]
pub struct InvalidImage(pub &'static str);

pub struct Image<'a> {
    pub file_id: &'a str,
    pub data_url: &'a str,
    pub width: u32,
    pub height: u32,
    pub provenance: Value,
}
/// Returns the new document and element ID. The caller saves using optimistic revision checks.
pub fn insert(
    document: &Value,
    image: Image<'_>,
    x: f64,
    y: f64,
) -> Result<(Value, String), InvalidImage> {
    let mime_type = if image.data_url.starts_with("data:image/png;base64,") {
        "image/png"
    } else if image.data_url.starts_with("data:image/svg+xml;base64,") {
        "image/svg+xml"
    } else {
        return Err(InvalidImage("expected embedded PNG or normalized SVG"));
    };
    if !x.is_finite()
        || !y.is_finite()
        || x.abs() > 1e6
        || y.abs() > 1e6
        || image.width == 0
        || image.height == 0
        || !image.provenance.is_object()
        || image.file_id.is_empty()
    {
        return Err(InvalidImage("invalid coordinates or image metadata"));
    }
    crate::scene::apply(document, &[]).map_err(|_| InvalidImage("invalid scene"))?;
    let mut result = document.clone();
    install_file(&mut result, &image, mime_type)?;
    let id = uuid::Uuid::new_v4().to_string();
    let scale = (240.0 / f64::from(image.width.max(image.height))).min(1.0);
    let element = json!({"id":id,"type":"image","x":x,"y":y,"width":f64::from(image.width)*scale,"height":f64::from(image.height)*scale,"angle":0,"strokeColor":"transparent","backgroundColor":"transparent","fillStyle":"solid","strokeWidth":1,"strokeStyle":"solid","roughness":0,"opacity":100,"groupIds":[],"frameId":null,"roundness":null,"seed":1,"version":1,"versionNonce":1,"isDeleted":false,"boundElements":[],"updated":chrono::Utc::now().timestamp_millis(),"link":null,"locked":false,"fileId":image.file_id,"status":"saved","scale":[1,1],"crop":null,"customData":image.provenance});
    result["elements"]
        .as_array_mut()
        .ok_or(InvalidImage("elements must be an array"))?
        .push(element);
    Ok((result, id))
}
fn install_file(
    result: &mut Value,
    image: &Image<'_>,
    mime_type: &str,
) -> Result<(), InvalidImage> {
    if result.get("files").is_none() {
        result["files"] = json!({});
    }
    let files = result["files"]
        .as_object_mut()
        .ok_or(InvalidImage("files must be an object"))?;
    if let Some(existing) = files.get(image.file_id) {
        if existing["dataURL"].as_str() != Some(image.data_url) {
            return Err(InvalidImage("file id collision"));
        }
    } else {
        files.insert(image.file_id.into(), json!({"id":image.file_id,"dataURL":image.data_url,"mimeType":mime_type,"created":chrono::Utc::now().timestamp_millis()}));
    }
    Ok(())
}
/// Resolve an image directly or through its existing semantic anchor.
pub fn element<'a>(document: &'a Value, id: &str) -> Result<&'a Value, InvalidImage> {
    let elements = document["elements"]
        .as_array()
        .ok_or(InvalidImage("invalid scene"))?;
    let target = elements
        .iter()
        .find(|e| e["id"] == id && e["isDeleted"] != true)
        .ok_or(InvalidImage("image not found"))?;
    if target["type"] == "image" {
        return Ok(target);
    }
    let visual = target["customData"]["visualAssetElementId"]
        .as_str()
        .ok_or(InvalidImage("target is not an image or illustrated anchor"))?;
    elements
        .iter()
        .find(|e| {
            e["id"] == visual
                && e["type"] == "image"
                && e["isDeleted"] != true
                && e["customData"]["semanticAnchorId"] == id
        })
        .ok_or(InvalidImage("illustrated image not found"))
}
/// Change one image's file/provenance, preserving its ID and full geometry.
/// Existing file records remain immutable for other instances and old revisions.
pub fn replace_file(document: &Value, id: &str, image: Image<'_>) -> Result<Value, InvalidImage> {
    crate::scene::apply(document, &[]).map_err(|_| InvalidImage("invalid scene"))?;
    let target = element(document, id)?;
    let mime = if image.data_url.starts_with("data:image/svg+xml;base64,") {
        "image/svg+xml"
    } else if image.data_url.starts_with("data:image/png;base64,") {
        "image/png"
    } else {
        return Err(InvalidImage("unsupported image type"));
    };
    if image.file_id.is_empty()
        || image.width == 0
        || image.height == 0
        || !image.provenance.is_object()
    {
        return Err(InvalidImage("invalid replacement metadata"));
    }
    let target_id = target["id"]
        .as_str()
        .ok_or(InvalidImage("invalid image ID"))?;
    let mut result = document.clone();
    install_file(&mut result, &image, mime)?;
    let next = result["elements"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|e| e["id"] == target_id)
        .unwrap();
    next["fileId"] = json!(image.file_id);
    next["customData"] = image.provenance;
    crate::scene::touch(next);
    Ok(result)
}
/// Replace a visual placeholder with an image while preserving its semantic anchor.
/// The anchor keeps its ID, bounds and arrow bindings. Image and detached labels
/// form one movable group; the image retains its aspect ratio inside those bounds.
pub fn illustrate(
    document: &Value,
    target_id: &str,
    image: Image<'_>,
) -> Result<(Value, String), InvalidImage> {
    let source = document["elements"]
        .as_array()
        .ok_or(InvalidImage("elements must be an array"))?;
    let index = source
        .iter()
        .position(|element| {
            element["id"].as_str() == Some(target_id) && element["isDeleted"] != true
        })
        .ok_or(InvalidImage("target not found"))?;
    let index = if source[index]["type"] == "image" {
        source[index]["customData"]["semanticAnchorId"]
            .as_str()
            .and_then(|parent| {
                source.iter().position(|element| {
                    element["id"].as_str() == Some(parent)
                        && element["type"] == "rectangle"
                        && element["customData"]["visualAssetElementId"].as_str() == Some(target_id)
                })
            })
            .unwrap_or(index)
    } else {
        index
    };
    let target = &source[index];
    let target_id = target["id"].as_str().unwrap();
    if !matches!(
        target["type"].as_str(),
        Some("rectangle" | "ellipse" | "diamond" | "image")
    ) {
        return Err(InvalidImage("target must be a shape or image"));
    }
    let number = |key: &str| {
        target[key]
            .as_f64()
            .filter(|value| value.is_finite())
            .ok_or(InvalidImage("target geometry"))
    };
    let (x, y, width, height) = (
        number("x")?,
        number("y")?,
        number("width")?,
        number("height")?,
    );
    if width <= 0.0 || height <= 0.0 {
        return Err(InvalidImage("target dimensions"));
    }
    let aspect = f64::from(image.width) / f64::from(image.height);
    let image_width = width.min(height * aspect);
    let image_height = image_width / aspect;
    let (mut result, new_id) = insert(document, image, x, y)?;
    let group = target["customData"]["visualAssetGroupId"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let previous = target["customData"]["visualAssetElementId"].as_str();
    let mut groups = target["groupIds"].as_array().cloned().unwrap_or_default();
    if !groups.iter().any(|value| value.as_str() == Some(&group)) {
        groups.push(json!(group));
    }
    let mut label_ids: Vec<String> = target["customData"]["visualLabelIds"]
        .as_array()
        .map(|ids| {
            ids.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    label_ids.retain(|id| {
        source.iter().any(|element| {
            element["id"].as_str() == Some(id)
                && element["type"] == "text"
                && element["groupIds"]
                    .as_array()
                    .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(&group)))
        })
    });
    for element in source {
        if element["type"] == "text"
            && element["containerId"].as_str() == Some(target_id)
            && !label_ids
                .iter()
                .any(|id| element["id"].as_str() == Some(id))
        {
            if let Some(id) = element["id"].as_str() {
                label_ids.push(id.into());
            }
        }
    }
    let elements = result["elements"].as_array_mut().unwrap();
    let mut visual = elements.pop().unwrap();
    visual["x"] = json!(x + (width - image_width) / 2.0);
    visual["y"] = json!(y + (height - image_height) / 2.0);
    visual["width"] = json!(image_width);
    visual["height"] = json!(image_height);
    visual["angle"] = target.get("angle").cloned().unwrap_or(json!(0));
    visual["frameId"] = target.get("frameId").cloned().unwrap_or(Value::Null);
    visual["groupIds"] = json!(groups);
    visual["customData"]["semanticAnchorId"] = json!(target_id);
    let previous_position = previous.and_then(|id| {
        elements.iter().enumerate().position(|(position, element)| {
            position != index
                && element["id"].as_str() == Some(id)
                && element["type"] == "image"
                && element["customData"]["semanticAnchorId"].as_str() == Some(target_id)
        })
    });
    let visual_id = if let Some(position) = previous_position {
        let id = elements[position]["id"].as_str().unwrap().to_owned();
        visual["id"] = json!(id);
        visual["version"] = json!(
            elements[position]["version"]
                .as_u64()
                .unwrap_or(0)
                .saturating_add(1)
        );
        elements[position] = visual;
        id
    } else {
        elements.push(visual);
        new_id
    };
    let anchor = &mut elements[index];
    let anchor_object = anchor.as_object_mut().unwrap();
    for field in ["fileId", "status", "scale", "crop"] {
        anchor_object.remove(field);
    }
    anchor["type"] = json!("rectangle");
    anchor["strokeColor"] = json!("transparent");
    anchor["backgroundColor"] = json!("transparent");
    anchor["strokeWidth"] = json!(0);
    anchor["groupIds"] = json!(groups);
    if let Some(bindings) = anchor
        .get_mut("boundElements")
        .and_then(Value::as_array_mut)
    {
        bindings.retain(|binding| binding["type"] != "text");
    }
    if !anchor.get("customData").is_some_and(Value::is_object) {
        anchor["customData"] = json!({});
    }
    anchor["customData"]["visualAssetElementId"] = json!(visual_id);
    anchor["customData"]["visualAssetGroupId"] = json!(group);
    anchor["customData"]["visualLabelIds"] = json!(label_ids);
    anchor["version"] = json!(target["version"].as_u64().unwrap_or(0).saturating_add(1));
    anchor["updated"] = json!(chrono::Utc::now().timestamp_millis());
    for element in elements.iter_mut().filter(|element| {
        label_ids
            .iter()
            .any(|id| element["id"].as_str() == Some(id))
    }) {
        element["containerId"] = Value::Null;
        element["groupIds"] = json!(groups);
        element["x"] = json!(x + (width - element["width"].as_f64().unwrap_or(width)) / 2.0);
        element["y"] = json!(y + height + 12.0);
    }
    Ok((result, visual_id))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn illustration_keeps_arrow_anchors_and_reuses_the_visual_slot() {
        let scene = json!({"elements":[{"id":"cell","type":"ellipse","x":10,"y":20,"width":220,"height":100,"boundElements":[{"id":"label","type":"text"},{"id":"arrow","type":"arrow"}],"customData":{"scientificId":"dendritic-cell"}},{"id":"label","type":"text","containerId":"cell","width":200,"text":"Cell"},{"id":"arrow","type":"arrow","startBinding":{"elementId":"cell","gap":10}}],"files":{}});
        let image = || Image {
            file_id: "hash",
            data_url: "data:image/png;base64,abc",
            width: 200,
            height: 200,
            provenance: json!({"source":"NIH BioArt"}),
        };
        let (result, id) = illustrate(&scene, "cell", image()).unwrap();
        assert_eq!(result["elements"][0]["id"], "cell");
        assert_eq!(result["elements"][0]["width"], 220);
        assert_eq!(result["elements"][2], scene["elements"][2]);
        assert_eq!(result["elements"][1]["containerId"], Value::Null);
        assert_eq!(result["elements"][3]["width"], 100.0);
        assert_eq!(result["elements"][3]["height"], 100.0);
        assert_eq!(
            result["elements"][0]["customData"]["scientificId"],
            "dendritic-cell"
        );
        let (again, same_id) = illustrate(&result, "cell", image()).unwrap();
        assert_eq!(id, same_id);
        assert_eq!(again["elements"].as_array().unwrap().len(), 4);
        let (by_visual, same_visual) = illustrate(&again, &id, image()).unwrap();
        assert_eq!(same_visual, id);
        assert_eq!(by_visual["elements"].as_array().unwrap().len(), 4);
        assert!(illustrate(&scene, "arrow", image()).is_err());
    }
    #[test]
    fn insertion_preserves_scene_and_provenance() {
        let scene = json!({"elements":[{"id":"label","type":"text","text":"Cell"}],"appState":{"zoom":1},"files":{}});
        let image = || Image {
            file_id: "hash",
            data_url: "data:image/png;base64,abc",
            width: 480,
            height: 240,
            provenance: json!({"source":"NIH BioArt","licenses":["CC0"]}),
        };
        let (result, id) = insert(&scene, image(), 10.0, 20.0).unwrap();
        assert_eq!(result["elements"][0], scene["elements"][0]);
        assert_eq!(result["elements"][1]["id"], id);
        assert_eq!(result["elements"][1]["width"], 240.0);
        assert_eq!(result["elements"][1]["customData"]["licenses"][0], "CC0");
        assert_eq!(result["appState"], scene["appState"]);
        let (again, _) = insert(&result, image(), 30.0, 40.0).unwrap();
        assert_eq!(again["files"].as_object().unwrap().len(), 1);
        assert!(insert(&scene, image(), f64::NAN, 0.0).is_err());
    }
}
