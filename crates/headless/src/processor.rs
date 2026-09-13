//! Output processors — render a translated [`RunEvent`] stream.
//!
//! Both the human-readable and the JSONL presentation are consumers of
//! the *same* translated event stream (codex's `EventProcessor` split);
//! translation from internal `AgentEvent`s happens once, in the run
//! loop, before events reach a processor.
//!
//! Processors never decide control flow — termination is the event
//! loop's job, driven by the authoritative `turn.completed` /
//! `turn.failed` / `run.ended` events. They also never print: all output
//! goes through injected [`Write`] targets so tests run against buffers.
//!
//! # stdout discipline
//!
//! - [`HumanProcessor`]: progress goes to the progress writer (stderr in
//!   the CLI); the result writer (stdout) receives **only** the final
//!   agent message, written once in [`OutputProcessor::finish`].
//! - [`JsonlProcessor`]: every event is one JSON line on the single
//!   writer; the final message is also available via
//!   [`OutputProcessor::last_message`] for `-o` files.

use std::io::Write;

use crate::event::{
    AgentMessageItem, ItemEvent, NoticeEvent, RunEndedEvent, RunEvent, RunItem, RunItemDetails,
    RunStartedEvent, RunStatus, TurnFailedEvent,
};

/// Terminal outcome of a run, derived from the observed event stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Outcome {
    /// No terminal event observed yet.
    #[default]
    Unknown,
    Completed,
    Failed,
    Cancelled,
}

impl From<RunStatus> for Outcome {
    fn from(status: RunStatus) -> Self {
        match status {
            RunStatus::Completed => Outcome::Completed,
            RunStatus::Failed => Outcome::Failed,
            RunStatus::Cancelled => Outcome::Cancelled,
        }
    }
}

/// Consumes translated run events and renders them.
pub trait OutputProcessor {
    /// Render one event. Must not panic on write errors to closed pipes —
    /// implementations report them once via their error slot.
    fn process(&mut self, event: &RunEvent);

    /// Emit end-of-run output (the final agent message in human mode) and
    /// flush all writers.
    fn finish(&mut self);

    /// The last complete agent message observed, if any.
    fn last_message(&self) -> Option<&str>;

    /// The terminal outcome observed in the stream.
    fn outcome(&self) -> Outcome;
}

/// State shared by both processors: what the stream said, independent of
/// how it is rendered.
#[derive(Debug, Default)]
struct Tracker {
    last_message: Option<String>,
    outcome: Outcome,
}

