//! Flat numeric entry for native wrapped text on a reviewed candidate.
use super::*;
#[tool(
    name = "visualization_add_text",
    description = "Add complete editable text to the canvas using numeric x/y/max_width/font_size. element_id must be unique. Optional fit_width shrinks the measured text box; optional frame_id must contain it. Directly commits one revision."
)]
#[serde(deny_unknown_fields)]
pub struct AddTextInput {
    pub project_id: String,
    pub figure_id: String,
    pub expected_revision: i64,
    pub element_id: String,
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub max_width: f64,
    #[desc = "Optional compact text box: true fits width to the measured wrapped lines without changing text, wrapping, left origin or font. Defaults false, preserving max_width as the fixed box width. Normal frame/geometry checks still apply to max_width."]
    pub fit_width: Option<bool>,
    pub font_size: f64,
    #[serde(default)]
    pub frame_id: Option<String>,
}
struct AddTextTool {
    path: PathBuf,
}
#[async_trait]
impl ToolFunction for AddTextTool {
    type Input = AddTextInput;
    async fn run(&self, input: AddTextInput) -> Result<ToolResult, ToolError> {
        let mut operation = json!({"op":"add_wrapped_text","id":input.element_id,"text":input.text,"x":input.x,"y":input.y,"max_width":input.max_width,"font_size":input.font_size,"fit_width":input.fit_width.unwrap_or(false)});
        if let Some(frame) = input.frame_id {
            operation["frame_id"] = json!(frame);
        }
        let value = json!({"action":"apply","project_id":input.project_id,"figure_id":input.figure_id,"expected_revision":input.expected_revision,"operations":[operation]});
        FigureTool {
            path: self.path.clone(),
        }
        .run(serde_json::from_value(value).map_err(failure)?)
        .await
    }
}
pub(crate) fn registration(path: PathBuf) -> ToolRegistration {
    ToolRegistration::from(AddTextTool { path })
}
