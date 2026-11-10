//! BioIcons search, PNG vision previews, and reviewed vector insertion/replacement.
use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration};
use agentik_proc::tool;
use agentik_sdk::types::{ToolResult, tools::ToolResultBlock};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{path::PathBuf, sync::LazyLock};
static CLIENT: LazyLock<::bioicons::Result<::bioicons::BioIconsClient>> =
    LazyLock::new(::bioicons::BioIconsClient::new);
fn error(e: impl std::fmt::Display) -> ToolError {
    ToolError::ExecutionFailed {
        source: Box::new(std::io::Error::other(e.to_string())),
    }
}
#[tool(
    name = "bioicons",
    description = "Search BioIcons SVG illustrations. Actions search, preview, insert, replace, palette, recolor, parts, geometry, edit_parts. parts inspects rendered SVG paths and returns snapshot-scoped IDs, bounds and a numbered PNG; numbers identify graphical paths, never biological identities. edit_parts proposes fill/stroke (#RRGGBB or none) local opacity (0..1) and translate [dx,dy] edits in normalized SVG root coordinates on specified paths, using sha256 and edits from parts. Shared masks, clips and pattern resources are not editable paths. Omitted attributes remain unchanged. Translation compensates ancestor scaling, rotation and reflection; paint follows the path, parent clipping and the SVG viewport stay fixed. Use preview and review the result; geometry returns numbered endpoint/control handles and command topology for one part_id and sha256 from parts. Use edit_parts points:[{index,position:[x,y]}] for sparse absolute root-coordinate control/anchor edits from geometry; at most 64 edited points on paths with at most 512 points. Keep topology and other points unchanged. Do not combine points and translate for the same path; make separate reviewed proposals. Shape edits keep paint coordinates and parent clips fixed. Read parts again after adoption because IDs belong to the old file snapshot. palette reads a saved SVG image (or its semantic anchor) and returns actual #RRGGBB paints with a PNG preview. recolor uses colors mapping old_hex to new_hex to propose simultaneous exact paint replacements; preserves text glyphs, geometry, original author/license/source hash and unrelated image instances. Both require project_id, figure_id, expected_revision and element_id; neither requires network access. Read palette first, do not invent source colours. SVG text must already be outlined so recoloring cannot silently substitute its font. Search then inspect a PNG preview and per-icon author/license before selecting; do not infer scientific specificity from generic art. insert and replace create reviewable proposals on the exact current figure revision; they never commit directly. SVG remains a scalable vector image with provenance and source/normalized hashes; internal paths are not individual canvas objects. Unsupported raster/active SVG or unsupported text is rejected rather than silently dropped. Use visualization read/read_image to inspect saved objects and current revision. Preserve unrelated elements, scientific labels and arrow anchors. User must save local edits before agent modification."
)]
pub struct Input {
    #[desc = "search | preview | insert | replace | palette | recolor | parts | geometry | edit_parts"]
    pub action: String,
    pub query: Option<String>,
    pub asset_id: Option<String>,
    pub project_id: Option<String>,
    pub figure_id: Option<String>,
    pub expected_revision: Option<i64>,
    pub element_id: Option<String>,
    pub x: Option<f64>,
    pub y: Option<f64>,
    #[desc = "Exact simultaneous paint replacements as an object mapping #RRGGBB to #RRGGBB; required for recolor"]
    pub colors: Option<std::collections::BTreeMap<String, String>>,
    #[desc = "Exact SVG snapshot hash from parts; required for edit_parts"]
    pub sha256: Option<String>,
    #[desc = "Path ID from parts; required for geometry"]
    pub part_id: Option<String>,
    #[desc = "1..32 path edits using part_id from parts; translate [dx,dy] uses source SVG coordinates, not canvas pixels; points uses absolute positions and indices from geometry; omitted attributes stay unchanged"]
    pub edits: Option<Vec<::bioicons::parts::Edit>>,
}
struct BioIconsTool {
    path: PathBuf,
}
#[async_trait]
impl ToolFunction for BioIconsTool {
    type Input = Input;
    async fn run(&self, input: Input) -> Result<ToolResult, ToolError> {
        if matches!(
            input.action.as_str(),
            "palette" | "recolor" | "parts" | "geometry" | "edit_parts"
        ) {
            let path = self.path.clone();
            return tokio::task::spawn_blocking(move || {
                let project=input.project_id.ok_or_else(||error("project_id required"))?;
                let figure=input.figure_id.ok_or_else(||error("figure_id required"))?;
                let revision=input.expected_revision.ok_or_else(||error("expected_revision required"))?;
                let target=input.element_id.ok_or_else(||error("element_id required"))?;
                let store=visualization::Store::open(path).map_err(error)?;
                if matches!(input.action.as_str(),"palette"|"parts"|"geometry") {
                    let mut value=if input.action=="geometry" {visualization_assets::geometry(&store,&project,&figure,revision,&target,&input.sha256.ok_or_else(||error("sha256 required"))?,&input.part_id.ok_or_else(||error("part_id required"))?)} else if input.action=="parts" {visualization_assets::parts(&store,&project,&figure,revision,&target)} else {visualization_assets::palette(&store,&project,&figure,revision,&target)}.map_err(error)?;
                    let image_key=if input.action!="palette" {"annotatedDataURL"} else {"previewDataURL"};
                    let url=value.as_object_mut().unwrap().remove(image_key).and_then(|v|v.as_str().map(str::to_owned)).ok_or_else(||error("preview unavailable"))?;
                    value.as_object_mut().unwrap().remove("previewDataURL");
                    let data=url.split_once(',').ok_or_else(||error("invalid preview"))?.1;
                    Ok(ToolResult::with_blocks(vec![ToolResultBlock::text(value.to_string()),ToolResultBlock::image_base64("image/png",data)]))
                } else {
                    let proposal=if input.action=="edit_parts" {
                        visualization_assets::edit_parts(&store,&project,&figure,revision,&target,&input.sha256.ok_or_else(||error("sha256 required"))?,&input.edits.ok_or_else(||error("edits required"))?)
                    } else { visualization_assets::recolor(&store,&project,&figure,revision,&target,&input.colors.ok_or_else(||error("colors required"))?) }.map_err(error)?;
                    Ok(ToolResult::success_json(json!({"proposal_id":proposal.id,"base_revision":revision,"summary":proposal.summary,"status":"pending"})))
                }
            }).await.map_err(error)?;
        }
        let client = CLIENT.as_ref().map_err(error)?;
        if input.action == "search" {
            return Ok(ToolResult::success_json(json!(
                client
                    .search(
                        input
                            .query
                            .as_deref()
                            .ok_or_else(|| error("query required"))?,
                        0,
                        12
                    )
                    .await
                    .map_err(error)?
            )));
        }
        if !matches!(input.action.as_str(), "preview" | "insert" | "replace") {
            return Err(error("unknown bioicons action"));
        }
        let asset = client
            .vector(
                input
                    .asset_id
                    .as_deref()
                    .ok_or_else(|| error("asset_id required"))?,
            )
            .await
            .map_err(error)?;
        if input.action == "preview" {
            let data = asset
                .vector
                .preview_data_url
                .split_once(',')
                .ok_or_else(|| error("invalid preview"))?
                .1;
            return Ok(ToolResult::with_blocks(vec![ToolResultBlock::text(json!({"asset":asset.asset,"source_sha256":asset.vector.source_sha256,"sha256":asset.vector.sha256,"colors":asset.vector.colors,"normalization":asset.vector.normalization}).to_string()),ToolResultBlock::image_base64("image/png",data)]));
        }
        let path = self.path.clone();
        let result = tokio::task::spawn_blocking(move || persist(path, input, asset))
            .await
            .map_err(error)??;
        Ok(ToolResult::success_json(result))
    }
}
fn persist(
    path: PathBuf,
    input: Input,
    asset: ::bioicons::vector::VectorAsset,
) -> Result<Value, ToolError> {
    let project = input
        .project_id
        .ok_or_else(|| error("project_id required"))?;
    let figure = input.figure_id.ok_or_else(|| error("figure_id required"))?;
    let revision = input
        .expected_revision
        .ok_or_else(|| error("expected_revision required"))?;
    let store = visualization::Store::open(path).map_err(error)?;
    if store.get(&project, &figure).map_err(error)?.revision != revision {
        return Err(error(visualization::Error::Conflict));
    }
    let scene = store.revision(&project, &figure, revision).map_err(error)?;
    let summary = format!(
        "{} BioIcons {}",
        if input.action == "replace" {
            "Replace with"
        } else {
            "Insert"
        },
        asset.asset.name
    );
    let image = visualization::image::Image {
        file_id: &asset.vector.sha256,
        data_url: &asset.vector.data_url,
        width: asset.vector.width,
        height: asset.vector.height,
        provenance: json!({"source":"BioIcons","asset":asset.asset,"sha256":asset.vector.sha256,"source_sha256":asset.vector.source_sha256,"colors":asset.vector.colors,"normalization":asset.vector.normalization}),
    };
    let (document, id) = if input.action == "replace" {
        visualization::image::illustrate(
            &scene.document,
            input
                .element_id
                .as_deref()
                .ok_or_else(|| error("element_id required"))?,
            image,
        )
    } else {
        visualization::image::insert(
            &scene.document,
            image,
            input.x.unwrap_or(0.),
            input.y.unwrap_or(0.),
        )
    }
    .map_err(error)?;
    let proposal = store
        .propose(&project, &figure, revision, &summary, &document)
        .map_err(error)?;
    Ok(
        json!({"proposal_id":proposal.id,"element_id":id,"base_revision":revision,"summary":summary,"status":"pending"}),
    )
}
pub fn registrations(path: PathBuf) -> Vec<ToolRegistration> {
    vec![ToolRegistration::from(BioIconsTool { path })]
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn svg_replacement_is_reviewed_preserves_provenance_and_returns_png_vision() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("figures.sqlite");
        let mut store = visualization::Store::open(&path).unwrap();
        store.create_project("p", "Study").unwrap();
        let plan=serde_json::from_value(json!({"message":"Conceptual relation","audience":"Researchers","layout":"horizontal","nodes":[{"id":"a","label":"Receptor"},{"id":"b","label":"Response"}],"edges":[{"from":"a","to":"b","label":"explicit relation"}]})).unwrap();
        let scene = visualization::sketch::compile(&plan).unwrap();
        let figure = store
            .create("p", "Figure", "excalidraw/v2", &scene)
            .unwrap();
        let vector=::bioicons::vector::normalize(b"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 80 100'><path d='M10 10h60v80H10z' fill='#0088cc'/></svg>").unwrap();
        let asset = ::bioicons::Asset {
            id: "fixture".into(),
            name: "Generic receptor".into(),
            category: "Receptors".into(),
            license: "cc-by-4.0".into(),
            license_url: Some("https://creativecommons.org/licenses/by/4.0/".into()),
            author: "Fixture Author".into(),
            author_url: None,
            source_url: "https://example.org/asset".into(),
            svg_url: "https://example.org/asset.svg".into(),
            catalog_sha256: "catalog-fixture".into(),
        };
        let result = persist(
            path.clone(),
            Input {
                action: "replace".into(),
                query: None,
                asset_id: Some("fixture".into()),
                project_id: Some("p".into()),
                figure_id: Some(figure.id.clone()),
                expected_revision: Some(1),
                element_id: Some("node-0".into()),
                x: None,
                y: None,
                colors: None,
                sha256: None,
                part_id: None,
                edits: None,
            },
            ::bioicons::vector::VectorAsset { asset, vector },
        )
        .unwrap();
        assert_eq!(store.get("p", &figure.id).unwrap().revision, 1);
        let proposal = store
            .proposals("p", &figure.id)
            .unwrap()
            .into_iter()
            .find(|p| p.id == result["proposal_id"])
            .unwrap();
        let image = proposal.document["elements"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["type"] == "image")
            .unwrap();
        let id = image["id"].as_str().unwrap().to_string();
        assert_eq!(image["customData"]["asset"]["license"], "cc-by-4.0");
        assert_eq!(
            proposal.document["files"][image["fileId"].as_str().unwrap()]["mimeType"],
            "image/svg+xml"
        );
        assert_eq!(
            proposal.document["elements"]
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["id"] == "node-1"),
            scene["elements"]
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["id"] == "node-1")
        );
        store
            .accept_proposal("p", &figure.id, &proposal.id)
            .unwrap();
        let tool = crate::registrations(path.clone()).remove(0);
        let result=serde_json::to_value(tool.implementation.execute(json!({"action":"read_image","project_id":"p","figure_id":figure.id,"element_id":id,"expected_revision":2})).await.unwrap()).unwrap();
        assert_eq!(result["content"][1]["source"]["media_type"], "image/png");
        assert!(
            result["content"][1]["source"]["data"]
                .as_str()
                .unwrap()
                .starts_with("iVBOR")
        );
        let local = registrations(path).remove(0);
        let palette=serde_json::to_value(local.implementation.execute(json!({"action":"palette","project_id":"p","figure_id":figure.id,"expected_revision":2,"element_id":"node-0"})).await.unwrap()).unwrap();
        assert_eq!(palette["content"][1]["source"]["media_type"], "image/png");
        let caption: Value =
            serde_json::from_str(palette["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(caption["colors"], json!(["#0088cc"]));
        assert!(!caption.to_string().contains("data:image"));
        let recolored=serde_json::to_value(local.implementation.execute(json!({"action":"recolor","project_id":"p","figure_id":figure.id,"expected_revision":2,"element_id":"node-0","colors":{"#0088cc":"#00aa88"}})).await.unwrap()).unwrap();
        assert_eq!(recolored["content"]["status"], "pending");
        assert_eq!(store.get("p", &figure.id).unwrap().revision, 2);
        let parts=serde_json::to_value(local.implementation.execute(json!({"action":"parts","project_id":"p","figure_id":figure.id,"expected_revision":2,"element_id":"node-0"})).await.unwrap()).unwrap();
        assert_eq!(parts["content"][1]["source"]["media_type"], "image/png");
        let detail: Value =
            serde_json::from_str(parts["content"][0]["text"].as_str().unwrap()).unwrap();
        assert!(!detail.to_string().contains("data:image"));
        let proposal=serde_json::to_value(local.implementation.execute(json!({"action":"edit_parts","project_id":"p","figure_id":figure.id,"expected_revision":2,"element_id":"node-0","sha256":detail["sha256"],"edits":[{"part_id":detail["parts"][0]["part_id"],"fill":"#aa8844","translate":[2,3]}]})).await.unwrap()).unwrap();
        assert_eq!(proposal["content"]["status"], "pending");
        let geometry=serde_json::to_value(local.implementation.execute(json!({"action":"geometry","project_id":"p","figure_id":figure.id,"expected_revision":2,"element_id":"node-0","sha256":detail["sha256"],"part_id":detail["parts"][0]["part_id"]})).await.unwrap()).unwrap();
        assert_eq!(geometry["content"][1]["source"]["media_type"], "image/png");
        let handles: Value =
            serde_json::from_str(geometry["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(handles["revision"], 2);
        assert!(!handles.to_string().contains("data:image"));
        let point = handles["points"][0]["position"].as_array().unwrap();
        let result=serde_json::to_value(local.implementation.execute(json!({"action":"edit_parts","project_id":"p","figure_id":figure.id,"expected_revision":2,"element_id":"node-0","sha256":detail["sha256"],"edits":[{"part_id":detail["parts"][0]["part_id"],"points":[{"index":0,"position":[point[0].as_f64().unwrap()+2.,point[1].as_f64().unwrap()+3.]}]}]})).await.unwrap()).unwrap();
        assert_eq!(result["content"]["status"], "pending");
        let count = store.proposals("p", &figure.id).unwrap().len();
        assert!(local.implementation.execute(json!({"action":"recolor","project_id":"p","figure_id":figure.id,"expected_revision":2,"element_id":"node-0","colors":{"#123456":"#00aa88"}})).await.is_err());
        assert_eq!(store.proposals("p", &figure.id).unwrap().len(), count);
    }
}
