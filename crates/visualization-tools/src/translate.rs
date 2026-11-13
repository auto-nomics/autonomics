//! Flat typed parameters for reviewed component translations.
use super::*;

#[tool(
    name = "visualization_translate",
    description = "Rigidly move existing objects on a pending candidate without replacing their IDs. dx/dy are required JSON numbers in canvas coordinates, never quoted strings. element_ids is a JSON array of existing IDs. Groups, frames, owned illustrations/labels and supported attached connectors follow the same protected translate operation as visualization. Selecting a grouped illustration label moves its whole component, including image, anchor and supported connectors. For only one detached label position, prefer visualization_move_text with one element_id and numeric dx/dy, or use visualization revise_proposal with numeric update coordinates; group translation is not a label-only operation. Read the candidate first, preserve unrelated scientific content, and inspect the returned child preview. Dependent edits must continue from the returned child proposal_id/document_sha256; sibling proposals do not merge. This never adopts a candidate."
)]
pub struct TranslateInput {
    pub project_id: String,
    pub figure_id: String,
    pub proposal_id: String,
    pub document_sha256: String,
    pub expected_revision: i64,
    pub title: String,
    pub element_ids: Vec<String>,
    pub dx: f64,
    pub dy: f64,
    #[serde(
        default,
        deserialize_with = "crate::asset_candidate::deserialize_markup_ids"
    )]
    #[desc = "Optional sketch review IDs as a JSON array. For no sketch links use [] or omit this field; never use an empty string or a quoted array. Object annotation IDs are not sketch markup IDs."]
    pub markup_ids: Option<Vec<String>>,
}
struct TranslateTool {
    path: PathBuf,
}
#[async_trait]
impl ToolFunction for TranslateTool {
    type Input = TranslateInput;
    async fn run(&self, input: TranslateInput) -> Result<ToolResult, ToolError> {
        let mut value = json!({"action":"revise_proposal","project_id":input.project_id,"figure_id":input.figure_id,"proposal_id":input.proposal_id,"document_sha256":input.document_sha256,"expected_revision":input.expected_revision,"title":input.title,"operations":[{"op":"translate","element_ids":input.element_ids,"dx":input.dx,"dy":input.dy}]});
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
    ToolRegistration::from(TranslateTool { path })
}

#[cfg(test)]
mod tests {
    use super::TranslateInput;
    use serde_json::json;
    #[test]
    fn translation_review_links_require_an_array_not_an_empty_string() {
        let mut input=json!({"project_id":"p","figure_id":"f","proposal_id":"candidate","document_sha256":"hash","expected_revision":2,"title":"move frame","element_ids":["panel-1"],"dx":30,"dy":20});
        assert!(serde_json::from_value::<TranslateInput>(input.clone()).unwrap().markup_ids.is_none());
        input["markup_ids"]=json!("");
        let error=serde_json::from_value::<TranslateInput>(input.clone()).err().unwrap().to_string();
        assert!(error.contains("empty string"));
        input["markup_ids"]=json!([]);
        assert_eq!(serde_json::from_value::<TranslateInput>(input).unwrap().markup_ids,Some(vec![]));
    }
}
