//! Read-only, revision-bound native rectangle placement suggestions.
use super::*;

#[tool(
    name = "visualization_find_space",
    description = "Read-only free-space suggestions for a rectangle near relative_to. Supply numeric width/height/gap and current revision. Ranks bounded samples by added area and proximity; not exhaustive packing or pixel/scientific validation. Does not reserve space."
)]
#[serde(deny_unknown_fields)]
pub struct FindSpaceInput {
    pub project_id: String,
    pub figure_id: String,
    pub expected_revision: i64,
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
        let path = self.path.clone();
        let result=tokio::task::spawn_blocking(move || {
            let store=Store::open(path).map_err(failure)?;
            let current=store.get(&input.project_id,&input.figure_id).map_err(failure)?.revision;
            if current!=input.expected_revision { return Err(failure(visualization::Error::Conflict)); }
            let document=store.revision(&input.project_id,&input.figure_id,current).map_err(failure)?.document;
            let review=visualization::placement::find(&document,&spec).map_err(invalid)?;
            Ok::<_,ToolError>(json!({"project_id":input.project_id,"figure_id":input.figure_id,"current_revision":current,"placement_review":review}))
        }).await.map_err(failure)??;
        Ok(ToolResult::success_json(result))
    }
}
pub(crate) fn registration(path: PathBuf) -> ToolRegistration {
    ToolRegistration::from(FindSpaceTool { path })
}
