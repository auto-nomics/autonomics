//! Typed entry for reflowing one existing formal or candidate text object.
use super::*;
#[tool(
    name = "visualization_reflow_text",
    description = "Directly edit or rewrap existing text. Supply complete original text; preserve scientific wording. Optional numeric max_width/font_size retain current values when omitted. Shape-bound text needs container resizing for width changes. Arrow captions retain bindings. Inspect read_preview for overlap."
)]
#[serde(deny_unknown_fields)]
pub struct ReflowTextInput {
    pub project_id: String,
    pub figure_id: String,
    pub expected_revision: i64,
    pub element_id: String,
    pub text: String,
    pub max_width: Option<f64>,
    pub font_size: Option<f64>,
}
struct ReflowTextTool {
    path: PathBuf,
}
#[async_trait]
impl ToolFunction for ReflowTextTool {
    type Input = ReflowTextInput;
    async fn run(&self, input: ReflowTextInput) -> Result<ToolResult, ToolError> {
        let operation = json!({"op":"set_wrapped_text","id":input.element_id,"text":input.text,"max_width":input.max_width,"font_size":input.font_size});
        let value = json!({"action":"apply","project_id":input.project_id,"figure_id":input.figure_id,"expected_revision":input.expected_revision,"operations":[operation]});
        FigureTool {
            path: self.path.clone(),
        }
        .run(serde_json::from_value(value).map_err(failure)?)
        .await
    }
}
pub(crate) fn registration(path: PathBuf) -> ToolRegistration {
    ToolRegistration::from(ReflowTextTool { path })
}
