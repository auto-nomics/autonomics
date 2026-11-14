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
    description = "Search and preview NIH BioArt scientific artwork. insert/replace directly update the shared canvas against expected_revision, preserving labels, connectors and asset provenance. Default PNG; use format:svg for editable vector artwork. Inspect saved SVG using bioicons palette/parts/geometry and edit with recolor/edit_parts. Preview assets before selection; generic artwork is not evidence of a specific molecule."
)]
#[serde(deny_unknown_fields)]
pub struct Input {
    #[desc = "search | preview | insert | replace"]
    pub action: String,
    #[desc = "For preview/insert/replace only: png (default) or svg. SVG must exist and pass vector normalization; no silent PNG fallback. For saved SVG palette/path edits use bioicons palette/parts/geometry/recolor/edit_parts, regardless of catalogue origin."]
    pub format: Option<visualization_assets::bioart::Format>,
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
        if input.action == "replace" && (input.x.is_some() || input.y.is_some()) {
            return Err(missing(
                "replace preserves current placement; omit x/y. Use visualization apply set_image_geometry to move or resize a standalone image",
            ));
        }
        if input.action == "insert" && input.element_id.is_some() {
            return Err(missing(
                "insert creates a new image at x/y; omit element_id or use replace for an existing object",
            ));
        }
        if input.format.is_some()
            && !matches!(input.action.as_str(), "preview" | "insert" | "replace")
        {
            return Err(missing(
                "format is only supported for preview, insert or replace",
            ));
        }
        let client = bioart::BioArtClient::new().map_err(error)?;
        if input.action == "search" {
            let query = input.query.ok_or_else(|| missing("query required"))?;
            if query.trim().is_empty() || query.len() > 300 {
                return Err(missing("query must contain 1..300 characters"));
            }
            let page = client.search(query.trim(), 0, 12).await.map_err(error)?;
            return Ok(ToolResult::success_json(crate::catalog_search::with_scope(
                "NIH BioArt",
                query.trim(),
                json!(page),
            )));
        }
        if input.action == "preview" {
            let asset = visualization_assets::bioart::prepare(
                &client,
                input.asset_id.ok_or_else(|| missing("asset_id required"))?,
                input.format.unwrap_or_default(),
            )
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
        let asset = visualization_assets::bioart::prepare(
            &client,
            input.asset_id.ok_or_else(|| missing("asset_id required"))?,
            input.format.unwrap_or_default(),
        )
        .await
        .map_err(error)?;
        let path = self.path.clone();
        let placement = Placement {
            project,
            figure,
            revision,
            replacement,

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

fn asset_preview(
    asset: impl Into<visualization_assets::bioart::PreparedAsset>,
) -> Result<ToolResult, ToolError> {
    let asset = asset.into();
    let data = asset
        .preview_url()
        .strip_prefix("data:image/png;base64,")
        .ok_or_else(|| missing("PNG preview unavailable"))?;
    if data.len() > 6 * 1024 * 1024 {
        return Err(missing(
            "asset exceeds vision transfer limit; choose a smaller asset representation",
        ));
    }
    Ok(ToolResult::with_blocks(vec![
        ToolResultBlock::text(asset.metadata().to_string()),
        ToolResultBlock::image_base64("image/png", data),
    ]))
}

struct Placement {
    project: String,
    figure: String,
    revision: i64,
    replacement: bool,

    element_id: Option<String>,
    x: f64,
    y: f64,
}
fn persist_asset(
    path: PathBuf,
    placement: Placement,
    asset: impl Into<visualization_assets::bioart::PreparedAsset>,
) -> Result<serde_json::Value, ToolError> {
    let asset = asset.into();
    let Placement {
        project,
        figure,
        revision,
        replacement,

        element_id,
        x,
        y,
    } = placement;

    let store = visualization::Store::open(path).map_err(error)?;
    let metadata = store.get(&project, &figure).map_err(error)?;
    if metadata.revision != revision {
        return Err(error(visualization::Error::Conflict));
    }

    let image = visualization::image::Image {
        file_id: &asset.sha256,
        data_url: &asset.data_url,
        width: asset.width,
        height: asset.height,
        provenance: asset.provenance(),
    };
    let current = store.revision(&project, &figure, revision).map_err(error)?;
    let (document, id) = if replacement {
        visualization::image::illustrate(
            &current.document,
            element_id
                .as_deref()
                .ok_or_else(|| missing("element_id required"))?,
            image,
        )
    } else {
        visualization::image::insert(&current.document, image, x, y)
    }
    .map_err(error)?;
    let record = store
        .save(&project, &figure, revision, &metadata.title, &document)
        .map_err(error)?;
    let mut result = crate::receipt(&store, &record)?;
    result["element_id"] = json!(id);
    Ok(result)
}
