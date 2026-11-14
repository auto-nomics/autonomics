//! One unbound text object moves independently of its native group.
use super::*;
#[tool(
    name = "visualization_move_text",
    description = "Directly move one unbound text object by numeric dx/dy, without moving its component. Retains text, typography and ownership; bound labels must move with their container. Returns the committed revision."
)]
#[serde(deny_unknown_fields)]
pub struct MoveTextInput {
    pub project_id: String,
    pub figure_id: String,
    pub expected_revision: i64,
    pub element_id: String,
    pub dx: f64,
    pub dy: f64,
}
struct MoveTextTool {
    path: PathBuf,
}
fn number(e: &Value, key: &str) -> Result<f64, ToolError> {
    e[key]
        .as_f64()
        .filter(|n| n.is_finite() && n.abs() <= 1e6)
        .ok_or_else(|| {
            invalid(&format!(
                "text/frame {key} must be a finite JSON number within coordinate limits"
            ))
        })
}
fn unrotated(e: &Value) -> bool {
    e["angle"].is_null() || e["angle"].as_f64() == Some(0.)
}
#[async_trait]
impl ToolFunction for MoveTextTool {
    type Input = MoveTextInput;
    async fn run(&self, input: MoveTextInput) -> Result<ToolResult, ToolError> {
        if ![input.dx, input.dy]
            .iter()
            .all(|n| n.is_finite() && n.abs() <= 1e6)
        {
            return Err(invalid(
                "dx/dy must be finite JSON numbers within coordinate limits",
            ));
        }
        let store = Store::open(&self.path).map_err(failure)?;
        if store
            .get(&input.project_id, &input.figure_id)
            .map_err(failure)?
            .revision
            != input.expected_revision
        {
            return Err(failure(visualization::Error::Conflict));
        }
        let document = store
            .revision(&input.project_id, &input.figure_id, input.expected_revision)
            .map_err(failure)?
            .document;
        let elements = document["elements"]
            .as_array()
            .ok_or_else(|| invalid("source has no native elements"))?;
        let text = elements
            .iter()
            .find(|e| e["id"].as_str() == Some(&input.element_id))
            .ok_or_else(|| invalid("element_id is not present in this source"))?;
        if text["type"] != "text"
            || text["isDeleted"] == true
            || text["locked"] == true
            || !text["containerId"].is_null()
            || !unrotated(text)
        {
            return Err(invalid(
                "requires live unlocked unbound unrotated text; groups are retained, but bound node/arrow text must be reflowed or moved with its container",
            ));
        }
        let x = number(text, "x")? + input.dx;
        let y = number(text, "y")? + input.dy;
        let width = number(text, "width")?;
        let height = number(text, "height")?;
        if ![x, y].iter().all(|n| n.is_finite() && n.abs() <= 1e6) || width <= 0. || height <= 0. {
            return Err(invalid(
                "resulting text position or existing text dimensions are invalid",
            ));
        }
        if let Some(frame_id) = text["frameId"].as_str() {
            let frame = elements
                .iter()
                .find(|e| e["id"] == frame_id && e["isDeleted"] != true)
                .ok_or_else(|| invalid("text frame is missing"))?;
            if !matches!(frame["type"].as_str(), Some("frame" | "magicframe")) || !unrotated(frame)
            {
                return Err(invalid("text frame must have valid unrotated geometry"));
            }
            let fx = number(frame, "x")?;
            let fy = number(frame, "y")?;
            let fw = number(frame, "width")?;
            let fh = number(frame, "height")?;
            if fw <= 0.
                || fh <= 0.
                || x < fx - 1e-6
                || y < fy - 1e-6
                || x + width > fx + fw + 1e-6
                || y + height > fy + fh + 1e-6
            {
                return Err(invalid(
                    "text movement must stay inside its current frame; frame membership is not reassigned",
                ));
            }
        } else if !text["frameId"].is_null() {
            return Err(invalid("text frame identity is invalid"));
        }
        drop(store);
        let value = json!({"action":"apply","project_id":input.project_id,"figure_id":input.figure_id,"expected_revision":input.expected_revision,"operations":[{"op":"set_text_position","id":input.element_id,"x":x,"y":y}]});
        FigureTool {
            path: self.path.clone(),
        }
        .run(serde_json::from_value(value).map_err(failure)?)
        .await
    }
}
pub(crate) fn registration(path: PathBuf) -> ToolRegistration {
    ToolRegistration::from(MoveTextTool { path })
}
