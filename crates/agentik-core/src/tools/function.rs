use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;

use super::error::ToolError;
use agentik_sdk::types::{ToolDefinition, ToolInput, ToolResult};

/// One structured progress entry a tool pushes to its task's live-output
/// channel. Unlike a flat text line, this carries machine-readable fields so
/// `view_task_status` can return JSON the agent parses directly (e.g. a node's
/// `elapsed_ms` or a `current/total` progress pair).
///
/// `kind` is a free-form category chosen by the tool (e.g. `"status"`,
/// `"progress"`, `"log"`, `"finished"`); every other field is optional and
/// only serialized when set, so a record stays compact.
#[derive(Clone, Debug, Serialize)]
pub struct ProgressRecord {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<u64>,
}

impl ProgressRecord {
    /// Start a record of `kind`; set further fields via the builders below.
    pub fn new(kind: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            label: None,
            status: None,
            message: None,
            level: None,
            current: None,
            total: None,
            elapsed_ms: None,
        }
    }
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
    pub fn status(mut self, status: impl Into<String>) -> Self {
        self.status = Some(status.into());
        self
    }
    pub fn message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }
    pub fn level(mut self, level: impl Into<String>) -> Self {
        self.level = Some(level.into());
        self
    }
    pub fn current(mut self, current: u64) -> Self {
        self.current = Some(current);
        self
    }
    pub fn total(mut self, total: u64) -> Self {
        self.total = Some(total);
        self
    }
    pub fn elapsed_ms(mut self, elapsed_ms: u64) -> Self {
        self.elapsed_ms = Some(elapsed_ms);
        self
    }
}

/// Hard ceiling on retained progress records per task. Pushes beyond this
/// evict the oldest record, bounding memory and — combined with the `limit`
/// parameter on `view_task_status` — the context cost of polling a
/// long-running task.
pub const MAX_PROGRESS_RECORDS: usize = 1000;

/// Append-only, cap-bounded log of [`ProgressRecord`]s. Shared between the
/// executing tool (writer, via [`ToolContext::emit`]) and the task entry
/// (reader, via [`TaskEntry::output`](super::task_runtime::TaskEntry::output)).
///
/// When [`MAX_PROGRESS_RECORDS`] is reached the oldest record is evicted and
/// `dropped` is incremented, so an observer can tell it is seeing a tail
/// rather than the full history.
pub struct ProgressLog {
    records: Vec<ProgressRecord>,
    dropped: usize,
}

impl ProgressLog {
    pub fn new() -> Self {
        Self {
            records: Vec::new(),
            dropped: 0,
        }
    }

    /// Append a record, evicting the oldest if at the cap.
    pub fn push(&mut self, record: ProgressRecord) {
        if self.records.len() >= MAX_PROGRESS_RECORDS {
            // O(n) shift, but n ≤ MAX_PROGRESS_RECORDS (1000) — negligible,
            // and only once the log is saturated.
            self.records.remove(0);
            self.dropped += 1;
        }
        self.records.push(record);
    }

    /// Clone of all retained records, oldest-first.
    pub fn snapshot(&self) -> Vec<ProgressRecord> {
        self.records.clone()
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Number of records evicted by the cap (oldest, lost).
    pub fn dropped(&self) -> usize {
        self.dropped
    }
}

impl Default for ProgressLog {
    fn default() -> Self {
        Self::new()
    }
}

/// Shared, cap-bounded log of [`ProgressRecord`]s backing a task's live output.
/// Tools push via [`ToolContext::emit`]; observers read a snapshot via
/// [`TaskEntry::output`](super::task_runtime::TaskEntry::output).
pub type ProgressBuffer = Arc<Mutex<ProgressLog>>;

/// Per-invocation context handed to a tool's [`ToolFunction::execute_with_context`].
///
/// Carries optional handles a tool may use to interact with its surrounding
/// task infrastructure while it runs. Today the only field is `output`: a
/// shared append-only [`ProgressBuffer`] mirroring the background-task entry's
/// live-output channel, so a long-running tool can push structured progress
/// that `view_task_status` surfaces. Tools that don't care about progress
/// simply ignore the context (the default `execute_with_context` does so and
/// delegates to [`ToolFunction::execute`]).
#[derive(Clone, Default)]
pub struct ToolContext {
    /// Live-output buffer. `None` when the toolset did not wire one (e.g. in
    /// tests); the tool must treat it as optional.
    pub output: Option<ProgressBuffer>,
}

impl ToolContext {
    /// Push a structured [`ProgressRecord`] onto the live-output buffer.
    /// No-op when no buffer is wired.
    pub fn emit(&self, record: ProgressRecord) {
        if let Some(buf) = &self.output {
            // O(1) amortized push under a brief lock — no read-modify-write
            // of the whole history (the old String approach was O(n²)).
            if let Ok(mut v) = buf.lock() {
                v.push(record);
            }
        }
    }

