//! Flat numeric entry for native wrapped text on a reviewed candidate.
use super::*;
#[tool(
    name = "visualization_add_text",
    description = "Add a new editable scope note or heading to an existing pending candidate. Supply x/y/max_width/font_size as JSON numbers, never strings. Coordinates and width use canvas units; font_size is 4..200. element_id must be a new unique ID. For compact notes, set fit_width:true to shrink the box to its measured wrapped lines; omitted/false retains the fixed max_width. This does not shorten text or change line breaks. This uses the same scientific-font measurement/wrapping and candidate revision path as add_wrapped_text, preserving existing objects, sources and parent. Omit frame_id for notes below the figure; a supplied frame must contain the new text. Read the returned child and preview, and continue dependent edits from its proposal_id/document_sha256. Never adopts a candidate or verifies scientific claims."
)]
pub struct AddTextInput {
    pub project_id: String,
    pub figure_id: String,
    pub proposal_id: String,
    pub document_sha256: String,
    pub expected_revision: i64,
    pub title: String,
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
    #[serde(
        default,
        deserialize_with = "crate::asset_candidate::deserialize_markup_ids"
    )]
    pub markup_ids: Option<Vec<String>>,
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
        let mut value = json!({"action":"revise_proposal","project_id":input.project_id,"figure_id":input.figure_id,"proposal_id":input.proposal_id,"document_sha256":input.document_sha256,"expected_revision":input.expected_revision,"title":input.title,"operations":[operation]});
        if let Some(ids) = input.markup_ids {
            value["markup_ids"] = json!(ids);
        }
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
