use agentik_types::AgentEvent;
use futures::future::join_all;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use crate::tools::task_runtime::{TaskStatus, TaskStore};
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
    /// background; when it completes, the real result is auto-injected into
    /// the agent's context via `BgTaskComplete`.
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

            let seq = self.tasks.read().await.alloc_seq();
            let cancel_token = CancellationToken::new();
            let cancel = cancel_token.clone();

            let output: ProgressBuffer = Arc::new(std::sync::Mutex::new(ProgressLog::new()));
            let ctx = ToolContext {
                output: Some(output.clone()),
            };

            let task_handle = tokio::spawn(async move {
                let exec_fut = implementation.execute_with_context(input, &ctx);
                tokio::pin!(exec_fut);

                let result = tokio::select! {
                    r = &mut exec_fut => r,
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

        // ---- Insert all entries into the task store ----
        // Then move sync entries back out for waiting (same pattern as the
        // old Fg/Bg partition, but simpler: we know which entries are sync
        // at spawn time rather than discovering it via run_mode).
        let mut to_wait: Vec<TaskEntry> = {
            let mut tasks = self.tasks.write().await;
            tasks.extend(new_entries);
            // Remove sync entries (those whose id is NOT in async_meta).
            let async_ids: Vec<&str> = async_meta.iter().map(|(_, id, _)| id.as_str()).collect();
            let mut fg = Vec::new();
            let mut i = 0;
            while i < tasks.len() {
                if async_ids.contains(&tasks[i].id()) {
                    i += 1; // async — leave in store
                } else {
                    fg.push(tasks.swap_remove(i)); // sync — take out to wait
                }
            }
            fg
        };

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

        // ---- Cleanup: remove completed sync tasks, keep async tasks ----
        {
            let mut tasks = self.tasks.write().await;
            tasks.extend(to_wait);
            tasks.retain(|t| !t.is_read());
        }

        results.extend(immediate_results);

        Ok(results)
    }

    pub fn tools(&self) -> Vec<ToolDefinition> {
        self.registry.definitions()
    }

    /// Look up a finished async task by seq and return the real result
    /// for auto-injection into the agent's context.
    ///
    /// Returns `None` when the task is unknown or still running.
    pub async fn finished_task_result(&self, seq: u64) -> Option<(String, ToolResult)> {
        let tasks = self.tasks.read().await;
        let entry = tasks.iter().find(|t| t.seq() == seq)?;
        let result = entry.tool_result()?;
        Some((entry.name().to_string(), result))
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
}
