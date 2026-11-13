//! Read-only, revision-bound native rectangle placement suggestions.
use super::*;

#[tool(
    name = "visualization_find_space",
    description = "Find conservative free rectangles for a new image or caption near an existing object, using complete native object extents. Supply relative_to (e.g. a panel ID), width/height and gap as JSON numbers. Returns up to eight adjacent, bounded internal obstacle-edge and outside-scene candidates ranked by added bounding area then proximity to the reference. Internal sampling uses at most 32 coordinates per axis and can miss a valid gap; this is not exhaustive packing. The query does not move objects, reserve space or create proposals. Each candidate includes resulting_bounds and resulting_aspect_ratio for the complete scene with the proposed rectangle. Compare these against the requested composition: the minimum added area may still make the figure too tall or wide; do not automatically choose the first candidate. Includes frames, text boxes, image viewports and sharp paths; excludes glyph/stroke ink and scientific suitability. Unsupported or omitted geometry returns unavailable with no candidates: inspect PNG and request reviewed placement. For a pending candidate supply BOTH proposal_id and its exact document_sha256, plus the current expected_revision; formal reads omit both. A later edit must still use current revision/candidate guards. Never replace a molecule with organelle context artwork; insert standalone context artwork and explain its scope."
)]
pub struct FindSpaceInput {
    pub project_id: String,
    pub figure_id: String,
    pub expected_revision: i64,
    pub proposal_id: Option<String>,
    pub document_sha256: Option<String>,
    pub relative_to: String,
    #[schemars(range(min = 1.0, max = 4096.0))]
    pub width: f64,
    #[schemars(range(min = 1.0, max = 4096.0))]
    pub height: f64,
    #[schemars(range(min = 4.0, max = 256.0))]
    pub gap: f64,
}
struct FindSpaceTool {
    path: PathBuf,
}
#[async_trait]
impl ToolFunction for FindSpaceTool {
    type Input = FindSpaceInput;
    async fn run(&self, input: FindSpaceInput) -> Result<ToolResult, ToolError> {
        let spec = visualization::placement::Spec {
            relative_to: input.relative_to,
            width: input.width,
            height: input.height,
            gap: input.gap,
        };
        visualization::placement::validate(&spec).map_err(invalid)?;
        crate::asset_candidate::validate(
            "insert",
            input.proposal_id.as_deref(),
            input.document_sha256.as_deref(),
            None,
        )
        .map_err(invalid)?;
        let path = self.path.clone();
        let result=tokio::task::spawn_blocking(move || {
            let store=Store::open(path).map_err(failure)?;
            let current=store.get(&input.project_id,&input.figure_id).map_err(failure)?.revision;
            if current!=input.expected_revision { return Err(failure(visualization::Error::Conflict)); }
            let (document,candidate)=if let Some(id)=input.proposal_id {
                let proposal=store.proposal(&input.project_id,&input.figure_id,&id).map_err(failure)?;
                if proposal.document_sha256!=input.document_sha256.unwrap_or_default() || proposal.base_revision!=current || proposal.status!="pending" {
                    return Err(invalid("candidate hash, current base revision and pending status must match"));
                }
                (proposal.document,json!({"proposal_id":proposal.id,"document_sha256":proposal.document_sha256,"base_revision":proposal.base_revision}))
            } else {(store.revision(&input.project_id,&input.figure_id,current).map_err(failure)?.document,Value::Null)};
            let review=visualization::placement::find(&document,&spec).map_err(invalid)?;
            Ok::<_,ToolError>(json!({"project_id":input.project_id,"figure_id":input.figure_id,"current_revision":current,"candidate_context":candidate,"placement_review":review}))
        }).await.map_err(failure)??;
        Ok(ToolResult::success_json(result))
    }
}
pub(crate) fn registration(path: PathBuf) -> ToolRegistration {
    ToolRegistration::from(FindSpaceTool { path })
}
