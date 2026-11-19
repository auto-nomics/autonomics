//! MCP (Model Context Protocol) server surface for the gateway daemon.
//!
//! Exposes a curated, agent-independent tool face over Streamable HTTP at
//! `POST/GET /mcp`. External MCP clients (Claude Code, Cursor, ...) reach
//! the daemon's data-engine capabilities — node discovery, DAG editing,
//! flow execution — through the same `ToolRegistration` implementations
//! the in-daemon agents use, so schemas, validation, timeouts and progress
//! semantics stay identical on both faces.
//!
//! Deliberate scope decisions:
//!
//! * **Dedicated engine agent** ([`MCP_AGENT_ID`]): the MCP face gets its
//!   own `DataEngineClient` actor and DAG history ref, so an MCP-driven
//!   flow never aliases a live agent's snapshot lineage.
//! * **Whitelist, not passthrough**: only `data_engine_tools` is exposed.
//!   Agent-private tools (message injection, context ops) and VFS writes
//!   are intentionally absent; widening the surface is a deliberate
//!   decision recorded here, not an accidental drift.
//! * **No Toolset machinery**: `tools/call` invokes
//!   [`DynToolFunction::execute_with_context`] directly. The Toolset's
//!   agent-session coupling (ToolUse ids, task store, BgTask
//!   notifications) doesn't apply to a headless MCP client; the tool's
//!   own `timeout_seconds` and the MCP cancellation token are enforced
//!   here instead.
//! * **Results as text**: tool output is returned as MCP text content
//!   (JSON pretty-printed). Large artifacts stay in the VFS and are
//!   referenced by URI, never inlined into JSON-RPC responses.

use std::sync::Arc;
use std::time::Duration;

use agentik_core::tools::{ProgressLog, ToolContext, ToolRegistry, ToolRegistration};
use agentik_types::tools::{ToolInputSchema, ToolResult, ToolResultBlock, ToolResultContent};
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ContentBlock, Implementation, ListToolsResult,
    ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::ServerHandler;
use runtime::SharedInfra;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

/// The data-engine agent identity used for MCP-driven work. Also the DAG
/// history ref, giving the MCP face an isolated snapshot lineage.
pub const MCP_AGENT_ID: &str = "mcp";

#[derive(Clone)]
pub struct McpTools {
    registry: Arc<ToolRegistry>,
}

impl McpTools {
    /// Build the MCP tool face from daemon shared infrastructure.
    pub fn from_infra(infra: &SharedInfra) -> Self {
        let client = Arc::new(infra.engine_manager.client_for_agent(MCP_AGENT_ID));
        let mut registry = ToolRegistry::new();
        registry
            .register_all(data_engine_tools::registrations(client))
            .expect("data-engine tool names are unique");
        Self {
            registry: Arc::new(registry),
        }
    }

    /// Build from an explicit registration list (tests / future widening).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn from_registrations(registrations: Vec<ToolRegistration>) -> Self {
        let mut registry = ToolRegistry::new();
        registry
            .register_all(registrations)
            .expect("test tool names are unique");
        Self {
            registry: Arc::new(registry),
        }
    }

    fn definitions_as_mcp_tools(&self) -> Vec<Tool> {
        self.registry
            .definitions()
            .iter()
            .map(|def| {
                Tool::new(
                    def.name.clone(),
                    def.description.clone(),
                    tool_input_schema_to_json_object(&def.input_schema),
                )
            })
            .collect()
    }

    /// Core `tools/call` path, transport-independent.
    ///
    /// `cancel` is the MCP request's cancellation token (fired on
    /// `notifications/cancelled`).
    async fn call(&self, name: &str, arguments: Value, cancel: CancellationToken) -> CallToolResult {
        let Some(registration) = self.registry.get(name) else {
            let available: Vec<String> = self.registry.definitions().into_iter().map(|d| d.name).collect();
            // A wrong tool name is a caller-visible failure, not a protocol
            // error: the client (and its user) should see the valid names.
            return error_result(format!(
                "Unknown tool '{name}'. Available tools: {}",
                available.join(", ")
            ));
        };

        if let Err(error) = registration.implementation.validate_input(&arguments) {
            return error_result(format!("validation failed: {error}"));
        }

        // Same context shape the toolset wires per invocation: a
        // live-output buffer the tool may stream into (not forwarded to
        // the MCP client yet — see the progress-notification follow-up)
        // and a metadata slot.
        let ctx = ToolContext {
            output: Some(Arc::new(std::sync::Mutex::new(ProgressLog::new()))),
            metadata: Arc::new(std::sync::Mutex::new(Value::Null)),
            session: None,
        };

        let implementation = registration.implementation.clone();
        // Enforce the tool's own timeout, mirroring toolset semantics, and
        // let an MCP `notifications/cancelled` preempt it.
        let timeout = Duration::from_secs(implementation.timeout_seconds());
        let exec_fut = implementation.execute_with_context(arguments, &ctx);
        let outcome = tokio::select! {
            _ = cancel.cancelled() => {
                return error_result(format!("tool '{name}' cancelled by client"));
            }
            result = tokio::time::timeout(timeout, exec_fut) => match result {
                Ok(result) => result,
                Err(_) => {
                    return error_result(format!(
                        "tool '{name}' timed out after {}s",
                        timeout.as_secs()
                    ));
                }
            },
        };

        match outcome {
            Ok(tool_result) => to_call_tool_result(tool_result),
            Err(error) => error_result(format!("tool '{name}' failed: {error}")),
        }
    }
}

