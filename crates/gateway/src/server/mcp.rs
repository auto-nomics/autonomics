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
//! * **Progress forwarding**: when the client passes a `_meta.progressToken`,
//!   the tool's live-output buffer (`ToolContext::output`) is polled and
//!   forwarded as MCP `notifications/progress` — the same record stream
//!   the GUI renders for background tasks.
//! * **Read-only VFS resources**: `resources/list` paginates one VFS
//!   directory (root by default) and `resources/read` serves any
//!   `local://` (or legacy `vfs://`) path as bounded UTF-8 text, capped
//!   at [`RESOURCE_READ_MAX_BYTES`]. Binary or non-UTF-8 paths are not
//!   inlined into JSON-RPC responses. Large artifacts stay in the VFS
//!   and are referenced by URI, never inlined.

use std::sync::Arc;
use std::time::Duration;

use agentik_core::tools::{
    ProgressBuffer, ProgressLog, ProgressRecord, ToolContext, ToolRegistration, ToolRegistry,
};
use agentik_types::tools::{ToolInputSchema, ToolResult, ToolResultBlock, ToolResultContent};
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ContentBlock, Implementation, ListResourcesResult,
    ListToolsResult, PaginatedRequestParams, ProgressNotification, ProgressNotificationParam,
    ProgressToken, ReadResourceRequestParams, ReadResourceResult, RequestParamsMeta, Resource,
    ResourceContents, ResourceTemplate, ServerCapabilities, ServerConfig, ServerNotification, Tool,
};
use rmcp::service::{Peer, RequestContext, RoleServer};
use rmcp::{ErrorData as McpError, ServerHandler};
use runtime::SharedInfra;
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use vfs::OpendalFileStorage;
use vfs::browse::{self, VfsEntry};

/// The data-engine agent identity used for MCP-driven work. Also the DAG
/// history ref, giving the MCP face an isolated snapshot lineage.
pub const MCP_AGENT_ID: &str = "mcp";

/// How often the progress forwarder polls the tool's output buffer.
const PROGRESS_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Entries per `resources/list` page.
const RESOURCE_PAGE_LIMIT: usize = 200;

/// Upper bound for `resources/read` text bodies — matches the gateway's
/// `/api/v1/vfs/text` default read bound (1 MiB).
const RESOURCE_READ_MAX_BYTES: usize = 1024 * 1024;

#[derive(Clone)]
pub struct McpTools {
    registry: Arc<ToolRegistry>,
    /// Read-only VFS face for `resources/*`. `None` disables the
    /// resources capability entirely (unit-test construction).
    vfs: Option<Arc<OpendalFileStorage>>,
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
            vfs: Some(Arc::clone(&infra.file_storage)),
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
            vfs: None,
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
    /// `notifications/cancelled`); `progress` carries the client's
    /// progress token and the outbound peer when it requested progress
    /// notifications.
    async fn call(
        &self,
        name: &str,
        arguments: Value,
        cancel: CancellationToken,
        progress: Option<(ProgressToken, Peer<RoleServer>)>,
    ) -> CallToolResult {
        let Some(registration) = self.registry.get(name) else {
            let available: Vec<String> = self
                .registry
                .definitions()
                .into_iter()
                .map(|d| d.name)
                .collect();
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
        // live-output buffer the tool may stream into, forwarded to the
        // MCP client as notifications/progress when a token was supplied.
        let output: ProgressBuffer = Arc::new(std::sync::Mutex::new(ProgressLog::new()));
        let ctx = ToolContext {
            output: Some(Arc::clone(&output)),
            metadata: Arc::new(std::sync::Mutex::new(Value::Null)),
            session: None,
        };

        let forwarder = progress.map(|(token, peer)| {
            tokio::spawn(progress_forwarder(
                Arc::clone(&output),
                token,
                peer,
                cancel.clone(),
            ))
        });

        let implementation = registration.implementation.clone();
        // Enforce the tool's own timeout, mirroring toolset semantics, and
        // let an MCP `notifications/cancelled` preempt it.
        let timeout = Duration::from_secs(implementation.timeout_seconds());
        let exec_fut = implementation.execute_with_context(arguments, &ctx);
        let outcome = tokio::select! {
            _ = cancel.cancelled() => {
                if let Some(forwarder) = forwarder { forwarder.abort(); }
                return error_result(format!("tool '{name}' cancelled by client"));
            }
            result = tokio::time::timeout(timeout, exec_fut) => match result {
                Ok(result) => result,
                Err(_) => {
                    if let Some(forwarder) = forwarder { forwarder.abort(); }
                    return error_result(format!(
                        "tool '{name}' timed out after {}s",
                        timeout.as_secs()
                    ));
                }
            },
        };
        if let Some(forwarder) = forwarder {
            forwarder.abort();
        }

        match outcome {
            Ok(tool_result) => to_call_tool_result(tool_result),
            Err(error) => error_result(format!("tool '{name}' failed: {error}")),
        }
    }
}

// ───────────────────────── progress forwarding ─────────────────────────

/// Poll a tool's live-output buffer and forward new records to the MCP
/// client as `notifications/progress`.
///
/// One notification per poll cycle carries the newest record's message
/// and the cumulative record count as the progress value (no total — the
/// buffer is an append-only log, not a bounded job). The task is aborted
/// when the tool call resolves or is cancelled.
async fn progress_forwarder(
    buffer: ProgressBuffer,
    token: ProgressToken,
    peer: Peer<RoleServer>,
    cancel: CancellationToken,
) {
    let mut seen = 0usize;
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tokio::time::sleep(PROGRESS_POLL_INTERVAL) => {}
        }
        let (newest, total) = {
            let log = buffer.lock().expect("progress log poisoned");
            let snapshot = log.snapshot();
            let total = snapshot.len();
            let newest = snapshot.get(seen..).and_then(|fresh| fresh.last().cloned());
            (newest, total)
        };
        if total > seen {
            seen = total;
            if let Some(record) = newest {
                let mut param = ProgressNotificationParam::new(token.clone(), seen as f64);
                if let Some(message) = progress_message(&record) {
                    param = param.with_message(message);
                }
                let notification =
                    ServerNotification::ProgressNotification(ProgressNotification::new(param));
                if peer.send_notification(notification).await.is_err() {
                    // Client went away; stop forwarding.
                    return;
                }
            }
        }
    }
}

