//! Shared API/agent operations over current SVG files, independent of web UI.
pub mod bioart;
mod rendered_bounds;
pub mod svg;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use visualization::{Store, image};
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    BioArt(#[from] ::bioart::Error),
    #[error(transparent)]
    Store(#[from] visualization::Error),
    #[error(transparent)]
    Image(#[from] image::InvalidImage),
    #[error(transparent)]
    Vector(#[from] bioicons::Error),
    #[error("{0}")]
    Invalid(&'static str),
}
pub type Result<T> = std::result::Result<T, Error>;
fn current(store: &Store, project: &str, figure: &str, revision: i64) -> Result<Value> {
    if store.get(project, figure)?.revision != revision {
        return Err(visualization::Error::Conflict.into());
    }
    Ok(store.revision(project, figure, revision)?.document)
}
fn svg(document: &Value, target: &str) -> Result<Vec<u8>> {
    let element = image::element(document, target)?;
    let file = element["fileId"]
        .as_str()
        .ok_or(Error::Invalid("image has no file"))?;
    let data = document["files"][file]["dataURL"]
        .as_str()
        .and_then(|v| v.strip_prefix("data:image/svg+xml;base64,"))
        .ok_or(Error::Invalid("select an SVG image"))?;
    if data.len() > 6 * 1024 * 1024 {
        return Err(Error::Invalid("SVG exceeds size limit"));
    }
    STANDARD
        .decode(data)
        .map_err(|_| Error::Invalid("invalid SVG encoding"))
}
pub fn palette(
    store: &Store,
    project: &str,
    figure: &str,
    revision: i64,
    target: &str,
) -> Result<Value> {
    let document = current(store, project, figure, revision)?;
    let mut value = palette_document(&document, target)?;
    value["revision"] = json!(revision);
    Ok(value)
}
pub fn palette_document(document: &Value, target: &str) -> Result<Value> {
    let bytes = svg(document, target)?;
    let vector = bioicons::vector::normalize(&bytes)?;
    if vector.text_outlined {
        return Err(Error::Invalid(
            "outline SVG text before recoloring; font substitution is not part of a colour edit",
        ));
    }
    let element = image::element(document, target)?;
    Ok(
        json!({"element_id":element["id"],"file_id":element["fileId"],"colors":vector.colors,"previewDataURL":vector.preview_data_url,"provenance":element["customData"]}),
    )
}
pub fn recolor_document(
    document: &Value,
    target: &str,
    colors: &BTreeMap<String, String>,
) -> Result<Value> {
    let bytes = svg(document, target)?;
    let vector = bioicons::vector::recolor(&bytes, colors)?;
    replace_vector(
        document,
        target,
        vector,
        json!({"kind":"recolor","colors":colors}),
    )
}
fn replace_vector(
    document: &Value,
    target: &str,
    vector: bioicons::vector::PreparedSvg,
    mut change: Value,
) -> Result<Value> {
    let before = image::element(document, target)?;
    let based_on_sha256 = format!("{:x}", Sha256::digest(svg(document, target)?));
    let mut provenance = before
        .get("customData")
        .filter(|v| v.is_object())
        .cloned()
        .unwrap_or(json!({}));
    if provenance.get("source_sha256").is_none() {
        provenance["source_sha256"] = json!(based_on_sha256);
    }
    if provenance.get("modifications").is_none() {
        provenance["modifications"] = json!([]);
    }
    change["based_on_file_id"] = before["fileId"].clone();
    change["based_on_sha256"] = json!(based_on_sha256);
    change["result_sha256"] = json!(vector.sha256);
    change["created_at"] = json!(chrono::Utc::now().to_rfc3339());
    provenance["modifications"]
        .as_array_mut()
        .ok_or(Error::Invalid("image modifications must be a list"))?
        .push(change);
    provenance["sha256"] = json!(vector.sha256);
    provenance["colors"] = json!(vector.colors);
    Ok(image::replace_file(
        document,
        target,
        image::Image {
            file_id: &vector.sha256,
            data_url: &vector.data_url,
            width: vector.width,
            height: vector.height,
            provenance,
        },
    )?)
}
pub fn recolor(
    store: &Store,
    project: &str,
    figure: &str,
    revision: i64,
    target: &str,
    colors: &BTreeMap<String, String>,
) -> Result<visualization::Visualization> {
    let document = current(store, project, figure, revision)?;
    let next = recolor_document(&document, target, colors)?;
    Ok(store.save(
        project,
        figure,
        revision,
        &store.get(project, figure)?.title,
        &next,
    )?)
}

/// Read paths from this exact saved file, including a numbered review visual.
pub fn parts(
    store: &Store,
    project: &str,
    figure: &str,
    revision: i64,
    target: &str,
) -> Result<Value> {
    let document = current(store, project, figure, revision)?;
    let mut value = parts_document(&document, target)?;
    value["revision"] = json!(revision);
    Ok(value)
}
pub fn parts_document(document: &Value, target: &str) -> Result<Value> {
    let bytes = svg(document, target)?;
    let mut value = serde_json::to_value(bioicons::parts::inspect(&bytes)?)
        .map_err(|_| Error::Invalid("cannot serialize SVG parts"))?;
    let element = image::element(document, target)?;
    value["element_id"] = element["id"].clone();
    value["file_id"] = element["fileId"].clone();
    Ok(value)
}
pub fn edit_parts_document(
    document: &Value,
    target: &str,
    sha256: &str,
    edits: &[bioicons::parts::Edit],
) -> Result<Value> {
    let vector = bioicons::parts::edit(&svg(document, target)?, sha256, edits)?;
    replace_vector(
        document,
        target,
        vector,
        json!({"kind":if edits.iter().any(|e|e.translate.is_some() || e.points.is_some()) {"path_edit"}else{"path_style"},"edits":edits}),
    )
}
pub fn edit_parts(
    store: &Store,
    project: &str,
    figure: &str,
    revision: i64,
    target: &str,
    sha256: &str,
    edits: &[bioicons::parts::Edit],
) -> Result<visualization::Visualization> {
    let document = current(store, project, figure, revision)?;
    let next = edit_parts_document(&document, target, sha256, edits)?;
    Ok(store.save(
        project,
        figure,
        revision,
        &store.get(project, figure)?.title,
        &next,
    )?)
}

/// Read endpoint/control handles for one exact file snapshot.
pub fn geometry(
    store: &Store,
    project: &str,
    figure: &str,
    revision: i64,
    target: &str,
    sha256: &str,
    part_id: &str,
) -> Result<Value> {
    let document = current(store, project, figure, revision)?;
    let mut value = geometry_document(&document, target, sha256, part_id)?;
    value["revision"] = json!(revision);
    Ok(value)
}
pub fn geometry_document(
    document: &Value,
    target: &str,
    sha256: &str,
    part_id: &str,
) -> Result<Value> {
    let geometry = bioicons::geometry::inspect(&svg(document, target)?, sha256, part_id)?;
    let mut value = serde_json::to_value(geometry)
        .map_err(|_| Error::Invalid("cannot serialize path geometry"))?;
    let image = image::element(document, target)?;
    value["element_id"] = image["id"].clone();
    value["file_id"] = image["fileId"].clone();
    Ok(value)
}

/// Stored file dimensions plus the current visible crop; no dimensions are guessed by the agent.
pub fn image_info(
    store: &Store,
    project: &str,
    figure: &str,
    revision: i64,
    target: &str,
) -> Result<Value> {
    let document = current(store, project, figure, revision)?;
    let mut info = image_info_document(&document, target)?;
    info["revision"] = json!(revision);
    Ok(info)
}
/// Inspect exactly the supplied image snapshot; the caller must validate its scope.
pub fn image_info_document(document: &Value, target: &str) -> Result<Value> {
    let source = image::element(document, target)?;
    let file = source["fileId"]
        .as_str()
        .ok_or(Error::Invalid("image has no file"))?;
    let url = document["files"][file]["dataURL"]
        .as_str()
        .ok_or(Error::Invalid("image file is missing"))?;
    let (header, data) = url
        .split_once(',')
        .ok_or(Error::Invalid("invalid image data"))?;
    if !header.ends_with(";base64") || data.len() > 48 * 1024 * 1024 {
        return Err(Error::Invalid("requires an embedded image under 36 MiB"));
    }
    let bytes = STANDARD
        .decode(data)
        .map_err(|_| Error::Invalid("invalid image encoding"))?;
    let (width, height, svg_alpha_bounds) = if header == "data:image/svg+xml;base64" {
        let v = bioicons::vector::normalize(&bytes)?;
        let bounds = rendered_bounds::preview_alpha_bounds(&v.preview_data_url)?;
        (v.width, v.height, Some(bounds))
    } else {
        let (width, height) = ::image::ImageReader::new(std::io::Cursor::new(&bytes))
            .with_guessed_format()
            .map_err(|_| Error::Invalid("invalid image header"))?
            .into_dimensions()
            .map_err(|_| Error::Invalid("unsupported image dimensions"))?;
        (width, height, None)
    };
    if width == 0 || height == 0 || width > 100000 || height > 100000 {
        return Err(Error::Invalid("invalid natural image size"));
    }
    let crop = visualization::inset::crop(source, width, height)?;
    let mut info = json!({"file_id":file,"element_id":source["id"],"natural_width":width,"natural_height":height,"visible_crop":crop,"scale":source["scale"],"angle":source["angle"],"sha256":format!("{:x}",Sha256::digest(&bytes)),"dataURL":url,"provenance":source["customData"]});
    if let Some(mut bounds) = svg_alpha_bounds {
        bounds["file_sha256"] = info["sha256"].clone();
        info["svg_preview_alpha_bounds"] = bounds;
    }
    Ok(info)
}
pub fn inset(
    store: &Store,
    project: &str,
    figure: &str,
    revision: i64,
    spec: &visualization::inset::Spec,
) -> Result<visualization::Visualization> {
    let info = image_info(store, project, figure, revision, &spec.element_id)?;
    let document = current(store, project, figure, revision)?;
    let next = visualization::inset::compile(
        &document,
        spec,
        info["natural_width"].as_u64().unwrap() as u32,
        info["natural_height"].as_u64().unwrap() as u32,
        revision,
        info["sha256"].as_str().unwrap(),
    )?;
    Ok(store.save(
        project,
        figure,
        revision,
        &store.get(project, figure)?.title,
        &next,
    )?)
}

/// Compare against the recorded source snapshot, not the figure revision number alone.
pub fn inset_status(
    store: &Store,
    project: &str,
    figure: &str,
    revision: i64,
    target: &str,
) -> Result<Value> {
    let document = current(store, project, figure, revision)?;
    let m = visualization::inset_refresh::members(&document, target)?;
    let source_id = m.reference["source_element_id"].as_str().unwrap();
    let source_revision = m.reference["source_revision"]
        .as_i64()
        .filter(|v| *v > 0)
        .ok_or(Error::Invalid("invalid source revision"))?;
    let base = json!({"anchor_id":m.anchor["id"],"image_id":m.image["id"],"source_element_id":source_id,"source_revision":source_revision,"current_revision":revision,"region":m.reference["region"]});
    let mut result = base;
    if image::element(&document, source_id).is_err() {
        result["source_available"] = json!(false);
        result["changed"] = json!(true);
        return Ok(result);
    }
    let source = image::element(&document, source_id)?;
    let info = image_info(store, project, figure, revision, source_id)?;
    let historical = match store.revision(project, figure, source_revision) {
        Ok(scene) => Some(scene.document),
        Err(visualization::Error::NotFound) => None,
        Err(error) => return Err(error.into()),
    };
    let old = historical
        .as_ref()
        .and_then(|doc| image::element(doc, source_id).ok());
    let geometry_changed = old.is_none_or(|old| {
        [
            "x", "y", "width", "height", "angle", "scale", "crop", "frameId", "opacity",
        ]
        .iter()
        .any(|key| old[*key] != source[*key])
    });
    let metadata_changed = old.is_none_or(|old| old["customData"] != source["customData"]);
    let file_changed = info["sha256"] != m.reference["source_sha256"]
        || source["fileId"] != m.reference["source_file_id"];
    let detail = image_info(
        store,
        project,
        figure,
        revision,
        m.image["id"].as_str().unwrap(),
    )?;
    result.as_object_mut().unwrap().extend(json!({"source_available":true,"historical_source_available":old.is_some(),"geometry_changed":geometry_changed,"metadata_changed":metadata_changed,"file_changed":file_changed,"changed":geometry_changed||metadata_changed||file_changed,"detail_edited":detail["sha256"]!=m.reference["source_sha256"] || m.image["crop"]!=m.reference["crop"],"source_sha256":info["sha256"],"snapshot_sha256":m.reference["source_sha256"],"current_detail_sha256":detail["sha256"]}).as_object().unwrap().clone());
    Ok(result)
}
pub fn refresh_inset(
    store: &Store,
    project: &str,
    figure: &str,
    revision: i64,
    target: &str,
    discard_detail_edits: bool,
) -> Result<visualization::Visualization> {
    let status = inset_status(store, project, figure, revision, target)?;
    if status["source_available"] != true {
        return Err(Error::Invalid(
            "source image is missing; restore it before refreshing",
        ));
    }
    if status["detail_edited"] == true && !discard_detail_edits {
        return Err(Error::Invalid(
            "inset has local file or crop edits; inspect and explicitly allow replacing them",
        ));
    }
    let info = image_info(
        store,
        project,
        figure,
        revision,
        status["source_element_id"].as_str().unwrap(),
    )?;
    let document = current(store, project, figure, revision)?;
    let next = visualization::inset_refresh::compile(
        &document,
        target,
        info["natural_width"].as_u64().unwrap() as u32,
        info["natural_height"].as_u64().unwrap() as u32,
        revision,
        info["sha256"].as_str().unwrap(),
        status["current_detail_sha256"].as_str().unwrap(),
    )?;
    Ok(store.save(
        project,
        figure,
        revision,
        &store.get(project, figure)?.title,
        &next,
    )?)
}
