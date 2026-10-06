use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;
use data_engine::dag::{ChannelOperator, LogicalGraph};
use schemars::schema_for;
use serde_json::json;

#[tool(
    name = "dag_logical_schema",
    description = "Return the complete LogicalGraph JSON Schema and executable examples \
                  for Registry nodes, Channel operators, Gather, Once, ForEach, \
                  DynamicForEach, and dynamic fan-out collection. Use this before \
                  calling dag_shell's add_logical_graph; Registry definitions still \
                  need their node-specific schema from get_node_spec."
)]
pub struct DagLogicalSchemaInput {}

pub struct DagLogicalSchemaTool {}

impl DagLogicalSchemaTool {
    pub fn new() -> Self {
        Self {}
    }
}

impl Default for DagLogicalSchemaTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolFunction for DagLogicalSchemaTool {
    type Input = DagLogicalSchemaInput;

    async fn run(&self, _input: Self::Input) -> Result<ToolResult, ToolError> {
        let result = json!({
            "logical_graph_schema": schema_for!(LogicalGraph),
            "channel_operator_schema": schema_for!(ChannelOperator),
            "execution_strategies": {
                "Once": "one physical job",
                "ForEach": "one job per declared item",
                "DynamicForEach": "one job per item received from an upstream Channel",
                "Gather": "collapse expanded upstream jobs into one DataFrame or FileSet"
            },
            "preflight_rules": [
                "Every Registry kind must be registered in the live engine registry.",
                "Every concrete Registry spec must build successfully.",
                "DynamicForEach requires exactly one upstream Channel edge.",
                "Channel operators other than collect require Channel inputs.",
                "channel.collect accepts Channel, File, and FileSet but rejects DataFrame.",
                "Gather requires homogeneous DataFrame, File, or FileSet inputs."
            ],
            "examples": {
                "dynamic_for_each": {
                    "path_note": "Replace /absolute/path/input-a.tsv with an existing file before running.",
                    "dag_shell_script": r#"
add_logical_graph(#{
    nodes: [
        #{id: "items", definition: #{Channel: #{operator: "of_items", items: ["/absolute/path/input-a.tsv"]}}, strategy: "Once"},
        #{id: "jobs", definition: #{Registry: #{kind: "file_reference", spec: #{path: "{{item}}", hash_content: false}}}, strategy: #{DynamicForEach: #{axis: "item"}}},
        #{id: "collected", definition: #{Channel: #{operator: "collect"}}, strategy: "Once"}
    ],
    edges: [
        #{from: "items", from_port: 0, to: "jobs", to_port: 0},
        #{from: "jobs", from_port: 0, to: "collected", to_port: 0}
    ]
});
commit();
"#
                },
                "static_for_each": {
                    "path_note": "Replace /absolute/path/input-a.tsv and /absolute/path/input-b.tsv with existing files before running.",
                    "dag_shell_script": r#"
add_logical_graph(#{
    nodes: [
        #{id: "jobs", definition: #{Registry: #{kind: "file_reference", spec: #{path: "{{item}}", hash_content: false}}}, strategy: #{ForEach: #{axis: "sample", items: ["/absolute/path/input-a.tsv", "/absolute/path/input-b.tsv"]}}},
        #{id: "gathered", definition: "Gather", strategy: "Gather"}
    ],
    edges: [
        #{from: "jobs", from_port: 0, to: "gathered", to_port: 0}
    ]
});
commit();
"#
                }
            }
        });
        Ok(ToolResult::success_json(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_shell_tool::DagShellTool;
    use agentik_sdk::types::ToolResultContent;
    use std::sync::Arc;

    #[tokio::test]
    async fn examples_are_executable() {
        let schema = DagLogicalSchemaTool::new()
            .run(DagLogicalSchemaInput {})
            .await
            .unwrap();
        let ToolResultContent::Json(schema) = schema.content else {
            panic!("schema result should be JSON");
        };

        for example in ["dynamic_for_each", "static_for_each"] {
            let script = schema["examples"][example]["dag_shell_script"]
                .as_str()
                .unwrap_or_else(|| panic!("{example} example should contain a script"))
                .to_string();
            let input_a = tempfile::NamedTempFile::new().unwrap();
            std::fs::write(input_a.path(), b"logical graph example a").unwrap();
            let input_b = tempfile::NamedTempFile::new().unwrap();
            std::fs::write(input_b.path(), b"logical graph example b").unwrap();
            let script = script.replace(
                "/absolute/path/input-a.tsv",
                &input_a.path().to_string_lossy(),
            );
            let script = script.replace(
                "/absolute/path/input-b.tsv",
                &input_b.path().to_string_lossy(),
            );
            let engine = data_engine::data_engine::DataEngine::builder().build();
            let (client, _handle) = data_engine::runtime::spawn_with_engine(engine);
            let shell = DagShellTool::new(Arc::new(client.clone()));
            let built = shell
                .run(crate::dag_shell_tool::DagShellInput {
                    script,
                    dry_run: Some(false),
                    timeout_ms: Some(1000),
                })
                .await
                .unwrap();
            let ToolResultContent::Json(report) = built.content else {
                panic!("{example} build result should be JSON");
            };
            assert_eq!(report["ok"], true, "{example}: {report}");

            let run = client.run_dag(Some(example.into())).await.unwrap();
            assert!(run.ok, "{example}: {run:?}");
        }
    }
}
