//! Direct agent access to the project canvas. Every edit commits one atomic revision.
mod add_text;
mod bioart;
mod bioicons;
mod bioicons_batch;
mod catalog_search;
mod crop;
mod evidence;
mod find_space;
mod fit;
mod fit_svg;
mod fit_svg_group;
mod move_text;
mod reflow_text;
mod scale;
mod scoped_read;
mod translate;
use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration};
use agentik_proc::tool;
use agentik_sdk::types::{ToolResult, tools::ToolResultBlock};
use async_trait::async_trait;
pub use bioart::registrations as bioart_registrations;
pub use bioicons::registrations as bioicons_registrations;
pub use evidence::registrations as evidence_registrations;
use serde_json::{Value, json};
use std::path::PathBuf;
use visualization::{Store, service::VisualizationService};
#[allow(dead_code)]
#[derive(schemars::JsonSchema)]
#[serde(untagged)]
enum DocumentSchema {
    Sketch(visualization::sketch::Plan),
    Scene(SceneSchema),
}
#[allow(dead_code)]
#[derive(schemars::JsonSchema)]
struct SceneSchema {
    #[serde(rename = "type")]
    kind: Option<String>,
    version: Option<u32>,
    #[serde(rename = "appState")]
    app_state: Option<Value>,
    elements: Vec<Value>,
    files: Option<std::collections::BTreeMap<String, Value>>,
}
#[allow(dead_code)]
#[derive(schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum ActionSchema {
    Projects,
    List,
    Read,
    Create,
    Sketch,
    Apply,
    Arrange,
    ReadImage,
    ReadPreview,
    ImageInfo,
    Inset,
    InsetStatus,
    RefreshInset,
    ReadMarkup,
    ListMarkups,
}
#[tool(
    name = "visualization",
    description = "Read and directly edit the shared canvas. read returns current objects, revision, annotations and geometry; use element_ids for focused reads. apply atomically commits an operations batch against expected_revision and returns the new revision. Use that revision for the next edit; on conflict read again. Batch related edits. Use insert_sketch to append a diagram; sketch replaces a scene only with replace_existing:true when occupied. arrange changes layout. read_preview returns the current canvas PNG; inspect after substantive edits. Preserve unrelated objects, complete scientific labels and asset provenance. Geometry checks are layout hints, not scientific validation. All edits immediately update the canvas and remain in revision history."
)]
#[serde(deny_unknown_fields)]
pub struct FigureInput {
    #[desc = "projects | list | read | create | sketch | apply | arrange | read_image | read_preview | image_info | inset | inset_status | refresh_inset | read_markup | list_markups"]
    #[schemars(with = "ActionSchema")]
    pub action: String,
    pub project_id: Option<String>,
    pub figure_id: Option<String>,
    pub expected_revision: Option<i64>,
    pub title: Option<String>,
    pub element_id: Option<String>,
    pub element_ids: Option<Vec<String>>,
    pub replace_existing: Option<bool>,
    #[schemars(with = "Option<DocumentSchema>")]
    pub document: Option<Value>,
    #[schemars(with = "Option<Vec<visualization::scene::Operation>>")]
    pub operations: Option<Value>,
    pub arrangement: Option<visualization::arrange::Spec>,
    pub inset: Option<visualization::inset::Spec>,
    pub discard_detail_edits: Option<bool>,
    pub markup_id: Option<String>,
    pub before: Option<String>,
    pub status: Option<String>,
    pub limit: Option<u32>,
}
struct FigureTool {
    path: PathBuf,
}
pub(crate) fn receipt(
    store: &Store,
    record: &visualization::Visualization,
) -> Result<Value, ToolError> {
    let scene = store
        .revision(&record.project_id, &record.id, record.revision)
        .map_err(failure)?;
    Ok(
        json!({"id":record.id,"figure_id":record.id,"project_id":record.project_id,"revision":record.revision,"scene_geometry_review":visualization::geometry_review::review(&scene.document),"follow_up_inputs":{"read":{"action":"read","project_id":record.project_id,"figure_id":record.id,"expected_revision":record.revision},"read_preview":{"action":"read_preview","project_id":record.project_id,"figure_id":record.id,"expected_revision":record.revision}}}),
    )
}
fn failure(error: impl std::error::Error + Send + Sync + 'static) -> ToolError {
    ToolError::ExecutionFailed {
        source: Box::new(error),
    }
}
fn invalid(message: &str) -> ToolError {
    failure(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        message,
    ))
}
fn required(value: Option<String>, name: &str) -> Result<String, ToolError> {
    value
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| invalid(name))
}
fn decode_document<T: serde::de::DeserializeOwned>(
    document: Value,
    root: &str,
) -> Result<T, ToolError> {
    // Internally tagged enum deserialization loses field paths inside variants.
    // Keep precise placement feedback without coercing quoted numbers.
    if root == "operations"
        && let Some(operations) = document.as_array()
    {
        for (index, operation) in operations.iter().enumerate() {
            if operation["op"] == "update" {
                let misplaced = operation.as_object().and_then(|object| {
                    object
                        .keys()
                        .find(|key| !matches!(key.as_str(), "op" | "id" | "changes"))
                });
                if let Some(field) = misplaced {
                    return Err(invalid(&format!(
                        "Invalid operations at operations[{index}].{field}: update native fields belong inside changes, never at the operation top level. Use {{\"op\":\"update\",\"id\":\"existing-id\",\"changes\":{{\"text\":\"complete text\",\"x\":120,\"y\":80}}}}. For scientific text wrapping use set_wrapped_text with id/text/font_size instead. Read the current candidate and retry the intended edit; nothing was saved."
                    )));
                }
                if !operation.get("changes").is_some_and(Value::is_object) {
                    return Err(invalid(&format!(
                        "Invalid operations at operations[{index}].changes: update requires a JSON object of native fields, not an array, string or null. Use {{\"op\":\"update\",\"id\":\"existing-id\",\"changes\":{{\"x\":120,\"y\":80}}}}. Keep numeric values as JSON numbers. Read the current candidate and retry; nothing was saved."
                    )));
                }
            }
            let positional = if operation["op"] == "translate" {
                Some((operation, vec!["dx", "dy"], ""))
            } else if operation["op"] == "update" {
                Some((
                    &operation["changes"],
                    visualization::scene::NUMERIC_UPDATE_FIELDS.to_vec(),
                    ".changes",
                ))
            } else {
                None
            };
            if let Some((value, fields, suffix)) = positional {
                for field in fields {
                    if let Some(value) = value.get(field)
                        && !value.as_f64().is_some_and(f64::is_finite)
                    {
                        return Err(invalid(&format!(
                            "Invalid operations at operations[{index}]{suffix}.{field}: expected a finite JSON number, not a quoted string, null or container. Retry with real JSON numbers, for example {{op:update,id:existing_text_id,changes:{{y:120}}}} for a label-only position; prefer visualization_move_text with one text element_id and top-level numeric dx/dy for an unbound text object. translate moves complete groups/components and must not replace a single grouped-label edit. For text reflow use set_wrapped_text with numeric font_size/max_width. Read the candidate and retry; nothing was saved."
                        )));
                    }
                }
            }
            if operation["op"] == "set_node_geometry" {
                for field in ["x", "y", "width", "height"] {
                    if let Some(value) = operation.get(field)
                        && !value.as_f64().is_some_and(f64::is_finite)
                    {
                        return Err(invalid(&format!(
                            "Invalid operations at operations[{index}].{field}: native node geometry requires a finite JSON number. Use {{\"op\":\"set_node_geometry\",\"id\":\"existing-node\",\"x\":40,\"y\":100,\"width\":260,\"height\":120}} with dimensions read from the current candidate. Text reflows in the new bounds and centered sharp connectors remain attached; nothing was saved."
                        )));
                    }
                }
            }
            if operation["op"] == "set_wrapped_text" || operation["op"] == "add_wrapped_text" {
                for field in ["font_size", "max_width"] {
                    if let Some(value) = operation.get(field)
                        && !value.is_null()
                        && !value.as_f64().is_some_and(f64::is_finite)
                    {
                        return Err(invalid(&format!(
                            "Invalid operations at operations[{index}].{field}: expected a finite JSON number, not a quoted string or container. Use {{op:set_wrapped_text,id:existing_text_id,text:complete_text,font_size:18}} with a real number; max_width applies to unbound text or arrow captions, not shape-bound labels. Read the candidate and retry; nothing was saved."
                        )));
                    }
                }
            }
            if operation["op"] == "fit_illustration_anchor"
                || operation["op"] == "set_illustration_geometry"
            {
                let bounds = operation.get("normalized_bounds").and_then(Value::as_array);
                if !bounds.is_some_and(|values| {
                    values.len() == 4
                        && values
                            .iter()
                            .all(|value| value.as_f64().is_some_and(f64::is_finite))
                }) {
                    return Err(invalid(&format!(
                        "Invalid operations at operations[{index}].normalized_bounds: expected exactly four finite JSON numbers in an array [x,y,width,height], for example [0.35,0.02,0.27,0.96]. Objects such as {{x:...,y:...}}, quoted arrays and quoted numbers are invalid. Read and preserve the reviewed bounds from the exact candidate anchor's customData.visualAnchorFit.normalizedBounds array, or inspect the source image to review new bounds; values are fractions of the whole uncropped, unflipped source image (SVG or PNG), not canvas pixels. Never replace reviewed bounds with the example values. Nothing was saved."
                    )));
                }
            }
            if operation["op"] == "set_image_geometry"
                || operation["op"] == "set_illustration_geometry"
            {
                for field in ["x", "y", "width", "height"] {
                    if let Some(value) = operation.get(field)
                        && !value.as_f64().is_some_and(f64::is_finite)
                    {
                        return Err(invalid(&format!(
                            "Invalid operations at operations[{index}].{field}: expected a finite JSON number, not a quoted string, null or container. Read the image geometry and retry; nothing was saved."
                        )));
                    }
                }
            }
        }
    }
    serde_path_to_error::deserialize(document).map_err(|error| {
        let path=error.path().to_string();
        let location=if path=="." || path.is_empty(){root.to_owned()}else if path.starts_with('['){format!("{root}{path}")}else{format!("{root}.{path}")};
        if root == "operations" {
            return invalid(&format!("Invalid operations at {location}: {}. Use native JSON types in each operation: numbers without quotes, booleans true/false without quotes, and arrays [...] rather than objects or quoted JSON. Match the selected op schema; do not switch operation kinds to bypass a parameter error. Read the current candidate before retrying; nothing was saved.", error.inner()));
        }
        invalid(&format!("Invalid {root} at {location}: {}. Array fields must be JSON arrays [...] (for example {{\"nodes\":[{{\"id\":\"n1\",\"label\":\"Cell\"}}]}}), not {{item:...}} objects or quoted JSON. Read the current document or candidate before retrying; nothing was saved.", error.inner()))
    })
}
fn execute(path: PathBuf, input: FigureInput) -> Result<Value, ToolError> {
    let mut service = VisualizationService::new(Store::open(path).map_err(failure)?);
    if input.element_ids.is_some() && input.action != "read" {
        return Err(invalid("element_ids is only supported for read"));
    }
    if input.action == "projects" {
        return Ok(json!(service.store().projects().map_err(failure)?));
    }
    let project = required(input.project_id, "project_id is required")?;
    if input.action == "list" {
        return Ok(json!(service.store().list(&project).map_err(failure)?));
    }
    if matches!(input.action.as_str(), "create" | "sketch") {
        let title = required(input.title, "title is required")?;
        let document = input
            .document
            .ok_or_else(|| invalid("document is required"))?;
        let document = if input.action == "sketch" {
            visualization::sketch::compile(&decode_document(document, "document")?)
                .map_err(failure)?
        } else {
            document
        };
        let record = if let Some(id) = input.figure_id {
            let rev = input
                .expected_revision
                .ok_or_else(|| invalid("expected_revision is required"))?;
            let current = service.current(&project, &id).map_err(failure)?;
            if current.revision != rev {
                return Err(failure(visualization::Error::Conflict));
            }
            let occupied = current.document["elements"]
                .as_array()
                .is_some_and(|v| v.iter().any(|e| e["isDeleted"] != true))
                || current.document["files"]
                    .as_object()
                    .is_some_and(|f| !f.is_empty());
            if occupied && input.replace_existing != Some(true) {
                return Err(invalid(
                    "use apply with insert_sketch to append; replacing an occupied canvas requires replace_existing:true",
                ));
            }
            service
                .store()
                .save(&project, &id, rev, &title, &document)
                .map_err(failure)?
        } else {
            service
                .store_mut()
                .create(&project, &title, "excalidraw/v2", &document)
                .map_err(failure)?
        };
        return receipt(service.store(), &record);
    }
    let id = required(input.figure_id, "figure_id is required")?;
    let metadata = service.store().get(&project, &id).map_err(failure)?;
    if input
        .expected_revision
        .is_some_and(|r| r != metadata.revision)
    {
        return Err(failure(visualization::Error::Conflict));
    }
    match input.action.as_str() {
        "read" => {
            let mut scene = service
                .store()
                .revision(&project, &id, metadata.revision)
                .map_err(failure)?;
            if let Some(files) = scene
                .document
                .get_mut("files")
                .and_then(Value::as_object_mut)
            {
                for f in files.values_mut() {
                    if let Some(f) = f.as_object_mut() {
                        f.remove("dataURL");
                    }
                }
            }
            let result = json!({"metadata":metadata,"scene_geometry_review":visualization::geometry_review::review(&scene.document),"scene":scene,"image_data_omitted":true,"annotations":service.store().annotations(&project,&id).map_err(failure)?,"markups":service.store().markups_page(&project,&id,None,None,50).map_err(failure)?});
            if let Some(ids) = input.element_ids {
                scoped_read::project_saved(result, &ids).map_err(|e| invalid(&e))
            } else {
                Ok(result)
            }
        }
        "apply" => {
            let rev = input
                .expected_revision
                .ok_or_else(|| invalid("expected_revision is required"))?;
            let operations: Vec<visualization::scene::Operation> = decode_document(
                input
                    .operations
                    .ok_or_else(|| invalid("operations is required"))?,
                "operations",
            )?;
            let record = service
                .apply(&project, &id, rev, &operations)
                .map_err(failure)?;
            receipt(service.store(), &record)
        }
        "arrange" => {
            let rev = input
                .expected_revision
                .ok_or_else(|| invalid("expected_revision is required"))?;
            let doc = service.current(&project, &id).map_err(failure)?.document;
            let next = visualization::arrange::compile(
                &doc,
                &input
                    .arrangement
                    .ok_or_else(|| invalid("arrangement is required"))?,
            )
            .map_err(failure)?;
            let record = service
                .store()
                .save(&project, &id, rev, &metadata.title, &next)
                .map_err(failure)?;
            receipt(service.store(), &record)
        }
        "read_preview" => Ok(json!(
            service
                .store()
                .preview(&project, &id, metadata.revision)
                .map_err(failure)?
        )),
        "list_markups" => Ok(json!(
            service
                .store()
                .markups_page(
                    &project,
                    &id,
                    input.status.as_deref(),
                    input.before.as_deref(),
                    input.limit.unwrap_or(50)
                )
                .map_err(failure)?
        )),
        "read_markup" => {
            let mut value = json!(
                service
                    .store()
                    .markup(
                        &project,
                        &id,
                        &required(input.markup_id, "markup_id is required")?
                    )
                    .map_err(failure)?
            );
            value["current_revision"] = json!(metadata.revision);
            value["historical"] = json!(value["at_revision"] != metadata.revision);
            Ok(value)
        }
        "image_info" => {
            let mut value = visualization_assets::image_info(
                service.store(),
                &project,
                &id,
                metadata.revision,
                &required(input.element_id, "element_id is required")?,
            )
            .map_err(failure)?;
            value.as_object_mut().unwrap().remove("dataURL");
            Ok(value)
        }
        "inset_status" => visualization_assets::inset_status(
            service.store(),
            &project,
            &id,
            metadata.revision,
            &required(input.element_id, "element_id is required")?,
        )
        .map_err(failure),
        "inset" | "refresh_inset" => {
            let rev = input
                .expected_revision
                .ok_or_else(|| invalid("expected_revision is required"))?;
            let record = if input.action == "inset" {
                visualization_assets::inset(
                    service.store(),
                    &project,
                    &id,
                    rev,
                    &input.inset.ok_or_else(|| invalid("inset is required"))?,
                )
            } else {
                visualization_assets::refresh_inset(
                    service.store(),
                    &project,
                    &id,
                    rev,
                    &required(input.element_id, "element_id is required")?,
                    input.discard_detail_edits.unwrap_or(false),
                )
            }
            .map_err(failure)?;
            receipt(service.store(), &record)
        }
        "read_image" => {
            let scene = service.current(&project, &id).map_err(failure)?;
            if input
                .expected_revision
                .is_some_and(|revision| revision != scene.revision)
            {
                return Err(failure(visualization::Error::Conflict));
            }
            let element_id = required(input.element_id, "element_id is required")?;
            let element = scene.document["elements"]
                .as_array()
                .and_then(|elements| {
                    elements.iter().find(|element| {
                        element["id"].as_str() == Some(&element_id)
                            && element["type"] == "image"
                            && element["isDeleted"] != true
                    })
                })
                .ok_or_else(|| invalid("image object not found"))?;
            let file_id = element["fileId"]
                .as_str()
                .ok_or_else(|| invalid("image object has no file"))?;
            let data = scene.document["files"][file_id]["dataURL"]
                .as_str()
                .ok_or_else(|| invalid("image bytes unavailable"))?;
            let raster;
            let data = if let Some(encoded) = data.strip_prefix("data:image/svg+xml;base64,") {
                use base64::Engine;
                if encoded.len() > 6 * 1024 * 1024 {
                    return Err(invalid("SVG exceeds image size limit"));
                }
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .map_err(failure)?;
                raster = ::bioicons::vector::normalize(&bytes)
                    .map_err(failure)?
                    .preview_data_url;
                raster.as_str()
            } else {
                data
            };

            Ok(
                json!({"dataURL":data,"figure_id":id,"element_id":element_id,"revision":scene.revision,"provenance":element.get("customData")}),
            )
        }
        _ => Err(invalid("unknown visualization action")),
    }
}
#[async_trait]
impl ToolFunction for FigureTool {
    type Input = FigureInput;
    async fn run(&self, input: FigureInput) -> Result<ToolResult, ToolError> {
        let image = matches!(
            input.action.as_str(),
            "read_image" | "read_preview" | "read_markup"
        );
        let preview_scope = if input.action == "read_preview" {
            Some((
                required(input.project_id.clone(), "project_id is required")?,
                required(input.figure_id.clone(), "figure_id is required")?,
                input.expected_revision,
            ))
        } else {
            None
        };
        let path = self.path.clone();
        let mut value = if let Some((project, figure, expected)) = preview_scope {
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(6);
            loop {
                let (path, project, figure) = (path.clone(), project.clone(), figure.clone());
                let result = tokio::task::spawn_blocking(move || {
                    let store = Store::open(path)?;
                    let revision = store.get(&project, &figure)?.revision;
                    if expected.is_some_and(|r| r != revision) {
                        return Err(visualization::Error::Conflict);
                    }
                    store.preview(&project, &figure, revision)
                })
                .await
                .map_err(failure)?;
                match result {
                    Ok(preview) => break json!(preview),
                    Err(visualization::Error::NotFound)
                        if tokio::time::Instant::now() < deadline =>
                    {
                        tokio::time::sleep(std::time::Duration::from_millis(150)).await
                    }
                    Err(visualization::Error::NotFound) => {
                        return Err(invalid(
                            "Current canvas preview unavailable; open the canvas to render its current revision",
                        ));
                    }
                    Err(error) => return Err(failure(error)),
                }
            }
        } else {
            tokio::task::spawn_blocking(move || execute(path, input))
                .await
                .map_err(failure)??
        };
        if !image {
            return Ok(ToolResult::success_json(value));
        }
        let url = value
            .as_object_mut()
            .and_then(|v| v.remove("dataURL"))
            .and_then(|v| v.as_str().map(str::to_owned))
            .ok_or_else(|| invalid("image bytes unavailable"))?;
        let (header, data) = url
            .split_once(',')
            .ok_or_else(|| invalid("invalid image encoding"))?;
        let mime = match header {
            "data:image/png;base64" => "image/png",
            "data:image/jpeg;base64" => "image/jpeg",
            "data:image/webp;base64" => "image/webp",
            _ => return Err(invalid("unsupported image encoding")),
        };
        if data.is_empty() || data.len() > 6 * 1024 * 1024 {
            return Err(invalid("image exceeds vision transfer limit"));
        }
        Ok(ToolResult::with_blocks(vec![
            ToolResultBlock::text(value.to_string()),
            ToolResultBlock::image_base64(mime, data),
        ]))
    }
}
pub fn registrations(path: PathBuf) -> Vec<ToolRegistration> {
    vec![
        ToolRegistration::from(FigureTool { path: path.clone() }),
        fit::registration(path.clone()),
        fit_svg::registration(path.clone()),
        fit_svg_group::registration(path.clone()),
        translate::registration(path.clone()),
        add_text::registration(path.clone()),
        scale::registration(path.clone()),
        crop::registration(path.clone()),
        move_text::registration(path.clone()),
        find_space::registration(path.clone()),
        reflow_text::registration(path),
    ]
}
