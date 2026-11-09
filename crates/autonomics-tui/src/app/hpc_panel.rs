//! Compute panel app glue — gateway fetches for the selected target and the
//! job monitor inventory, plus key routing for the panel widget. Async
//! replies come back as [`crate::app_event::AppEvent`] variants applied in
//! `runtime_events`.

use crossterm::event::{KeyCode, KeyEvent};

use super::*;

impl App {
    /// Open the panel and kick off both fetches.
    pub(super) fn open_hpc_panel(&mut self) {
        self.state.hpc_panel.visible = true;
        self.state.hpc_panel.dashboard_ready = true;
        self.state.hpc_panel.tab = crate::widgets::hpc_panel_widget::HpcTab::Targets;
        self.state.hpc_panel.dashboard_focus_form = false;
        self.state.hpc_panel.dashboard_show_jobs = false;
        self.state.hpc_panel.loading = true;
        self.state.hpc_panel.status = None;
        self.fetch_hpc_targets();
        self.fetch_hpc_jobs();
        self.dirty = true;
    }

    fn fetch_hpc_targets(&mut self) {
        self.state.hpc_panel.loading = true;
        let client = self.client.clone();
        let event_tx = self.app_event_tx.clone();
        self.spawn_client_task("hpc_targets_fetch", move || async move {
            let result = client
                .compute_status()
                .await
                .map_err(|error| error.to_string())
                .and_then(|status| serde_json::to_value(status).map_err(|error| error.to_string()));
            event_tx.send(crate::app_event::AppEvent::HpcTargetsLoaded(result));
        });
    }

    fn fetch_hpc_jobs(&mut self) {
        let client = self.client.clone();
        let event_tx = self.app_event_tx.clone();
        self.spawn_client_task("hpc_jobs_fetch", move || async move {
            let reply = client.hpc_jobs().await.unwrap_or_else(
                |error| serde_json::json!({"monitors":[],"error":error.to_string()}),
            );
            event_tx.send(crate::app_event::AppEvent::HpcJobsLoaded(reply));
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
            event_tx.send(crate::app_event::AppEvent::HpcProfileSaved(result));
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
            Some(HpcPanelCommand::SelectTarget(id, revision)) => {
                self.state.hpc_panel.loading = true;
                self.state.hpc_panel.status = Some("Selecting target…".into());
                let client = self.client.clone();
                let event_tx = self.app_event_tx.clone();
                self.spawn_client_task("compute_select", move || async move {
                    let result = client
                        .select_compute_target_by_id(revision, id)
                        .await
                        .map_err(|error| error.to_string())
                        .and_then(|state| {
                            serde_json::to_value(state).map_err(|error| error.to_string())
                        });
                    event_tx.send(crate::app_event::AppEvent::HpcProfileSaved(result));
                });
            }
            Some(HpcPanelCommand::SaveTarget(request)) => {
                self.state.hpc_panel.loading = true;
                self.state.hpc_panel.dashboard_pending_id =
                    request["id"].as_str().map(str::to_owned);
                self.state.hpc_panel.status = Some("Saving target…".into());
                let client = self.client.clone();
                let event_tx = self.app_event_tx.clone();
                self.spawn_client_task("compute_save", move || async move {
                    let result = async {
                        let saved = client.save_compute_target_json(&request).await.map_err(|error| error.to_string())?;
                        let id = request["id"].as_str().unwrap_or_default();
                        let state = if saved.policy.target_id == id {
                            client.select_compute_target_by_id(saved.revision, id.to_string()).await.map_err(|error| format!("Target saved, but activating the new revision failed: {error}"))?
                        } else { saved };
                        serde_json::to_value(state).map_err(|error| error.to_string())
                    }.await;
                    event_tx.send(crate::app_event::AppEvent::HpcProfileSaved(result));
                });
            }
            Some(HpcPanelCommand::RepairWorker(request)) => {
                self.state.hpc_panel.loading = true;
                self.state.hpc_panel.repair_show_events = true;
                self.state.hpc_panel.repair_events.clear();
                self.state.hpc_panel.push_repair_event("[start] 正在恢复连接…".into());
                self.state.hpc_panel.dashboard_pending_id = request["target_id"].as_str().map(str::to_owned);
                self.state.hpc_panel.status = Some("Reconnecting and repairing worker automatically; this may take several minutes…".into());
                let client = self.client.clone();
                let event_tx = self.app_event_tx.clone();
                self.spawn_client_task("compute_repair_worker", move || async move {
                    let result = client.repair_compute_worker_with_progress(&request, |stage, message| {
                        event_tx.send(crate::app_event::AppEvent::HpcRepairProgress(format!("[{stage}] {message}")));
                    }).await
                        .map_err(|error| error.to_string())
                        .and_then(|state| serde_json::to_value(state).map_err(|error| error.to_string()));
                    event_tx.send(crate::app_event::AppEvent::HpcRepairFinished(result));
                });
            }
            Some(HpcPanelCommand::DeleteTarget(id, revision)) => {
                self.state.hpc_panel.loading = true;
                self.state.hpc_panel.status = Some("Deleting target…".into());
                let client = self.client.clone();
                let event_tx = self.app_event_tx.clone();
                self.spawn_client_task("compute_delete", move || async move {
                    let result = client
                        .delete_compute_target(&id, revision)
                        .await
                        .map_err(|error| error.to_string())
                        .and_then(|state| {
                            serde_json::to_value(state).map_err(|error| error.to_string())
                        });
                    event_tx.send(crate::app_event::AppEvent::HpcProfileSaved(result));
                });
            }
            Some(HpcPanelCommand::RefreshSnapshot) => {
                self.state.hpc_panel.status = Some("Reloading gateway snapshot (no connection test)…".into());
                self.fetch_hpc_targets();
                self.fetch_hpc_jobs();
            }
            None => {}
        }
    }

    // ── AppEvent appliers (invoked from runtime_events) ──────────────

    pub(super) fn apply_hpc_save_reply(&mut self, reply: serde_json::Value) {
        self.state.hpc_panel.status = Some("Compute target updated.".into());
        let _ = reply;
        // Reload the selected target and its current registry revision.
        self.fetch_hpc_targets();
        self.dirty = true;
    }

    pub(super) fn apply_hpc_save_failed(&mut self, error: String) {
        self.state.hpc_panel.loading = false;
        self.state.hpc_panel.dashboard_pending_id = None;
        self.state.hpc_panel.status = Some(format!("save failed: {error}"));
        self.dirty = true;
    }
}
