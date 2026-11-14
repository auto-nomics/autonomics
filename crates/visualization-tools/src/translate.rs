//! Flat typed parameters for reviewed component translations.
use super::*;

#[tool(
    name = "visualization_translate",
    description = "Directly translate selected components by numeric dx/dy. Groups, frames, owned artwork and connectors move together. For an individual unbound label use visualization_move_text. Batch compatible edits with visualization apply. Returns the committed revision."
)]
#[serde(deny_unknown_fields)]
pub struct TranslateInput {
    pub project_id: String,
    pub figure_id: String,
    pub expected_revision: i64,
    pub element_ids: Vec<String>,
    pub dx: f64,
    pub dy: f64,
}
struct TranslateTool {
    path: PathBuf,
}
#[async_trait]
impl ToolFunction for TranslateTool {
    type Input = TranslateInput;
    async fn run(&self, input: TranslateInput) -> Result<ToolResult, ToolError> {
        let value = json!({"action":"apply","project_id":input.project_id,"figure_id":input.figure_id,"expected_revision":input.expected_revision,"operations":[{"op":"translate","element_ids":input.element_ids,"dx":input.dx,"dy":input.dy}]});
        FigureTool {
            path: self.path.clone(),
        }
        .run(serde_json::from_value(value).map_err(failure)?)
        .await
    }
}
pub(crate) fn registration(path: PathBuf) -> ToolRegistration {
    ToolRegistration::from(TranslateTool { path })
}
