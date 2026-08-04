use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;
use data_engine::codegen::CodegenTarget;
use data_engine::runtime::DataEngineClient;
use serde_json::json;

use crate::ExecError;

/// Reverse-compile the current DAG into equivalent R or Python source code
/// that calls the original reference packages (TwoSampleMR, LDSC, lava, ...).
///
/// The generated script is fully runnable standalone — use it to audit the
/// analysis, cross-validate results, or produce publication-ready methodology
/// code.
#[tool(
    name = "compile_dag",
    description = "Reverse-compile the current DAG into equivalent R or Python \
                  source code that calls the original reference packages \
                  (TwoSampleMR, LDSC, lava, survival, etc.). The generated \
                  script is fully runnable standalone. Use this to audit the \
                  analysis, cross-validate results against the Rust engine, \
                  or produce publication-ready methodology code. Nodes that \
                  do not yet have codegen support are emitted as comments."
)]
pub struct CompileDagInput {
    /// Target language: `"r"` or `"python"`.
    pub language: String,
}

pub struct CompileDagTool {
    client: Arc<DataEngineClient>,
}

impl CompileDagTool {
    pub fn new(client: Arc<DataEngineClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ToolFunction for CompileDagTool {
    type Input = CompileDagInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let target = match input.language.to_lowercase().as_str() {
            "r" => CodegenTarget::R,
            "python" | "py" => CodegenTarget::Python,
            other => {
                return Err(ToolError::from(ExecError::from(format!(
                    "unsupported language '{other}': expected 'r' or 'python'"
                ))));
            }
        };

        let script = self
            .client
            .compile_dag(target)
            .await
            .map_err(ExecError::from)?;

        let result = json!({
            "source": script.source,
            "target": format!("{}", script.target),
            "packages": script.packages,
            "skipped_nodes": script.skipped_nodes,
            "warnings": script.warnings,
        });

        Ok(ToolResult::success(
            serde_json::to_string_pretty(&result).unwrap(),
        ))
    }
}
