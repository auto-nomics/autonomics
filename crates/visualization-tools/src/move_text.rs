//! One unbound text object moves independently of its native group.
use super::*;
#[tool(
    name = "visualization_move_text",
    description = "Move exactly one existing live unlocked unbound text object by numeric dx/dy. Omit BOTH proposal_id and document_sha256 to propose directly from the saved formal expected_revision; this creates one pending candidate without an unchanged seed and never reads an unadopted candidate. To continue a pending candidate supply BOTH its proposal_id and exact document_sha256. A partial or blank pair is rejected. Unlike visualization_translate, this never expands a group: an illustration label moves without its image, semantic anchor, connectors or other labels. Preserve its text, wrapping, font, size, ownership and metadata. Read the intended formal revision or current candidate first; element_id is the text ID, not its image or anchor. Bound node/arrow text follows its container and is rejected here; use text reflow or move the container instead. Existing frame membership is preserved and text must remain inside that frame. dx/dy are top-level JSON numbers, not strings or operations. Continue from the returned child ID/hash and inspect its PNG. This only proposes a change and never adopts it."
)]
pub struct MoveTextInput {
    pub project_id: String,
    pub figure_id: String,
    pub proposal_id: Option<String>,
    pub document_sha256: Option<String>,
    pub expected_revision: i64,
    pub title: String,
    pub element_id: String,
    pub dx: f64,
    pub dy: f64,
    #[serde(
        default,
        deserialize_with = "crate::asset_candidate::deserialize_markup_ids"
    )]
    pub markup_ids: Option<Vec<String>>,
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
        let candidate = match (&input.proposal_id, &input.document_sha256) {
            (None, None) => false,
            (Some(id), Some(hash)) if !id.trim().is_empty() && !hash.trim().is_empty() => true,
            _ => {
                return Err(invalid(
                    "provide both nonblank proposal_id and document_sha256 for a candidate, or omit both for the saved formal revision",
                ));
            }
        };
        let store = Store::open(&self.path).map_err(failure)?;
        if store
            .get(&input.project_id, &input.figure_id)
            .map_err(failure)?
            .revision
            != input.expected_revision
        {
            return Err(failure(visualization::Error::Conflict));
        }
        let document = if candidate {
            let parent = store
                .proposal(
                    &input.project_id,
                    &input.figure_id,
                    input.proposal_id.as_deref().unwrap(),
                )
                .map_err(failure)?;
            if Some(parent.document_sha256.as_str()) != input.document_sha256.as_deref() {
                return Err(invalid(
                    "document_sha256 does not match the immutable parent; read that candidate before retrying",
                ));
            }
            if parent.status != "pending" || parent.base_revision != input.expected_revision {
                return Err(failure(visualization::Error::Conflict));
            }
            parent.document
        } else {
            store
                .revision(&input.project_id, &input.figure_id, input.expected_revision)
                .map_err(failure)?
                .document
        };
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
        let mut value = json!({"action":if candidate {"revise_proposal"} else {"propose"},"project_id":input.project_id,"figure_id":input.figure_id,"proposal_id":input.proposal_id,"document_sha256":input.document_sha256,"expected_revision":input.expected_revision,"title":input.title,"operations":[{"op":"set_text_position","id":input.element_id,"x":x,"y":y}]});
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
    ToolRegistration::from(MoveTextTool { path })
}
