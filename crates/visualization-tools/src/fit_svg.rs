//! Explicit opt-in sampled SVG fit; scalar reviewed-bound fitting stays unchanged.
use super::*;
#[tool(
    name = "visualization_fit_svg_preview",
    description = "Fit a primary SVG illustration anchor using backend-measured nonzero-alpha bounds. Requires current file_id and expected_revision; supports uncropped unrotated nonempty SVGs. Directly commits the fit. Bounds include decoration and are not scientific identity checks. For accessories use visualization_fit_svg_group_preview."
)]
#[serde(deny_unknown_fields)]
pub struct SvgFitInput {
    pub project_id: String,
    pub figure_id: String,
    pub expected_revision: i64,
    pub element_id: String,
    pub file_id: String,
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
        .revision(&input.project_id, &input.figure_id, input.expected_revision)
        .map_err(failure)?;
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
        let value = json!({"action":"apply","project_id":input.project_id,"figure_id":input.figure_id,"expected_revision":input.expected_revision,"operations":[{"op":"fit_illustration_anchor","id":input.element_id,"file_id":input.file_id,"normalized_bounds":bounds["normalized_bounds"]}]});
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
