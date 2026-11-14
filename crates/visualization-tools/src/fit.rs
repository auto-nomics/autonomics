//! Scalar-parameter entry to the same reviewed illustration fitting operation.
use super::*;

#[tool(
    name = "visualization_fit",
    description = "Fit an illustration anchor to explicit normalized source bounds x/y/width/height. Requires current file_id. Directly updates the canvas while preserving artwork, labels and sources. Inspect read_preview afterward."
)]
#[serde(deny_unknown_fields)]
pub struct FitInput {
    pub project_id: String,
    pub figure_id: String,
    pub expected_revision: i64,
    pub element_id: String,
    pub file_id: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
struct FitTool {
    path: PathBuf,
}
#[async_trait]
impl ToolFunction for FitTool {
    type Input = FitInput;
    async fn run(&self, input: FitInput) -> Result<ToolResult, ToolError> {
        let value = json!({"action":"apply","project_id":input.project_id,"figure_id":input.figure_id,"expected_revision":input.expected_revision,"operations":[{"op":"fit_illustration_anchor","id":input.element_id,"file_id":input.file_id,"normalized_bounds":[input.x,input.y,input.width,input.height]}]});
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
