mod branch_from_snapshot_tool;
mod checkout_dag_tool;
mod dag_export_run_tool;
mod dag_history_log_tool;
mod dag_runs_log_tool;
mod dag_shell_tool;
mod diff_snapshots_tool;
mod get_node_doc_tool;
mod get_node_ports_tool;
mod get_node_spec_tool;
mod get_output_tool;
mod inspect_node_tool;
mod list_dag_refs_tool;
mod list_node_factories_tool;
mod new_dag_ref_tool;
mod run_dag_tool;
mod show_snapshot_tool;
mod switch_dag_ref_tool;
mod view_dag_tool;

use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolRegistration};
use data_engine::runtime::DataEngineClient;

/// Shared tool execution error — replaces `anyhow` with typed variants
/// so each error source is identifiable without string matching.
#[derive(Debug)]
pub(crate) enum ExecError {
    /// A parse/validation issue in tool input (e.g. unknown format string).
    Format(String),
    /// An error from the data engine actor.
    Client(data_engine::runtime::error::ClientError),
}

impl From<String> for ExecError {
    fn from(msg: String) -> Self {
        Self::Format(msg)
    }
}

impl From<data_engine::runtime::error::ClientError> for ExecError {
    fn from(e: data_engine::runtime::error::ClientError) -> Self {
        Self::Client(e)
    }
}

impl std::fmt::Display for ExecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Format(msg) => write!(f, "{msg}"),
            Self::Client(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ExecError {}

// Safety: ExecError only contains String and ClientError, both of which
// are Send + Sync.
unsafe impl Send for ExecError {}
unsafe impl Sync for ExecError {}

impl From<ExecError> for ToolError {
    fn from(e: ExecError) -> Self {
        ToolError::ExecutionFailed {
            source: Box::new(e),
        }
    }
}

/// Build the default set of data-engine DAG tools.
///
/// Each tool sends commands to the [`DataEngineClient`] actor and awaits
/// replies via oneshot channels. Graph mutation is centralized in the
/// transactional `dag_shell` tool.
pub fn registrations(client: Arc<DataEngineClient>) -> Vec<ToolRegistration> {
    vec![
        // ── node discovery ────────────────────────────────────────────────
        ToolRegistration::from(list_node_factories_tool::ListNodeFactoriesTool::new(
            client.clone(),
        )),
        ToolRegistration::from(get_node_spec_tool::GetNodeSpecTool::new(client.clone())),
        ToolRegistration::from(get_node_ports_tool::GetNodePortsTool::new(client.clone())),
        ToolRegistration::from(get_node_doc_tool::GetNodeDocTool::new(client.clone())),
        ToolRegistration::from(inspect_node_tool::InspectNodeTool::new(client.clone())),
        // ── DAG building ──────────────────────────────────────────────────
        ToolRegistration::from(dag_shell_tool::DagShellTool::new(client.clone())),
        // ── DAG execution & inspection ────────────────────────────────────
        ToolRegistration::from(run_dag_tool::RunDagTool::new(client.clone())),
        ToolRegistration::from(get_output_tool::GetOutputTool::new(client.clone())),
        ToolRegistration::from(view_dag_tool::ViewDagTool::new(client.clone())),
        // ── history / ref management ──────────────────────────────────────
        ToolRegistration::from(new_dag_ref_tool::NewDagRefTool::new(client.clone())),
        ToolRegistration::from(switch_dag_ref_tool::SwitchDagRefTool::new(client.clone())),
        ToolRegistration::from(list_dag_refs_tool::ListDagRefsTool::new(client.clone())),
        ToolRegistration::from(dag_history_log_tool::DagHistoryLogTool::new(client.clone())),
        ToolRegistration::from(dag_runs_log_tool::DagRunsLogTool::new(client.clone())),
        ToolRegistration::from(dag_export_run_tool::DagExportRunTool::new(client.clone())),
        ToolRegistration::from(checkout_dag_tool::CheckoutDagTool::new(client.clone())),
        ToolRegistration::from(show_snapshot_tool::ShowSnapshotTool::new(client.clone())),
        ToolRegistration::from(diff_snapshots_tool::DiffSnapshotsTool::new(client.clone())),
        ToolRegistration::from(branch_from_snapshot_tool::BranchFromSnapshotTool::new(
            client,
        )),
    ]
}

#[cfg(test)]
mod tests {
    use agentik_proc::tool;
    use agentik_sdk::types::ToolInput;
    use data_engine::data_engine::DataEngine;

    /// Regression: a `serde_json::Value` field must NOT be advertised as a
    /// `string` in the generated tool schema. The original hand-rolled type
    /// mapping fell back to `"string"` for unknown types, which made callers
    /// serialize the spec to a string and node factories reject it with
    /// "invalid type: string, expected ...". Under schemars, `Value` produces
    /// an unconstrained schema (no restrictive `type`).
    #[tool(name = "test_value_field", description = "test harness tool")]
    pub struct TestValueInput {
        /// arbitrary json
        pub spec: serde_json::Value,
    }

    #[tokio::test]
    async fn graph_mutation_tools_are_replaced_by_dag_shell() {
        let engine = DataEngine::builder().build();
        let (client, _handle) = data_engine::runtime::spawn_with_engine(engine);
        let names = super::registrations(std::sync::Arc::new(client))
            .into_iter()
            .map(|tool| tool.definition.name)
            .collect::<Vec<_>>();

        assert!(names.iter().any(|name| name == "dag_shell"));
        for retired in [
            "add_node",
            "update_node",
            "add_edge",
            "add_logical_graph",
            "remove_edge",
            "remove_node",
        ] {
            assert!(!names.iter().any(|name| name == retired));
        }
    }

    #[test]
    fn serde_json_value_field_is_not_typed_as_string() {
        let def = TestValueInput::definition();
        let spec_schema = def
            .input_schema
            .properties
            .get("spec")
            .expect("spec property present");
        assert_ne!(
            spec_schema.get("type").and_then(|v| v.as_str()),
            Some("string"),
            "serde_json::Value must not be advertised as 'string' (regression), got: {spec_schema}"
        );
    }

    /// The `#[tool]` macro must still derive accurate JSON Schema types for
    /// primitive fields and surface `#[desc]` as the property description.
    #[tool(name = "test_typed_fields", description = "test harness tool")]
    pub struct TestTypedInput {
        #[desc = "the name"]
        pub name: String,
        #[desc = "the count"]
        pub count: Option<i64>,
        pub tags: Vec<String>,
        pub flag: bool,
    }

    #[test]
    fn typed_fields_get_correct_schema_and_descriptions() {
        let def = TestTypedInput::definition();
        let props = &def.input_schema.properties;

        let name = props.get("name").expect("name property");
        assert_eq!(name.get("type").and_then(|v| v.as_str()), Some("string"));
        assert_eq!(
            name.get("description").and_then(|v| v.as_str()),
            Some("the name")
        );

        let count = props.get("count").expect("count property");
        // `count` is optional -> not required.
        assert!(!def.input_schema.required.contains(&"count".to_string()));
        assert_eq!(
            count.get("description").and_then(|v| v.as_str()),
            Some("the count")
        );

        let tags = props.get("tags").expect("tags property");
        assert_eq!(tags.get("type").and_then(|v| v.as_str()), Some("array"));

        let flag = props.get("flag").expect("flag property");
        assert_eq!(flag.get("type").and_then(|v| v.as_str()), Some("boolean"));

        // `name` is required (non-Option, no default).
        assert!(def.input_schema.required.contains(&"name".to_string()));
    }
}
