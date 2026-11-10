//! Explicit read-only continuations for the exact scope validated by a tool call.
//! These are copied arguments, never automatic execution or scope fallback.
use serde_json::{Value, json};

/// Asset tools keep their legacy proposal_id/element_id keys and expose the
/// same exact read continuations as visualization, without embedding a scene.
pub(crate) fn proposal_result(
    project: &str,
    proposal: &visualization::proposal::Proposal,
    element: Option<&str>,
) -> Value {
    let mut result = json!({"proposal_id":proposal.id,"figure_id":proposal.visualization_id,"visualization_id":proposal.visualization_id,"base_revision":proposal.base_revision,"summary":proposal.summary,"status":proposal.status,"document_sha256":proposal.document_sha256,"parent_proposal_id":proposal.parent_proposal_id,"parent_document_sha256":proposal.parent_document_sha256});
    if let Some(element) = element {
        result["element_id"] = json!(element);
    }
    attach(&mut result, Some(project), Some(&proposal.visualization_id));
    result
}

pub(crate) fn attach(value: &mut Value, project: Option<&str>, figure: Option<&str>) {
    let (Some(project), Some(object)) = (project, value.as_object()) else {
        return;
    };
    let candidate = object.get("proposal").unwrap_or(value);
    let candidate_figure = candidate.get("visualization_id").and_then(Value::as_str);
    let Some(figure) = figure.or(candidate_figure).map(str::to_owned) else {
        return;
    };
    let mut inputs = json!({
        "read":{"action":"read","project_id":project,"figure_id":figure},
        "read_plan":{"action":"read_plan","project_id":project,"figure_id":figure}
    });
    let descriptor = (
        candidate
            .get("id")
            .or_else(|| candidate.get("proposal_id"))
            .and_then(Value::as_str),
        candidate.get("base_revision").and_then(Value::as_i64),
        candidate.get("document_sha256").and_then(Value::as_str),
    );
    let mut context = None;
    if let (Some(proposal), Some(base), Some(hash)) = descriptor
        && candidate_figure == Some(figure.as_str())
    {
        // A read can inspect an old candidate at a newer head; keep the two
        // revisions distinct. Plan version is never a figure revision.
        let current = value
            .get("current_revision")
            .or_else(|| candidate.get("current_revision"))
            .and_then(Value::as_i64)
            .unwrap_or(base);
        for action in ["read_proposal", "read_proposal_preview"] {
            inputs[action] = json!({"action":action,"project_id":project,"figure_id":figure,"proposal_id":proposal,"expected_revision":current});
        }
        context = Some(
            json!({"project_id":project,"figure_id":figure,"proposal_id":proposal,"expected_revision":current,"base_revision":base,"document_sha256":hash}),
        );
    }
    let object = value.as_object_mut().expect("object was checked above");
    object.insert(
        "figure_scope".into(),
        json!({"project_id":project,"figure_id":figure}),
    );
    object.insert("follow_up_inputs".into(), inputs);
    if let Some(context) = context {
        object.insert("candidate_context".into(), context);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_validated_scope_and_read_only_inputs_are_exposed() {
        let mut candidate = json!({"proposal":{"id":"candidate","visualization_id":"figure","base_revision":2,"document_sha256":"hash"},"current_revision":7,"planning_version":99});
        attach(&mut candidate, Some("project"), Some("figure"));
        assert_eq!(candidate["candidate_context"]["expected_revision"], 7);
        assert_eq!(candidate["candidate_context"]["base_revision"], 2);
        assert_eq!(
            candidate["follow_up_inputs"]["read_proposal"]["expected_revision"],
            7
        );
        assert_eq!(
            candidate["follow_up_inputs"]["read_proposal"]["figure_id"],
            "figure"
        );
        assert_eq!(candidate["follow_up_inputs"].as_object().unwrap().len(), 4);
        let mut missing_scope = json!({"id":"candidate","visualization_id":"figure","base_revision":2,"document_sha256":"hash"});
        attach(&mut missing_scope, None, Some("figure"));
        assert!(missing_scope.get("follow_up_inputs").is_none());
        let mut foreign = candidate["proposal"].clone();
        attach(&mut foreign, Some("project"), Some("another-figure"));
        assert!(foreign.get("candidate_context").is_none());
        assert!(foreign["follow_up_inputs"].get("read_proposal").is_none());
        let mut listing = json!([]);
        attach(&mut listing, Some("project"), None);
        assert_eq!(listing, json!([]));
    }
}
