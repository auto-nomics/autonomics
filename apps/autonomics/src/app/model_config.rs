//! Model catalog loading and provider configuration — thin-client edition.
//!
//! All state lives in the gateway daemon's app DB; this module only
//! renders the catalog (fetched via `GET /model-config`) and issues
//! writes (`PUT /model-config/...`). The ChatGPT OAuth flows (callback
//! listener, token persistence, startup refresh) moved to the daemon.

use super::*;

impl App {
    /// Load the daemon-side catalogue into [`ModelConfigState`] for the
    /// model config widget to render.
    pub(super) fn load_model_config(
        catalog: &gateway::proto::ModelCatalog,
        state: &mut crate::widgets::model_config_widget::ModelConfigState,
    ) {
        // The widget builds its catalog from the SDK registry (available
        // in-process) + DB credentials + DB-imported remote models.
        let db_tuples: Vec<(String, String, String)> = catalog
            .providers
            .iter()
            .map(|p| {
                (
                    p.provider_type.clone(),
                    p.api_key.clone(),
                    p.base_url.clone(),
                )
            })
            .collect();
        *state = crate::widgets::model_config_widget::build_catalog(&db_tuples, &catalog.models);

        if let Some(spec) = catalog.active_model.as_deref() {
            if let Some((_, model_name)) = spec.split_once(':') {
                state.active_model_name = Some(model_name.to_string());
            }
        }
    }

    /// Persist a display toggle (daemon settings table).
    pub(super) fn persist_display_setting(&self, key: &str, value: bool) {
        let client = self.client.clone();
        let key = key.to_string();
        let value = if value { "1" } else { "0" }.to_string();
        self.spawn_client_task("persist_display_setting", move || async move {
            if let Err(e) = client.put_setting(&key, &value).await {
                tracing::warn!(key = %key, error = %e, "failed to persist display setting");
            }
        });
    }

    pub(super) fn handle_model_config_key(&mut self, key: &KeyEvent) {
        use crate::widgets::model_config_widget::ConfigCommand;

        let cmd = self.state.model_config_state.handle_key(*key);
        match cmd {
            ConfigCommand::Close => {
                self.state.model_config_visible = false;
            }
            ConfigCommand::SaveProvider {
                provider_name,
                api_key,
                base_url,
            } => {
                self.save_provider_config(&provider_name, &api_key, &base_url);
            }
            ConfigCommand::SelectModel {
                provider_name,
                model_name,
            } => {
                let spec = format!("{provider_name}:{model_name}");
                let agent_name = self
                    .state
                    .sessions
                    .get(self.state.active_agent_idx)
                    .map(|s| s.name.clone());
                if let Some(an) = agent_name {
                    // The daemon resolves the spec (attaching the ChatGPT
                    // refresh callback), hot-swaps the agent's model, and
                    // persists the preference in its stored record.
                    tracing::info!(agent = %an, model = %spec, "model hot-swap requested");
                    let client = self.client.clone();
                    let spec_for_task = spec.clone();
                    let an_for_log = an.clone();
                    self.spawn_client_task("set_agent_model", move || async move {
                        if let Err(e) = client.set_agent_model(&an_for_log, &spec_for_task).await {
                            tracing::error!(agent = %an_for_log, model = %spec_for_task, error = %e, "model hot-swap failed");
                        }
                    });
                    self.refresh_agent_model_info(&an);
                }
                self.state.model_config_visible = false;
            }
            ConfigCommand::SetDefaultModel {
                provider_name,
                model_name,
            } => {
                self.set_default_model_for_new_agents(&provider_name, &model_name);
            }
            ConfigCommand::FetchRemoteCatalog {
                provider_name,
                base_url,
            } => {
                self.fetch_remote_model_catalog(&provider_name, &base_url);
            }
            ConfigCommand::StartChatgptLogin { provider_name } => {
                self.start_chatgpt_login(&provider_name);
            }
            ConfigCommand::ReloadCatalog => {
                self.spawn_catalog_reload();
            }
            ConfigCommand::None => {}
        }
    }

    /// Persist and activate the model used when creating agents without a
    /// profile-specific override. Validation and slot updates happen
    /// daemon-side; a `ModelChanged` notice triggers the catalog reload.
    pub(super) fn set_default_model_for_new_agents(
        &mut self,
        provider_name: &str,
        model_name: &str,
    ) {
        let spec = format!("{provider_name}:{model_name}");
        let client = self.client.clone();
        let spec_for_task = spec.clone();
        self.spawn_client_task("set_active_model", move || async move {
            if let Err(e) = client.set_active_model(&spec_for_task).await {
                tracing::warn!(model = %spec_for_task, error = %e, "failed to set default model");
            }
        });
        // Optimistic UI update; the notice-driven reload overwrites it.
        self.state.active_model_spec = Some(spec);
        self.state.active_model_display = None;
        self.state.model_config_state.active_model_name = Some(model_name.to_string());
        self.state.toasts.success(
            "Default model set",
            Some(format!("{provider_name}:{model_name}")),
        );
    }

