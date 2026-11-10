//! BioArt search and reviewed insertion/replacement in shared project figures.
use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration};
use agentik_proc::tool;
use agentik_sdk::types::{ToolResult, tools::ToolResultBlock};
use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;

fn error(error: impl std::error::Error + Send + Sync + 'static) -> ToolError {
    ToolError::ExecutionFailed {
        source: Box::new(error),
    }
}
fn missing(field: &str) -> ToolError {
    error(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        field.to_owned(),
    ))
}
#[tool(
    name = "bioart",
    description = "Search NIH BioArt scientific illustration assets, insert an image, or replace a sketch placeholder. insert and replace create reviewable proposals while preserving layout, labels and arrow anchors; they do not commit directly. Search first, use preview to inspect the image and its provenance before choosing a scientifically appropriate asset, then insert using the current revision from visualization read. Retains source, creators, license and SHA-256. Images remain movable/resizable; internal paths are not editable. Never infer a license when absent. Unsaved GUI edits are not available; save first and review the proposal before adoption. insert appends a standalone image at x/y; use replace for an existing semantic node. For insert or replace on a pending candidate, first use visualization read_proposal, then supply proposal_id, its exact document_sha256 and expected_revision. This creates a child candidate that preserves all previous edits and leaves the parent/formal figure untouched. Explicitly provide markup_ids to retain or drop open review links; [] intentionally drops them. Inspect the new child with visualization read_proposal_preview before adoption. Standalone image replacement keeps its image ID, viewport, rotation, flip signs, user notes and bindings. Existing crop fractions map to the new source dimensions; this is a graphical crop, not the same biological region. The new source/license and prior-source history are recorded. Inspect the candidate crop before adoption. Do not infer protein-specific structure or additional mechanism from generic artwork."
)]
pub struct Input {
    #[desc = "search | preview | insert | replace"]
    pub action: String,
    #[desc = "Search terms; required for search"]
    pub query: Option<String>,
    #[desc = "Numeric asset ID returned by search; required for insert"]
    pub asset_id: Option<u64>,
    #[desc = "Target project ID; required for insert"]
    pub project_id: Option<String>,
    #[desc = "Target figure ID; required for insert"]
    pub figure_id: Option<String>,
    #[desc = "Placeholder, semantic anchor or image object ID; required for replace"]
    pub element_id: Option<String>,
    #[desc = "Optional pending candidate ID for insert/replace; requires document_sha256 from visualization read_proposal. Creates a child candidate, preserving the parent."]
    pub proposal_id: Option<String>,
    #[desc = "Exact parent candidate document_sha256; use only with proposal_id on insert/replace"]
    pub document_sha256: Option<String>,
    #[desc = "Explicit review IDs to retain/address; [] explicitly drops links; omission rejects a candidate with open linked reviews"]
    pub markup_ids: Option<Vec<String>>,
    #[desc = "Current figure revision from visualization read; required for insert"]
    pub expected_revision: Option<i64>,
    #[desc = "Scene x coordinate, defaults to zero"]
    pub x: Option<f64>,
    #[desc = "Scene y coordinate, defaults to zero"]
    pub y: Option<f64>,
}
struct BioArtTool {
    path: PathBuf,
}
#[async_trait]
impl ToolFunction for BioArtTool {
    type Input = Input;
    async fn run(&self, input: Input) -> Result<ToolResult, ToolError> {
        crate::asset_candidate::validate(
            &input.action,
            input.proposal_id.as_deref(),
            input.document_sha256.as_deref(),
            input.markup_ids.as_deref(),
        )
        .map_err(missing)?;
        if input.action == "replace" && (input.x.is_some() || input.y.is_some()) {
            return Err(missing(
                "replace preserves current placement; omit x/y. Use visualization revise_proposal set_image_geometry to move or resize a standalone image",
            ));
        }
        if input.action == "insert" && input.element_id.is_some() {
            return Err(missing(
                "insert creates a new image at x/y; omit element_id or use replace for an existing object",
            ));
        }
        let client = bioart::BioArtClient::new().map_err(error)?;
        if input.action == "search" {
            let query = input.query.ok_or_else(|| missing("query required"))?;
            if query.trim().is_empty() || query.len() > 300 {
                return Err(missing("query must contain 1..300 characters"));
            }
            return Ok(ToolResult::success_json(json!(
                client.search(query.trim(), 0, 12).await.map_err(error)?
            )));
        }
        if input.action == "preview" {
            let asset = client
                .canvas_image(input.asset_id.ok_or_else(|| missing("asset_id required"))?)
                .await
                .map_err(error)?;
            return asset_preview(asset);
        }
        if !matches!(input.action.as_str(), "insert" | "replace") {
            return Err(missing("unknown bioart action"));
        }
        let replacement = input.action == "replace";
        if replacement && input.element_id.is_none() {
            return Err(missing("element_id required"));
        }
        let project = input
            .project_id
            .ok_or_else(|| missing("project_id required"))?;
        let figure = input
            .figure_id
            .ok_or_else(|| missing("figure_id required"))?;
        let revision = input
            .expected_revision
            .ok_or_else(|| missing("expected_revision required"))?;
        let asset = client
            .canvas_image(input.asset_id.ok_or_else(|| missing("asset_id required"))?)
            .await
            .map_err(error)?;
        let path = self.path.clone();
        let placement = Placement {
            project,
            figure,
            revision,
            replacement,
            proposal_id: input.proposal_id,
            document_sha256: input.document_sha256,
            markup_ids: input.markup_ids,
            element_id: input.element_id,
            x: input.x.unwrap_or(0.0),
            y: input.y.unwrap_or(0.0),
        };
        let result = tokio::task::spawn_blocking(move || persist_asset(path, placement, asset))
            .await
            .map_err(error)??;
        Ok(ToolResult::success_json(result))
    }
}
pub fn registrations(path: PathBuf) -> Vec<ToolRegistration> {
    vec![ToolRegistration::from(BioArtTool { path })]
}

