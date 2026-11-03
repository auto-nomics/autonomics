//! Node execution events flowing back to the scheduler over the single mpsc
//! that already drives [`super::graph::DAG::run`].
//!
//! The channel carries [`NodeEvent`]; [`NodeEventKind::Done`] is the only
//! authoritative terminal signal — the scheduler advances the ready queue on
//! it, exactly as it did when the channel element was a bare [`JobResult`].
//! `Status` / `Progress` / `Log` are best-effort observations a node emits
//! *mid-`execute`* via a [`NodeReporter`]; the scheduler forwards/stores them
//! but its correctness never depends on them (they may be dropped on a full
//! channel — see [`NodeReporter`]).

use serde::Serialize;
use tokio::sync::mpsc;

use crate::dag::DagError;
use crate::dag::NodeId;
use crate::dag::graph::PortOutputs;
use crate::dag::runtime::{NodeRunDetails, RuntimeStatus};

// NOTE: `NodeEvent`/`NodeEventKind` deliberately do NOT derive `Serialize` —
// `Done(JobResult)` carries a `DagError` (which wraps the non-Serializable
// `DataFusionError`). When we surface events to the agent we translate to a
// Serializable DTO, mirroring how `runtime::DagErrorReport` already bridges
// `DagError` for `NodeReport`.

/// Terminal outcome of a dispatched node — the only variant the scheduler
/// *acts* on. Lifted unchanged from its previous home in `graph.rs` so the
/// `Success`/`Failed` pattern matches there keep working.
#[derive(Debug)]
pub enum JobResult {
    Success {
        id: NodeId,
        outputs: PortOutputs,
        duration: std::time::Duration,
        /// Execution evidence the node attached via
        /// [`NodeReporter::set_run_details`], harvested when the terminal
        /// result is built.
        details: Option<NodeRunDetails>,
    },
    Failed {
        id: NodeId,
        error: DagError,
        duration: std::time::Duration,
        /// Same as [`JobResult::Success::details`] — kept on the failure path
        /// so a failed execution still carries its exit code / logs.
        details: Option<NodeRunDetails>,
    },
}

impl JobResult {
    /// The node id this result pertains to, regardless of variant.
    pub fn node_id(&self) -> &str {
        match self {
            JobResult::Success { id, .. } => id,
            JobResult::Failed { id, .. } => id,
        }
    }

    /// Elapsed wall-clock of the node's `execute()`.
    pub fn duration(&self) -> std::time::Duration {
        match self {
            JobResult::Success { duration, .. } | JobResult::Failed { duration, .. } => *duration,
        }
    }
}

/// Severity for a [`NodeEventKind::Log`] event. Mirrors `tracing::Level` but is
/// a small `Serialize` enum so the event stays dependency-light and agent-
/// surfaceable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EventLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

/// What a node is telling the world right now.
#[derive(Debug)]
pub enum NodeEventKind {
    Execution(super::live::ExecutionObservation),
    /// A lifecycle transition the scheduler also tracks independently; useful
    /// as an explicit, timestamped observation for observers (e.g. a TUI).
    Status {
        status: RuntimeStatus,
    },
    /// Incremental progress for long-running nodes (e.g. MiXeR "fitting block
    /// 3/100"). `current`/`total` are opaque counters chosen by the node.
    Progress {
        current: u64,
        total: u64,
    },
    /// A free-form log line.
    Log {
        level: EventLevel,
        message: String,
    },
    /// One item emitted by a streaming Channel producer.
    ///
    /// Unlike ephemeral observations, this event drives scheduler readiness and
    /// uses a bounded `send().await` so a slow downstream expansion applies
    /// backpressure to the producer.
    ChannelItem {
        port: u8,
        sequence: u64,
        item: serde_json::Value,
    },
    /// A streaming Channel producer has emitted its final item.
    ChannelClosed {
        port: u8,
        item_count: u64,
    },
    /// Run-level memory observation emitted by the scheduler's memory guard.
    Resource {
        usage_bytes: u64,
        limit_bytes: u64,
        usage_ratio: f64,
        threshold_ratio: f64,
    },
    /// Authoritative terminal result — drives the scheduler's ready-queue
    /// advancement. Sent by the dispatch wrapper after `execute` returns, not
    /// by the node itself.
    Done(JobResult),
    /// Lightweight terminal observation for EXTERNAL observers only: carries
    /// the node's final status and elapsed time but **no output DataFrames**
    /// (unlike [`Done`], which holds `PortOutputs` for routing). The scheduler
    /// emits this to the optional external event sink in [`super::graph::DAG::run`];
    /// it never flows through the internal scheduling channel.
    Finished {
        status: RuntimeStatus,
        elapsed_ms: u64,
    },
}