/// Human-readable summary of a progress record for the notification
/// message field: the explicit message when present, else label/status.
fn progress_message(record: &ProgressRecord) -> Option<String> {
    if let Some(message) = &record.message {
        return Some(message.clone());
    }
    match (&record.label, &record.status) {
        (Some(label), Some(status)) => Some(format!("{label}: {status}")),
        (Some(label), None) => Some(label.clone()),
        (None, Some(status)) => Some(status.clone()),
        (None, None) => None,
    }
}

// ─────────────────────────── result mapping ───────────────────────────

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
            vec![ContentBlock::text(
                serde_json::to_string_pretty(value).unwrap_or_default(),
            )]
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

// ────────────────────────── VFS resource face ──────────────────────────

/// Encode a `resources/list` continuation cursor: the directory being
/// listed and the entry offset within it. Newline separator — unambiguous
/// in virtual paths.
fn encode_cursor(path: &str, offset: usize) -> String {
    format!("{path}\n{offset}")
}

fn decode_cursor(cursor: &str) -> Option<(String, usize)> {
    let (path, offset) = cursor.rsplit_once('\n')?;
    let offset = offset.parse().ok()?;
    Some((path.to_string(), offset))
}

/// Resolve a resource URI to a virtual path. Accepts the canonical
/// `local://` spelling, the legacy `vfs://` one, and a bare `/path`.
fn resource_uri_to_path(uri: &str) -> String {
    let stripped = uri
        .strip_prefix("local://")
        .or_else(|| uri.strip_prefix("vfs://"))
        .unwrap_or(uri);
    if stripped.starts_with('/') {
        stripped.to_string()
    } else {
        format!("/{stripped}")
    }
}

/// Best-effort MIME guess for VFS resource entries.
fn guess_mime(name: &str) -> Option<&'static str> {
    let extension = name.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match extension.as_str() {
        "md" | "markdown" => "text/markdown",
        "txt" | "log" => "text/plain",
        "csv" | "tsv" => "text/csv",
        "json" | "jsonl" | "geojson" => "application/json",
        "yaml" | "yml" | "toml" => "application/yaml",
        "html" | "htm" => "text/html",
        "parquet" => "application/vnd.apache.parquet",
        "arrow" | "ipc" => "application/vnd.apache.arrow.file",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        _ => return None,
    })
}

/// Map a VFS entry to an MCP resource. Directories are surfaced with a
/// description marking them as navigation aids, not readable bodies.
fn entry_to_resource(entry: &VfsEntry) -> Resource {
    let uri = format!("local://{}", entry.path.trim_end_matches('/'));
    let mut resource = Resource::new(uri, entry.name.clone());
    if entry.is_dir {
        resource.description = Some(
            "directory — children are listed by paginating with its path as the cursor".to_string(),
        );
    } else {
        resource.mime_type = guess_mime(&entry.name).map(str::to_string);
        resource.size = Some(entry.size);
    }
    resource
}

