use agentik_types::AgentEvent;
use futures::future::join_all;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use crate::tools::task_runtime::TaskStore;
use crate::tools::{ExecutionMode, ProgressBuffer, ProgressLog, ToolContext};

use super::DynToolFunction;
use super::error::ToolError;
use super::task_runtime::TaskEntry;
use agentik_sdk::types::ToolDefinition;
use agentik_sdk::types::tools::{ToolResult, ToolUse};

// ─────────────────────────── ToolRegistration ───────────────────────────

#[derive(Clone)]
pub struct ToolRegistration {
    pub definition: ToolDefinition,
    pub implementation: std::sync::Arc<dyn DynToolFunction>,
}

impl ToolRegistration {
    pub fn new(
        definition: ToolDefinition,
        implementation: std::sync::Arc<dyn DynToolFunction>,
    ) -> Self {
        Self {
            definition,
            implementation,
        }
    }
}

impl<T: super::ToolFunction + 'static> From<T> for ToolRegistration {
    fn from(tool: T) -> Self {
        let definition = tool.definition();
        Self {
            definition,
            // T: ToolFunction implies T: DynToolFunction via the blanket impl,
            // so this coercion is automatic.
            implementation: std::sync::Arc::new(tool),
        }
    }
}

// ─────────────────────────── ToolRegistry ───────────────────────────

/// Immutable registry of tool definitions, shared across sessions within one
/// agent. Built mutably, then frozen into [`Arc<ToolRegistry>`] and cloned
/// cheaply for each [`Toolset`].
///
/// `ToolRegistration` values hold `Arc<dyn DynToolFunction>` internally, so
/// cloning the HashMap is just ref-count bumps.
#[derive(Clone)]
pub struct ToolRegistry {
    tools: HashMap<String, ToolRegistration>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    pub fn register(&mut self, registration: ToolRegistration) -> Result<(), ToolError> {
        let name = registration.definition.name.clone();
        if self.tools.contains_key(&name) {
            return Err(ToolError::RegistryError {
                message: format!("Tool '{}' is already registered", name),
            });
        }
        self.tools.insert(name, registration);
        Ok(())
    }

    pub fn register_all(&mut self, registrations: Vec<ToolRegistration>) -> Result<(), ToolError> {
        for reg in registrations {
            self.register(reg)?;
        }
        Ok(())
    }