    /// Convenience: push a plain log line as a `kind = "log"` record. Useful
    /// for tools that only need free-form text progress.
    pub fn emit_line(&self, line: impl Into<String>) {
        self.emit(ProgressRecord::new("log").message(line));
    }
}

/// A tool that the agent can invoke.
///
/// Tools receive their inputs as a strongly-typed `Input` struct rather
/// than a raw [`serde_json::Value`]. The framework takes care of the
/// `Value -> Self::Input` conversion at the trait boundary; tool
/// implementations override [`run`](Self::run) and receive the
/// deserialized struct directly.
///
/// ## Choosing `Input`
///
/// - For tools with structured parameters, define a `#[derive(Deserialize)]`
///   struct and use it as `type Input`. Required fields become required
///   JSON fields; `Option<T>` fields default to `None`; `#[serde(default)]`
///   makes a field optional with the type's default value.
/// - For tools that take no input, use `type Input = serde_json::Value`
///   and override [`execute`](Self::execute) directly (skipping the
///   deserialize step).
///
/// ## Implementing a tool
///
/// Override [`run`](Self::run) to receive typed input:
///
/// ```ignore
/// #[derive(Deserialize)]
/// struct MyInput { name: String }
///
/// #[async_trait]
/// impl ToolFunction for MyTool {
///     type Input = MyInput;
///     async fn run(&self, input: MyInput) -> Result<ToolResult, ToolError> {
///         Ok(ToolResult::success(format!("hi {}", input.name)))
///     }
/// }
/// ```
///
/// Override [`execute`](Self::execute) instead when you need raw
/// `Value` access (e.g. to accept arbitrary extra fields).
///
/// ## Storage
///
/// `ToolFunction` has an associated type (`Input`), so `dyn ToolFunction`
/// cannot hold tools with different `Input` types. To store tools
/// heterogeneously (registry / toolset), erase to
/// [`DynToolFunction`]: every `T: ToolFunction` implements it via a
/// blanket impl, so storage sites can hold `Box<dyn DynToolFunction>`
/// while concrete call sites keep full type information.
#[async_trait]
pub trait ToolFunction: Send + Sync {
    /// Strongly-typed input parameter struct. See trait docs.
    ///
    /// The `ToolInput` bound means `Input` can describe its own
    /// [`ToolDefinition`] (typically derived via `#[derive(ToolInput)]`),
    /// which lets `definition()` delegate automatically.
    type Input: DeserializeOwned + ToolInput + Send + Sync;

    /// Framework entry point. Deserializes `input` into `Self::Input`
    /// and dispatches to [`run`](Self::run). Tool implementations
    /// should override `run` (typed) rather than this method unless
    /// they need to bypass the deserialize step.
    async fn execute(&self, input: Value) -> Result<ToolResult, ToolError> {
        let typed: Self::Input = serde_json::from_value(input)?;
        self.run(typed).await
    }

    /// Context-aware entry point. The toolset always calls this (never
    /// [`execute`](Self::execute) directly). The default implementation
    /// ignores `ctx` and delegates to `execute`, so existing tools that only
    /// override `run` keep working unchanged. Override this when the tool
    /// needs the per-invocation [`ToolContext`] — e.g. to stream progress to
    /// its background-task entry via [`ToolContext::emit_line`]. An override
    /// must deserialize `input` itself (it bypasses the default deserialize).
    async fn execute_with_context(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        self.execute(input).await
    }