impl Tracker {
    fn observe(&mut self, event: &RunEvent) {
        match event {
            RunEvent::ItemCompleted(ItemEvent {
                item:
                    RunItem {
                        details: RunItemDetails::AgentMessage(AgentMessageItem { text }),
                        ..
                    },
                ..
            }) => self.last_message = Some(text.clone()),
            RunEvent::RunEnded(RunEndedEvent { status, .. }) => {
                self.outcome = (*status).into();
            }
            _ => {}
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// Human-readable processor
// ─────────────────────────────────────────────────────────────────────

/// Renders progress lines to `P` (stderr in the CLI) and defers the final
/// agent message to `O` (stdout) — the codex exec human-output contract.
pub struct HumanProcessor<P: Write, O: Write> {
    progress: P,
    output: O,
    tracker: Tracker,
}

impl<P: Write, O: Write> HumanProcessor<P, O> {
    pub fn new(progress: P, output: O) -> Self {
        Self {
            progress,
            output,
            tracker: Tracker::default(),
        }
    }

    fn say(&mut self, line: &str) {
        let _ = writeln!(self.progress, "{line}");
    }

    /// Recover the writers (for tests asserting on rendered bytes).
    pub fn into_parts(self) -> (P, O) {
        (self.progress, self.output)
    }
}

impl<P: Write, O: Write> OutputProcessor for HumanProcessor<P, O> {
    fn process(&mut self, event: &RunEvent) {
        self.tracker.observe(event);
        match event {
            RunEvent::RunStarted(RunStartedEvent {
                profile, model, ..
            }) => {
                let model = model.as_deref().unwrap_or("unknown model");
                self.say(&format!("▸ run started — profile {profile}, {model}"));
            }
            RunEvent::ItemStarted(ItemEvent {
                item: RunItem { details: RunItemDetails::ToolCall(tool), .. },
                ..
            }) => {
                self.say(&format!("▶ {}", tool.tool));
            }
            RunEvent::ItemCompleted(ItemEvent {
                item: RunItem { details: RunItemDetails::ToolCall(tool), .. },
                ..
            }) => {
                let mark = if tool.ok.unwrap_or(false) { "✓" } else { "✗" };
                let line = match one_line_preview(tool.result.as_deref(), 72) {
                    preview if preview.is_empty() => format!("{mark} {}", tool.tool),
                    preview => format!("{mark} {} {preview}", tool.tool),
                };
                self.say(&line);
            }
            RunEvent::TurnFailed(TurnFailedEvent { message, .. }) => {
                self.say(&format!("✗ turn failed: {message}"));
            }
            RunEvent::Notice(NoticeEvent { message, .. }) => {
                self.say(&format!("· {message}"));
            }
            // Turn boundaries, aggregated agent/reasoning items, and the
            // run end line carry no dedicated progress output — the final
            // message (if any) is the run's stdout artifact.
            _ => {}
        }
    }

    fn finish(&mut self) {
        if let Some(message) = self.tracker.last_message.clone() {
            let _ = writeln!(self.output, "{message}");
        }
        let _ = self.progress.flush();
        let _ = self.output.flush();
    }

    fn last_message(&self) -> Option<&str> {
        self.tracker.last_message.as_deref()
    }

    fn outcome(&self) -> Outcome {
        self.tracker.outcome
    }
}

// ─────────────────────────────────────────────────────────────────────
// JSONL processor
// ─────────────────────────────────────────────────────────────────────

/// Emits every event as one JSON line on a single writer.
pub struct JsonlProcessor<W: Write> {
    writer: W,
    tracker: Tracker,
}

impl<W: Write> JsonlProcessor<W> {
    pub fn new(writer: W) -> Self {
        Self {
            writer,
            tracker: Tracker::default(),
        }
    }

    /// Recover the writer (for tests asserting on emitted lines).
    pub fn into_parts(self) -> W {
        self.writer
    }
}

impl<W: Write> OutputProcessor for JsonlProcessor<W> {
    fn process(&mut self, event: &RunEvent) {
        self.tracker.observe(event);
        if serde_json::to_writer(&mut self.writer, event).is_ok() {
            let _ = self.writer.write_all(b"\n");
        }
    }

    fn finish(&mut self) {
        let _ = self.writer.flush();
    }

    fn last_message(&self) -> Option<&str> {
        self.tracker.last_message.as_deref()
    }

    fn outcome(&self) -> Outcome {
        self.tracker.outcome
    }
}

/// Collapse a tool result to a single bracketed preview line, truncated
/// to `max` characters on a char boundary.
fn one_line_preview(result: Option<&str>, max: usize) -> String {
    let Some(result) = result else {
        return String::new();
    };
    let first = result.lines().next().unwrap_or_default();
    let collapsed: String = first.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= max {
        collapsed
    } else {
        let truncated: String = collapsed.chars().take(max).collect();
        format!("{truncated}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{
        AgentMessageItem, ItemEvent, NoticeKind, RunEndedEvent, RunItem, RunStartedEvent,
        ToolCallItem, TurnCompletedEvent, TurnFailedEvent, TurnStartedEvent, Usage,
    };
    use serde_json::Value;
    use uuid::Uuid;

    fn tool_event(id: &str, ok: Option<bool>, result: Option<&str>) -> RunEvent {
        RunEvent::ItemCompleted(ItemEvent {
            item: RunItem {
                id: id.into(),
                details: RunItemDetails::ToolCall(ToolCallItem {
                    tool: "run_bash".into(),
                    input: Value::Null,
                    result: result.map(String::from),
                    ok,
                }),
            },
        })
    }

    fn message_event(text: &str) -> RunEvent {
        RunEvent::ItemCompleted(ItemEvent {
            item: RunItem {
                id: "m1".into(),
                details: RunItemDetails::AgentMessage(AgentMessageItem {
                    text: text.into(),
                }),
            },
        })
    }

    fn ended(status: RunStatus) -> RunEvent {
        RunEvent::RunEnded(RunEndedEvent {
            status,
            wall_time_secs: 0.1,
            usage: None,
            turns: 1,
        })
    }

    /// The canonical happy-path event sequence.
    fn happy_flow() -> Vec<RunEvent> {
        vec![
            RunEvent::RunStarted(RunStartedEvent {
                agent_id: Uuid::nil(),
                session_id: Uuid::nil(),
                profile: "default".into(),
                model: Some("test-model".into()),
            }),
            RunEvent::TurnStarted(TurnStartedEvent {
                turn_id: Uuid::nil(),
            }),
            RunEvent::ItemStarted(ItemEvent {
                item: RunItem {
                    id: "t1".into(),
                    details: RunItemDetails::ToolCall(ToolCallItem {
                        tool: "run_bash".into(),
                        input: Value::Null,
                        result: None,
                        ok: None,
                    }),
                },
            }),
            tool_event("t1", Some(true), Some("42\n")),
            message_event("the answer is 42"),
            RunEvent::TurnCompleted(TurnCompletedEvent {
                turn_id: Uuid::nil(),
                usage: Usage::default(),
            }),
            ended(RunStatus::Completed),
        ]
    }

    #[test]
    fn human_output_writer_receives_only_the_final_message() {
        let mut processor = HumanProcessor::new(Vec::new(), Vec::new());
        for event in happy_flow() {
            processor.process(&event);
        }
        processor.finish();

        let (_, output) = processor.into_parts();
        let output = String::from_utf8(output).unwrap();
        assert_eq!(output, "the answer is 42\n");
    }

    #[test]
    fn human_progress_contains_tool_lines_but_no_final_message() {
        let mut processor = HumanProcessor::new(Vec::new(), Vec::new());
        for event in happy_flow() {
            processor.process(&event);
        }
        processor.finish();

        let (progress, _) = processor.into_parts();
        let progress = String::from_utf8(progress).unwrap();
        assert!(progress.contains("▶ run_bash"));
        assert!(progress.contains("✓ run_bash 42"));
        assert!(!progress.contains("the answer is 42"));
    }

    #[test]
    fn human_failed_turn_writes_nothing_to_output() {
        let mut processor = HumanProcessor::new(Vec::new(), Vec::new());
        processor.process(&RunEvent::TurnStarted(TurnStartedEvent {
            turn_id: Uuid::nil(),
        }));
        processor.process(&RunEvent::TurnFailed(TurnFailedEvent {
            turn_id: Uuid::nil(),
            message: "api down".into(),
        }));
        processor.process(&ended(RunStatus::Failed));
        processor.finish();

        assert_eq!(processor.outcome(), Outcome::Failed);
        let (_, output) = processor.into_parts();
        assert!(output.is_empty());
    }

    #[test]
    fn jsonl_emits_one_parseable_line_per_event() {
        let mut processor = JsonlProcessor::new(Vec::new());
        let events = happy_flow();
        for event in &events {
            processor.process(event);
        }
        processor.finish();

        assert_eq!(processor.outcome(), Outcome::Completed);
        let lines = processor.into_parts();
        let text = String::from_utf8(lines).unwrap();
        assert!(text.ends_with('\n'));
        let parsed: Vec<RunEvent> = text
            .lines()
            .map(|line| serde_json::from_str(line).expect("each line is valid JSON"))
            .collect();
        assert_eq!(parsed, events);
    }

    #[test]
    fn notice_preview_truncates_on_char_boundary() {
        assert_eq!(one_line_preview(None, 10), "");
        assert_eq!(one_line_preview(Some("a\nb"), 10), "a");
        let long = "ä".repeat(80);
        let preview = one_line_preview(Some(&long), 10);
        assert_eq!(preview.chars().count(), 11); // 10 + ellipsis
        assert!(preview.ends_with('…'));
    }
}
