//! Event fan-out hub: the single point through which the daemon's event
//! stream reaches every connected frontend.
//!
//! The driver loop is the only publisher of agent/host events (plus
//! daemon-side notices from the model store); every SSE connection holds
//! its own broadcast receiver. Hard rule: **publishing never blocks the
//! driver** — a slow consumer lags, gets a `lag` frame, and re-hydrates;
//! it never applies back-pressure to the host pump.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use agentik_types::AgentEvent;
use tokio::sync::broadcast;

use crate::proto::{GatewayNotice, HostEventView, LagFrame};

/// Default ring-buffer capacity (events kept for `Last-Event-ID` replay).
const RING_CAPACITY: usize = 4096;

/// Default broadcast channel capacity per subscriber before lagging.
const BROADCAST_CAPACITY: usize = 1024;

/// One sequenced event ready for the wire.
#[derive(Debug, Clone)]
pub struct Envelope {
    pub seq: u64,
    pub ts_ms: i64,
    pub kind: EventKind,
}

/// Payload of an [`Envelope`].
#[derive(Debug, Clone)]
pub enum EventKind {
    /// An `AgentEvent` tagged with the producing agent's full path.
    Agent {
        agent: String,
        event: AgentEvent,
    },
    Host(HostEventView),
    Notice(GatewayNotice),
}

impl EventKind {
    /// SSE `event:` name for this payload.
    pub fn event_name(&self) -> &'static str {
        match self {
            EventKind::Agent { .. } => "agent",
            EventKind::Host(_) => "host",
            EventKind::Notice(_) => "notice",
        }
    }
}

struct HubCore {
    /// Monotonic sequence counter. Guarded by the same mutex as the ring
    /// so seq assignment, ring insertion, and broadcast dispatch always
    /// happen in the same order — every subscriber sees one total order.
    next_seq: u64,
    ring: VecDeque<Arc<Envelope>>,
}

#[derive(Clone)]
pub struct EventHub {
    core: Arc<Mutex<HubCore>>,
    tx: broadcast::Sender<Arc<Envelope>>,
}

impl Default for EventHub {
    fn default() -> Self {
        Self::new()
    }
}

impl EventHub {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            core: Arc::new(Mutex::new(HubCore {
                next_seq: 0,
                ring: VecDeque::with_capacity(RING_CAPACITY),
            })),
            tx,
        }
    }

    /// Publish one event; returns its assigned seq.
    ///
    /// `broadcast::send` does not block (it drops on a full receiver
    /// queue, which surfaces as `Lagged` to that receiver), so holding
    /// the core mutex across it is safe and preserves total ordering.
    pub fn publish(&self, kind: EventKind) -> u64 {
        let envelope = {
            let mut core = self.core.lock().expect("event hub mutex poisoned");
            core.next_seq += 1;
            let envelope = Arc::new(Envelope {
                seq: core.next_seq,
                ts_ms: chrono::Utc::now().timestamp_millis(),
                kind,
            });
            core.ring.push_back(envelope.clone());
            if core.ring.len() > RING_CAPACITY {
                core.ring.pop_front();
            }
            envelope
        };
        // No receivers yet is fine (send errors are ignored by design).
        let _ = self.tx.send(envelope.clone());
        envelope.seq
    }

    /// Latest published seq.
    pub fn last_seq(&self) -> u64 {
        self.core.lock().expect("event hub mutex poisoned").next_seq
    }

    /// Subscribe to the live stream.
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<Envelope>> {
        self.tx.subscribe()
    }

    /// Events with `seq > after`, in order, from the replay ring.
    /// Returns `(events, complete)`: `complete == false` means `after`
    /// was already evicted from the ring — the caller must emit a `lag`
    /// frame and the client must re-hydrate instead of applying a gap.
    pub fn replay_after(&self, after: u64) -> (Vec<Arc<Envelope>>, bool) {
        let core = self.core.lock().expect("event hub mutex poisoned");
        let oldest = core.ring.front().map(|e| e.seq);
        let events = core
            .ring
            .iter()
            .filter(|e| e.seq > after)
            .cloned()
            .collect();
        // Complete when the ring covers `after` (or is empty and `after`
        // is at/before the current head — nothing was missed).
        let complete = match oldest {
            Some(oldest) => oldest <= after + 1,
            None => core.next_seq <= after,
        };
        (events, complete)
    }
}

