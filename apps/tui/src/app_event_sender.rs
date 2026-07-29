//! A cloneable handle for sending `AppEvent`s into the main loop's channel.
//!
//! Uses `tokio::sync::mpsc` so the main loop can await events via `tokio::select!`.

use tokio::sync::mpsc::UnboundedSender;

use crate::app_event::AppEvent;

/// Cloneable handle for pushing [`AppEvent`]s into the main loop's channel.
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub(crate) struct AppEventSender {
    pub tx: UnboundedSender<AppEvent>,
}

impl AppEventSender {
    pub fn new(tx: UnboundedSender<AppEvent>) -> Self {
        Self { tx }
    }

    /// Send an event. Errors are logged but not propagated — a disconnected
    /// receiver means the app is shutting down.
    #[allow(dead_code)]
    pub fn send(&self, event: AppEvent) {
        if let Err(e) = self.tx.send(event) {
            tracing::error!("app event send failed (receiver likely dropped): {e}");
        }
    }
}