    pub fn contains(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    pub fn get(&self, name: &str) -> Option<&ToolRegistration> {
        self.tools.get(name)
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools.values().map(|r| r.definition.clone()).collect()
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

// ─────────────────────────── Toolset ───────────────────────────

/// Session-level tool runtime: shares an immutable [`ToolRegistry`] (tool
/// definitions + implementations) across sessions while maintaining
/// independent per-session state (background task list, event channel).
///
/// Construct via [`Toolset::from_registry`] after building and freezing a
/// [`ToolRegistry`] into an [`Arc`].
pub struct Toolset {
    registry: Arc<ToolRegistry>,
    tasks: Arc<RwLock<TaskStore>>,
    agent_event_tx: Option<UnboundedSender<AgentEvent>>,
    /// Session-level cancel token. When set (via [`set_cancel_token`]),
    /// each spawned tool task gets a child token so that cancelling the
    /// session immediately interrupts running tools.
    session_cancel: Option<CancellationToken>,
}

impl Toolset {
    /// Create a Toolset that shares the given frozen registry.
    ///
    /// Each call produces a fresh, independent task list — background tools
    /// in one Toolset are invisible to another.
    pub fn from_registry(
        registry: Arc<ToolRegistry>,
        agent_event_tx: Option<UnboundedSender<AgentEvent>>,
    ) -> Self {
        Self::from_registry_with_tasks(
            registry,
            Arc::new(RwLock::new(TaskStore::new())),
            agent_event_tx,
        )
    }

    /// Create a Toolset with an explicitly provided task list.
    ///
    /// Used by [`AgentBuilder`](crate::agent_builder::AgentBuilder) where the
    /// builtin task tools (registered into the `ToolRegistry`) already hold a
    /// reference to the tasks handle — so the Toolset must share that exact
    /// Arc rather than creating a new one.
    pub fn from_registry_with_tasks(
        registry: Arc<ToolRegistry>,
        tasks: Arc<RwLock<TaskStore>>,
        agent_event_tx: Option<UnboundedSender<AgentEvent>>,
    ) -> Self {
        Self {
            registry,
            tasks,
            agent_event_tx,
            session_cancel: None,
        }
    }

    /// Convenience: create a Toolset with a brand-new (empty) registry.
    ///
    /// Intended for tests and callers that don't need cross-session sharing.
    pub fn with_empty_registry(agent_event_tx: Option<UnboundedSender<AgentEvent>>) -> Self {
        Self::from_registry(Arc::new(ToolRegistry::new()), agent_event_tx)
    }

    /// Return a clone of the shared task-list handle.
    ///
    /// Used by builtin tools (e.g. `view_task_results`) that need to
    /// inspect background tasks without going through the agent loop.
    pub fn tasks_handle(&self) -> Arc<RwLock<TaskStore>> {
        self.tasks.clone()
    }

    /// Set the session-level cancel token. Each spawned tool task will
    /// get a child token derived from this, so cancelling the session
    /// immediately interrupts running tools.
    pub fn set_cancel_token(&mut self, token: CancellationToken) {
        self.session_cancel = Some(token);
    }

    /// Access the shared tool registry.
    pub fn registry(&self) -> &Arc<ToolRegistry> {
        &self.registry
    }

    /// Mutable access to the registry Arc (for `Arc::get_mut` patterns).
    pub fn registry_mut(&mut self) -> &mut Arc<ToolRegistry> {
        &mut self.registry
    }

    /// Execute a batch of tool calls, returning one `ToolResult` per call.
    ///
    /// **Sync tools** block the caller until the tool completes or times out.
    /// The result is injected directly as a `tool_result` message.
    ///
    /// **Async tools** return a placeholder immediately. The tool runs in the
    /// background; when it completes, a lightweight notification (task name +
    /// status) is injected via `BgTaskComplete`. The agent then pulls the
    /// full result on demand using `view_task_results` or `wait_task`.
    pub async fn execute(
        &self,
        toolcalls: &[ToolUse],
        notify_tx: Option<super::task_runtime::BgTaskNotifyTx>,
    ) -> Result<Vec<ToolResult>, ToolError> {
        let mut immediate_results: Vec<ToolResult> = Vec::new();
        // Collect metadata for async tasks before they're moved into the store.
        let mut async_meta: Vec<(u64, String, String)> = Vec::new(); // (seq, id, name)
        let mut new_entries: Vec<TaskEntry> = Vec::with_capacity(toolcalls.len());

        // ---- Spawn all tool tasks ----
        for tc in toolcalls {
            let Some(registration) = self.registry.get(&tc.name) else {
                // Unknown tool name. Don't silently skip — the LLM
                // will not see a tool_result for this id and may
                // produce another round of tool_use blocks without
                // results, masking the underlying issue. Emit a stub
                // error ToolResult so the adjacency invariant holds
                // and the model sees a clear error explaining what
                // happened. The list of available tool names is
                // embedded so the model can self-correct on the next
                // turn.
                let available: Vec<String> = self
                    .registry
                    .definitions()
                    .into_iter()
                    .map(|d| d.name)
                    .collect();
                let msg = format!(
                    "Unknown tool '{}'. Available tools: {}",
                    tc.name,
                    available.join(", ")
                );
                immediate_results.push(ToolResult::error_with_id(tc.id.clone(), msg));
                continue;
            };

            if let Err(e) = registration.implementation.validate_input(&tc.input) {
                immediate_results.push(ToolResult::error_with_id(tc.id.clone(), e.to_string()));
                continue;
            }

            let mode = registration.implementation.execution_mode();
            let timeout_secs = registration.implementation.timeout_seconds();

            let implementation = registration.implementation.clone();
            let input = tc.input.clone();
            let task_id = tc.id.clone();
            let tool_name = tc.name.clone();

            let seq = self.tasks.read().await.alloc_seq();
            // Derive the per-task cancel token from the session token
            // (if set) so that cancelling the session immediately
            // interrupts running tools. Otherwise create a standalone
            // token (preserving the original behaviour for tests).
            let cancel_token = match &self.session_cancel {
                Some(parent) => parent.child_token(),
                None => CancellationToken::new(),
            };
            let cancel = cancel_token.clone();

            let output: ProgressBuffer = Arc::new(std::sync::Mutex::new(ProgressLog::new()));
            let ctx = ToolContext {
                output: Some(output.clone()),
            };

            let task_handle = tokio::spawn(async move {
                // Catch panics from tool execution so they become proper
                // ToolErrors instead of crashing the task silently.
                use futures::FutureExt;
                use std::panic::AssertUnwindSafe;

                let exec_fut = implementation.execute_with_context(input, &ctx);

                let result = tokio::select! {
                    r = AssertUnwindSafe(exec_fut).catch_unwind() => {
                        match r {
                            Ok(r) => r,
                            Err(payload) => {
                                let msg = payload
                                    .downcast_ref::<&'static str>()
                                    .map(|s| (*s).to_string())
                                    .or_else(|| payload.downcast_ref::<String>().map(|s| s.clone()))
                                    .unwrap_or_else(|| "<non-string panic>".to_string());
                                let bt = std::backtrace::Backtrace::force_capture();
                                tracing::error!(
                                    target: "spawn_safe",
                                    tool = %tool_name,
                                    panic = %msg,
                                    backtrace = %bt,
                                    "tool execution panicked"
                                );
                                Err(ToolError::ExecutionFailed {
                                    source: format!("tool panicked: {msg}").into(),
                                })
                            }
                        }
                    }
                    _ = cancel.cancelled() => Err(ToolError::Cancel),
                    _ = tokio::time::sleep(Duration::from_secs(timeout_secs)) => {
                        Err(ToolError::Timeout { seconds: timeout_secs })
                    }
                };

                match result {
                    Ok(mut tool_result) => {
                        tool_result.tool_use_id = task_id;
                        Ok(tool_result)
                    }
                    Err(e) => Err(e),
                }
            });

            match mode {
                ExecutionMode::Sync => {
                    // Sync: no notify_tx (consumed inline by execute()).
                    let entry = TaskEntry::with_notify(
                        seq,
                        tc.id.clone(),
                        tc.name.clone(),
                        task_handle,
                        cancel_token,
                        None,
                        output,
                    );
                    new_entries.push(entry);
                }
                ExecutionMode::Async => {
                    // Async: pass notify_tx so BgTaskComplete fires on completion.
                    async_meta.push((seq, tc.id.clone(), tc.name.clone()));
                    let entry = TaskEntry::with_notify(
                        seq,
                        tc.id.clone(),
                        tc.name.clone(),
                        task_handle,
                        cancel_token,
                        notify_tx.clone(),
                        output,
                    );
                    new_entries.push(entry);
                }
            }
        }

        // ---- Partition new entries: sync → wait inline, async → store ----
        // Sync entries are never inserted into the shared task store — they
        // live only for the duration of this `execute()` call. This prevents
        // the store-scanning partition loop from accidentally pulling out
        // async tasks from *previous* batches (which caused background tasks
        // to disappear when a subsequent execute() ran).
        let async_ids: std::collections::HashSet<&str> =
            async_meta.iter().map(|(_, id, _)| id.as_str()).collect();
        let mut to_wait: Vec<TaskEntry> = Vec::new();
        let mut to_store: Vec<TaskEntry> = Vec::new();
        for entry in new_entries {
            if async_ids.contains(entry.id()) {
                to_store.push(entry);
            } else {
                to_wait.push(entry);
            }
        }

        // Insert only async entries into the task store.
        {
            let mut tasks = self.tasks.write().await;
            tasks.extend(to_store);
        }

        // ---- Wait for sync tasks (block until done/timeout/cancel) ----
        let sync_results = join_all(to_wait.iter_mut().map(|t| t.wait_for_result())).await;

        // ---- Build results vector ----
        let mut results: Vec<ToolResult> = Vec::new();
        results.extend(sync_results);

        // Async tasks: emit ToolCallBackground + return placeholder.
        for (seq, id, name) in &async_meta {
            if let Some(tx) = &self.agent_event_tx {
                let _ = tx.send(AgentEvent::ToolCallBackground {
                    seq: *seq,
                    name: name.clone(),
                });
            }
            results.push(ToolResult::from_pending_task(id, *seq));
        }

        // ---- GC: remove consumed async tasks (marked read by ----
        // view_task_results / wait_task in a prior turn). Sync entries were
        // never in the store so they don't need re-removal.
        {
            let mut tasks = self.tasks.write().await;
            tasks.retain(|t| !t.is_read());
        }

        results.extend(immediate_results);

        Ok(results)
    }

    pub fn tools(&self) -> Vec<ToolDefinition> {
        self.registry.definitions()
    }

    /// Look up a task by seq and return `(name, is_success)` for the
    /// lightweight completion notification.
    ///
    /// Does **not** return the full result — the agent pulls the actual
    /// output on demand via `view_task_results` or `wait_task`.
    /// Returns `None` when the task is unknown or still running.
    pub async fn task_brief(&self, seq: u64) -> Option<(String, bool)> {
        let tasks = self.tasks.read().await;
        let entry = tasks.iter().find(|t| t.seq() == seq)?;
        Some((entry.name().to_string(), entry.is_done()))
    }

    /// Check whether any background tasks are still running.
    pub async fn has_background_tasks(&self) -> bool {
        let tasks = self.tasks.read().await;
        !tasks.is_empty()
    }

    /// Signal cancellation to every running background task.
    ///
    /// Called during agent shutdown so that long-running tool invocations
    /// (e.g. `run_dag`, `run_bash`) exit promptly instead of outliving the
    /// runtime. The spawned tasks check their `CancellationToken` in a
    /// `select!` arm and break with `Err(ToolError::Cancel)`.
    pub async fn cancel_all_tasks(&self) {
        let tasks = self.tasks.read().await;
        for task in tasks.iter() {
            task.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use crate::tools::{ExecutionMode, ToolFunction};
    use agentik_sdk::types::tools::ToolUse;
    use agentik_types::AgentEvent;
    use async_trait::async_trait;
    use serde_json::Value;
    use serde_json::json;
    use tokio::sync::mpsc;

    use super::{ToolRegistry, Toolset};
    use crate::tools::ToolContext;
    use agentik_proc::tool;

    #[tool(name = "test_tool", description = "A test tool")]
    struct MockInput {
        reason: String,
    }

    struct MockTool {
        result_text: String,
    }

    impl MockTool {
        fn new(text: &str) -> Self {
            Self {
                result_text: text.to_string(),
            }
        }
    }

    #[async_trait]
    impl ToolFunction for MockTool {
        type Input = MockInput;

        async fn run(
            &self,
            _input: MockInput,
        ) -> Result<crate::tools::ToolResult, crate::tools::error::ToolError> {
            Ok(crate::tools::ToolResult::success(self.result_text.clone()))
        }
    }

    // ── Async mock tool ──

    #[tool(name = "test_async_tool", description = "An async tool")]
    struct MockAsyncInput {
        reason: String,
    }

    struct MockAsyncTool {
        result_text: String,
    }

    impl MockAsyncTool {
        fn new(text: &str) -> Self {
            Self {
                result_text: text.to_string(),
            }
        }
    }

    #[async_trait]
    impl ToolFunction for MockAsyncTool {
        type Input = MockAsyncInput;

        fn execution_mode(&self) -> ExecutionMode {
            ExecutionMode::Async
        }

        async fn run(
            &self,
            _input: MockAsyncInput,
        ) -> Result<crate::tools::ToolResult, crate::tools::error::ToolError> {
            tokio::time::sleep(Duration::from_millis(100)).await;
            Ok(crate::tools::ToolResult::success(self.result_text.clone()))
        }
    }

    // ── Progress mock tool (Async, pushes progress via context) ──

    #[tool(name = "test_progress_tool", description = "emits progress")]
    struct MockProgressInput {
        reason: String,
    }

    struct MockProgressTool;

    #[async_trait]
    impl ToolFunction for MockProgressTool {
        type Input = MockProgressInput;

        fn execution_mode(&self) -> ExecutionMode {
            ExecutionMode::Async
        }

        async fn execute_with_context(
            &self,
            input: Value,
            ctx: &ToolContext,
        ) -> Result<crate::tools::ToolResult, crate::tools::error::ToolError> {
            let _typed: MockProgressInput = serde_json::from_value(input)?;
            ctx.emit_line("step 1");
            ctx.emit_line("step 2");
            tokio::time::sleep(Duration::from_millis(100)).await;
            Ok(crate::tools::ToolResult::success("done"))
        }
    }

    /// Build a `ToolRegistry` from a list of tools and freeze it into `Arc`.
    fn build_registry(tools: Vec<crate::tools::ToolRegistration>) -> Arc<ToolRegistry> {
        let mut registry = ToolRegistry::new();
        registry.register_all(tools).unwrap();
        Arc::new(registry)
    }

    #[tokio::test]
    async fn test_register_and_list_tools() {
        let (tx, _rx) = mpsc::unbounded_channel::<AgentEvent>();
        let registry = build_registry(vec![MockTool::new("mock result").into()]);
        let toolset = Toolset::from_registry(registry, Some(tx));

        let tools = toolset.tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "test_tool");
    }

    #[tokio::test]
    async fn test_execute_sync_tool() {
        let (tx, _rx) = mpsc::unbounded_channel::<AgentEvent>();
        let registry = build_registry(vec![MockTool::new("mock result").into()]);
        let toolset = Toolset::from_registry(registry, Some(tx));

        let tool_call = ToolUse {
            id: "tc1".to_string(),
            name: "test_tool".to_string(),
            input: json!({ "reason": "test" }),
        };

        let results = toolset.execute(&[tool_call], None).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].tool_use_id, "tc1");
        // Sync tool returns the real result, not a placeholder.
        assert!(results[0].text_content().contains("mock result"));
    }

    /// Sync tool blocks until it completes; result is the actual content.
    #[tokio::test]
    async fn test_sync_tool_returns_real_result() {
        let (tx, _rx) = mpsc::unbounded_channel::<AgentEvent>();
        let registry = build_registry(vec![MockTool::new("hello world").into()]);
        let toolset = Toolset::from_registry(registry, Some(tx));

        let results = toolset
            .execute(
                &[ToolUse {
                    id: "tc1".to_string(),
                    name: "test_tool".to_string(),
                    input: json!({ "reason": "test" }),
                }],
                None,
            )
            .await
            .unwrap();

        assert_eq!(results[0].text_content(), "hello world");

        // Sync tasks are cleaned up after completion.
        let tasks = toolset.tasks_handle();
        let tasks = tasks.read().await;
        assert!(tasks.is_empty(), "sync task should be cleaned up");
    }

    /// Async tool returns a placeholder immediately.
    #[tokio::test]
    async fn test_async_tool_returns_placeholder() {
        let (tx, _rx) = mpsc::unbounded_channel::<AgentEvent>();
        let registry = build_registry(vec![MockAsyncTool::new("async result").into()]);
        let toolset = Toolset::from_registry(registry, Some(tx));

        let results = toolset
            .execute(
                &[ToolUse {
                    id: "tc1".to_string(),
                    name: "test_async_tool".to_string(),
                    input: json!({ "reason": "test" }),
                }],
                None,
            )
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert!(results[0].text_content().contains("running in background"));
        assert!(!results[0].text_content().contains("async result"));

        // Async task is retained in the task store.
        let tasks = toolset.tasks_handle();
        let tasks = tasks.read().await;
        assert!(tasks.iter().any(|t| t.id() == "tc1"), "async task retained");
    }

    /// Async tool eventually completes and stores the real result.
    #[tokio::test]
    async fn test_async_task_completes_with_result() {
        let (tx, _rx) = mpsc::unbounded_channel::<AgentEvent>();
        let registry = build_registry(vec![MockAsyncTool::new("done!").into()]);
        let toolset = Toolset::from_registry(registry, Some(tx));

        toolset
            .execute(
                &[ToolUse {
                    id: "tc1".to_string(),
                    name: "test_async_tool".to_string(),
                    input: json!({ "reason": "test" }),
                }],
                None,
            )
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(200)).await;

        let tasks = toolset.tasks_handle();
        let tasks = tasks.read().await;
        let entry = tasks
            .iter()
            .find(|t| t.id() == "tc1")
            .expect("async task retained");
        let result = entry.tool_result().expect("async task completed");
        assert!(result.text_content().contains("done!"));
    }

    /// Mixed sync + async execution in the same batch.
    #[tokio::test]
    async fn test_mixed_sync_and_async() {
        let (tx, _rx) = mpsc::unbounded_channel::<AgentEvent>();
        let registry = build_registry(vec![
            MockAsyncTool::new("async").into(),
            MockTool::new("sync").into(),
        ]);
        let toolset = Toolset::from_registry(registry, Some(tx));

        let results = toolset
            .execute(
                &[
                    ToolUse {
                        id: "tc1".to_string(),
                        name: "test_async_tool".to_string(),
                        input: json!({ "reason": "test" }),
                    },
                    ToolUse {
                        id: "tc2".to_string(),
                        name: "test_tool".to_string(),
                        input: json!({ "reason": "test" }),
                    },
                ],
                None,
            )
            .await
            .unwrap();

        assert_eq!(results.len(), 2);
        let async_r = results.iter().find(|r| r.tool_use_id == "tc1").unwrap();
        let sync_r = results.iter().find(|r| r.tool_use_id == "tc2").unwrap();
        assert!(sync_r.text_content().contains("sync"));
        assert!(async_r.text_content().contains("background"));
    }

    /// Async tool with context pushes progress to its TaskEntry output.
    #[tokio::test]
    async fn test_async_tool_surfaces_progress() {
        let (tx, _rx) = mpsc::unbounded_channel::<AgentEvent>();
        let registry = build_registry(vec![MockProgressTool.into()]);
        let toolset = Toolset::from_registry(registry, Some(tx));

        toolset
            .execute(
                &[ToolUse {
                    id: "tc1".to_string(),
                    name: "test_progress_tool".to_string(),
                    input: json!({ "reason": "test" }),
                }],
                None,
            )
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(50)).await;

        let tasks = toolset.tasks_handle();
        let tasks = tasks.read().await;
        let entry = tasks
            .iter()
            .find(|t| t.id() == "tc1")
            .expect("async task retained");
        let out = entry.output();
        let messages: Vec<&str> = out
            .iter()
            .filter(|r| r.kind == "log")
            .filter_map(|r| r.message.as_deref())
            .collect();
        assert!(
            messages.contains(&"step 1") && messages.contains(&"step 2"),
            "output should carry both emitted records"
        );
    }

