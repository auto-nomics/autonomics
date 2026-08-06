use std::sync::Arc;
use tokio::sync::RwLock;

use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::tools::task_runtime::{TaskStatus, TaskStore};
use crate::tools::{MAX_PROGRESS_RECORDS, ToolError, ToolFunction};

/// Default page size when `limit` is omitted.
const DEFAULT_LIMIT: usize = 50;

#[tool(
    name = "view_task_status",
    description = "View a background task's status AND query its structured live log. \
                  The log is a bounded, append-only list of progress records the tool \
                  emitted while running (e.g. per-node status/elapsed/progress from a \
                  backgrounded `run_dag`). \
                  \
                  Log query (modeled on kubectl/journalctl `logs`): \
                  - `tail=true` (default): return the most recent `limit` records. \
                  - `tail=false`: page oldest-first with `offset` + `limit` (SQL-style). \
                  - `kind`: filter by record kind (`status` | `progress` | `log` | \
                    `finished` | `error`), e.g. `kind=\"finished\"` for per-node timing. \
                  - `limit=0`: return counts only, no records (cheap status poll). \
                  \
                  The response carries `log.returned`, `log.filtered_total`, \
                  `log.retained_total`, `log.dropped` (records evicted by the cap, \
                  unrecoverable), and `log.has_more` to drive further paging. \
                  Records older than the cap are gone; page with `offset`/`tail` over \
                  what remains."
)]
pub struct ViewTaskStatusInput {
    #[desc = "Task number (#N) of the target background task, as shown when it was spawned"]
    task: u64,
    #[desc = "Max records to return. Default 50; 0 = counts only (no records); \
              clamped to MAX_PROGRESS_RECORDS otherwise."]
    limit: Option<usize>,
    #[desc = "Records to skip from the oldest (after `kind` filter). Only used when \
              `tail=false`. Default 0."]
    offset: Option<usize>,
    #[desc = "If true (default), return the most recent `limit` records (ignore \
              `offset`). If false, page oldest-first from `offset`."]
    tail: Option<bool>,
    #[desc = "Optional filter: only records with this `kind` \
              (`status` | `progress` | `log` | `finished` | `error`)."]
    kind: Option<String>,
}

pub struct TaskStatusViewerTool {
    tasks: Arc<RwLock<TaskStore>>,
}

impl TaskStatusViewerTool {
    pub fn new(tasks: Arc<RwLock<TaskStore>>) -> Self {
        Self { tasks }
    }
}

impl From<TaskStatus> for &'static str {
    fn from(status: TaskStatus) -> &'static str {
        match status {
            TaskStatus::Running => "running",
            TaskStatus::Done(_) => "done",
            TaskStatus::Failed(_) => "failed",
        }
    }
}

/// Compute the record window to return over a filtered log of length
/// `filtered_total`, following log-tool conventions:
/// - `counts_only` (limit == 0): empty window, `has_more` = any records exist.
/// - `tail`: last `limit` records (most recent).
/// - otherwise (`offset` paging): `limit` records from `offset`, oldest-first.
///
/// Returns `(start..end, has_more)` where the range is clamped to
/// `filtered_total`. Pure function — extracted so the paging semantics are
/// unit-testable without the task plumbing.
fn select_window(
    filtered_total: usize,
    limit: usize,
    offset: usize,
    tail: bool,
    counts_only: bool,
) -> (std::ops::Range<usize>, bool) {
    if counts_only {
        return (0..0, filtered_total > 0);
    }
    if tail {
        let start = filtered_total.saturating_sub(limit);
        return (start..filtered_total, filtered_total > limit);
    }
    let start = offset.min(filtered_total);
    let end = start.saturating_add(limit).min(filtered_total);
    (start..end, end < filtered_total)
}

#[async_trait]
impl ToolFunction for TaskStatusViewerTool {
    type Input = ViewTaskStatusInput;

    fn sync_seconds(&self) -> u64 {
        30
    }

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let tasks = self.tasks.read().await;
        let Some(task) = tasks.iter().find(|t| t.seq() == input.task) else {
            return Ok(AgentToolResult::error(format!(
                "no background task #{}, use `view_task_status` with no filter to list active tasks",
                input.task
            )));
        };

