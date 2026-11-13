//! Explicit native node geometry with coherent text and centered connectors.
use crate::scene::{self, SceneError};
use serde_json::{Value, json};
fn fail(message: impl Into<String>) -> SceneError {
    SceneError::InvalidNodeGeometry(message.into())
}
fn unrotated(e: &Value) -> bool {
    e["angle"].is_null() || e["angle"].as_f64() == Some(0.)
}
fn rect(e: &Value) -> Result<[f64; 4], SceneError> {
    let mut result = [0.; 4];
    for (i, key) in ["x", "y", "width", "height"].iter().enumerate() {
        result[i] = e[key]
            .as_f64()
            .filter(|v| v.is_finite() && v.abs() <= 1e6)
            .ok_or_else(|| fail(format!("invalid node/frame {key}")))?;
    }
    if result[2] < 1. || result[3] < 1. {
        return Err(fail(
            "node/frame dimensions must be at least one scene unit",
        ));
    }
    Ok(result)
}
pub(crate) fn apply(elements: &mut [Value], id: &str, bounds: [f64; 4]) -> Result<(), SceneError> {
    if !bounds.iter().all(|v| v.is_finite() && v.abs() <= 1e6) || bounds[2] < 1. || bounds[3] < 1. {
        return Err(fail(
            "x/y and positive width/height require finite numbers within scene limits",
        ));
    }
    let index = elements
        .iter()
        .position(|e| e["id"] == id)
        .ok_or_else(|| SceneError::NotFound(id.into()))?;
    let node = &elements[index];
    if node["isDeleted"] == true
        || node["locked"] == true
        || !unrotated(node)
        || !matches!(
            node["type"].as_str(),
            Some("rectangle" | "ellipse" | "diamond")
        )
    {
        return Err(fail(
            "requires a live unlocked unrotated rectangle, ellipse or diamond",
        ));
    }
    let data = &node["customData"];
    if (!data.is_null() && !data.is_object())
        || [
            "visualAssetElementId",
            "visualLabelIds",
            "visualAssetGroupId",
            "semanticAnchorId",
            "inset",
            "annotationKind",
        ]
        .iter()
        .any(|key| !data[key].is_null())
    {
        return Err(fail(
            "owned illustrations and annotations require their dedicated geometry operation",
        ));
    }
    let old = rect(node)?;
    if let Some(frame_id) = node["frameId"].as_str() {
        let frame = elements
            .iter()
            .find(|e| e["id"] == frame_id && e["isDeleted"] != true)
            .ok_or_else(|| fail("missing node frame"))?;
        if frame["locked"] == true
            || !unrotated(frame)
            || !matches!(frame["type"].as_str(), Some("frame" | "magicframe"))
        {
            return Err(fail("requires a live unlocked unrotated frame"));
        }
        let [x, y, w, h] = rect(frame)?;
        if bounds[0] < x - 1e-6
            || bounds[1] < y - 1e-6
            || bounds[0] + bounds[2] > x + w + 1e-6
            || bounds[1] + bounds[3] > y + h + 1e-6
        {
            return Err(fail("node must remain in its existing frame"));
        }
    } else if !node["frameId"].is_null() {
        return Err(fail("invalid node frame identity"));
    }
    let labels = elements
        .iter()
        .filter(|e| e["type"] == "text" && e["isDeleted"] != true && e["containerId"] == id)
        .map(|e| {
            e["id"]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| fail("missing bound text identity"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if labels.len() > 1 {
        return Err(fail("resolve multiple bound text objects before resizing"));
    }
    let references = node["boundElements"].as_array();
    if !node["boundElements"].is_null() && references.is_none() {
        return Err(fail("invalid reciprocal bindings"));
    }
    let text_refs = references
        .into_iter()
        .flatten()
        .filter(|r| r["type"] == "text")
        .collect::<Vec<_>>();
    if text_refs.len() != labels.len()
        || labels
            .iter()
            .any(|label| !text_refs.iter().any(|r| r["id"] == *label))
    {
        return Err(fail("bound text must have exact reciprocal ownership"));
    }
    let connected = elements
        .iter()
        .filter(|e| {
            e["isDeleted"] != true
                && (e["startBinding"]["elementId"] == id || e["endBinding"]["elementId"] == id)
        })
        .map(|e| {
            e["id"]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| fail("missing connector identity"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    for arrow in &connected {
        if !references
            .into_iter()
            .flatten()
            .any(|r| r["id"] == *arrow && r["type"] == "arrow")
        {
            return Err(fail("connected arrow must have reciprocal node ownership"));
        }
    }
    crate::illustration_fit::adjust_connected(elements, id, old, bounds)
        .map_err(|error| fail(error.to_string()))?;
    for (key, value) in ["x", "y", "width", "height"].into_iter().zip(bounds) {
        elements[index][key] = json!(value);
    }
    scene::touch(&mut elements[index]);
    for label in labels {
        let e = elements.iter().find(|e| e["id"] == label).unwrap();
        let source = e["originalText"]
            .as_str()
            .or_else(|| e["text"].as_str())
            .ok_or_else(|| fail("bound text has no complete source"))?
            .to_owned();
        crate::text_edit::apply(elements, &label, &source, None, None)?;
    }
    for arrow in connected {
        if elements
            .iter()
            .any(|e| e["id"] == arrow && e["customData"]["crossPanel"] == true)
        {
            crate::panel_routes::reroute(elements, &arrow).map_err(|error| fail(error.0))?;
        }
    }
    Ok(())
}
