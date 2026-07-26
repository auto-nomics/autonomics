use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;
use data_engine::runtime::DataEngineClient;

use crate::ExecError;

#[tool(
    name = "add_node",
    description = "Add a node to the DAG by its registered kind and a JSON spec. \
                  \
                  WORKFLOW — discover then create: \
                  1. Use `list_node_factories` to see available node kinds and \
                     their short descriptions. \
                  2. Use `get_node_spec` to fetch the full JSON Schema for the \
                     chosen kind — this tells you exactly which fields the `spec` \
                     object requires. \
                  3. Pass the node `id`, `kind`, and a `spec` object conforming \
                     to the schema into this tool. \
                  \
                  Each node kind expects different spec fields — always call \
                  `get_node_spec` first. Common examples: \
                  - \"sql\":            {\"sql_query\": \"SELECT * FROM port_0\"} \
                  - \"source_file\":    {\"path\": \"/data/sample.vcf.gz\", \"format\": null} \
                  - \"source_iceberg\": {\"ident\": \"gwas.study\"} \
                  - \"sink_file\":      {\"path\": \"/out/result.csv\", \"format\": \"csv\", \"mode\": \"overwrite\"} \
                  - \"sink_iceberg\":   {\"ident\": \"gwas.study\", \"mode\": \"overwrite\"} \
                  - \"linear_regression\": {\"x_columns\": [\"x1\"], \"y_column\": \"y\", \"intercept\": true} \
                  - \"ldsc\":           {\"n_blocks\": 200, \"intercept\": null} \
                  - \"mock\":           {} \
                  \
                  WARNING — DO NOT combine `add_node` and `add_edge` in the same \
                  response turn. An edge requires both endpoints to already exist, \
                  so calling `add_node` and `add_edge` together in one turn causes \
                  a race: the edge may be applied before the node lands, and the \
                  DAG ends up with a dangling edge or a failed connection. Create \
                  ALL nodes first, wait for their results, then add edges in a \
                  SEPARATE turn. Multiple `add_node` calls within one turn are fine."
)]
pub struct AddNodeInput {
    /// Unique identifier for this node in the DAG.
    pub id: String,
    /// The node kind — one of the kinds returned by `list_node_factories`
    /// (e.g. "sql", "source_file", "source_iceberg", "sink_file", "sink_iceberg", "linear_regression", "ldsc", "mock").
    pub kind: String,
    /// JSON object conforming to the node's JSON Schema. Can include extra
    /// fields — the node factory ignores unknown keys.
    pub spec: serde_json::Value,
}

pub struct AddNodeTool {
    client: Arc<DataEngineClient>,
}

impl AddNodeTool {
    pub fn new(client: Arc<DataEngineClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ToolFunction for AddNodeTool {
    type Input = AddNodeInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        self.client
            .add_node(input.id, input.kind, input.spec)
            .await
            .map_err(ExecError::from)?;

        Ok(ToolResult::success("node added to DAG"))
    }
}

#[cfg(test)]
mod tests {
    use agentik_core::tools::ToolFunction;
    use agentik_sdk::types::ToolInput;

    /// Normal round-trip: JSON input → `AddNodeInput`.  Exercises the exact
    /// path the framework uses when an LLM returns a tool_use payload.
    #[tokio::test]
    async fn test_spec_parse_roundtrip() {
        // Simulate an LLM tool_use input payload.
        let raw_input = serde_json::json!({
            "id": "my_sql",
            "kind": "sql",
            "spec": { "sql_query": "SELECT * FROM port_0" }
        });

        // Deserialize into AddNodeInput (same path as ToolFunction::execute).
        let typed: super::AddNodeInput = serde_json::from_value(raw_input.clone()).unwrap();

        assert_eq!(typed.id, "my_sql");
        assert_eq!(typed.kind, "sql");
        assert_eq!(typed.spec["sql_query"], "SELECT * FROM port_0");

        // Validate the schema also accepts the input.
        let def = super::AddNodeInput::definition();
        def.validate_input(&raw_input).unwrap();
    }

    /// Spec as a JSON string — simulates what happens when the LLM doesn't
    /// know `spec` must be an object (the generated schema lacks `type: "object"`)
    /// and serializes it as a string instead.
    ///
    /// serde happily accepts this at the tool boundary (because `Value` accepts
    /// anything), but the downstream node factory
    /// `serde_json::from_value::<SqlNodeSpec>` rejects it with "invalid type:
    /// string, expected struct".
    #[tokio::test]
    async fn test_spec_parse_as_string_rejected_by_factory() {
        let raw_input = serde_json::json!({
            "id": "broken",
            "kind": "sql",
            "spec": "{\"sql_query\": \"SELECT 1\"}"
        });

        // Tool-level deserialization succeeds — `serde_json::Value` is permissive.
        let typed: super::AddNodeInput = serde_json::from_value(raw_input).unwrap();
        assert!(typed.spec.is_string(), "spec should be a string");

        // But the downstream factory would fail: Value::String ≠ expected struct.
        let factory_err =
            serde_json::from_value::<data_engine::nodes::sql_node::SqlNodeSpec>(typed.spec);
        assert!(
            factory_err.is_err(),
            "a string-valued spec must be rejected by the node factory: {factory_err:?}"
        );
    }

    /// Verify the generated schema for `spec` contains `type: "object"`.
    /// schemars emits an unconstrained schema for `serde_json::Value` (no type),
    /// but `tool_definition_from_schema` post-processes every property and
    /// injects `type: "object"` so the LLM knows to serialise it as a JSON
    /// object rather than a string.
    #[test]
    fn test_spec_schema_has_object_type() {
        let def = super::AddNodeInput::definition();
        let spec_schema = def
            .input_schema
            .properties
            .get("spec")
            .expect("spec property must be present");
        assert_eq!(
            spec_schema.get("type").and_then(|v| v.as_str()),
            Some("object"),
            "spec schema must have type 'object', got: {spec_schema}"
        );
    }
}