        let status: &str = task.status().into();

        // Snapshot the retained log (oldest-first) and the eviction count.
        let all = task.output();
        let retained_total = all.len();
        let dropped = task.dropped();

        // Apply the optional `kind` filter.
        let filtered: Vec<&crate::tools::ProgressRecord> = match &input.kind {
            Some(k) => all.iter().filter(|r| r.kind == *k).collect(),
            None => all.iter().collect(),
        };
        let filtered_total = filtered.len();

        // Resolve paging params.
        let raw_limit = input.limit.unwrap_or(DEFAULT_LIMIT);
        let counts_only = raw_limit == 0;
        let limit = if counts_only {
            0
        } else {
            raw_limit.clamp(1, MAX_PROGRESS_RECORDS)
        };
        let tail = input.tail.unwrap_or(true);

        // Select the window over the filtered slice.
        let (window, has_more) = select_window(
            filtered_total,
            limit,
            input.offset.unwrap_or(0),
            tail,
            counts_only,
        );
        let records: Vec<&crate::tools::ProgressRecord> = filtered[window.clone()].to_vec();
        let window_start = window.start;

        let mut payload = serde_json::json!({
            "task": task.seq(),
            "name": task.name(),
            "status": status,
            "log": {
                "returned": records.len(),
                "filtered_total": filtered_total,
                "retained_total": retained_total,
                "dropped": dropped,
                "has_more": has_more,
                "filter": input.kind,
                "tail": tail,
                "offset": window_start,
                "limit": limit,
            },
        });
        if !records.is_empty() {
            payload["log"]["records"] =
                serde_json::to_value(&records).unwrap_or(serde_json::Value::Null);
        }

        Ok(AgentToolResult::success_json(payload))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_tail_returns_most_recent() {
        // 10 records, tail 3 → indices 7..10, has_more true.
        let (w, more) = select_window(10, 3, 0, true, false);
        assert_eq!(w, 7..10);
        assert!(more);
    }

    #[test]
    fn window_tail_limited_subset_has_no_more() {
        // Fewer records than the limit → whole range, no more.
        let (w, more) = select_window(2, 5, 0, true, false);
        assert_eq!(w, 0..2);
        assert!(!more);
    }

    #[test]
    fn window_offset_pages_oldest_first() {
        // Page 2 of size 3 over 10 → offset 3, indices 3..6, has_more.
        let (w, more) = select_window(10, 3, 3, false, false);
        assert_eq!(w, 3..6);
        assert!(more);
        // Last page.
        let (w, more) = select_window(10, 3, 9, false, false);
        assert_eq!(w, 9..10);
        assert!(!more);
        // Offset past end → empty.
        let (w, more) = select_window(10, 3, 20, false, false);
        assert_eq!(w, 10..10);
        assert!(!more);
    }

    #[test]
    fn window_counts_only_returns_empty() {
        let (w, more) = select_window(5, 0, 0, true, true);
        assert_eq!(w, 0..0);
        assert!(more); // records exist
        let (_, more) = select_window(0, 0, 0, true, true);
        assert!(!more);
    }

    #[test]
    fn progress_log_evicts_oldest_at_cap() {
        use crate::tools::function::ProgressLog;
        let mut log = ProgressLog::new();
        for i in 0..MAX_PROGRESS_RECORDS {
            log.push(crate::tools::ProgressRecord::new("log").message(format!("r{i}")));
        }
        assert_eq!(log.len(), MAX_PROGRESS_RECORDS);
        assert_eq!(log.dropped(), 0);
        // One past the cap evicts the oldest ("r0").
        log.push(crate::tools::ProgressRecord::new("log").message("overflow"));
        assert_eq!(log.len(), MAX_PROGRESS_RECORDS);
        assert_eq!(log.dropped(), 1);
        let snap = log.snapshot();
        assert_eq!(snap.first().unwrap().message.as_deref(), Some("r1"));
        assert_eq!(snap.last().unwrap().message.as_deref(), Some("overflow"));
    }
}