fn asset_preview(asset: bioart::CanvasImage) -> Result<ToolResult, ToolError> {
    let data = asset
        .data_url
        .strip_prefix("data:image/png;base64,")
        .ok_or_else(|| missing("PNG preview unavailable"))?;
    if data.len() > 6 * 1024 * 1024 {
        return Err(missing(
            "asset exceeds vision transfer limit; choose a smaller asset representation",
        ));
    }
    let caption = json!({"asset":asset.asset,"file":asset.file,"sha256":asset.sha256,"width":asset.width,"height":asset.height});
    Ok(ToolResult::with_blocks(vec![
        ToolResultBlock::text(caption.to_string()),
        ToolResultBlock::image_base64("image/png", data),
    ]))
}
struct Placement {
    project: String,
    figure: String,
    revision: i64,
    replacement: bool,
    proposal_id: Option<String>,
    document_sha256: Option<String>,
    markup_ids: Option<Vec<String>>,
    element_id: Option<String>,
    x: f64,
    y: f64,
}
fn persist_asset(
    path: PathBuf,
    placement: Placement,
    asset: bioart::CanvasImage,
) -> Result<serde_json::Value, ToolError> {
    let Placement {
        project,
        figure,
        revision,
        replacement,
        proposal_id,
        document_sha256,
        markup_ids,
        element_id,
        x,
        y,
    } = placement;

    let store = visualization::Store::open(path).map_err(error)?;
    let metadata = store.get(&project, &figure).map_err(error)?;
    if metadata.revision != revision {
        return Err(error(visualization::Error::Conflict));
    }

    let summary = format!(
        "{} BioArt {}",
        if replacement {
            "Replace with"
        } else {
            "Insert"
        },
        asset.asset.title
    );
    let image = visualization::image::Image {
        file_id: &asset.sha256,
        data_url: &asset.data_url,
        width: asset.width,
        height: asset.height,
        provenance: json!({"source":"NIH BioArt","asset":asset.asset,"file":asset.file,"sha256":asset.sha256}),
    };
    if let (Some(parent_id), Some(hash)) = (&proposal_id, &document_sha256) {
        let (proposal, element_id) = if !replacement {
            visualization_assets::candidate::insert(
                &store,
                &project,
                &figure,
                revision,
                visualization_assets::candidate::Insertion {
                    reference: visualization_assets::candidate::Reference {
                        proposal_id: parent_id,
                        document_sha256: hash,
                    },
                    image,
                    x,
                    y,
                    summary: &summary,
                    markup_ids: markup_ids.as_deref(),
                },
            )
        } else {
            visualization_assets::candidate::replace(
                &store,
                &project,
                &figure,
                revision,
                visualization_assets::candidate::Replacement {
                    parent_id,
                    parent_document_sha256: hash,
                    target: element_id
                        .as_deref()
                        .ok_or_else(|| missing("element_id required"))?,
                    image,
                    summary: &summary,
                    markup_ids: markup_ids.as_deref(),
                },
            )
        }
        .map_err(error)?;
        return Ok(
            json!({"proposal_id":proposal.id,"figure_id":figure,"base_revision":revision,"element_id":element_id,"summary":summary,"status":"pending","parent_proposal_id":proposal.parent_proposal_id,"parent_document_sha256":proposal.parent_document_sha256,"document_sha256":proposal.document_sha256}),
        );
    }
    let current = store.revision(&project, &figure, revision).map_err(error)?;
    if replacement {
        let (document, element_id) = visualization::image::illustrate(
            &current.document,
            element_id.as_deref().unwrap(),
            image,
        )
        .map_err(error)?;
        let proposal = store
            .propose_with_markups(
                &project,
                &figure,
                revision,
                &summary,
                &document,
                markup_ids.as_deref().unwrap_or(&[]),
            )
            .map_err(error)?;
        return Ok(
            json!({"proposal_id":proposal.id,"figure_id":figure,"base_revision":revision,"element_id":element_id,"summary":summary,"status":"pending"}),
        );
    }
    let (document, element_id) =
        visualization::image::insert(&current.document, image, x, y).map_err(error)?;
    let proposal = store
        .propose_with_markups(
            &project,
            &figure,
            revision,
            &summary,
            &document,
            markup_ids.as_deref().unwrap_or(&[]),
        )
        .map_err(error)?;
    Ok(
        json!({"proposal_id":proposal.id,"figure_id":figure,"base_revision":revision,"element_id":element_id,"summary":summary,"status":"pending","document_sha256":proposal.document_sha256}),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn downloaded_asset_becomes_a_version_checked_proposal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("figures.sqlite");
        let mut store = visualization::Store::open(&path).unwrap();
        store.create_project("p", "Study").unwrap();
        let plan=serde_json::from_value(json!({"message":"Layout test","audience":"researchers","nodes":[{"id":"cell","label":"Cell","row":0,"column":0},{"id":"response","label":"Response","row":0,"column":1}],"edges":[{"from":"cell","to":"response"}]})).unwrap();
        let original = visualization::sketch::compile(&plan).unwrap();
        let figure = store
            .create("p", "Figure", "excalidraw/v2", &original)
            .unwrap();
        let asset:bioart::CanvasImage=serde_json::from_value(json!({"asset":{"id":82,"accession":"mock","title":"Cell","description":"","source_url":"https://bioart.niaid.nih.gov/asset/82","licenses":["Public Domain"],"creators":["NIH"],"collections":[],"keywords":[],"ontology_terms":[],"created":null,"updated":null,"files":[]},"file":{"asset_id":82,"file_id":1,"format":"PNG","url":"https://bioart.niaid.nih.gov"},"sha256":"fixture","dataURL":"data:image/png;base64,abc","width":100,"height":100})).unwrap();
        let preview = serde_json::to_value(asset_preview(asset.clone()).unwrap()).unwrap();
        assert_eq!(preview["content"][1]["type"], "image");
        assert!(
            preview["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("Public Domain")
        );
        assert_eq!(store.get("p", &figure.id).unwrap().revision, 1);
        let placement = || Placement {
            project: "p".into(),
            figure: figure.id.clone(),
            revision: 1,
            replacement: true,
            proposal_id: None,
            document_sha256: None,
            markup_ids: None,
            element_id: Some("node-0".into()),
            x: 0.0,
            y: 0.0,
        };
        let result = persist_asset(path.clone(), placement(), asset.clone()).unwrap();
        assert!(!result.to_string().contains("data:image"));
        assert_eq!(store.get("p", &figure.id).unwrap().revision, 1);
        let proposal = store.proposals("p", &figure.id).unwrap().remove(0);
        assert_eq!(
            proposal.document["elements"][4]["startBinding"]["elementId"],
            "node-0"
        );
        assert_eq!(
            proposal.document["elements"]
                .as_array()
                .unwrap()
                .last()
                .unwrap()["customData"]["asset"]["licenses"][0],
            "Public Domain"
        );
        let mut on_candidate = placement();
        on_candidate.proposal_id = Some(proposal.id.clone());
        on_candidate.document_sha256 = Some(proposal.document_sha256.clone());
        on_candidate.markup_ids = Some(vec![]);
        on_candidate.element_id = Some("node-1".into());
        let child_result = persist_asset(path.clone(), on_candidate, asset.clone()).unwrap();
        let child = store
            .proposal(
                "p",
                &figure.id,
                child_result["proposal_id"].as_str().unwrap(),
            )
            .unwrap();
        assert_eq!(
            child.parent_proposal_id.as_deref(),
            Some(proposal.id.as_str())
        );
        assert_eq!(
            child.document["elements"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|e| e["type"] == "image")
                .count(),
            2
        );
        assert_eq!(
            store
                .proposal("p", &figure.id, &proposal.id)
                .unwrap()
                .document,
            proposal.document
        );
        assert_eq!(
            store.revision("p", &figure.id, 1).unwrap().document,
            original
        );
        assert_eq!(store.get("p", &figure.id).unwrap().revision, 1);
        let mut inserted = placement();
        inserted.replacement = false;
        inserted.element_id = None;
        inserted.proposal_id = Some(child.id.clone());
        inserted.document_sha256 = Some(child.document_sha256.clone());
        inserted.markup_ids = Some(vec![]);
        inserted.x = 700.;
        inserted.y = 440.;
        let result = persist_asset(path.clone(), inserted, asset.clone()).unwrap();
        let appended = store
            .proposal("p", &figure.id, result["proposal_id"].as_str().unwrap())
            .unwrap();
        assert_eq!(
            appended.parent_proposal_id.as_deref(),
            Some(child.id.as_str())
        );
        assert_eq!(
            &appended.document["elements"].as_array().unwrap()
                [..child.document["elements"].as_array().unwrap().len()],
            child.document["elements"].as_array().unwrap()
        );
        assert_eq!(
            appended.document["elements"]
                .as_array()
                .unwrap()
                .last()
                .unwrap()["x"],
            700.
        );
        assert_eq!(appended.document["files"], child.document["files"]);
        let mut root_insert = placement();
        root_insert.replacement = false;
        root_insert.element_id = None;
        let root_result = persist_asset(path.clone(), root_insert, asset.clone()).unwrap();
        assert_eq!(root_result["status"], "pending");
        assert!(root_result.get("figure").is_none());
        let root = store
            .proposal(
                "p",
                &figure.id,
                root_result["proposal_id"].as_str().unwrap(),
            )
            .unwrap();
        assert!(root.parent_proposal_id.is_none());
        assert_eq!(
            root.document["elements"].as_array().unwrap().len(),
            original["elements"].as_array().unwrap().len() + 1
        );
        assert_eq!(store.get("p", &figure.id).unwrap().revision, 1);
        store
            .accept_proposal("p", &figure.id, &proposal.id)
            .unwrap();
        assert!(persist_asset(path, placement(), asset).is_err());
        assert_eq!(store.get("p", &figure.id).unwrap().revision, 2);
    }
}
