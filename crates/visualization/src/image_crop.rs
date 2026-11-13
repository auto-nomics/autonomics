//! Non-destructive source cropping; retained pixels keep their canvas positions.
use crate::scene::{self, SceneError};
use serde_json::{Value, json};
pub(crate) fn apply(
    elements: &mut [Value],
    id: &str,
    file_id: &str,
    size: [f64; 2],
    region: [f64; 4],
) -> Result<(), SceneError> {
    let fail = |s: &str| SceneError::InvalidImageGeometry(format!("crop_image: {s}"));
    if size
        .iter()
        .any(|n| !n.is_finite() || *n <= 0. || *n > 100000.)
        || region.iter().any(|n| !n.is_finite())
        || region[0] < 0.
        || region[1] < 0.
        || region[2] <= 0.
        || region[3] <= 0.
        || region[0] + region[2] > 1. + 1e-9
        || region[1] + region[3] > 1. + 1e-9
    {
        return Err(fail(
            "requires actual positive source dimensions <=100000 and a nonempty normalized crop inside [0,1]",
        ));
    }
    let selected = elements
        .iter()
        .find(|e| e["id"] == id && e["isDeleted"] != true)
        .ok_or_else(|| SceneError::NotFound(id.into()))?;
    let image_id = if selected["type"] == "image" {
        id
    } else {
        selected["customData"]["visualAssetElementId"]
            .as_str()
            .ok_or_else(|| fail("select an image or illustrated anchor"))?
    };
    let index = elements
        .iter()
        .position(|e| e["id"] == image_id && e["type"] == "image" && e["isDeleted"] != true)
        .ok_or_else(|| fail("missing image"))?;
    let image = &elements[index];
    if image["locked"] == true
        || image["angle"].as_f64() != Some(0.)
        || image["fileId"] != file_id
        || !image["customData"]["inset"].is_null()
        || !image["customData"]["annotationKind"].is_null()
    {
        return Err(fail(
            "requires the exact unlocked unrotated image; insets/annotations use their own controls",
        ));
    }
    let number = |v: &Value, key: &str| {
        v[key]
            .as_f64()
            .filter(|n| n.is_finite() && n.abs() <= 1e6)
            .ok_or_else(|| {
                fail("invalid existing image/crop geometry; clear a malformed crop before retrying")
            })
    };
    let x = number(image, "x")?;
    let y = number(image, "y")?;
    let w = number(image, "width")?;
    let h = number(image, "height")?;
    if w <= 0. || h <= 0. {
        return Err(fail("image dimensions must be positive"));
    }
    let scale = image["scale"]
        .as_array()
        .filter(|a| a.len() == 2 && a.iter().all(|v| matches!(v.as_f64(), Some(1.) | Some(-1.))))
        .ok_or_else(|| fail("requires native numeric flips"))?;
    let old = if image["crop"].is_null() {
        [0., 0., 1., 1.]
    } else {
        let c = &image["crop"];
        if (number(c, "naturalWidth")? - size[0]).abs() > 1e-6
            || (number(c, "naturalHeight")? - size[1]).abs() > 1e-6
        {
            return Err(fail("source dimensions do not match the current crop"));
        }
        [
            number(c, "x")? / size[0],
            number(c, "y")? / size[1],
            number(c, "width")? / size[0],
            number(c, "height")? / size[1],
        ]
    };
    if old[2] <= 0.
        || old[3] <= 0.
        || region[0] < old[0] - 1e-9
        || region[1] < old[1] - 1e-9
        || region[0] + region[2] > old[0] + old[2] + 1e-9
        || region[1] + region[3] > old[1] + old[3] + 1e-9
    {
        return Err(fail(
            "new crop must be inside the current visible source region; return to the previous candidate to restore removed margins",
        ));
    }
    let primary = image["customData"]["semanticAnchorId"].as_str();
    let badge = image["customData"]["semanticBadgeAnchorId"].as_str();
    if primary.is_some() && badge.is_some() {
        return Err(fail("ambiguous image ownership"));
    }
    let owner = if let Some(owner) = primary.or(badge) {
        let idx = elements
            .iter()
            .position(|e| e["id"] == owner && e["isDeleted"] != true)
            .ok_or_else(|| fail("missing image owner"))?;
        let anchor = &elements[idx];
        let group = anchor["customData"]["visualAssetGroupId"]
            .as_str()
            .ok_or_else(|| fail("missing owner group"))?;
        let reciprocal = if primary.is_some() {
            anchor["customData"]["visualAssetElementId"] == image_id
        } else {
            anchor["customData"]["visualBadgeIds"]
                .as_array()
                .is_some_and(|a| a.iter().any(|v| v == image_id))
        };
        if !reciprocal
            || anchor["locked"] == true
            || image["frameId"] != anchor["frameId"]
            || !image["groupIds"]
                .as_array()
                .is_some_and(|a| a.iter().any(|v| v == group))
        {
            return Err(fail(
                "requires reciprocal unlocked same-frame component ownership and group",
            ));
        }
        Some(idx)
    } else {
        if elements.iter().any(|e| {
            e["customData"]["visualAssetElementId"] == image_id
                || e["customData"]["visualBadgeIds"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|v| v == image_id))
        }) {
            return Err(fail("image ownership is inconsistent"));
        }
        None
    };
    if elements.iter().any(|e| {
        e["isDeleted"] != true
            && ["containerId", "startBinding", "endBinding"]
                .iter()
                .any(|key| {
                    if *key == "containerId" {
                        e[*key] == image_id
                    } else {
                        e[*key]["elementId"] == image_id
                    }
                })
    }) {
        return Err(fail("image has directly bound objects; use native editing"));
    }
    let dx = if scale[0].as_f64() == Some(1.) {
        region[0] - old[0]
    } else {
        old[0] + old[2] - region[0] - region[2]
    };
    let dy = if scale[1].as_f64() == Some(1.) {
        region[1] - old[1]
    } else {
        old[1] + old[3] - region[1] - region[3]
    };
    let next = [
        x + dx / old[2] * w,
        y + dy / old[3] * h,
        w * region[2] / old[2],
        h * region[3] / old[3],
    ];
    if next.iter().any(|n| !n.is_finite() || n.abs() > 1e6) {
        return Err(fail("cropped geometry outside canvas range"));
    }
    let crop = json!({"x":region[0]*size[0],"y":region[1]*size[1],"width":region[2]*size[0],"height":region[3]*size[1],"naturalWidth":size[0],"naturalHeight":size[1]});
    if image["crop"] == crop {
        return Ok(());
    }
    for (key, value) in ["x", "y", "width", "height"].into_iter().zip(next) {
        elements[index][key] = json!(value);
    }
    elements[index]["crop"] = crop;
    scene::touch(&mut elements[index]);
    if let Some(owner) = owner {
        if let Some(data) = elements[owner]["customData"].as_object_mut() {
            data.remove("visualAnchorFit");
        }
        scene::touch(&mut elements[owner]);
    }
    Ok(())
}
