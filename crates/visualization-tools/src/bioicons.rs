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
    description = "Search BioIcons (search/search_batch), preview assets, and directly insert or replace artwork on the current canvas. Read current revision before editing. palette/parts/geometry inspect saved SVG; recolor/edit_parts directly update it, preserving provenance. Use sha256 from parts for path edits. Preview assets before selection and inspect the canvas after changes. SVG paths have no inferred biological identity."
)]
#[serde(deny_unknown_fields)]
pub struct Input {
    #[desc = "search | search_batch | preview | insert | replace | palette | recolor | parts | geometry | edit_parts"]
    pub action: String,
    pub query: Option<String>,
    #[desc = "Only search/search_batch: exact author name from catalogue attribution, ignoring case/outer whitespace. Restricts every query; omit to search all authors. Same author does not guarantee matching style or scientific suitability."]
    pub author: Option<String>,
    #[desc = "Only for search_batch: 1..8 distinct nonblank query strings, each at most 256 UTF-8 bytes. Omit query; each result retains its own retrieval scope and lexical counts."]
    pub queries: Option<Vec<String>>,
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
        if let Some(author) = &input.author {
            if !matches!(input.action.as_str(), "search" | "search_batch")
                || author.trim().is_empty()
                || author.len() > 300
            {
                return Err(error(
                    "author is only supported for search/search_batch and must be a nonblank name of at most 300 UTF-8 bytes",
                ));
            }
        }
        if input.queries.is_some() && input.action != "search_batch" {
            return Err(error(
                "queries is supported only for search_batch; use query for search",
            ));
        }
        if matches!(
            input.action.as_str(),
            "palette" | "recolor" | "parts" | "geometry" | "edit_parts"
        ) {
            let path = self.path.clone();
            return tokio::task::spawn_blocking(move || local_svg(path, input))
                .await
                .map_err(error)?;
        }

        if input.action == "replace" && (input.x.is_some() || input.y.is_some()) {
            return Err(error(
                "replace preserves current placement; omit x/y. Use visualization apply set_image_geometry to move or resize a standalone image",
            ));
        }
        if input.action == "insert" && input.element_id.is_some() {
            return Err(error(
                "insert creates a new image at x/y; omit element_id or use replace for an existing object",
            ));
        }
        if input.action == "search_batch" {
            if input.query.is_some() {
                return Err(error("search_batch requires queries; omit singular query"));
            }
            let queries = input
                .queries
                .as_deref()
                .ok_or_else(|| error("search_batch requires queries"))?;
            crate::bioicons_batch::validate(queries).map_err(error)?;
            let client = CLIENT.as_ref().map_err(error)?;
            return Ok(ToolResult::success_json(
                crate::bioicons_batch::search_with_author(client, queries, input.author.as_deref())
                    .await,
            ));
        }
        let client = CLIENT.as_ref().map_err(error)?;
        if input.action == "search" {
            let query = input
                .query
                .as_deref()
                .ok_or_else(|| error("query required"))?;
            let page = client
                .search_with_author(query, 0, 12, input.author.as_deref())
                .await
                .map_err(error)?;
            return Ok(ToolResult::success_json(crate::catalog_search::with_scope(
                "BioIcons",
                query,
                json!(page),
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
fn local_svg(path: PathBuf, input: Input) -> Result<ToolResult, ToolError> {
    use visualization_assets::svg::{self, Inspection, SvgEdit, SvgRefinement};
    let project = input
        .project_id
        .as_deref()
        .ok_or_else(|| error("project_id required"))?;
    let figure = input
        .figure_id
        .as_deref()
        .ok_or_else(|| error("figure_id required"))?;
    let revision = input
        .expected_revision
        .ok_or_else(|| error("expected_revision required"))?;
    let target = input
        .element_id
        .as_deref()
        .ok_or_else(|| error("element_id required"))?;
    let store = visualization::Store::open(path).map_err(error)?;
    if matches!(input.action.as_str(), "palette" | "parts" | "geometry") {
        let kind = match input.action.as_str() {
            "palette" => Inspection::Palette,
            "parts" => Inspection::Parts,
            _ => Inspection::Geometry {
                sha256: input
                    .sha256
                    .as_deref()
                    .ok_or_else(|| error("sha256 required"))?,
                part_id: input
                    .part_id
                    .as_deref()
                    .ok_or_else(|| error("part_id required"))?,
            },
        };
        let mut value =
            svg::inspect_svg(&store, project, figure, revision, target, kind).map_err(error)?;
        let image_key = if input.action == "palette" {
            "previewDataURL"
        } else {
            "annotatedDataURL"
        };
        let url = value
            .as_object_mut()
            .unwrap()
            .remove(image_key)
            .and_then(|v| v.as_str().map(str::to_owned))
            .ok_or_else(|| error("preview unavailable"))?;
        value.as_object_mut().unwrap().remove("previewDataURL");
        let data = url
            .split_once(',')
            .ok_or_else(|| error("invalid preview"))?
            .1;
        return Ok(ToolResult::with_blocks(vec![
            ToolResultBlock::text(value.to_string()),
            ToolResultBlock::image_base64("image/png", data),
        ]));
    }
    let edit = if input.action == "edit_parts" {
        SvgEdit::Parts {
            sha256: input
                .sha256
                .as_deref()
                .ok_or_else(|| error("sha256 required"))?,
            edits: input
                .edits
                .as_deref()
                .ok_or_else(|| error("edits required"))?,
        }
    } else {
        SvgEdit::Recolor(
            input
                .colors
                .as_ref()
                .ok_or_else(|| error("colors required"))?,
        )
    };
    let record = svg::refine_svg(
        &store,
        project,
        figure,
        revision,
        SvgRefinement { target, edit },
    )
    .map_err(error)?;
    Ok(ToolResult::success_json(crate::receipt(&store, &record)?))
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

    let image = visualization::image::Image {
        file_id: &asset.vector.sha256,
        data_url: &asset.vector.data_url,
        width: asset.vector.width,
        height: asset.vector.height,
        provenance: json!({"source":"BioIcons","asset":asset.asset,"sha256":asset.vector.sha256,"source_sha256":asset.vector.source_sha256,"colors":asset.vector.colors,"normalization":asset.vector.normalization}),
    };
    let scene = store.revision(&project, &figure, revision).map_err(error)?;
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
    let record = store
        .save(
            &project,
            &figure,
            revision,
            &store.get(&project, &figure).map_err(error)?.title,
            &document,
        )
        .map_err(error)?;
    let mut result = crate::receipt(&store, &record)?;
    result["element_id"] = json!(id);
    Ok(result)
}

pub fn registrations(path: PathBuf) -> Vec<ToolRegistration> {
    vec![ToolRegistration::from(BioIconsTool { path })]
}
