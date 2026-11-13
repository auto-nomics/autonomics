//! Typed entry for reflowing one existing formal or candidate text object.
use super::*;
#[tool(
    name = "visualization_reflow_text",
    description = "Edit or rewrap one existing text object. Omit BOTH proposal_id and document_sha256 to generate one pending candidate directly from the saved formal expected_revision. Supply BOTH the exact pending proposal_id and document_sha256 to continue a reviewed candidate. Partial or blank pairs are rejected; unadopted candidates never substitute for the formal revision. Supply the complete intended originalText, preserving scientific wording and explicit newlines unless the user requested a wording change. Optional max_width and font_size are JSON numbers, never strings; font_size is 4..200. Omit font_size to retain typography. Omit max_width to retain the current width budget. Explicit max_width supports unbound text and sharp arrow captions; shape-bound text requires resizing its container. Arrow captions remain centered on the existing path, preserving the arrow and bindings. Uses the native set_wrapped_text operation and standard exact-hash/current-revision/leaf-parent checks; never adopts or edits the parent or historical plan. Continue dependent edits from the returned child proposal_id/document_sha256. Inspect the candidate PNG for overlap and clipping. For multiple compatible edits use visualization revise_proposal operations."
)]
pub struct ReflowTextInput {
    pub project_id: String,
    pub figure_id: String,
    pub proposal_id: Option<String>,
    pub document_sha256: Option<String>,
    pub expected_revision: i64,
    pub title: String,
    pub element_id: String,
    pub text: String,
    pub max_width: Option<f64>,
    pub font_size: Option<f64>,
    #[serde(
        default,
        deserialize_with = "crate::asset_candidate::deserialize_markup_ids"
    )]
    pub markup_ids: Option<Vec<String>>,
}
struct ReflowTextTool {
    path: PathBuf,
}
#[async_trait]
impl ToolFunction for ReflowTextTool {
    type Input = ReflowTextInput;
    async fn run(&self, input: ReflowTextInput) -> Result<ToolResult, ToolError> {
        let candidate = match (&input.proposal_id, &input.document_sha256) {
            (None, None) => false,
            (Some(id), Some(hash)) if !id.trim().is_empty() && !hash.trim().is_empty() => true,
            _ => {
                return Err(invalid(
                    "provide both nonblank proposal_id and document_sha256 for a candidate, or omit both for the saved formal revision",
                ));
            }
        };
        let operation = json!({"op":"set_wrapped_text","id":input.element_id,"text":input.text,"max_width":input.max_width,"font_size":input.font_size});
        let mut value = json!({"action":if candidate {"revise_proposal"} else {"propose"},"project_id":input.project_id,"figure_id":input.figure_id,"proposal_id":input.proposal_id,"document_sha256":input.document_sha256,"expected_revision":input.expected_revision,"title":input.title,"operations":[operation]});
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
    ToolRegistration::from(ReflowTextTool { path })
}