/// Map browse errors onto MCP protocol errors by HTTP-ish status.
fn browse_error_to_mcp(error: browse::BrowseError) -> McpError {
    if error.status() == 404 {
        McpError::resource_not_found(error.to_string(), None)
    } else {
        McpError::internal_error(error.to_string(), None)
    }
}

// ─────────────────────────── ServerHandler ───────────────────────────

impl ServerHandler for McpTools {
    fn get_info(&self) -> ServerConfig {
        // The builder is type-state: resources are advertised by a
        // separate chain rather than a conditional `enable_*` call.
        let capabilities = if self.vfs.is_some() {
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build()
        } else {
            ServerCapabilities::builder().enable_tools().build()
        };
        ServerConfig::new(capabilities)
            .with_server_info(Implementation::new("autonomics", crate::GATEWAY_VERSION))
            .with_instructions(
                "Autonomics data-engine face: discover DAG node factories, \
                 inspect specs/ports/docs, build and run flows. Use \
                 `list_node_factories` / `get_node_spec` to explore, \
                 `set_flow` / `view_flow` to edit, `run_flow` to execute. \
                 Outputs live in the VFS: read them via `resources/read` \
                 with a `local://` URI, discover paths via `resources/list`.",
            )
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
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
    ) -> Result<rmcp::model::CallToolResponse, McpError> {
        let progress = request
            .progress_token()
            .map(|token| (token, context.peer.clone()));
        let arguments = Value::Object(request.arguments.unwrap_or_default());
        let result = self
            .call(
                request.name.as_ref(),
                arguments,
                context.ct.clone(),
                progress,
            )
            .await;
        Ok(result.into())
    }