    /// Two sessions sharing the same registry must have independent task lists.
    #[tokio::test]
    async fn test_shared_registry_independent_tasks() {
        let registry = build_registry(vec![MockTool::new("ok").into()]);

        let toolset_a = Toolset::from_registry(registry.clone(), None);
        let toolset_b = Toolset::from_registry(registry, None);

        assert!(!Arc::ptr_eq(
            &toolset_a.tasks_handle(),
            &toolset_b.tasks_handle(),
        ));
        assert!(Arc::ptr_eq(toolset_a.registry(), toolset_b.registry(),));

        toolset_a
            .execute(
                &[ToolUse {
                    id: "tc_a".to_string(),
                    name: "test_tool".to_string(),
                    input: json!({ "reason": "a" }),
                }],
                None,
            )
            .await
            .unwrap();

        let b_tasks = toolset_b.tasks_handle();
        let b_tasks = b_tasks.read().await;
        assert!(b_tasks.is_empty(), "session B should have no tasks from A");
    }

    /// Regression: an async task spawned in a prior `execute()` call must
    /// survive a subsequent `execute()` that runs a sync tool. The old code
    /// scanned the entire store and pulled prior-batch async entries into
    /// `to_wait`, causing them to be waited on (blocking) and then evicted
    /// by `retain(|t| !t.is_read())`.
    #[tokio::test]
    async fn test_prior_async_task_survives_next_execute() {
        let (tx, _rx) = mpsc::unbounded_channel::<AgentEvent>();
        let registry = build_registry(vec![
            MockAsyncTool::new("async result").into(),
            MockTool::new("sync result").into(),
        ]);
        let toolset = Toolset::from_registry(registry, Some(tx));

        // Batch 1: spawn an async task (e.g. delegate_to).
        toolset
            .execute(
                &[ToolUse {
                    id: "async_1".to_string(),
                    name: "test_async_tool".to_string(),
                    input: json!({ "reason": "a" }),
                }],
                None,
            )
            .await
            .unwrap();

        // Batch 2: execute a sync tool in the same toolset.
        let results = toolset
            .execute(
                &[ToolUse {
                    id: "sync_1".to_string(),
                    name: "test_tool".to_string(),
                    input: json!({ "reason": "s" }),
                }],
                None,
            )
            .await
            .unwrap();

        // Sync result should be correct.
        assert_eq!(results[0].text_content(), "sync result");

        // The async task from batch 1 must still be in the store.
        let tasks = toolset.tasks_handle();
        let tasks = tasks.read().await;
        let async_entry = tasks
            .iter()
            .find(|t| t.id() == "async_1")
            .expect("prior async task must survive a subsequent execute() call");
        assert!(
            !async_entry.is_read(),
            "prior async task should not be marked read"
        );
    }

