//! Center-fixed uniform resize without re-entering reviewed source bounds.
use super::*;
#[tool(
    name = "visualization_scale_illustration",
    description = "Uniformly resize an owned illustration on a pending candidate around its unchanged center. When the reviewed fit has scope illustration_group, this scales ALL primary/accessory images around the unchanged group center, preserving relative positions and their source aspect ratios. Every member snapshot and the union must still match; stale, locked or out-of-frame groups reject atomically. factor is a positive JSON number: 1.1 enlarges both dimensions by 10%, 0.9 shrinks by 10%; it is not a percentage or the native flip scale. Read the candidate first and pass its exact image or semantic anchor ID and file_id. Reuses the current reviewed visualAnchorFit.normalizedBounds; absent/stale fits are rejected rather than guessed. Files, crop, flips, angle, labels and unrelated objects remain unchanged; supported connector endpoints follow the fitted anchor. This creates a child through the standard protected revision path and never adopts. Continue from the returned child ID/hash and inspect its PNG; labels may need separate explicit repositioning."
)]
pub struct ScaleInput {
    pub project_id: String,
    pub figure_id: String,
    pub proposal_id: String,
    pub document_sha256: String,
    pub expected_revision: i64,
    pub title: String,
    pub element_id: String,
    pub file_id: String,
    pub factor: f64,
    #[serde(
        default,
        deserialize_with = "crate::asset_candidate::deserialize_markup_ids"
    )]
    pub markup_ids: Option<Vec<String>>,
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
        let mut value = json!({"action":"revise_proposal","project_id":input.project_id,"figure_id":input.figure_id,"proposal_id":input.proposal_id,"document_sha256":input.document_sha256,"expected_revision":input.expected_revision,"title":input.title,"operations":[{"op":"scale_illustration","id":input.element_id,"file_id":input.file_id,"factor":input.factor}]});
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
    ToolRegistration::from(ScaleTool { path })
}