    async fn list_resources(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        resources_list(
            self.vfs.as_ref(),
            request.as_ref().and_then(|p| p.cursor.as_deref()),
        )
        .await
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<rmcp::model::ListResourceTemplatesResult, McpError> {
        resource_templates(self.vfs.as_ref())
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<rmcp::model::ReadResourceResponse, McpError> {
        Ok(resource_read(self.vfs.as_ref(), &request.uri).await?.into())
    }
}

/// Transport-independent `resources/list`: paginate one VFS directory.
async fn resources_list(
    storage: Option<&Arc<OpendalFileStorage>>,
    cursor: Option<&str>,
) -> Result<ListResourcesResult, McpError> {
    let Some(storage) = storage else {
        return Err(McpError::method_not_found::<
            rmcp::model::ListResourcesRequestMethod,
        >());
    };
    let (path, offset) = cursor
        .and_then(decode_cursor)
        .unwrap_or_else(|| ("/".to_string(), 0usize));

    let listing = browse::list(
        storage,
        &path,
        false,
        Some(RESOURCE_PAGE_LIMIT),
        Some(offset),
    )
    .await
    .map_err(browse_error_to_mcp)?;

    let next_cursor = listing
        .truncated
        .then(|| {
            listing
                .next_offset
                .map(|next| encode_cursor(&listing.path, next))
        })
        .flatten();

    Ok(ListResourcesResult {
        result_type: None,
        meta: None,
        next_cursor,
        ttl_ms: None,
        cache_scope: None,
        resources: listing.entries.iter().map(entry_to_resource).collect(),
    })
}

/// Transport-independent `resources/templates/list`.
fn resource_templates(
    storage: Option<&Arc<OpendalFileStorage>>,
) -> Result<rmcp::model::ListResourceTemplatesResult, McpError> {
    if storage.is_none() {
        return Err(McpError::method_not_found::<
            rmcp::model::ListResourceTemplatesRequestMethod,
        >());
    }
    let template = ResourceTemplate::new("local://{+path}", "vfs-file").with_description(
        "Any file in the daemon VFS. Substitute the virtual path \
         (leading slash): local:///results/panel.parquet",
    );
    Ok(rmcp::model::ListResourceTemplatesResult {
        result_type: None,
        meta: None,
        next_cursor: None,
        ttl_ms: None,
        cache_scope: None,
        resource_templates: vec![template],
    })
}

/// Transport-independent `resources/read`: bounded UTF-8 text body.
async fn resource_read(
    storage: Option<&Arc<OpendalFileStorage>>,
    uri: &str,
) -> Result<ReadResourceResult, McpError> {
    let Some(storage) = storage else {
        return Err(McpError::method_not_found::<
            rmcp::model::ReadResourceRequestMethod,
        >());
    };
    let path = resource_uri_to_path(uri);
    let read = browse::read_text(storage, &path, RESOURCE_READ_MAX_BYTES)
        .await
        .map_err(browse_error_to_mcp)?;

    let uri = vfs::uri::uri(&read.path);
    let mut text = read.contents;
    if read.truncated {
        text.push_str(&format!(
            "\n…[truncated at {} bytes of {}]",
            RESOURCE_READ_MAX_BYTES, read.size
        ));
    }
    let contents = ResourceContents::TextResourceContents {
        uri,
        mime_type: guess_mime(&read.path).map(str::to_string),
        text,
        meta: None,
    };
    Ok(ReadResourceResult::new(vec![contents]))
}

/// Build the `/mcp` router: the rmcp Streamable HTTP service behind the
/// gateway's bearer-token auth middleware.
///
/// Auth reuses `bearer_or_session` — MCP clients authenticate with the
/// same daemon token frontends use. The service itself keeps rmcp's
/// loopback-only default `Host` validation (the daemon binds loopback).
pub fn router(infra: &SharedInfra, auth: Arc<super::auth::AuthContext>) -> axum::Router {
    use rmcp::transport::streamable_http_server::StreamableHttpService;
    use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;

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
    fn get_info_advertises_tools_but_not_resources_without_vfs() {
        let info = bridge().get_info();
        assert!(info.capabilities.tools.is_some());
        assert!(info.capabilities.resources.is_none());
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
    async fn call_routes_to_implementation_without_progress() {
        let result = bridge()
            .call(
                "echo",
                json!({"message": "hi"}),
                CancellationToken::new(),
                None,
            )
            .await;
        // rmcp's `success()` constructor sets `is_error: Some(false)`.
        assert!(
            !result.is_error.unwrap_or(false),
            "text was: {}",
            text_of(&result)
        );
        assert_eq!(text_of(&result), "echo: hi");
    }

    #[tokio::test]
    async fn unknown_tool_is_a_caller_visible_error() {
        let result = bridge()
            .call("nope", Value::Null, CancellationToken::new(), None)
            .await;
        assert_eq!(result.is_error, Some(true));
        let text = text_of(&result);
        assert!(text.contains("Available tools: echo"), "got: {text}");
    }

    #[tokio::test]
    async fn invalid_arguments_report_validation_failure() {
        let result = bridge()
            .call("echo", json!({}), CancellationToken::new(), None)
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
        let result = bridge.call("slow", json!({}), token, None).await;
        assert_eq!(result.is_error, Some(true));
        assert!(text_of(&result).contains("cancelled"));
    }

    #[tokio::test]
    async fn resources_disabled_without_vfs() {
        assert!(resources_list(None, None).await.is_err());
        assert!(resource_templates(None).is_err());
        assert!(resource_read(None, "local:///x").await.is_err());
    }

    #[test]
    fn cursor_roundtrip() {
        let encoded = encode_cursor("/data/sub dir", 40);
        assert_eq!(decode_cursor(&encoded), Some(("/data/sub dir".into(), 40)));
        assert_eq!(decode_cursor("not-a-cursor"), None);
        assert_eq!(decode_cursor("/path\nnotanumber"), None);
    }

    #[test]
    fn resource_uri_resolution() {
        assert_eq!(resource_uri_to_path("local:///a/b.tsv"), "/a/b.tsv");
        assert_eq!(resource_uri_to_path("vfs:///a/b"), "/a/b");
        assert_eq!(resource_uri_to_path("/direct/path"), "/direct/path");
        assert_eq!(resource_uri_to_path("relative.csv"), "/relative.csv");
    }

    #[test]
    fn mime_guessing() {
        assert_eq!(guess_mime("notes.md"), Some("text/markdown"));
        assert_eq!(
            guess_mime("panel.parquet"),
            Some("application/vnd.apache.parquet")
        );
        assert_eq!(guess_mime("archive.tar.gz"), None);
        assert_eq!(guess_mime("noextension"), None);
    }

    #[test]
    fn progress_message_prefers_explicit_message() {
        let mut record = ProgressRecord {
            kind: "progress".into(),
            label: Some("node".into()),
            status: None,
            message: None,
            level: None,
            current: None,
            total: None,
            elapsed_ms: None,
        };
        assert_eq!(progress_message(&record).as_deref(), Some("node"));
        record.status = Some("running".into());
        assert_eq!(progress_message(&record).as_deref(), Some("node: running"));
        record.message = Some("step 3".into());
        assert_eq!(progress_message(&record).as_deref(), Some("step 3"));
    }

    #[test]
    fn json_results_render_as_pretty_text() {
        let blocks = tool_result_content_to_blocks(&ToolResultContent::Json(json!({"a": 1})));
        assert_eq!(blocks.len(), 1);
        let text = blocks[0]
            .as_text()
            .map(|t| t.text.to_string())
            .unwrap_or_default();
        assert!(text.contains("\"a\": 1"), "got: {text}");
    }
}
