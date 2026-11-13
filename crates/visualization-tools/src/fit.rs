//! Scalar-parameter entry to the same reviewed illustration fitting operation.
use super::*;

#[tool(
    name = "visualization_fit",
    description = "This tool fits only the primary image. For the full footprint of an uncropped unrotated SVG group, prefer visualization_fit_svg_group_preview; it derives all primary/accessory samples from the exact saved candidate. For other explicitly reviewed group bounds, use visualization propose/revise_proposal with fit_illustration_group and current samples for every member. Fit an existing scientific anchor to reviewed source-image bounds on a pending candidate. Read the current candidate and review the actual asset preview first. For SVG, bioicons parts can provide path bounds; PNG and other raster images have no SVG parts, so use the reviewed original image dimensions and an explicit visible-content rectangle. x/y/width/height are finite JSON numbers in normalized original source coordinates [0,1], not canvas pixels or cropped viewport coordinates. This creates a child proposal through the standard visualization revision path; it never adopts or changes the parent. Continue dependent edits from the returned child ID/hash, not the old parent. For an explicitly intended sampled fit of uncropped unrotated SVG, visualization_fit_svg_preview can derive padded alpha bounds in the backend; use this scalar tool when supplying an explicitly reviewed rectangle. Generic art does not verify molecular structure."
)]
pub struct FitInput {
    pub project_id: String,
    pub figure_id: String,
    pub proposal_id: String,
    pub document_sha256: String,
    pub expected_revision: i64,
    pub title: String,
    pub element_id: String,
    pub file_id: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    #[serde(
        default,
        deserialize_with = "crate::asset_candidate::deserialize_markup_ids"
    )]
    pub markup_ids: Option<Vec<String>>,
}
struct FitTool {
    path: PathBuf,
}
#[async_trait]
impl ToolFunction for FitTool {
    type Input = FitInput;
    async fn run(&self, input: FitInput) -> Result<ToolResult, ToolError> {
        let mut value = json!({"action":"revise_proposal","project_id":input.project_id,"figure_id":input.figure_id,"proposal_id":input.proposal_id,"document_sha256":input.document_sha256,"expected_revision":input.expected_revision,"title":input.title,"operations":[{"op":"fit_illustration_anchor","id":input.element_id,"file_id":input.file_id,"normalized_bounds":[input.x,input.y,input.width,input.height]}]});
        if let Some(ids) = input.markup_ids {
            value["markup_ids"] = json!(ids);
        }
        let input: FigureInput = serde_json::from_value(value).map_err(failure)?;
        FigureTool {
            path: self.path.clone(),
        }
        .run(input)
        .await
    }
}
pub(crate) fn registration(path: PathBuf) -> ToolRegistration {
    ToolRegistration::from(FitTool { path })
}
