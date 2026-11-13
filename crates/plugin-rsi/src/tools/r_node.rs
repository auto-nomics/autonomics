use super::{
    PluginToolState,
    helpers::{resolve_target, tool_error},
};
use agentik_core::tools::{ToolError, ToolFunction, ToolResult};
use agentik_proc::tool;
use async_trait::async_trait;
#[tool(
    name = "plugin_r_node_create",
    description = "Wrap an existing plugin R script as a file-analysis node with one labeled input and results.tsv/warnings.txt outputs. Defaults: 1 CPU, 1 GB, 120 seconds. Script reads AUTONOMICS_INPUT0 and writes AUTONOMICS_OUTPUT0/1. Existing algorithm is preserved; install explicitly afterwards."
)]
pub(super) struct PluginRNodeCreateInput {
    plugin_path: String,
    kind: String,
    desc: String,
    /// Relative existing R script in this plugin, e.g. scripts/analyze.R.
    script_file: String,
}
pub(super) struct PluginRNodeCreateTool {
    pub(super) state: PluginToolState,
}
#[async_trait]
impl ToolFunction for PluginRNodeCreateTool {
    type Input = PluginRNodeCreateInput;
    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let target = resolve_target(&self.state, &input.plugin_path)
            .await
            .map_err(tool_error)?;
        target
            .workspace
            .read_text(&input.script_file)
            .map_err(tool_error)?;
        let node = serde_json::from_value(serde_json::json!({"kind":input.kind,"desc":input.desc,
       "doc":"File-analysis R adapter. Reads AUTONOMICS_INPUT0 and writes AUTONOMICS_OUTPUT0 (TSV), AUTONOMICS_OUTPUT1 (warnings). Inspect parameters and documented limitations before reuse.",
       "timeout_secs":120,"ports":{"inputs":[{"type":"file","label":"input"}],"outputs":[{"path":"results.tsv","format":"tsv"},{"path":"warnings.txt","format":"txt"}]},
       "command":{"interpreter":"Rscript","script_file":input.script_file},"resources":{"cpus":1.0,"memory":"1g","read_only_rootfs":true}})).map_err(tool_error)?;
        super::node_upsert::PluginNodeUpsertTool {
            state: self.state.clone(),
        }
        .run(super::node_upsert::PluginNodeUpsertInput {
            plugin_path: input.plugin_path,
            node,
        })
        .await
    }
}
