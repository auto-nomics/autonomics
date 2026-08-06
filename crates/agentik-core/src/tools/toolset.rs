use agentik_types::AgentEvent;
use futures::future::join_all;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use crate::tools::task_runtime::{RunMode, TaskStatus, WaitResultKind};
use crate::tools::{ProgressBuffer, ProgressLog, ToolContext};

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
    tasks: Arc<RwLock<Vec<TaskEntry>>>,
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
            Arc::new(RwLock::new(Vec::new())),
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
        tasks: Arc<RwLock<Vec<TaskEntry>>>,
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
    pub fn tasks_handle(&self) -> Arc<RwLock<Vec<TaskEntry>>> {
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

    /// Spawn independent threads to execute tool calls
    pub async fn execute(
        &self,
        toolcalls: &[ToolUse],
        // This tokio sender is prepared for waking up agent in IDLE status
        notify_tx: Option<super::task_runtime::BgTaskNotifyTx>,
    ) -> Result<Vec<ToolResult>, ToolError> {
        let mut immediate_results: Vec<ToolResult> = Vec::new();
        // Tool name for each task spawned *in this call*, keyed by `tool_use_id`.
        // We only emit `ToolCallBackground` for newly spawned tasks — retained
        // background tasks from a prior call already announced themselves.
        let mut spawned_names: HashMap<String, String> = HashMap::new();

        // ---- Phase 1: spawn tool tasks WITHOUT holding the tasks lock ----
        // `tokio::spawn` / `TaskEntry::with_notify` are instantaneous; no need
        // to hold any lock across them. Collect entries and push once.
        //
        // Turning ToolCall into TaskEntry
        let mut new_entries: Vec<TaskEntry> = Vec::with_capacity(toolcalls.len());

        for tc in toolcalls {
            let Some(registration) = self.registry.get(&tc.name) else {
                continue;
            };

            if let Err(e) = registration.implementation.validate_input(&tc.input) {
                immediate_results.push(ToolResult::error_with_id(tc.id.clone(), e.to_string()));
                continue;
            }

            let sync_secs = registration.implementation.sync_seconds();
            let timeout_secs = registration.implementation.timeout_seconds();

            let implementation = registration.implementation.clone();
            let input = tc.input.clone();
            let task_id = tc.id.clone();

            let cancel_token = CancellationToken::new();
            let cancel = cancel_token.clone();

            // Create the shared progress buffer BEFORE spawning so the tool
            // can push structured records through `ctx.output` while it runs.
            // The same buffer is handed to `TaskEntry` below; `view_task_status`
            // snapshots it as the task's accumulated output.
            let output: ProgressBuffer = Arc::new(std::sync::Mutex::new(ProgressLog::new()));
            let ctx = ToolContext {
                output: Some(output.clone()),
            };

            let task_handle = tokio::spawn(async move {
                let result = tokio::select! {
                    r = implementation.execute_with_context(input, &ctx) => r,
                    _ = cancel.cancelled() => Err(ToolError::Cancel),
                    _ = tokio::time::sleep(Duration::from_secs(timeout_secs)) => Err(ToolError::Timeout { seconds: timeout_secs }),
                };
                // Set tool_use_id at result construction time
                match result {
                    Ok(mut tool_result) => {
                        tool_result.tool_use_id = task_id;
                        Ok(tool_result)
                    }
                    Err(e) => Err(e),
                }
            });

            new_entries.push(TaskEntry::with_notify(
                tc.id.clone(),
                tc.name.clone(),
                task_handle,
                cancel_token,
                sync_secs,
                notify_tx.clone(),
                output,
            ));
            spawned_names.insert(tc.id.clone(), tc.name.clone());
        }

        // ---- Phase 2: insert + partition under a SHORT-lived write lock ----
        // Add the new entries, then move foreground tasks out so we can wait on
        // them *outside* the lock. Background tasks stay in the vec untouched.
        let mut to_wait: Vec<TaskEntry> = {
            let mut tasks = self.tasks.write().await;
            tasks.extend(new_entries);
            let mut fg = Vec::new();
            let mut i = 0;
            while i < tasks.len() {
                // swap_remove keeps this O(1); order within the vec is
                // irrelevant since results are matched by tool_use_id.
                if matches!(tasks[i].run_mode(), RunMode::Bg) {
                    i += 1;
                } else {
                    fg.push(tasks.swap_remove(i));
                }
            }
            fg
        };
        // ^ lock released here — the expensive await below is now lock-free.

        // ---- Phase 3: wait for foreground tasks WITHOUT holding the lock ----
        let wait_results = join_all(to_wait.iter_mut().map(|t| t.wait())).await;

        let mut results: Vec<ToolResult> = Vec::with_capacity(wait_results.len());
        for wait_result in wait_results {
            // When a tool didn't finish within its sync window, it is now
            // running in the background — notify frontend observers and agent immediately. Only
            // announce tasks spawned in this call; retained background tasks
            // from a prior turn already announced themselves.
            if let WaitResultKind::StillRunning(ref id) = wait_result.inner
                && let Some(name) = spawned_names.get(id)
                && let Some(tx) = &self.agent_event_tx
            {
                let _ = tx.send(AgentEvent::ToolCallBackground {
                    id: id.clone(),
                    name: name.clone(),
                });
            }
            results.push(wait_result.into());
        }

        // ---- Phase 4: reinsert + cleanup under a SHORT-lived write lock ----
        // `wait()` flips still-running tasks to RunMode::Bg; finished tasks are
        // marked read via the `WaitResult -> ToolResult` conversion, so the
        // retain drops exactly the consumed ones.
        {
            let mut tasks = self.tasks.write().await;
            tasks.extend(to_wait);
            tasks.retain(|t| !t.is_read());
        }

        // NOTE: intermediate result should be pull by agent rather than injected passively
        results.extend(immediate_results);

        Ok(results)
    }

    pub fn tools(&self) -> Vec<ToolDefinition> {
        self.registry.definitions()
    }

    /// Look up a finished background task by `tool_use_id` and return
    /// `(name, ok, content)` for a completion notification, **without**
    /// removing the entry from the task list.
    ///
    /// The real result stays in the `TaskEntry` (read on demand via
    /// `view_task_results`) so it is never injected into the LLM context.
    /// For background tasks the `Done` status only holds a placeholder, so the
    /// real content is read from `tool_result`.
    ///
    /// Returns `None` when the task is unknown or still running.
    pub async fn finished_task_notification(&self, id: &str) -> Option<(String, bool, String)> {
        let tasks = self.tasks.read().await;
        let entry = tasks.iter().find(|t| t.id() == id)?;
        match entry.status() {
            TaskStatus::Done(res) => Some((
                entry.name().to_string(),
                true,
                format!("Task {0} finished successfully", res.tool_use_id),
            )),
            TaskStatus::Failed(ref err) => Some((entry.name().to_string(), false, err.to_string())),
            TaskStatus::Running => None,
        }
    }

    /// Check whether any background tasks are still running.
    pub async fn has_background_tasks(&self) -> bool {
        let tasks = self.tasks.read().await;
        !tasks.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use crate::tools::ToolFunction;
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

    #[tool(name = "test_bg_tool", description = "A bg tool")]
    struct MockTwophaseInput {
        reason: String,
    }

    struct MockTwophaseTool {
        result_text: String,
    }

    impl MockTwophaseTool {
        fn new(text: &str) -> Self {
            Self {
                result_text: text.to_string(),
            }
        }
    }

    #[async_trait]
    impl ToolFunction for MockTwophaseTool {
        type Input = MockTwophaseInput;

        fn sync_seconds(&self) -> u64 {
            1
        }
        async fn run(
            &self,
            input: MockTwophaseInput,
        ) -> Result<crate::tools::ToolResult, crate::tools::error::ToolError> {
            dbg!(&input.reason);
            tokio::time::sleep(Duration::from_secs(3)).await;
            Ok(crate::tools::ToolResult::success(self.result_text.clone()))
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

    /// Build a `ToolRegistry` from a list of tools and freeze it into `Arc`.
    fn build_registry(
        tools: Vec<crate::tools::ToolRegistration>,
    ) -> Arc<ToolRegistry> {
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
    async fn test_execute_tool() {
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
    }

    #[tokio::test]
    async fn test_double_phase_tool_execution() {
        let (tx, _rx) = mpsc::unbounded_channel::<AgentEvent>();
        let registry = build_registry(vec![
            MockTwophaseTool::new("test").into(),
            MockTool::new("test").into(),
        ]);
        let toolset = Toolset::from_registry(registry, Some(tx));

        let tool_call = ToolUse {
            id: "tc1".to_string(),
            name: "test_bg_tool".to_string(),
            input: json!({ "reason": "test" }),
        };

        let tool_call_immediate = ToolUse {
            id: "tc2".to_string(),
            name: "test_tool".to_string(),
            input: json!({ "reason": "test" }),
        };

        let result = toolset
            .execute(&[tool_call, tool_call_immediate], None)
            .await
            .unwrap();

        dbg!(&result);
        assert!(result.len() == 2)
    }

    // A tool that opts into the per-invocation context and pushes live output.
    #[tool(name = "test_progress_tool", description = "emits progress")]
    struct MockProgressInput {
        reason: String,
    }

    struct MockProgressTool;

    #[async_trait]
    impl ToolFunction for MockProgressTool {
        type Input = MockProgressInput;

        // Tiny sync window so the tool flips to background while it sleeps,
        // keeping its TaskEntry (and accumulated output) queryable.
        fn sync_seconds(&self) -> u64 {
            1
        }

        async fn execute_with_context(
            &self,
            input: Value,
            ctx: &ToolContext,
        ) -> Result<crate::tools::ToolResult, crate::tools::error::ToolError> {
            let _typed: MockProgressInput = serde_json::from_value(input)?;
            // Emit immediately so the output is populated before the sync
            // window expires.
            ctx.emit_line("step 1");
            ctx.emit_line("step 2");
            tokio::time::sleep(Duration::from_secs(3)).await;
            Ok(crate::tools::ToolResult::success("done"))
        }
    }

    /// A tool overriding `execute_with_context` must be able to push progress
    /// into its TaskEntry's output channel, where `view_task_status` reads it.
    #[tokio::test]
    async fn test_execute_with_context_surfaces_output() {
        let (tx, _rx) = mpsc::unbounded_channel::<AgentEvent>();
        let registry = build_registry(vec![MockProgressTool.into()]);
        let toolset = Toolset::from_registry(registry, Some(tx));

        let tool_call = ToolUse {
            id: "tc1".to_string(),
            name: "test_progress_tool".to_string(),
            input: json!({ "reason": "test" }),
        };

        // Returns once the sync window expires; the tool is now a background
        // task still mid-sleep, with its output already emitted.
        let _ = toolset.execute(&[tool_call], None).await.unwrap();

        let tasks = toolset.tasks_handle();
        let tasks = tasks.read().await;
        let entry = tasks
            .iter()
            .find(|t| t.id() == "tc1")
            .expect("background task should be retained while still running");
        let out = entry.output();
        // emit_line pushes a structured `kind="log"` record carrying the text
        // in `message` (not a flat concatenated string).
        let messages: Vec<&str> = out
            .iter()
            .filter(|r| r.kind == "log")
            .filter_map(|r| r.message.as_deref())
            .collect();
        assert!(
            messages.contains(&"step 1") && messages.contains(&"step 2"),
            "TaskEntry.output should carry both emitted records in order; got: {out:?}"
        );
        assert_eq!(
            out.len(),
            2,
            "expected exactly two progress records; got: {out:?}"
        );
    }

    /// Two sessions sharing the same registry must have independent task lists.
    #[tokio::test]
    async fn test_shared_registry_independent_tasks() {
        let registry = build_registry(vec![MockTool::new("ok").into()]);

        let toolset_a = Toolset::from_registry(registry.clone(), None);
        let toolset_b = Toolset::from_registry(registry, None);

        // Task handles must NOT be the same Arc.
        assert!(!Arc::ptr_eq(
            &toolset_a.tasks_handle(),
            &toolset_b.tasks_handle(),
        ));

        // But registries ARE shared.
        assert!(Arc::ptr_eq(
            toolset_a.registry(),
            toolset_b.registry(),
        ));

        // Running a tool on A does not affect B's task list.
        let _ = toolset_a
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
        assert!(
            b_tasks.is_empty(),
            "session B should have no tasks from session A"
        );
    }
}