    /// Business implementation. Override this for typed input.
    ///
    /// Default panics — every concrete tool must override either
    /// `run` or `execute`.
    async fn run(&self, _input: Self::Input) -> Result<ToolResult, ToolError> {
        unimplemented!("override `run` (typed input) or `execute` (raw Value)")
    }

    fn validate_input(&self, _input: &Value) -> Result<(), ToolError> {
        Ok(())
    }

    /// Phase 1 threshold. Tool execution will convert from synchronous into asynchronus.
    fn sync_seconds(&self) -> u64 {
        30
    }

    /// Phase 2 timeout threshold
    fn timeout_seconds(&self) -> u64 {
        300
    }

    fn definition(&self) -> ToolDefinition {
        Self::Input::definition()
    }
}

/// Type-erased view of a tool, for heterogeneous storage.
///
/// `ToolFunction::Input` is an associated type, which makes
/// `Box<dyn ToolFunction>` incompatible with holding tools whose
/// `Input` types differ. Every `T: ToolFunction` automatically
/// implements `DynToolFunction` via a blanket impl, so callers can:
///
/// - keep full type info on the concrete side (impl `ToolFunction`
///   with a concrete `Input` struct),
/// - erase to `Box<dyn DynToolFunction>` only at the registry /
///   storage boundary.
#[async_trait]
pub trait DynToolFunction: Send + Sync {
    async fn execute(&self, input: Value) -> Result<ToolResult, ToolError>;

    /// Context-aware entry; the toolset calls this in preference to
    /// [`execute`](Self::execute) so tools that opt into [`ToolContext`] get
    /// it. Default forwarders delegate to `execute` (ignoring the context).
    async fn execute_with_context(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError>;

    fn validate_input(&self, input: &Value) -> Result<(), ToolError>;

    fn sync_seconds(&self) -> u64;
    fn timeout_seconds(&self) -> u64;

    fn definition(&self) -> ToolDefinition;
}

#[async_trait]
impl<T: ToolFunction + ?Sized> DynToolFunction for T {
    async fn execute(&self, input: Value) -> Result<ToolResult, ToolError> {
        ToolFunction::execute(self, input).await
    }

    async fn execute_with_context(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        ToolFunction::execute_with_context(self, input, ctx).await
    }

    fn validate_input(&self, input: &Value) -> Result<(), ToolError> {
        ToolFunction::validate_input(self, input)
    }

    fn sync_seconds(&self) -> u64 {
        ToolFunction::sync_seconds(self)
    }

    fn timeout_seconds(&self) -> u64 {
        ToolFunction::timeout_seconds(self)
    }

    fn definition(&self) -> ToolDefinition {
        ToolFunction::definition(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentik_sdk::types::tools::ToolResultContent;
    use async_trait::async_trait;
    use serde_json::json;

    use agentik_proc::tool;

    #[tool(name = "echo", description = "Echo test tool")]
    struct EchoInput {
        message: String,
    }

    struct TestEchoTool;

    #[async_trait]
    impl ToolFunction for TestEchoTool {
        type Input = EchoInput;

        async fn run(&self, input: EchoInput) -> Result<ToolResult, ToolError> {
            Ok(ToolResult::success(format!("Echo: {}", input.message)))
        }
    }

    #[tokio::test]
    async fn test_tool_function_execution() {
        let tool = TestEchoTool;
        let input = json!({"message": "Hello, World!"});
        let result = ToolFunction::execute(&tool, input).await.unwrap();
        if let ToolResultContent::Text(content) = result.content {
            assert_eq!(content, "Echo: Hello, World!");
        } else {
            panic!("Expected text content");
        }
    }

    #[tokio::test]
    async fn test_tool_function_missing_required_field() {
        let tool = TestEchoTool;
        let input = json!({});
        let err = ToolFunction::execute(&tool, input).await.unwrap_err();
        // missing field surfaces as a deserialization error
        assert!(matches!(err, ToolError::ExecutionFailed { .. }));
    }
}
