//! HPC site panel app glue — gateway fetches for the site config and the
//! job monitor inventory, plus key routing for the panel widget. Async
//! replies come back as [`crate::app_event::AppEvent`] variants applied in
//! `runtime_events`.

use crossterm::event::{KeyCode, KeyEvent};

use super::*;

impl App {
    /// Open the panel and kick off both fetches.
    pub(super) fn open_hpc_panel(&mut self) {
        self.state.hpc_panel.visible = true;
        self.state.hpc_panel.loading = true;
        self.state.hpc_panel.status = None;
        self.fetch_hpc_config();
        self.fetch_hpc_jobs();
        self.dirty = true;
    }

    fn fetch_hpc_config(&mut self) {
        let client = self.client.clone();
        let event_tx = self.app_event_tx.clone();
        self.spawn_client_task("hpc_config_fetch", move || async move {
            let result = client.hpc_config().await.map_err(|error| error.to_string());
            let _ = event_tx.send(crate::app_event::AppEvent::HpcConfigLoaded(result));
        });
    }

    fn fetch_hpc_jobs(&mut self) {
        let client = self.client.clone();
        let event_tx = self.app_event_tx.clone();
        self.spawn_client_task("hpc_jobs_fetch", move || async move {
            let reply = client.hpc_jobs().await;
            let _ = event_tx.send(crate::app_event::AppEvent::HpcJobsLoaded(
                reply.unwrap_or_else(
                    |error| serde_json::json!({ "monitors": [], "error": error.to_string() }),
                ),
            ));
        });
    }

    fn save_hpc_config(&mut self, profile: serde_json::Value) {
        let client = self.client.clone();
        let event_tx = self.app_event_tx.clone();
        self.spawn_client_task("hpc_config_save", move || async move {
            let result = client
                .update_hpc_config(&profile)
                .await
                .map_err(|error| error.to_string());
            let _ = event_tx.send(crate::app_event::AppEvent::HpcProfileSaved(result));
        });
    }

    pub(super) fn handle_hpc_panel_key(&mut self, key: &KeyEvent) {
        let command = self
            .state
            .hpc_panel
            .handle_key(KeyEvent::new(key.code, key.modifiers));
        self.dirty = true;
        use crate::widgets::hpc_panel_widget::HpcPanelCommand;
        match command {
            Some(HpcPanelCommand::Close) => {
                self.state.hpc_panel.visible = false;
            }
            Some(HpcPanelCommand::SaveProfile(profile)) => {
                self.state.hpc_panel.status = Some("Saving…".into());
                self.save_hpc_config(profile);
            }
            Some(HpcPanelCommand::RefreshJobs) => {
                self.state.hpc_panel.status = Some("Refreshing…".into());
                self.fetch_hpc_config();
                self.fetch_hpc_jobs();
            }
            None => {}
        }
    }

    // ── AppEvent appliers (invoked from runtime_events) ──────────────

    pub(super) fn apply_hpc_config_reply(&mut self, reply: serde_json::Value) {
        self.state.hpc_panel.loading = false;
        self.state.hpc_panel.load_config(&reply);
        self.dirty = true;
    }

    pub(super) fn apply_hpc_config_failed(&mut self, error: String) {
        self.state.hpc_panel.loading = false;
        self.state.hpc_panel.status = Some(format!("config: {error}"));
        self.dirty = true;
    }

    pub(super) fn apply_hpc_jobs_reply(&mut self, reply: serde_json::Value) {
        self.state.hpc_panel.load_jobs(&reply);
        if let Some(error) = reply.get("error").and_then(serde_json::Value::as_str) {
            self.state.hpc_panel.status = Some(error.to_string());
        }
        self.dirty = true;
    }

    pub(super) fn apply_hpc_save_reply(&mut self, reply: serde_json::Value) {
        self.state.hpc_panel.status = Some("Saved — restart the daemon to apply.".into());
        let _ = reply;
        // Re-fetch so the pending-restart banner reflects the new file.
        self.fetch_hpc_config();
        self.dirty = true;
    }

    pub(super) fn apply_hpc_save_failed(&mut self, error: String) {
        self.state.hpc_panel.status = Some(format!("save failed: {error}"));
        self.dirty = true;
    }
}