/// Convert a tool's input schema into the JSON object MCP expects.
///
/// `ToolInputSchema` serializes to a JSON object (`type`, `properties`,
/// `required`, plus flattened extras); if serialization ever produces a
/// non-object we fall back to an empty object rather than failing the
/// whole `tools/list`.
fn tool_input_schema_to_json_object(schema: &ToolInputSchema) -> serde_json::Map<String, Value> {
    match serde_json::to_value(schema) {
        Ok(Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    }
}

/// Render a tool result's content as MCP content blocks.
fn tool_result_content_to_blocks(content: &ToolResultContent) -> Vec<ContentBlock> {
    match content {
        ToolResultContent::Text(text) => vec![ContentBlock::text(text.clone())],
        ToolResultContent::Json(value) => {
            vec![ContentBlock::text(serde_json::to_string_pretty(value).unwrap_or_default())]
        }
        ToolResultContent::Blocks(blocks) => blocks
            .iter()
            .filter_map(|block| match block {
                ToolResultBlock::Text { text } => Some(ContentBlock::text(text.clone())),
                // Image blocks are not surfaced on the MCP face in this
                // iteration; data-engine tools emit text/JSON only.
                ToolResultBlock::Image { .. } => None,
            })
            .collect(),
    }
}

fn error_result(message: String) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(message)])
}

fn to_call_tool_result(tool_result: ToolResult) -> CallToolResult {
    let content = tool_result_content_to_blocks(&tool_result.content);
    if tool_result.is_error.unwrap_or(false) {
        let mut result = CallToolResult::success(content);
        result.is_error = Some(true);
        result
    } else {
        CallToolResult::success(content)
    }
}

