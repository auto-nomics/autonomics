use super::*;

impl App {
    pub(super) fn open_embedding_config(&mut self) {
        self.state.embedding_config_visible = true;
        let client = self.client.clone();
        let tx = self.app_event_tx.clone();
        self.spawn_client_task("load_embedding_config", move || async move {
            let result = client.embedding_config().await.map_err(|e| e.to_string());
            tx.send(crate::app_event::AppEvent::EmbeddingConfigLoaded { result });
        });
    }

    pub(super) fn handle_embedding_config_key(&mut self, key: &KeyEvent) {
        use crate::widgets::embedding_config_widget::EmbeddingConfigAction;
        match self.state.embedding_config_state.handle_key(key) {
            EmbeddingConfigAction::Close => self.state.embedding_config_visible = false,
            EmbeddingConfigAction::Save => self.save_embedding_config(),
            EmbeddingConfigAction::None => {}
        }
    }

    pub(super) fn save_embedding_config(&mut self) {
        let request = self.state.embedding_config_state.request();
        let client = self.client.clone();
        let tx = self.app_event_tx.clone();
        self.spawn_client_task("save_embedding_config", move || async move {
            let result = client
                .save_embedding_config(&request.config)
                .await
                .map_err(|e| e.to_string());
            tx.send(crate::app_event::AppEvent::EmbeddingConfigSaved { result });
        });
    }

    pub(super) fn handle_embedding_config_event(
        &mut self,
        result: std::result::Result<gateway::proto::EmbeddingConfigView, String>,
        saved: bool,
    ) {
        match result {
            Ok(config) => {
                self.state.embedding_config_state.load(config);
                if saved {
                    self.state.embedding_config_visible = false;
                    self.state.toasts.success("Embedding model saved", None);
                }
            }
            Err(error) => {
                self.state
                    .toasts
                    .error("Embedding config failed", Some(error));
            }
        }
    }
}
