//! Append a compiled native graph without replacing an existing reference/figure.
use crate::{scene::SceneError, sketch::Plan};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

fn fail(message: impl Into<String>) -> SceneError {
    SceneError::InvalidSketch(message.into())
}

pub(crate) fn apply(
    existing: &mut Vec<Value>,
    plan: &Plan,
    prefix: &str,
    x: f64,
    y: f64,
) -> Result<Vec<String>, SceneError> {
    if prefix.is_empty()
        || prefix.len() > 64
        || !prefix
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        || ![x, y].iter().all(|v| v.is_finite() && v.abs() <= 1e6)
    {
        return Err(fail(
            "requires a 1..64 ASCII identifier prefix and finite x/y within ±1000000",
        ));
    }
    let compiled = crate::sketch::compile(plan).map_err(|error| fail(error.to_string()))?;
    let review = crate::geometry_review::review(&compiled);
    if review.status != "available" {
        return Err(fail(
            "compiled graph does not have complete supported bounds",
        ));
    }
    let bounds = review
        .bounds
        .ok_or_else(|| fail("compiled graph is empty"))?;
    let mut added = compiled["elements"]
        .as_array()
        .ok_or_else(|| fail("compiled graph has no objects"))?
        .clone();
    if existing.len().saturating_add(added.len()) > 5000 {
        return Err(fail("combined scene exceeds 5000 objects"));
    }
    let mapping: HashMap<String, String> = added
        .iter()
        .map(|e| {
            let id = e["id"].as_str().expect("compiler emits native IDs");
            (id.to_owned(), format!("{prefix}:e:{id}"))
        })
        .collect();
    let occupied: HashSet<&str> = existing.iter().filter_map(|e| e["id"].as_str()).collect();
    if mapping.values().any(|id| occupied.contains(id.as_str())) {
        return Err(fail(
            "generated identity collides with an existing object; choose a fresh id_prefix",
        ));
    }
    let remap = |value: &mut Value| -> Result<(), SceneError> {
        if value.is_null() {
            return Ok(());
        }
        let id = value
            .as_str()
            .and_then(|id| mapping.get(id))
            .ok_or_else(|| fail("compiled graph contains an unresolved native reference"))?;
        *value = json!(id);
        Ok(())
    };
    for element in &mut added {
        remap(&mut element["id"])?;
        for field in ["containerId", "frameId"] {
            if let Some(value) = element.get_mut(field) {
                remap(value)?;
            }
        }
        for field in ["startBinding", "endBinding"] {
            if let Some(binding) = element.get_mut(field).filter(|b| !b.is_null()) {
                remap(&mut binding["elementId"])?;
            }
        }
        if let Some(bound) = element["boundElements"].as_array_mut() {
            for reference in bound {
                remap(&mut reference["id"])?;
            }
        }
        if element["groupIds"]
            .as_array()
            .is_some_and(|g| !g.is_empty())
        {
            return Err(fail("compiler returned unsupported native groups"));
        }
        for (field, offset) in [("x", x - bounds.x), ("y", y - bounds.y)] {
            let position = element[field]
                .as_f64()
                .ok_or_else(|| fail("compiled position is invalid"))?
                + offset;
            if !position.is_finite() || position.abs() > 1e6 {
                return Err(fail("placed graph exceeds scene coordinate limits"));
            }
            element[field] = json!(position);
        }
    }
    // No scene settings, files, historical plans or old native objects are rewritten.
    let ids = added
        .iter()
        .map(|e| e["id"].as_str().unwrap().to_owned())
        .collect();
    existing.extend(added);
    Ok(ids)
}