impl ServerHandler for McpTools {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("autonomics", crate::GATEWAY_VERSION))
            .with_instructions(
                "Autonomics data-engine face: discover DAG node factories, \
                 inspect specs/ports/docs, build and run flows. Use \
                 `list_node_factories` / `get_node_spec` to explore, \
                 `set_flow` / `view_flow` to edit, `run_flow` to execute. \
                 Outputs live in the VFS and are referenced by URI.",
            )
    }

    async fn list_tools(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, rmcp::ErrorData> {
        Ok(ListToolsResult {
            result_type: None,
            meta: None,
            next_cursor: None,
            ttl_ms: None,
            cache_scope: None,
            tools: self.definitions_as_mcp_tools(),
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<rmcp::model::CallToolResponse, rmcp::ErrorData> {
        let arguments = Value::Object(request.arguments.unwrap_or_default());
        let result = self
            .call(request.name.as_ref(), arguments, context.ct.clone())
            .await;
        Ok(result.into())
    }
}

/// Build the `/mcp` router: the rmcp Streamable HTTP service behind the
/// gateway's bearer-token auth middleware.
///
/// Auth reuses `bearer_or_session` — MCP clients authenticate with the
/// same daemon token frontends use. The service itself keeps rmcp's
/// loopback-only default `Host` validation (the daemon binds loopback).
pub fn router(infra: &SharedInfra, auth: Arc<super::auth::AuthContext>) -> axum::Router {
    use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
    use rmcp::transport::streamable_http_server::StreamableHttpService;

    let handler = Arc::new(McpTools::from_infra(infra));
    let session_manager = Arc::new(LocalSessionManager::default());
    let service = StreamableHttpService::new(
        move || Ok((*handler).clone()),
        session_manager,
        Default::default(),
    );

    axum::Router::new()
        .route_service("/mcp", service)
        .layer(axum::middleware::from_fn_with_state(
            auth,
            super::auth::bearer_or_session,
        ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentik_core::tools::{ToolError, ToolFunction};
    use agentik_types::tools::{ToolDefinition, ToolInput};
    use async_trait::async_trait;
    use serde_json::json;

    struct EchoTool;

    #[derive(serde::Deserialize, serde::Serialize)]
    struct EchoInput {
        #[allow(dead_code)]
        message: String,
    }

    impl ToolInput for EchoInput {
        fn definition() -> ToolDefinition {
            ToolDefinition {
                name: "echo".to_string(),
                description: "Echo the message back.".to_string(),
                input_schema: ToolInputSchema::from_root_schema(json!({
                    "type": "object",
                    "properties": {
                        "message": { "type": "string" }
                    },
                    "required": ["message"]
                })),
            }
        }
    }

    #[async_trait]
    impl ToolFunction for EchoTool {
        type Input = EchoInput;

        async fn run(&self, input: EchoInput) -> Result<ToolResult, ToolError> {
            Ok(ToolResult::success(format!("echo: {}", input.message)))
        }
    }

    fn bridge() -> McpTools {
        McpTools::from_registrations(vec![ToolRegistration::from(EchoTool)])
    }

    fn text_of(result: &CallToolResult) -> String {
        result
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.to_string())
            .unwrap_or_default()
    }

    #[test]
    fn get_info_advertises_tools_capability() {
        let info = bridge().get_info();
        assert!(info.capabilities.tools.is_some());
        assert_eq!(&*info.server_info.name, "autonomics");
        assert!(!info.instructions.unwrap_or_default().is_empty());
    }

    #[tokio::test]
    async fn list_tools_exposes_registered_definitions() {
        let tools = bridge().definitions_as_mcp_tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name.as_ref(), "echo");
        let required = tools[0]
            .input_schema
            .get("required")
            .and_then(|v| v.as_array())
            .map(|a| a.len());
        assert_eq!(required, Some(1));
    }

    #[tokio::test]
    async fn call_routes_to_implementation() {
        let result = bridge()
            .call(
                "echo",
                json!({"message": "hi"}),
                CancellationToken::new(),
            )
            .await;
        // rmcp's `success()` constructor sets `is_error: Some(false)`.
        assert!(!result.is_error.unwrap_or(false), "text was: {}", text_of(&result));
        assert_eq!(text_of(&result), "echo: hi");
    }

    #[tokio::test]
    async fn unknown_tool_is_a_caller_visible_error() {
        let result = bridge()
            .call("nope", Value::Null, CancellationToken::new())
            .await;
        assert_eq!(result.is_error, Some(true));
        let text = text_of(&result);
        assert!(text.contains("Available tools: echo"), "got: {text}");
    }

    #[tokio::test]
    async fn invalid_arguments_report_validation_failure() {
        let result = bridge()
            .call("echo", json!({}), CancellationToken::new())
            .await;
        assert_eq!(result.is_error, Some(true));
        // The default `validate_input` passes everything; the failure comes
        // from deserialization inside the tool's execute path.
        let text = text_of(&result);
        assert!(
            text.contains("failed") && text.contains("message"),
            "got: {text}"
        );
    }

    #[tokio::test]
    async fn cancellation_preempts_execution() {
        struct SlowTool;

        #[derive(serde::Deserialize, serde::Serialize)]
        struct SlowInput {}

        impl ToolInput for SlowInput {
            fn definition() -> ToolDefinition {
                ToolDefinition {
                    name: "slow".to_string(),
                    description: String::new(),
                    input_schema: ToolInputSchema::from_root_schema(
                        json!({"type": "object", "properties": {}}),
                    ),
                }
            }
        }

        #[async_trait]
        impl ToolFunction for SlowTool {
            type Input = SlowInput;

            async fn run(&self, _input: SlowInput) -> Result<ToolResult, ToolError> {
                tokio::time::sleep(Duration::from_secs(60)).await;
                Ok(ToolResult::success("done"))
            }
        }

        let bridge = McpTools::from_registrations(vec![ToolRegistration::from(SlowTool)]);
        let token = CancellationToken::new();
        token.cancel();
        let result = bridge.call("slow", json!({}), token).await;
        assert_eq!(result.is_error, Some(true));
        assert!(text_of(&result).contains("cancelled"));
    }

    #[test]
    fn json_results_render_as_pretty_text() {
        let blocks = tool_result_content_to_blocks(&ToolResultContent::Json(json!({"a": 1})));
        assert_eq!(blocks.len(), 1);
        let text = blocks[0].as_text().map(|t| t.text.to_string()).unwrap_or_default();
        assert!(text.contains("\"a\": 1"), "got: {text}");
    }
}
