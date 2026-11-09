//! Small, independently readable execution projection. Never holds the engine
//! lock, owns outputs, or certifies worker termination/scientific success.

use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

use super::status::RuntimeStatus;
use super::view::DagTuiSnapshot;

pub fn observation_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

/// Phase counters are scoped to one node and phase; worker observations carry
/// their own evidence source and timestamp. A heartbeat is not work progress.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExecutionObservation {
    pub phase: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker: Option<serde_json::Value>,
    #[serde(default)]
    pub observed_at_ms: u64,
    #[serde(default)]
    pub last_work_progress_at_ms: u64,
    #[serde(default)]
    pub phase_started_at_ms: u64,
}

/// The scheduler and reporters write before attempting best-effort streaming.
/// Readers therefore recover the latest state even after dropped SSE events.
#[derive(Debug, Clone, Default)]
pub struct LiveDagView(Arc<RwLock<Option<DagTuiSnapshot>>>);

impl LiveDagView {
    pub fn snapshot(&self) -> Option<DagTuiSnapshot> {
        self.0.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Replace topology/status from the scheduler, retaining current phase
    /// observations on existing nodes (including dynamically expanded nodes).
    pub fn publish(&self, mut snapshot: DagTuiSnapshot) {
        let mut view = self.0.write().unwrap_or_else(|e| e.into_inner());
        if let Some(previous) = view.as_ref() {
            snapshot.run_id = previous.run_id.clone();
            // Linear merge: index prior executions once instead of a linear
            // find per node (which made each publish O(N^2)).
            let prior: std::collections::HashMap<&str, &super::view::DagNodeView> = previous
                .nodes
                .iter()
                .map(|node| (node.id.as_str(), node))
                .collect();
            for node in &mut snapshot.nodes {
                node.execution = prior
                    .get(node.id.as_str())
                    .and_then(|n| n.execution.clone());
            }
        }
        snapshot.observed_at_ms = observation_time_ms();
        *view = Some(snapshot);
    }

    pub fn begin(&self, mut snapshot: DagTuiSnapshot) {
        for node in &mut snapshot.nodes {
            node.status = RuntimeStatus::Pending;
            node.execution = None;
        }
        snapshot.running = true;
        snapshot.observed_at_ms = observation_time_ms();
        *self.0.write().unwrap_or_else(|e| e.into_inner()) = Some(snapshot);
    }

    pub fn status(&self, id: &str, status: RuntimeStatus) {
        if let Some(view) = self.0.write().unwrap_or_else(|e| e.into_inner()).as_mut() {
            if let Some(node) = view.nodes.iter_mut().find(|n| n.id == id) {
                node.status = status;
            }
            view.observed_at_ms = observation_time_ms();
        }
    }

    pub fn observe(&self, id: &str, mut observation: ExecutionObservation) -> ExecutionObservation {
        observation.observed_at_ms = observation_time_ms();
        if let Some(view) = self.0.write().unwrap_or_else(|e| e.into_inner()).as_mut() {
            if let Some(node) = view.nodes.iter_mut().find(|n| n.id == id) {
                // A phase update without a probe must not erase the last probe.
                let previous = node.execution.as_ref();
                if observation.worker.is_none() {
                    observation.worker = previous.and_then(|e| e.worker.clone());
                }
                let same_phase = previous.is_some_and(|e| e.phase == observation.phase);
                let advanced = !same_phase
                    || observation.current.is_some_and(|current| {
                        current > previous.and_then(|e| e.current).unwrap_or(0)
                    });
                observation.last_work_progress_at_ms = if advanced {
                    observation.observed_at_ms
                } else {
                    previous.map(|e| e.last_work_progress_at_ms).unwrap_or(0)
                };
                observation.phase_started_at_ms = if same_phase {
                    previous
                        .map(|e| e.phase_started_at_ms)
                        .unwrap_or(observation.observed_at_ms)
                } else {
                    observation.observed_at_ms
                };
                if same_phase {
                    if let Some(previous) = previous {
                        observation.current = match (observation.current, previous.current) {
                            (Some(current), Some(old)) => Some(current.max(old)),
                            (current, old) => current.or(old),
                        };
                        observation.total = observation.total.or(previous.total);
                        observation.unit = observation.unit.or_else(|| previous.unit.clone());
                        observation.message =
                            observation.message.or_else(|| previous.message.clone());
                    }
                }
                node.execution = Some(observation.clone());
            }
            view.observed_at_ms = observation_time_ms();
        }
        observation
    }

    pub fn bind_run(&self, run_id: &str) {
        if let Some(view) = self.0.write().unwrap_or_else(|e| e.into_inner()).as_mut() {
            view.run_id = Some(run_id.into());
        }
    }

    pub fn end(&self) {
        if let Some(view) = self.0.write().unwrap_or_else(|e| e.into_inner()).as_mut() {
            view.running = false;
            view.observed_at_ms = observation_time_ms();
        }
    }

    /// A dropped or panicking run cannot leave nodes reported as executing.
    pub fn cancel(&self) {
        if let Some(view) = self.0.write().unwrap_or_else(|e| e.into_inner()).as_mut() {
            for node in &mut view.nodes {
                if matches!(
                    node.status,
                    RuntimeStatus::Running | RuntimeStatus::Ready | RuntimeStatus::Pending
                ) {
                    node.status = RuntimeStatus::Cancelled;
                }
            }
            view.running = false;
            view.observed_at_ms = observation_time_ms();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::node_event::{NodeEvent, NodeEventKind, NodeReporter};
    use super::*;

    #[test]
    fn cancel_ends_the_projection_and_marks_unfinished_nodes() {
        let view = view();
        view.status("s2", RuntimeStatus::Running);
        view.cancel();
        let snapshot = view.snapshot().unwrap();
        assert!(!snapshot.running);
        assert_eq!(
            snapshot.node("s2").unwrap().status,
            RuntimeStatus::Cancelled
        );
    }

    fn view() -> LiveDagView {
        let view = LiveDagView::default();
        view.begin(
            serde_json::from_value(serde_json::json!({"nodes":[{
            "id":"s2","kind":"synthetic","status":"pending","dirty":true,
            "inputs":[],"outputs":[]
        }],"edges":[]}))
            .unwrap(),
        );
        view
    }

    #[test]
    fn full_or_disconnected_event_channels_do_not_lose_live_state() {
        let view = view();
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        tx.try_send(NodeEvent::new(
            "noise",
            NodeEventKind::Progress {
                current: 1,
                total: 1,
            },
        ))
        .unwrap();
        let reporter = NodeReporter::new("s2", tx).with_live_view(view.clone());
        reporter.status(RuntimeStatus::Running);
        reporter.phase_progress("staging_input:0", 1024, Some(4096), "bytes");
        let snapshot = view.snapshot().unwrap();
        let node = snapshot.node("s2").unwrap();
        assert_eq!(node.status, RuntimeStatus::Running);
        assert_eq!(node.execution.as_ref().unwrap().current, Some(1024));
        rx.close();
        reporter.phase_progress("staging_input:0", 2048, Some(4096), "bytes");
        assert_eq!(
            view.snapshot()
                .unwrap()
                .node("s2")
                .unwrap()
                .execution
                .as_ref()
                .unwrap()
                .current,
            Some(2048)
        );
    }

    #[test]
    fn late_events_from_previous_invocation_cannot_update_a_new_view() {
        let previous = view();
        let next = view();
        previous.status("s2", RuntimeStatus::Success);
        previous.end();
        assert_eq!(
            next.snapshot().unwrap().node("s2").unwrap().status,
            RuntimeStatus::Pending
        );
        assert!(next.snapshot().unwrap().running);
    }

    #[test]
    fn probe_only_updates_preserve_work_time_and_counters_without_stopped_proof() {
        let view = view();
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let reporter = NodeReporter::new("s2", tx).with_live_view(view.clone());
        reporter.phase_progress("executing", 5, Some(10), "items");
        let before = view
            .snapshot()
            .unwrap()
            .node("s2")
            .unwrap()
            .execution
            .clone()
            .unwrap();
        reporter.worker("executing", "slurm", "42", "running", "scheduler");
        let after = view
            .snapshot()
            .unwrap()
            .node("s2")
            .unwrap()
            .execution
            .clone()
            .unwrap();
        assert_eq!(
            after.last_work_progress_at_ms,
            before.last_work_progress_at_ms
        );
        assert_eq!(after.current, Some(5));
        view.end();
        let ended = view.snapshot().unwrap();
        assert!(!ended.running);
        assert_eq!(
            ended
                .node("s2")
                .unwrap()
                .execution
                .as_ref()
                .unwrap()
                .worker
                .as_ref()
                .unwrap()["state"],
            "running"
        );
    }
}
