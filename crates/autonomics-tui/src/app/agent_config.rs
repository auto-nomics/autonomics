//! Per-agent runtime configuration transport and key dispatch.

use super::*;

impl App {
    pub(super) fn open_agent_config(&mut self) {
        let Some(agent) = self
            .state
            .sessions
            .get(self.state.active_agent_idx)
            .map(|s| s.name.clone())
        else {
            return;
        };

        self.state.agent_config.open(agent.clone());
        let client = self.client.clone();
        let tx = self.app_event_tx.clone();
        self.spawn_client_task("load_agent_config", move || async move {
            let result = client
                .agent_runtime_config(&agent)
                .await
                .map_err(|e| e.to_string());
            tx.send(crate::app_event::AppEvent::AgentConfigLoaded { agent, result });
        });
    }

    pub(super) fn handle_agent_config_key(&mut self, key: &KeyEvent) {
        match self.state.agent_config.handle_key(*key) {
            crate::widgets::agent_config_widget::AgentConfigCommand::Close => {
                self.state.agent_config.visible = false;
            }
            crate::widgets::agent_config_widget::AgentConfigCommand::Save { runtime } => {
                let Some(agent) = self
                    .state
                    .sessions
                    .get(self.state.active_agent_idx)
                    .map(|s| s.name.clone())
                else {
                    return;
                };
                self.state.agent_config.saving = true;
                let client = self.client.clone();
                let tx = self.app_event_tx.clone();
                self.spawn_client_task("save_agent_config", move || async move {
                    let result = client
                        .set_agent_runtime_config(&agent, runtime)
                        .await
                        .map_err(|e| e.to_string());
                    tx.send(crate::app_event::AppEvent::AgentConfigSaved { agent, result });
                });
            }
            crate::widgets::agent_config_widget::AgentConfigCommand::None => {}
        }
    }
}
