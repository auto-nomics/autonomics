//! Explicit opt-in sampled SVG fit; scalar reviewed-bound fitting stays unchanged.
use super::*;
#[tool(
    name = "visualization_fit_svg_preview",
    description = "This tool fits only the primary image. For the full footprint of an uncropped unrotated SVG group, prefer visualization_fit_svg_group_preview; it derives all primary/accessory samples from the exact saved candidate. For other explicitly reviewed group bounds, use visualization propose/revise_proposal with fit_illustration_group and current samples for every member. Fit an owned scientific SVG illustration anchor using backend-derived nonzero-alpha preview bounds with one preview-pixel padding. Read the candidate and review the artwork first. Pass the exact pending proposal ID/hash, current figure revision, image or anchor element_id and current file_id. No manual path arithmetic or normalized bounds input is needed. Only uncropped unrotated SVGs with nonempty rendered content are supported. Opaque backgrounds and decorations are included: this is sampled layout geometry, not exact vector geometry, scientific validation or semantic component detection. For an explicitly reviewed rectangle or raster artwork use visualization_fit instead. This creates a child through the same protected revision path, never adopts or changes the parent. Inspect the child PNG and preserve complete labels and sources."
)]
#[serde(deny_unknown_fields)]
pub struct SvgFitInput {
    pub project_id: String,
    pub figure_id: String,
    pub proposal_id: String,
    pub document_sha256: String,
    pub expected_revision: i64,
    pub title: String,
    pub element_id: String,
    pub file_id: String,
    #[desc = "Optional sketch review IDs as a JSON array, never an empty string. Omit or use [] when no sketch review is being addressed."]
    #[serde(
        default,
        deserialize_with = "crate::asset_candidate::deserialize_markup_ids"
    )]
    pub markup_ids: Option<Vec<String>>,
}
struct SvgFitTool {
    path: PathBuf,
}
pub(crate) fn measured_bounds(path: PathBuf, input: &SvgFitInput) -> Result<Value, ToolError> {
    let store = Store::open(path).map_err(failure)?;
    if store
        .get(&input.project_id, &input.figure_id)
        .map_err(failure)?
        .revision
        != input.expected_revision
    {
        return Err(failure(visualization::Error::Conflict));
    }
    let candidate = store
        .proposal(&input.project_id, &input.figure_id, &input.proposal_id)
        .map_err(failure)?;
    if candidate.status != "pending"
        || candidate.base_revision != input.expected_revision
        || candidate.document_sha256 != input.document_sha256
    {
        return Err(invalid(
            "SVG preview fit requires the exact pending candidate hash and current revision",
        ));
    }
    let elements = candidate.document["elements"]
        .as_array()
        .ok_or_else(|| invalid("candidate elements are missing"))?;
    let selected = elements
        .iter()
        .find(|e| e["id"] == input.element_id && e["isDeleted"] != true)
        .ok_or_else(|| invalid("fit element is missing"))?;
    let image_id = if selected["type"] == "image" {
        selected["id"].as_str()
    } else {
        selected["customData"]["visualAssetElementId"].as_str()
    }
    .ok_or_else(|| invalid("fit requires an owned SVG illustration or anchor"))?;
    let image = elements
        .iter()
        .find(|e| e["id"] == image_id && e["type"] == "image" && e["isDeleted"] != true)
        .ok_or_else(|| invalid("owned image is missing"))?;
    if image["fileId"] != input.file_id {
        return Err(invalid(
            "file_id does not match the current candidate image",
        ));
    }
    if !image["crop"].is_null() || image["angle"].as_f64().unwrap_or(0.) != 0. {
        return Err(invalid(
            "SVG preview fit requires uncropped unrotated artwork; use reviewed explicit bounds for this image",
        ));
    }
    let info = visualization_assets::image_info_document(&candidate.document, image_id)
        .map_err(failure)?;
    let bounds = &info["svg_preview_alpha_bounds"];
    if bounds["status"] != "available" || !bounds["normalized_bounds"].is_array() {
        return Err(invalid(
            "SVG preview fit requires nonempty SVG alpha bounds; raster or empty artwork needs explicit reviewed bounds",
        ));
    }
    Ok(bounds.clone())
}
#[async_trait]
impl ToolFunction for SvgFitTool {
    type Input = SvgFitInput;
    async fn run(&self, input: SvgFitInput) -> Result<ToolResult, ToolError> {
        if [
            &input.project_id,
            &input.figure_id,
            &input.proposal_id,
            &input.document_sha256,
            &input.title,
            &input.element_id,
            &input.file_id,
        ]
        .iter()
        .any(|v| v.trim().is_empty())
        {
            return Err(invalid(
                "SVG preview fit requires nonblank scope, title, element_id and file_id",
            ));
        }
        let path = self.path.clone();
        let (input, bounds) = tokio::task::spawn_blocking(move || {
            let bounds = measured_bounds(path, &input)?;
            Ok::<_, ToolError>((input, bounds))
        })
        .await
        .map_err(failure)??;
        let mut value = json!({"action":"revise_proposal","project_id":input.project_id,"figure_id":input.figure_id,"proposal_id":input.proposal_id,"document_sha256":input.document_sha256,"expected_revision":input.expected_revision,"title":input.title,"operations":[{"op":"fit_illustration_anchor","id":input.element_id,"file_id":input.file_id,"normalized_bounds":bounds["normalized_bounds"]}]});
        if let Some(ids) = input.markup_ids {
            value["markup_ids"] = json!(ids);
        }
        let mut result = FigureTool {
            path: self.path.clone(),
        }
        .run(serde_json::from_value(value).map_err(failure)?)
        .await?;
        if let agentik_sdk::types::tools::ToolResultContent::Json(value) = &mut result.content {
            value["svg_preview_fit_basis"] = bounds;
        }
        Ok(result)
    }
}
pub(crate) fn registration(path: PathBuf) -> ToolRegistration {
    ToolRegistration::from(SvgFitTool { path })
}
