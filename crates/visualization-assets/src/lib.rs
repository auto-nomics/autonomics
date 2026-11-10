//! Shared API/agent operations over current SVG files, independent of web UI.
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use visualization::{Store, image};
#[derive(Debug, thiserror::Error)]
pub enum Error {
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
    let bytes = svg(&document, target)?;
    let vector = bioicons::vector::normalize(&bytes)?;
    if vector.text_outlined {
        return Err(Error::Invalid(
            "outline SVG text before recoloring; font substitution is not part of a colour edit",
        ));
    }
    let element = image::element(&document, target)?;
    Ok(
        json!({"element_id":element["id"],"revision":revision,"file_id":element["fileId"],"colors":vector.colors,"previewDataURL":vector.preview_data_url,"provenance":element["customData"]}),
    )
}
pub fn recolor_document(
    document: &Value,
    target: &str,
    colors: &BTreeMap<String, String>,
) -> Result<Value> {
    let bytes = svg(document, target)?;
    let before = image::element(document, target)?;
    let based_on_sha256 = format!("{:x}", Sha256::digest(&bytes));
    let vector = bioicons::vector::recolor(&bytes, colors)?;
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
    provenance["modifications"].as_array_mut().ok_or(Error::Invalid("image modifications must be a list"))?.push(json!({"kind":"recolor","based_on_file_id":before["fileId"],"based_on_sha256":based_on_sha256,"colors":colors,"result_sha256":vector.sha256,"created_at":chrono::Utc::now().to_rfc3339()}));
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
) -> Result<visualization::proposal::Proposal> {
    let document = current(store, project, figure, revision)?;
    let next = recolor_document(&document, target, colors)?;
    Ok(store.propose(project, figure, revision, "调整矢量素材配色", &next)?)
}
