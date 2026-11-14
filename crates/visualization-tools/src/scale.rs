//! Center-fixed uniform resize without re-entering reviewed source bounds.
use super::*;
#[tool(
    name = "visualization_scale_illustration",
    description = "Directly scale an owned illustration, preserving aspect ratio and ownership. Read the current revision and inspect the canvas preview afterward."
)]
#[serde(deny_unknown_fields)]
pub struct ScaleInput {
    pub project_id: String,
    pub figure_id: String,
    pub expected_revision: i64,
    pub element_id: String,
    pub file_id: String,
    pub factor: f64,
}
struct ScaleTool {
    path: PathBuf,
}
#[async_trait]
impl ToolFunction for ScaleTool {
    type Input = ScaleInput;
    async fn run(&self, input: ScaleInput) -> Result<ToolResult, ToolError> {
        if !input.factor.is_finite() || input.factor <= 0. {
            return Err(invalid(
                "factor must be a finite positive JSON number; 1.1 means enlarge by 10%",
            ));
        }
        let value = json!({"action":"apply","project_id":input.project_id,"figure_id":input.figure_id,"expected_revision":input.expected_revision,"operations":[{"op":"scale_illustration","id":input.element_id,"file_id":input.file_id,"factor":input.factor}]});
        FigureTool {
            path: self.path.clone(),
        }
        .run(serde_json::from_value(value).map_err(failure)?)
        .await
    }
}
pub(crate) fn registration(path: PathBuf) -> ToolRegistration {
    ToolRegistration::from(ScaleTool { path })
}
