//! Explicit, reviewable refresh of a frozen detail. IDs and panel presentation survive.
use crate::{
    image::InvalidImage,
    inset::{self, Region, Spec},
};
use serde_json::{Value, json};

pub struct Members<'a> {
    pub anchor: &'a Value,
    pub image: &'a Value,
    pub region: &'a Value,
    pub callout: &'a Value,
    pub title: &'a Value,
    pub reference: &'a Value,
}
fn find<'a>(elements: &'a [Value], id: &str) -> Result<&'a Value, InvalidImage> {
    elements
        .iter()
        .find(|e| e["id"].as_str() == Some(id) && e["isDeleted"] != true)
        .ok_or(InvalidImage(
            "inset member is missing; restore the complete inset before refreshing",
        ))
}
pub fn members<'a>(document: &'a Value, target: &str) -> Result<Members<'a>, InvalidImage> {
    let elements = document["elements"]
        .as_array()
        .ok_or(InvalidImage("invalid scene"))?;
    let selected = find(elements, target)?;
    let anchor_id = selected["customData"]["inset"]["anchor_id"]
        .as_str()
        .ok_or(InvalidImage("select an inset anchor or image"))?;
    let anchor = find(elements, anchor_id)?;
    let reference = &anchor["customData"]["inset"];
    let id = |key: &str| {
        reference[key]
            .as_str()
            .ok_or(InvalidImage("invalid inset identity"))
    };
    if reference["kind"] != "image_inset_snapshot"
        || id("anchor_id")? != anchor_id
        || anchor["type"] != "rectangle"
    {
        return Err(InvalidImage(
            "inset identity changed; refresh cannot infer ownership",
        ));
    }
    let image = find(elements, id("image_id")?)?;
    if selected["id"] != anchor["id"] && selected["id"] != image["id"] {
        return Err(InvalidImage(
            "select the recorded inset anchor or image, not a copied member",
        ));
    }
    let region = find(elements, id("region_id")?)?;
    let callout = find(elements, id("callout_id")?)?;
    let title_id = anchor["customData"]["visualLabelIds"]
        .as_array()
        .filter(|v| v.len() == 1)
        .and_then(|v| v[0].as_str())
        .ok_or(InvalidImage("inset title identity changed"))?;
    let title = find(elements, title_id)?;
    let owned = [
        anchor_id,
        id("image_id")?,
        id("region_id")?,
        id("callout_id")?,
        title_id,
    ];
    if owned
        .into_iter()
        .collect::<std::collections::HashSet<_>>()
        .len()
        != 5
        || owned.contains(&id("source_element_id")?)
        || anchor["customData"]["visualAssetElementId"] != image["id"]
        || image["customData"]["semanticAnchorId"] != anchor["id"]
        || image["type"] != "image"
        || title["type"] != "text"
        || region["type"] != "rectangle"
        || region["customData"]["annotationKind"] != "inset_region"
        || callout["type"] != "arrow"
        || callout["customData"]["annotationKind"] != "inset_callout"
        || [image, region, callout]
            .iter()
            .any(|e| e["customData"]["inset"] != *reference)
    {
        return Err(InvalidImage(
            "inset ownership changed; refresh cannot infer members",
        ));
    }
    Ok(Members {
        anchor,
        image,
        region,
        callout,
        title,
        reference,
    })
}
fn number(e: &Value, key: &str) -> Result<f64, InvalidImage> {
    e[key]
        .as_f64()
        .filter(|v| v.is_finite())
        .ok_or(InvalidImage("invalid inset geometry"))
}
fn copy_fields(destination: &mut Value, source: &Value, fields: &[&str]) {
    for field in fields {
        destination[*field] = source[*field].clone();
    }
}
pub fn compile(
    document: &Value,
    target: &str,
    width: u32,
    height: u32,
    revision: i64,
    sha256: &str,
    previous_sha256: &str,
) -> Result<Value, InvalidImage> {
    let m = members(document, target)?;
    if number(m.anchor, "angle")?.abs() > 1e-9
        || number(m.callout, "angle")?.abs() > 1e-9
        || m.callout["elbowed"] == true
        || m.callout["points"].as_array().is_none_or(|p| p.len() != 2)
    {
        return Err(InvalidImage(
            "restore an unrotated inset panel and straight callout before refreshing",
        ));
    }
    let (x, y, w, h) = (
        number(m.anchor, "x")?,
        number(m.anchor, "y")?,
        number(m.anchor, "width")?,
        number(m.anchor, "height")?,
    );
    if x.abs() > 1e6 || y.abs() > 1e6 || w > 1e6 || h > 1e6 {
        return Err(InvalidImage(
            "inset panel geometry is outside the supported canvas range",
        ));
    }
    let content_top = (number(m.title, "y")? + number(m.title, "height")? + 8.).max(y + 12.);
    let available = [w - 24., y + h - 12. - content_top];
    if available.iter().any(|v| !v.is_finite() || *v < 1.) {
        return Err(InvalidImage("inset panel has no space below its title"));
    }
    let region: Region = serde_json::from_value(m.reference["region"].clone())
        .map_err(|_| InvalidImage("invalid recorded inset region"))?;
    let spec = Spec {
        element_id: m.reference["source_element_id"].as_str().unwrap().into(),
        region,
        x,
        y,
        max_size: available[0].max(available[1]).clamp(32., 2048.),
        title: "Detail".into(),
    };
    let generated = inset::compile(document, &spec, width, height, revision, sha256)?;
    let added = &generated["elements"].as_array().unwrap()
        [document["elements"].as_array().unwrap().len()..];
    let mut reference = added[2]["customData"]["inset"].clone();
    for (key, member) in [
        ("anchor_id", m.anchor),
        ("image_id", m.image),
        ("region_id", m.region),
        ("callout_id", m.callout),
    ] {
        reference[key] = member["id"].clone();
    }
    let mut image = m.image.clone();
    copy_fields(
        &mut image,
        &added[3],
        &["fileId", "crop", "angle", "scale", "status"],
    );
    let angle = number(&image, "angle")?;
    let (s, c) = angle.sin_cos();
    let display = [number(&added[3], "width")?, number(&added[3], "height")?];
    let bounds = [
        c.abs() * display[0] + s.abs() * display[1],
        s.abs() * display[0] + c.abs() * display[1],
    ];
    let factor = (available[0] / bounds[0]).min(available[1] / bounds[1]);
    image["width"] = json!(display[0] * factor);
    image["height"] = json!(display[1] * factor);
    image["x"] = json!(x + w / 2. - display[0] * factor / 2.);
    image["y"] = json!(content_top + available[1] / 2. - display[1] * factor / 2.);
    image["customData"] = added[3]["customData"].clone();
    image["customData"]["semanticAnchorId"] = m.anchor["id"].clone();
    image["customData"]["inset"] = reference.clone();
    for key in ["user_note", "user_notes"] {
        if let Some(value) = m.image["customData"].get(key) {
            image["customData"][key] = value.clone();
        }
    }
    let mut changes = m.image["customData"]["modifications"]
        .as_array()
        .cloned()
        .ok_or(InvalidImage("image modifications must be a list"))?;
    let mut change = added[3]["customData"]["modifications"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    change["kind"] = json!("inset_refresh");
    change["previous_source_revision"] = m.reference["source_revision"].clone();
    change["previous_file_id"] = m.image["fileId"].clone();
    change["previous_sha256"] = json!(previous_sha256);
    changes.push(change);
    image["customData"]["modifications"] = json!(changes);
    let mut region = m.region.clone();
    copy_fields(
        &mut region,
        &added[0],
        &["x", "y", "width", "height", "angle", "frameId"],
    );
    region["customData"]["inset"] = reference.clone();
    let roi_center = [
        number(&region, "x")? + number(&region, "width")? / 2.,
        number(&region, "y")? + number(&region, "height")? / 2.,
    ];
    let panel_center = [x + w / 2., y + h / 2.];
    if (panel_center[0] - roi_center[0]).hypot(panel_center[1] - roi_center[1]) < 1. {
        return Err(InvalidImage(
            "move the inset panel away from the source region before refreshing",
        ));
    }
    let start = inset::edge(
        roi_center,
        [number(&region, "width")?, number(&region, "height")?],
        number(&region, "angle")?,
        panel_center,
    );
    let end = inset::edge(panel_center, [w, h], 0., roi_center);
    let mut callout = m.callout.clone();
    callout["x"] = json!(start[0]);
    callout["y"] = json!(start[1]);
    callout["width"] = json!((end[0] - start[0]).abs());
    callout["height"] = json!((end[1] - start[1]).abs());
    callout["points"] = json!([[0., 0.], [end[0] - start[0], end[1] - start[1]]]);
    callout["startBinding"] = json!({"elementId":m.region["id"],"focus":0,"gap":8});
    callout["endBinding"] = json!({"elementId":m.anchor["id"],"focus":0,"gap":8});
    callout["customData"]["inset"] = reference.clone();
    let mut anchor = m.anchor.clone();
    anchor["customData"]["inset"] = reference;
    let mut next = document.clone();
    for mut replacement in [anchor, image, region, callout] {
        crate::scene::touch(&mut replacement);
        let old = next["elements"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|e| e["id"] == replacement["id"])
            .unwrap();
        *old = replacement;
    }
    Ok(next)
}