    /// Insert or update a provider's api_key and base_url (daemon-side).
    fn save_provider_config(&self, provider_name: &str, api_key: &str, base_url: &str) {
        // Look up the provider type in the catalog (client-side registry
        // data); empty base_url means "use the provider default" — the
        // daemon resolves it.
        let provider = self
            .state
            .model_config_state
            .providers
            .iter()
            .find(|p| p.name == provider_name);
        let Some(provider) = provider else {
            tracing::error!("provider not found in catalog: {provider_name}");
            return;
        };
        let provider_type = provider.provider_type.as_str().to_string();
        let base_url = if base_url.is_empty() {
            provider.selected_base_url.clone()
        } else {
            base_url.to_string()
        };
        let request = gateway::proto::SaveProviderRequest {
            name: provider_name.to_string(),
            provider_type,
            base_url,
            api_key: api_key.to_string(),
        };
        let client = self.client.clone();
        let provider_name = provider_name.to_string();
        let tx = self.app_event_tx.clone();
        self.spawn_client_task("save_provider", move || async move {
            let result = client
                .save_provider(&request)
                .await
                .map_err(|e| e.to_string());
            tx.send(crate::app_event::AppEvent::ProviderSaved {
                provider_name,
                result,
            });
        });
    }

    /// 发起 ChatGPT 订阅 OAuth 登录：daemon 绑定本机回调端口并返回
    /// 授权 URL（剪贴板 + 浏览器）；完成结果经 ChatgptLogin notice 回流。
    fn start_chatgpt_login(&mut self, provider_name: &str) {
        let client = self.client.clone();
        let tx = self.app_event_tx.clone();
        let _ = provider_name;
        self.spawn_client_task("chatgpt_login", move || async move {
            match client.start_chatgpt_login().await {
                Ok(url) => {
                    tx.send(crate::app_event::AppEvent::ChatgptLoginUrl(url));
                }
                Err(e) => {
                    tracing::warn!(error = %e, "chatgpt login failed to start");
                    tx.send(crate::app_event::AppEvent::ChatgptLoginCompleted {
                        result: Err(e.to_string()),
                    });
                }
            }
        });
    }

    /// Open OAuth link in default browser automatically
    pub(super) fn open_in_browser(url: &str) -> bool {
        use std::process::{Command, Stdio};
        #[cfg(target_os = "windows")]
        let spawned = Command::new("cmd")
            .args(["/c", "start", "", url])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        #[cfg(target_os = "macos")]
        let spawned = Command::new("open")
            .arg(url)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        #[cfg(all(unix, not(target_os = "macos")))]
        let spawned = Command::new("xdg-open")
            .arg(url)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        spawned.is_ok()
    }

    /// Kick off an async fetch of a provider's live remote model catalogue
    /// on the daemon. The result lands as a `CatalogFetched` notice.
    pub(super) fn fetch_remote_model_catalog(&mut self, provider_name: &str, base_url: &str) {
        use agentik_sdk::provider::registry;

        let provider_type = agentik_sdk::model::ProviderType::from(provider_name);
        if !registry::supports_remote_catalog(&provider_type) {
            tracing::warn!(provider = provider_name, "no remote catalogue support");
            return;
        }
        // openai 的目录端点需要 ChatGPT 登录态 —— daemon 侧校验，缺登录时
        // 通过 notice 回流错误。
        if matches!(provider_type, agentik_sdk::model::ProviderType::Openai) {
            let present = self
                .state
                .model_config_state
                .providers
                .iter()
                .find(|p| p.name == "openai")
                .is_some_and(|p| {
                    p.api_key
                        .as_deref()
                        .is_some_and(|k| k.contains("refresh_token"))
                });
            if !present {
                self.state.toasts.error(
                    "请先登录",
                    Some("openai 模型目录需要 ChatGPT 登录态".into()),
                );
                return;
            }
        }
        let client = self.client.clone();
        let name = provider_name.to_string();
        let base_url = base_url.to_string();
        self.spawn_client_task("fetch_remote_catalog", move || async move {
            if let Err(e) = client.fetch_catalog(&name, &base_url).await {
                tracing::warn!(provider = %name, error = %e, "catalog fetch request failed");
            }
        });
    }
}