/// One observation from one node.
#[derive(Debug)]
pub struct NodeEvent {
    pub node_id: NodeId,
    pub kind: NodeEventKind,
}

impl NodeEvent {
    pub fn new(node_id: impl Into<NodeId>, kind: NodeEventKind) -> Self {
        Self {
            node_id: node_id.into(),
            kind,
        }
    }
}

/// Node-facing handle for emitting mid-`execute` observations.
///
/// Cloneable and `Send` so the scheduler can hand a copy into the spawned
/// `'static` task alongside the cloned node. Ephemeral events (`Status` /
/// `Progress` / `Log`) use `try_send`: if the channel is saturated they are
/// **dropped silently** — losing a log line is acceptable, and this prevents a
/// chatty node from back-pressuring or deadlocking the run. The authoritative
/// `Done` is never sent through here.
#[derive(Clone)]
pub struct NodeReporter {
    node_id: NodeId,
    tx: mpsc::Sender<NodeEvent>,
    /// Side channel for execution-level evidence (image digest, exit code,
    /// persisted log URIs). Shared across clones but scoped to a single
    /// dispatch — the scheduler creates a fresh reporter per node execution.
    live_view: Option<super::live::LiveDagView>,
    external_sink: Option<mpsc::Sender<NodeEvent>>,
    observer: Option<std::sync::Arc<dyn Fn(super::live::ExecutionObservation) + Send + Sync>>,
    run_details: std::sync::Arc<std::sync::Mutex<Option<NodeRunDetails>>>,
    worker_software: std::sync::Arc<std::sync::Mutex<Option<serde_json::Value>>>,
    worker_stop: std::sync::Arc<std::sync::Mutex<Option<serde_json::Value>>>,
    logs: std::sync::Arc<std::sync::Mutex<Vec<ReportedLog>>>,
}

/// One log observation retained for task execution evidence.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ReportedLog {
    pub level: EventLevel,
    pub message: String,
}