/// Render an envelope as an axum SSE event.
pub fn envelope_to_sse(envelope: &Envelope) -> axum::response::sse::Event {
    use axum::response::sse::Event as SseEvent;

    let data = match &envelope.kind {
        EventKind::Agent { agent, event } => serde_json::json!({
            "agent": agent,
            "event": event,
        }),
        EventKind::Host(view) => serde_json::to_value(view).unwrap_or_default(),
        EventKind::Notice(notice) => serde_json::to_value(notice).unwrap_or_default(),
    };
    SseEvent::default()
        .id(envelope.seq.to_string())
        .event(envelope.kind.event_name())
        .data(data.to_string())
}

/// Render a lag notification as an axum SSE event.
pub fn lag_to_sse(lag: LagFrame) -> axum::response::sse::Event {
    use axum::response::sse::Event as SseEvent;

    SseEvent::default()
        .event("lag")
        .data(serde_json::to_string(&lag).unwrap_or_else(|_| "{}".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent_envelope_text(seq: u64, text: &str) -> Arc<Envelope> {
        Arc::new(Envelope {
            seq,
            ts_ms: 0,
            kind: EventKind::Agent {
                agent: "/root/a".into(),
                event: AgentEvent::TextDelta(text.into()),
            },
        })
    }

    #[test]
    fn seq_is_monotonic_across_publishers() {
        let hub = EventHub::new();
        let a = hub.publish(EventKind::Notice(GatewayNotice::ModelChanged {
            spec: None,
            reason: "t".into(),
        }));
        let b = hub.publish(EventKind::Notice(GatewayNotice::ModelChanged {
            spec: None,
            reason: "t".into(),
        }));
        assert_eq!((a, b), (1, 2));
        assert_eq!(hub.last_seq(), 2);
    }

    #[tokio::test]
    async fn subscribers_see_total_order_and_replay() {
        let hub = EventHub::new();
        let mut rx1 = hub.subscribe();
        let mut rx2 = hub.subscribe();
        let s1 = hub.publish(EventKind::Agent {
            agent: "a".into(),
            event: AgentEvent::TextDelta("1".into()),
        });
        let s2 = hub.publish(EventKind::Agent {
            agent: "a".into(),
            event: AgentEvent::TextDelta("2".into()),
        });

        assert_eq!(rx1.recv().await.unwrap().seq, s1);
        assert_eq!(rx1.recv().await.unwrap().seq, s2);
        assert_eq!(rx2.recv().await.unwrap().seq, s1);
        assert_eq!(rx2.recv().await.unwrap().seq, s2);

        // Replay from zero covers everything; from s1 covers only s2.
        let (all, complete) = hub.replay_after(0);
        assert!(complete);
        assert_eq!(all.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![s1, s2]);
        let (tail, complete) = hub.replay_after(s1);
        assert!(complete);
        assert_eq!(tail.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![s2]);
    }

    #[test]
    fn replay_reports_incomplete_when_ring_evicted() {
        let hub = EventHub::new();
        for i in 0..(RING_CAPACITY + 8) {
            hub.publish(EventKind::Agent {
                agent: "a".into(),
                event: AgentEvent::TextDelta(format!("m{i}")),
            });
        }
        // seq 1 was evicted — replaying from 0 must be flagged incomplete.
        let (_, complete) = hub.replay_after(0);
        assert!(!complete);
        // But a recent seq is still fully covered.
        let last = hub.last_seq();
        let (events, complete) = hub.replay_after(last - 1);
        assert!(complete);
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn sse_frame_carries_id_event_data() {
        let envelope = agent_envelope_text(42, "hi");
        let sse = envelope_to_sse(&envelope);
        // axum Event Debug-print includes the fields.
        let rendered = format!("{sse:?}");
        assert!(rendered.contains("42"), "{rendered}");
        assert!(rendered.contains("agent"), "{rendered}");
    }
}