    /// When the LLM emits a tool_use whose name is not in the registry
    /// (e.g. it remembered `opengwas_gwasinfo` from older docs, but the
    /// table-returning variant has been migrated to a DAG source node),
    /// the toolset must produce a stub error ToolResult for the missing
    /// id. Otherwise the conversation is left with a tool_use that has
    /// no matching tool_result, and Anthropic returns message 2013.
    #[tokio::test]
    async fn unknown_tool_name_emits_stub_error_result() {
        let (tx, _rx) = mpsc::unbounded_channel::<AgentEvent>();
        let registry = build_registry(vec![MockTool::new("ok").into()]);
        let toolset = Toolset::from_registry(registry, Some(tx));

        // Mix a real tool_use (unknown name) with a known one. Note:
        // the LLM may call `opengwas_gwasinfo` (an older name removed
        // when the table-returning variant migrated to a DAG source
        // node); the toolset must produce a stub error result so the
        // Anthropic adjacency invariant is preserved.
        let toolcalls = vec![
            ToolUse {
                id: "call_unknown".to_string(),
                name: "opengwas_gwasinfo".to_string(),
                input: serde_json::json!({}),
            },
            ToolUse {
                id: "call_known".to_string(),
                name: "test_tool".to_string(),
                input: json!({ "reason": "test" }),
            },
        ];
        let results = toolset
            .execute(&toolcalls, None)
            .await
            .expect("execute must succeed even with unknown tool names");

        // One result per tool_use must be produced — otherwise the
        // conversation has a tool_use block without a matching
        // tool_result, and Anthropic returns message 2013.
        assert_eq!(results.len(), 2);

        let unknown = results
            .iter()
            .find(|r| r.tool_use_id == "call_unknown")
            .expect("stub result for unknown tool must exist");
        assert_eq!(
            unknown.is_error,
            Some(true),
            "unknown tool must be marked is_error=true"
        );
        let content = unknown.text_content();
        assert!(
            content.contains("Unknown tool 'opengwas_gwasinfo'"),
            "stub error must name the unknown tool: {content}"
        );
        assert!(
            content.contains("test_tool"),
            "stub error must list available tools so the model can self-correct: {content}"
        );

        let known = results
            .iter()
            .find(|r| r.tool_use_id == "call_known")
            .expect("real result for known tool");
        assert!(
            known.is_error != Some(true),
            "known tool must not be flagged as error"
        );
        assert_eq!(known.text_content(), "ok");
    }
}