impl NodeReporter {
    /// Bind a reporter to `node_id`; every emission is tagged with it so the
    /// node never has to repeat its own id.
    pub fn new(node_id: impl Into<NodeId>, tx: mpsc::Sender<NodeEvent>) -> Self {
        Self {
            node_id: node_id.into(),
            tx,
            live_view: None,
            external_sink: None,
            observer: None,
            run_details: std::sync::Arc::new(std::sync::Mutex::new(None)),
            worker_software: std::sync::Arc::new(std::sync::Mutex::new(None)),
            worker_stop: std::sync::Arc::new(std::sync::Mutex::new(None)),
            logs: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    /// A reporter whose emissions go nowhere — for tests / dry-runs that invoke
    /// [`super::graph::DAG::run`] or `DagNode::execute` directly, outside the
    /// scheduler. Emissions are silently dropped (the channel is buffered and
    /// unread).
    pub fn noop() -> Self {
        let (tx, _rx) = mpsc::channel::<NodeEvent>(1);
        // `_rx` drops here; `try_send` then sees no receiver and the emission
        // is silently dropped — exactly the "goes nowhere" semantics we want.
        Self::new("noop", tx)
    }

    #[inline]
    fn emit(&self, kind: NodeEventKind) {
        // try_send: fire-and-forget. A full/closed channel just drops the
        // observation — never blocks the node, never panics.
        let sink = self.external_sink.as_ref().unwrap_or(&self.tx);
        let _ = sink.try_send(NodeEvent::new(self.node_id.clone(), kind));
    }

    pub fn status(&self, status: RuntimeStatus) {
        if let Some(view) = &self.live_view {
            view.status(&self.node_id, status);
        }
        self.emit(NodeEventKind::Status { status });
    }

    pub fn with_live_view(mut self, view: super::live::LiveDagView) -> Self {
        self.live_view = Some(view);
        self
    }

    pub fn with_external_sink(mut self, sink: Option<mpsc::Sender<NodeEvent>>) -> Self {
        self.external_sink = sink;
        self
    }

    pub fn with_observer(
        mut self,
        observer: impl Fn(super::live::ExecutionObservation) + Send + Sync + 'static,
    ) -> Self {
        self.observer = Some(std::sync::Arc::new(observer));
        self
    }

    pub fn execution(&self, mut observation: super::live::ExecutionObservation) {
        observation.observed_at_ms = super::live::observation_time_ms();
        if let Some(view) = &self.live_view {
            observation = view.observe(&self.node_id, observation);
        }
        if let Some(observer) = &self.observer {
            observer(observation.clone());
        }
        self.emit(NodeEventKind::Execution(observation));
    }

    pub fn worker(&self, phase: &str, backend: &str, id: &str, state: &str, evidence: &str) {
        self.execution(super::live::ExecutionObservation {
            phase: phase.into(),
            worker: Some(serde_json::json!({"backend": backend,
                "id": id, "state": state, "evidence": evidence,
                "observed_at_ms": super::live::observation_time_ms()})),
            ..Default::default()
        });
    }

    pub fn phase(&self, phase: &str, message: &str) {
        self.execution(super::live::ExecutionObservation {
            phase: phase.into(),
            message: Some(message.into()),
            ..Default::default()
        });
    }

    pub fn phase_progress(&self, phase: &str, current: u64, total: Option<u64>, unit: &str) {
        self.execution(super::live::ExecutionObservation {
            phase: phase.into(),
            current: Some(current),
            total,
            unit: Some(unit.into()),
            ..Default::default()
        });
    }

    pub fn progress(&self, current: u64, total: u64) {
        if let Some(view) = &self.live_view {
            view.observe(
                &self.node_id,
                super::live::ExecutionObservation {
                    phase: "executing".into(),
                    current: Some(current),
                    total: Some(total),
                    unit: Some("items".into()),
                    ..Default::default()
                },
            );
        }
        self.emit(NodeEventKind::Progress { current, total });
    }

    pub fn log(&self, level: EventLevel, message: impl Into<String>) {
        let message = message.into();
        self.logs
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(ReportedLog {
                level,
                message: message.clone(),
            });
        self.emit(NodeEventKind::Log { level, message });
    }

    pub fn info(&self, message: impl Into<String>) {
        self.log(EventLevel::Info, message);
    }

    pub fn warn(&self, message: impl Into<String>) {
        self.log(EventLevel::Warn, message);
    }

    pub fn error(&self, message: impl Into<String>) {
        self.log(EventLevel::Error, message);
    }

    pub async fn emit_channel_item(&self, port: u8, sequence: u64, item: serde_json::Value) {
        let _ = self
            .tx
            .send(NodeEvent::new(
                self.node_id.clone(),
                NodeEventKind::ChannelItem {
                    port,
                    sequence,
                    item,
                },
            ))
            .await;
    }

    pub async fn close_channel(&self, port: u8, item_count: u64) {
        let _ = self
            .tx
            .send(NodeEvent::new(
                self.node_id.clone(),
                NodeEventKind::ChannelClosed { port, item_count },
            ))
            .await;
    }

    /// Attach execution-level evidence (image, exit code, persisted log
    /// URIs, …) to this dispatch. Called by the node; harvested by the
    /// scheduler when the terminal `JobResult` is built, so the details
    /// survive both success and failure.
    ///
    /// First write wins — a node calling this twice keeps the earliest
    /// record, matching the "evidence at time of execution" audit intent.
    pub fn set_run_details(&self, details: NodeRunDetails) {
        let mut slot = self.run_details.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_none() {
            *slot = Some(details);
        }
    }

    /// Record software evidence independently of terminal details and lossy
    /// progress events. Only a backend that verified the actual worker calls
    /// this; configuration pins and advisory heartbeats are not evidence.
    pub fn set_worker_software(&self, evidence: serde_json::Value) {
        let mut slot = self.worker_software.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_none() {
            *slot = Some(evidence);
        }
    }

    pub fn set_worker_stop(&self, evidence: serde_json::Value) {
        let mut slot = self.worker_stop.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_none() {
            *slot = Some(evidence);
        }
    }

    /// Drain the recorded details. Called once by the scheduler after
    /// `execute` resolves (or the dispatch is cancelled).
    pub fn take_run_details(&self) -> Option<NodeRunDetails> {
        let mut details = self.run_details
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(evidence) = self.worker_software.lock().unwrap_or_else(|e| e.into_inner()).take() {
            details.get_or_insert_with(Default::default).worker_software = Some(evidence);
        }
        if let Some(evidence) = self.worker_stop.lock().unwrap_or_else(|e| e.into_inner()).take() {
            details.get_or_insert_with(Default::default).worker_stop = Some(evidence);
        }
        details
    }

    pub fn take_logs(&self) -> Vec<ReportedLog> {
        let mut logs = self.logs.lock().unwrap_or_else(|error| error.into_inner());
        std::mem::take(&mut logs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn worker_software_survives_closed_events_and_terminal_details_on_failure() {
        let reporter = NodeReporter::noop();
        let evidence = serde_json::json!({"worker_kind":"python-apptainer","job_id":"123"});
        reporter.clone().set_worker_software(evidence.clone());
        let stop = serde_json::json!({"job_id":"123","full_subtree_terminal":true});
        reporter.clone().set_worker_stop(stop.clone());
        reporter.set_worker_software(serde_json::json!({"job_id":"foreign"}));
        reporter.set_run_details(NodeRunDetails {exit_code:Some(125), ..Default::default()});
        reporter.set_run_details(NodeRunDetails {exit_code:Some(0), ..Default::default()});
        let details = reporter.take_run_details().unwrap();
        assert_eq!(details.exit_code, Some(125));
        assert_eq!(details.worker_software, Some(evidence));
        assert_eq!(details.worker_stop, Some(stop));
        assert!(reporter.take_run_details().is_none());
    }

    #[test]
    fn worker_software_does_not_invent_terminal_success() {
        let reporter = NodeReporter::noop();
        reporter.set_worker_software(serde_json::json!({"job_id":"123"}));
        let details = reporter.take_run_details().unwrap();
        assert!(details.exit_code.is_none());
        assert!(details.worker_software.is_some());
    }

    #[tokio::test]
    async fn channel_item_emission_applies_backpressure() {
        let (sender, mut receiver) = mpsc::channel::<NodeEvent>(1);
        let reporter = NodeReporter::new("source", sender);
        let first_sent = std::sync::Arc::new(AtomicBool::new(false));
        let producer_first_sent = std::sync::Arc::clone(&first_sent);
        let producer = tokio::spawn(async move {
            reporter
                .emit_channel_item(0, 0, serde_json::json!("first"))
                .await;
            producer_first_sent.store(true, Ordering::SeqCst);
            reporter
                .emit_channel_item(0, 1, serde_json::json!("second"))
                .await;
        });

        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        assert!(first_sent.load(Ordering::SeqCst));
        assert!(
            !producer.is_finished(),
            "second item must wait for capacity"
        );

        let first = receiver.recv().await.unwrap();
        let NodeEventKind::ChannelItem { item, sequence, .. } = first.kind else {
            panic!("expected a channel item event");
        };
        assert_eq!(item, serde_json::json!("first"));
        assert_eq!(sequence, 0);

        tokio::time::timeout(std::time::Duration::from_millis(500), producer)
            .await
            .expect("producer should resume after capacity is released")
            .unwrap();
    }
}
