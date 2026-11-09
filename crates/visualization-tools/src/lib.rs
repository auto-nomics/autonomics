//! Agent access to the same project figures edited in the GUI.
mod bioart;
use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration};
use agentik_proc::tool;
use agentik_sdk::types::{ToolResult, tools::ToolResultBlock};
use async_trait::async_trait;
pub use bioart::registrations as bioart_registrations;
use serde_json::{Value, json};
use std::path::PathBuf;
use visualization::{Store, service::VisualizationService};

#[tool(
    name = "visualization",
    description = "Manage editable scientific figures shared with the GUI. Actions: projects, list, read, read_image, read_preview, create, sketch, propose, apply. Plan the message, audience, scientific components and relationships with the user before creating a figure. For sketch, supply figure_id and expected_revision to propose a new sketch in the active figure; omit figure_id only when creating a separate figure. Prefer propose for changes that should be previewed in the GUI before acceptance. read omits embedded image bytes; use read_image with element_id to view a reference image as a vision tool result. read_preview returns the full canvas PNG for the current saved revision when the GUI has captured one; never fall back to a stale preview. Read returns object-linked review annotations as well as the scene; use these notes to interpret selected markup. Use read before apply/propose; use expected_revision from read, never guess it. apply operations are JSON objects: {op:add,element:{id,...}}, {op:update,id,changes:{...}}, {op:remove,id}. Labels, arrows and icons must remain separate editable elements. Position updates move owned labels/images and ordinary straight connector endpoints. For rotated or elbowed connectors, supply geometry edits explicitly. Preserve unrelated objects, do not flatten the figure into a bitmap. Scientific claims require user review. GUI unsaved changes are not included; ask the user to save before editing them."
)]
pub struct FigureInput {
    #[desc = "projects | list | read | read_image | read_preview | create | sketch | propose | apply"]
    pub action: String,
    #[desc = "Project ID from active tab context or projects action"]
    pub project_id: Option<String>,
    #[desc = "Figure ID from tab context or list action"]
    pub figure_id: Option<String>,
    #[desc = "Image object ID from read; required for read_image"]
    pub element_id: Option<String>,
    #[desc = "Required for create/sketch; proposal summary for propose"]
    pub title: Option<String>,
    #[desc = "For create: Excalidraw scene JSON. For sketch: plan {message,audience,nodes:[{id,label,column,row,shape:rectangle|ellipse,evidence:[]}],edges:[{from,to,label,inhibitory,evidence:[]}],assumptions:[]}. Grid positions must be unique. Sketch compiles to separate editable shapes, labels and arrows."]
    pub document: Option<Value>,
    #[desc = "Exact current revision returned by read; required for apply"]
    pub expected_revision: Option<i64>,
    #[desc = "Array of add/update/remove operations; required for apply. Updates are shallow merges and cannot change IDs."]
    pub operations: Option<Value>,
}
struct FigureTool {
    path: PathBuf,
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
fn execute(path: PathBuf, input: FigureInput) -> Result<Value, ToolError> {
    let mut service = VisualizationService::new(Store::open(path).map_err(failure)?);
    if input.action == "projects" {
        return Ok(json!(service.store().projects().map_err(failure)?));
    }
    let project = required(input.project_id, "project_id is required")?;
    if input.action == "list" {
        return Ok(json!(service.store().list(&project).map_err(failure)?));
    }
    if input.action == "create" || input.action == "sketch" {
        let title = required(input.title, "title is required")?;
        let document = input
            .document
            .ok_or_else(|| invalid("document is required"))?;
        let document = if input.action == "sketch" {
            let plan: visualization::sketch::Plan =
                serde_json::from_value(document).map_err(failure)?;
            visualization::sketch::compile(&plan).map_err(failure)?
        } else {
            document
        };
        visualization::scene::apply(&document, &[]).map_err(failure)?;
        if let Some(id) = input.figure_id {
            if input.action != "sketch" {
                return Err(invalid(
                    "create makes a separate figure; use sketch or propose for an existing figure",
                ));
            }
            let revision = input
                .expected_revision
                .ok_or_else(|| invalid("expected_revision is required for an existing figure"))?;
            let mut proposal = json!(
                service
                    .store()
                    .propose(&project, &id, revision, &title, &document)
                    .map_err(failure)?
            );
            proposal.as_object_mut().unwrap().remove("document");
            proposal["figure_id"] = json!(id);
            return Ok(proposal);
        }
        return Ok(json!(
            service
                .store_mut()
                .create(&project, &title, "excalidraw/v2", &document)
                .map_err(failure)?
        ));
    }
    let id = required(input.figure_id, "figure_id is required")?;
    match input.action.as_str() {
        "read" => {
            let metadata = service.store().get(&project, &id).map_err(failure)?;
            let mut scene = service
                .store()
                .revision(&project, &id, metadata.revision)
                .map_err(failure)?;
            if let Some(files) = scene
                .document
                .get_mut("files")
                .and_then(Value::as_object_mut)
            {
                for file in files.values_mut() {
                    if let Some(file) = file.as_object_mut() {
                        file.remove("dataURL");
                    }
                }
            }
            Ok(
                json!({"metadata":metadata,"scene":scene,"image_data_omitted":true,"annotations":service.store().annotations(&project,&id).map_err(failure)?}),
            )
        }
        "read_preview" => {
            let metadata = service.store().get(&project, &id).map_err(failure)?;
            if input
                .expected_revision
                .is_some_and(|revision| revision != metadata.revision)
            {
                return Err(failure(visualization::Error::Conflict));
            }
            Ok(json!(
                service
                    .store()
                    .preview(&project, &id, metadata.revision)
                    .map_err(|error| match error {
                        visualization::Error::NotFound => invalid("No preview for the current revision; ask the user to save and hand off from the canvas"),
                        other => failure(other),
                    })?
            ))
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
            Ok(
                json!({"dataURL":data,"figure_id":id,"element_id":element_id,"revision":scene.revision,"provenance":element.get("customData")}),
            )
        }
        "apply" | "propose" => {
            let revision = input
                .expected_revision
                .ok_or_else(|| invalid("expected_revision is required"))?;
            let operations: Vec<visualization::scene::Operation> = serde_json::from_value(
                input
                    .operations
                    .ok_or_else(|| invalid("operations is required"))?,
            )
            .map_err(failure)?;
            if operations.len() > 1000 {
                return Err(invalid("at most 1000 operations"));
            }
            if input.action == "propose" {
                let summary = required(input.title, "title must summarize the proposal")?;
                let mut proposal = json!(
                    service
                        .propose(&project, &id, revision, &summary, &operations)
                        .map_err(failure)?
                );
                proposal.as_object_mut().unwrap().remove("document");
                return Ok(proposal);
            }
            Ok(json!(
                service
                    .apply(&project, &id, revision, &operations)
                    .map_err(failure)?
            ))
        }
        _ => Err(invalid("unknown visualization action")),
    }
}
#[async_trait]
impl ToolFunction for FigureTool {
    type Input = FigureInput;
    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let image = input.action == "read_image" || input.action == "read_preview";
        let path = self.path.clone();
        let mut value = tokio::task::spawn_blocking(move || execute(path, input))
            .await
            .map_err(failure)??;
        if image {
            let data_url = value
                .as_object_mut()
                .and_then(|value| value.remove("dataURL"))
                .and_then(|value| value.as_str().map(str::to_owned))
                .ok_or_else(|| invalid("image bytes unavailable"))?;
            let (header, data) = data_url
                .split_once(',')
                .ok_or_else(|| invalid("invalid image encoding"))?;
            let mime = match header {
                "data:image/png;base64" => "image/png",
                "data:image/jpeg;base64" => "image/jpeg",
                "data:image/webp;base64" => "image/webp",
                _ => return Err(invalid("vision supports embedded PNG, JPEG or WebP images")),
            };
            if data.is_empty()
                || data.len() > 6 * 1024 * 1024
                || data.len() % 4 != 0
                || !data
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"+/=".contains(&byte))
            {
                return Err(invalid(
                    "invalid image encoding or image exceeds the 6 MiB base64 vision limit; resize the reference image",
                ));
            }
            return Ok(ToolResult::with_blocks(vec![
                ToolResultBlock::text(value.to_string()),
                ToolResultBlock::image_base64(mime, data),
            ]));
        }
        Ok(ToolResult::success_json(value))
    }
}
pub fn registrations(path: PathBuf) -> Vec<ToolRegistration> {
    vec![ToolRegistration::from(FigureTool { path })]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn sketch_in_an_existing_figure_proposes_without_creating_another_record() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("figures.sqlite");
        let mut store = Store::open(&path).unwrap();
        store.create_project("p", "Study").unwrap();
        let figure = store
            .create(
                "p",
                "Active canvas",
                "excalidraw/v2",
                &json!({"elements":[],"files":{}}),
            )
            .unwrap();
        let tool = registrations(path).remove(0);
        let input = |revision| json!({"action":"sketch","project_id":"p","figure_id":figure.id,"expected_revision":revision,"title":"Cell overview","document":{"message":"Show a cell","audience":"researchers","nodes":[{"id":"cell","label":"Cell","row":0,"column":0}]}});
        assert!(tool.implementation.execute(input(0)).await.is_err());
        let proposal =
            serde_json::to_value(tool.implementation.execute(input(1)).await.unwrap()).unwrap();
        assert_eq!(proposal["content"]["figure_id"], figure.id);
        assert_eq!(store.list("p").unwrap().len(), 1);
        assert_eq!(store.get("p", &figure.id).unwrap().revision, 1);
        store
            .accept_proposal("p", &figure.id, proposal["content"]["id"].as_str().unwrap())
            .unwrap();
        assert_eq!(
            store.revision("p", &figure.id, 2).unwrap().document["elements"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(store.list("p").unwrap().len(), 1);
    }
    #[tokio::test]
    async fn reference_images_use_vision_blocks_without_polluting_scene_reads() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("figures.sqlite");
        let mut store = Store::open(&path).unwrap();
        store.create_project("p", "Study").unwrap();
        let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg==";
        let document = json!({"elements":[{"id":"reference","type":"image","fileId":"pic"}],"files":{"pic":{"id":"pic","mimeType":"image/png","dataURL":format!("data:image/png;base64,{png}")}}});
        let figure = store
            .create("p", "Reference", "excalidraw/v2", &document)
            .unwrap();
        store
            .save_preview("p", &figure.id, 1, &format!("data:image/png;base64,{png}"))
            .unwrap();
        let tool = registrations(path).remove(0);
        let preview = tool.implementation.execute(json!({"action":"read_preview","project_id":"p","figure_id":figure.id,"expected_revision":1})).await.unwrap();
        assert_eq!(
            serde_json::to_value(preview).unwrap()["content"][1]["source"]["data"],
            png
        );
        assert!(tool.implementation.execute(json!({"action":"read_preview","project_id":"p","figure_id":figure.id,"expected_revision":0})).await.is_err());
        let read = tool
            .implementation
            .execute(json!({"action":"read","project_id":"p","figure_id":figure.id}))
            .await
            .unwrap();
        let read = serde_json::to_value(read).unwrap();
        assert!(
            read["content"]["scene"]["document"]["files"]["pic"]
                .get("dataURL")
                .is_none()
        );
        assert_eq!(
            store.revision("p", &figure.id, 1).unwrap().document,
            document
        );
        let image = tool.implementation.execute(json!({"action":"read_image","project_id":"p","figure_id":figure.id,"element_id":"reference","expected_revision":1})).await.unwrap();
        let image = serde_json::to_value(image).unwrap();
        assert_eq!(image["content"][1]["type"], "image");
        assert_eq!(image["content"][1]["source"]["media_type"], "image/png");
        assert_eq!(image["content"][1]["source"]["data"], png);
        assert!(tool.implementation.execute(json!({"action":"read_image","project_id":"p","figure_id":figure.id,"element_id":"reference","expected_revision":0})).await.is_err());
        assert!(tool.implementation.execute(json!({"action":"read_image","project_id":"p","figure_id":figure.id,"element_id":"missing"})).await.is_err());
    }
    #[tokio::test]
    async fn registered_tool_creates_sketch_and_proposes_a_local_text_edit() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("figures.sqlite");
        let mut store = Store::open(&path).unwrap();
        store.create_project("p", "Study").unwrap();
        let tool = registrations(path.clone()).remove(0);
        assert_eq!(tool.definition.name, "visualization");
        let created = tool.implementation.execute(json!({"action":"sketch","project_id":"p","title":"Signal","document":{"message":"Binding","audience":"researchers","nodes":[{"id":"a","label":"Ligand","column":0,"row":0},{"id":"b","label":"Receptor","column":1,"row":0}],"edges":[{"from":"a","to":"b","label":"binds"}]}})).await.unwrap();
        let created = serde_json::to_value(created).unwrap();
        let id = created["content"]["id"].as_str().unwrap();
        let before = store.revision("p", id, 1).unwrap();
        let proposal = tool.implementation.execute(json!({"action":"propose","project_id":"p","figure_id":id,"expected_revision":1,"title":"Clarify ligand","operations":[{"op":"update","id":"label-0","changes":{"text":"Ligand A"}}]})).await.unwrap();
        let proposal = serde_json::to_value(proposal).unwrap();
        assert_eq!(store.get("p", id).unwrap().revision, 1);
        let proposal_id = proposal["content"]["id"].as_str().unwrap();
        store.accept_proposal("p", id, proposal_id).unwrap();
        let after = store.revision("p", id, 2).unwrap();
        assert_eq!(after.document["elements"][1]["text"], "Ligand A");
        assert_eq!(after.document["elements"][1]["originalText"], "Ligand A");
        assert_eq!(
            after.document["elements"][2],
            before.document["elements"][2]
        );
        store
            .annotate("p", id, 2, &["label-0".into()], "Increase label emphasis")
            .unwrap();
        let read = tool
            .implementation
            .execute(json!({"action":"read","project_id":"p","figure_id":id}))
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(read).unwrap()["content"]["annotations"][0]["note"],
            "Increase label emphasis"
        );
        assert!(tool.implementation.execute(json!({"action":"apply","project_id":"p","figure_id":id,"expected_revision":1,"operations":[]})).await.is_err());
    }
    #[test]
    fn agent_edits_share_gui_storage_and_reject_stale_revisions() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("visualizations.sqlite");
        let mut store = Store::open(&path).unwrap();
        store.create_project("p", "Study").unwrap();
        let figure = store
            .create("p", "Figure", "excalidraw/v2", &json!({"elements":[]}))
            .unwrap();
        let input = |revision| FigureInput {
            action: "apply".into(),
            project_id: Some("p".into()),
            figure_id: Some(figure.id.clone()),
            element_id: None,
            title: None,
            document: None,
            expected_revision: Some(revision),
            operations: Some(json!([{"op":"add","element":{"id":"cell","type":"ellipse"}}])),
        };
        assert_eq!(execute(path.clone(), input(1)).unwrap()["revision"], 2);
        assert!(execute(path.clone(), input(1)).is_err());
        assert_eq!(
            store.revision("p", &figure.id, 2).unwrap().document["elements"][0]["id"],
            "cell"
        );
    }
}
