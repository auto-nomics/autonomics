//! Atomic compare-and-set label updates without round-tripping the scientific brief.
use super::*;

#[tool(
    name = "visualization_update_plan_label",
    description = "Save exactly one explicitly authorized label change in an existing scientific plan. Read the full plan first. Supply its planning_version, current expected_revision, canonical plan_field (nodes[0].label, edges[0].label or panels[0].title), exact complete expected_text and replacement text. Only this label changes; all relationships, sources, questions, layout and other fields remain server-side unchanged. Rejects stale versions, stale plan base, wrong old text, unsupported fields and no-op changes. This saves a new plan version, not a figure proposal; never generates, adopts or changes the canvas. Selection alone is not authorization to save. Subsequent changes must use the returned new planning_version. Preview a separately authorized sketch before adoption."
)]
pub struct PlanLabelInput {
    pub project_id: String,
    pub figure_id: String,
    pub expected_revision: i64,
    pub planning_version: i64,
    pub plan_field: String,
    pub expected_text: String,
    pub text: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LabelChange {
    pub plan_field: String,
    pub expected_text: String,
    pub text: String,
}
#[tool(
    name = "visualization_update_plan_labels",
    description = "Atomically save 1..20 explicitly authorized scientific-plan label changes in one planning version. Read the full plan first; pass its planning_version and current expected_revision. changes is a native JSON array of {plan_field,expected_text,text}; fields must be distinct existing nodes[i].label, edges[i].label or panels[i].title with canonical indices and exact complete old text. All entries must validate before anything is saved. A stale version, wrong old text, duplicate/invalid field, blank/overlong or unchanged replacement rejects the entire batch. Other plan fields, relationships, sources, questions and layout remain server-side unchanged. Selection is not save authorization. This changes only the saved plan, not canvas/candidates/adoption. Continue from the returned new version; never run dependent label batches in parallel."
)]
pub struct PlanLabelsInput {
    pub project_id: String,
    pub figure_id: String,
    pub expected_revision: i64,
    pub planning_version: i64,
    pub changes: Vec<LabelChange>,
}
struct PlanLabelTool {
    path: PathBuf,
}
struct PlanLabelsTool {
    path: PathBuf,
}
fn index(field: &str, prefix: &str, suffix: &str) -> Option<usize> {
    let value = field.strip_prefix(prefix)?.strip_suffix(suffix)?;
    let index = value.parse::<usize>().ok()?;
    (value == index.to_string()).then_some(index)
}
fn update(
    path: &std::path::Path,
    project: &str,
    figure: &str,
    revision: i64,
    version: i64,
    changes: &[LabelChange],
) -> Result<Value, ToolError> {
    if changes.is_empty() || changes.len() > 20 {
        return Err(invalid("changes must contain 1..20 label changes"));
    }
    let mut seen = std::collections::HashSet::new();
    for change in changes {
        if !seen.insert(&change.plan_field) {
            return Err(invalid("duplicate plan_field; nothing was saved"));
        }
        if change.text.trim().is_empty() || change.text.chars().count() > 120 {
            return Err(invalid(
                "text must contain 1..120 characters and cannot be blank",
            ));
        }
    }
    let mut store = Store::open(path).map_err(failure)?;
    let head = store.get(project, figure).map_err(failure)?;
    let plan = store
        .planning(project, figure)
        .map_err(failure)?
        .ok_or_else(|| invalid("read_plan must return an existing saved plan"))?;
    if head.revision != revision || plan.version != version || plan.based_on_revision != revision {
        return Err(invalid(
            "plan or figure version changed; read_plan and read again before editing",
        ));
    }
    let mut brief = plan.document;
    for change in changes {
        let label=if let Some(i)=index(&change.plan_field,"nodes[","].label") {
            brief.nodes.get_mut(i).map(|node|&mut node.label)
        } else if let Some(i)=index(&change.plan_field,"edges[","].label") {
            brief.edges.get_mut(i).map(|edge|&mut edge.label)
        } else if let Some(i)=index(&change.plan_field,"panels[","].title") {
            brief.panels.get_mut(i).map(|panel|&mut panel.title)
        } else {None}.ok_or_else(||invalid("plan_field must identify an existing nodes[i].label, edges[i].label or panels[i].title using a canonical index; nothing was saved"))?;
        if *label != change.expected_text {
            return Err(invalid(
                "expected_text does not match the complete saved label; nothing was saved",
            ));
        }
        if *label == change.text {
            return Err(invalid("label is unchanged; nothing was saved"));
        }
        *label = change.text.clone();
    }
    let saved = store
        .save_planning(project, figure, revision, version, &brief)
        .map_err(failure)?;
    let mut result = json!({"version":saved.version,"based_on_revision":saved.based_on_revision,"status":saved.status,"changed_labels":changes,"layout_review":saved.layout_review,"scope":"saved_plan_label_batch"});
    follow_up::attach(&mut result, Some(project), Some(figure));
    Ok(result)
}
#[async_trait]
impl ToolFunction for PlanLabelsTool {
    type Input = PlanLabelsInput;
    async fn run(&self, input: PlanLabelsInput) -> Result<ToolResult, ToolError> {
        Ok(ToolResult::success_json(update(
            &self.path,
            &input.project_id,
            &input.figure_id,
            input.expected_revision,
            input.planning_version,
            &input.changes,
        )?))
    }
}
#[async_trait]
impl ToolFunction for PlanLabelTool {
    type Input = PlanLabelInput;
    async fn run(&self, input: PlanLabelInput) -> Result<ToolResult, ToolError> {
        let change = LabelChange {
            plan_field: input.plan_field,
            expected_text: input.expected_text,
            text: input.text,
        };
        let mut result = update(
            &self.path,
            &input.project_id,
            &input.figure_id,
            input.expected_revision,
            input.planning_version,
            std::slice::from_ref(&change),
        )?;
        result["scope"] = json!("saved_plan_single_label");
        result["changed_plan_field"] = json!(change.plan_field);
        result["previous_text"] = json!(change.expected_text);
        result["text"] = json!(change.text);
        result
            .as_object_mut()
            .expect("result object")
            .remove("changed_labels");
        Ok(ToolResult::success_json(result))
    }
}
pub(crate) fn registration(path: PathBuf) -> ToolRegistration {
    ToolRegistration::from(PlanLabelTool { path })
}
pub(crate) fn batch_registration(path: PathBuf) -> ToolRegistration {
    ToolRegistration::from(PlanLabelsTool { path })
}
