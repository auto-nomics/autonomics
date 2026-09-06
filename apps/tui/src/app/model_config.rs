//! Model catalog loading, selection, and provider persistence.

use super::*;

impl App {
    /// Build a `Model` from the DB settings + built-in catalog.
    ///
    /// Reads `active_model` from the `settings` table (format:
    /// `"provider_name:model_name"`), looks up the model in the SDK registry,
    /// and joins it with provider credentials from the `providers` table.
    /// Returns `None` if no model is configured, credentials are missing,
    /// or any lookup fails — the caller treats that as "start without agent".
    pub(super) fn build_model(conn: &Connection) -> Option<Model> {
        // Read the active model setting.
        let active: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'active_model'",
                [],
                |row| row.get(0),
            )
            .ok()?;

        Self::build_model_from_spec(conn, &active)
    }

    /// Build a `Model` from a `"provider_name:model_name"` spec, using
    /// provider credentials from the DB.
    ///
    /// Returns `None` if the spec is malformed, the provider is not
    /// configured, or the model is not in the built-in catalog.
    pub(super) fn build_model_from_spec(conn: &Connection, spec: &str) -> Option<Model> {
        use agentik_sdk::provider::registry;

        // Parse "provider_name:model_name"
        let (provider_name, model_name) = spec.split_once(':')?;

        // Look up provider credentials + selected base_url from the DB.
        let (api_key, db_base_url): (String, String) = conn
            .query_row(
                "SELECT api_key, base_url FROM providers WHERE name = ?1",
                [provider_name],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .ok()?;

        if api_key.is_empty() {
            return None;
        }

        // Look up model info from the built-in catalog.
        let provider_type = ProviderType::from(provider_name);
        let base_url = if db_base_url.is_empty() {
            registry::default_base_url(&provider_type)
                .unwrap_or("")
                .to_string()
        } else {
            db_base_url
        };
        let auth_method = registry::default_auth_method(&provider_type);
        // Look up model info from the built-in catalog, falling back to the
        // local `models` table for entries imported from a remote catalogue.
        // `unwrap_or_default` (rather than `?`) so Custom providers whose
        // models only exist in the DB also resolve.
        let preset_models = registry::preset_models(&provider_type).unwrap_or_default();
        let mut model_info = preset_models
            .into_iter()
            .find(|m| m.model_name == model_name)
            .or_else(|| {
                crate::config_db::ModelRow::find(conn, provider_name, model_name)
                    .map(|row| row.to_model_info())
            })?;

        let provider_config = ProviderConfig {
            id: Uuid::nil(),
            name: provider_name.to_string(),
            provider_type,
            base_url,
            api_key,
            auth_method,
        };
        model_info.provider_id = provider_config.id;

        Model::new(model_info, &provider_config).ok()
    }

    /// Load the built-in provider catalogue, augmented with DB credentials,
    /// into [`ModelConfigState`] for the model config widget to render.
    pub(super) fn load_model_config(
        conn: &Connection,
        state: &mut crate::widgets::model_config_widget::ModelConfigState,
    ) {
        use crate::config_db::ProviderRow;

        // Read configured providers from DB.
        let providers = ProviderRow::all(conn).unwrap_or_default();
        let db_tuples: Vec<(String, String, String)> = providers
            .into_iter()
            .map(|p| (p.provider_type, p.api_key, p.base_url))
            .collect();

        // Build catalogue from SDK registry + DB credentials + DB-imported
        // remote-catalogue models.
        let db_models = crate::config_db::ModelRow::all(conn).unwrap_or_default();
        *state = crate::widgets::model_config_widget::build_catalog(&db_tuples, &db_models);

        // Load active model name from settings.
        if let Ok(value) = conn.query_row(
            "SELECT value FROM settings WHERE key = 'active_model'",
            [],
            |row| row.get::<_, String>(0),
        ) {
            // value format: "provider_name:model_name"
            if let Some((_, model_name)) = value.split_once(':') {
                state.active_model_name = Some(model_name.to_string());
            }
        }
    }

    /// Persist a display toggle to the `settings` table.
    pub(super) fn persist_display_setting(&self, key: &str, value: bool) {
        let _ = self.conn.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
            rusqlite::params![key, if value { "1" } else { "0" }],
        );
    }
    /// Persist a model spec into the agent's stored record so it survives restarts.
    pub(super) fn persist_agent_model(&mut self, agent_name: &str, model_spec: &str) {
        let Some(storage) = self.host.as_ref().map(|h| h.storage().clone()) else {
            tracing::warn!("no host available for model persistence");
            return;
        };
        let name = agent_name.to_string();
        let spec = model_spec.to_string();
        agentik_core::supervise::spawn_safe_on_drop(
            &self.runtime_handle,
            "persist_agent_model",
            async move {
                // Read the current record.
                let Some(mut record) = storage.get_agent_by_name(&name).await.ok().flatten() else {
                    tracing::warn!(agent = %name, "agent record not found for model persistence");
                    return;
                };
                // Update preferred_model inside config_json.
                if let Some(obj) = record.config_json.as_object_mut() {
                    obj.insert(
                        "preferred_model".to_string(),
                        serde_json::Value::String(spec.clone()),
                    );
                }
                record.last_active = chrono::Utc::now().timestamp_millis();
                // Upsert the updated record.
                if let Err(e) = storage.upsert_agent(record).await {
                    tracing::error!(agent = %name, error = %e, "failed to persist model spec");
                } else {
                    tracing::info!(agent = %name, model = %spec, "model spec persisted to agent record");
                }
            },
        );
    }

    /// 读 provider 行的 token blob（api_key 列存 blob JSON）。
    /// main 分支无 app-config crate，此为 chatgpt_token_blob 的内联等价。
    fn chatgpt_token_blob(
        conn: &Connection,
        provider: &str,
    ) -> Option<agentik_sdk::provider::openai::oauth::TokenBlob> {
        let api_key: String = conn
            .query_row(
                "SELECT api_key FROM providers WHERE name = ?1",
                [provider],
                |row| row.get(0),
            )
            .ok()?;
        agentik_sdk::provider::openai::oauth::TokenBlob::from_json(&api_key).ok()
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
                // Build the model and apply to the active agent via host.
                let spec = format!("{provider_name}:{model_name}");
                if let Some(model) = Self::build_model_from_spec(&self.conn, &spec) {
                    let model = self.attach_chatgpt_refresh_callback(Arc::new(model));
                    let agent_name = self
                        .state
                        .sessions
                        .get(self.state.active_agent_idx)
                        .map(|s| s.name.clone());
                    if let Some(an) = agent_name {
                        if let Some(host) = self.host.as_ref() {
                            host.control().set_agent_model(&an, (*model).clone());
                            tracing::info!(
                                agent = %an,
                                model = %spec,
                                "model hot-swapped for active agent"
                            );
                            self.persist_agent_model(&an, &spec);
                        }
                    }
                } else {
                    // openai + 过期 token 是可恢复的失败：后台刷新，落地
                    // 事件会重建默认槽；提示用户稍后重选。
                    if provider_name == "openai"
                        && Self::chatgpt_token_blob(&self.conn, "openai")
                            .is_some_and(|blob| blob.access_token_expired())
                    {
                        tracing::info!("openai token expired at select time; refreshing");
                        self.spawn_chatgpt_ensure_fresh();
                        self.state.toasts.info(
                            "ChatGPT token 已过期",
                            Some("正在后台刷新，完成后请重新选择模型".into()),
                        );
                    } else {
                        tracing::warn!(model = %spec, "model unbuildable at select time");
                    }
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
                Self::load_model_config(&self.conn, &mut self.state.model_config_state);
            }
            ConfigCommand::None => {}
        }
    }

    /// Persist and activate the model used when creating agents without a
    /// profile-specific override.
    pub(super) fn set_default_model_for_new_agents(
        &mut self,
        provider_name: &str,
        model_name: &str,
    ) {
        let spec = format!("{provider_name}:{model_name}");
        let Some(model) = Self::build_model_from_spec(&self.conn, &spec) else {
            tracing::warn!(model = %spec, "cannot set default model: model unavailable");
            self.state
                .toasts
                .error("Invalid model", Some("Configure its provider first".into()));
            return;
        };

        if let Err(e) = self.conn.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES ('active_model', ?1)",
            rusqlite::params![spec],
        ) {
            tracing::error!(model = %spec, error = %e, "failed to persist default model");
            self.state.toasts.error(
                "Save failed",
                Some("Could not update the default model".into()),
            );
            return;
        }

        // RuntimeHost shares this ArcSwapOption with AppState. New agents
        // spawned without a profile override read the updated value.
        self.state.active_model.store(Some(Arc::new(model)));
        self.state.model_config_state.active_model_name = Some(model_name.to_string());
        tracing::info!(model = %spec, "default model for new agents updated");
        self.state.toasts.success(
            "Default model set",
            Some(format!("{provider_name}:{model_name}")),
        );
    }

    /// Insert or update a provider's api_key and base_url in the database.
    fn save_provider_config(&self, provider_name: &str, api_key: &str, base_url: &str) {
        // Look up the built-in provider to get its type and default base_url.
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
        // Prefer the user-supplied base_url; fall back to the provider default
        // (the registry's first preset endpoint) when the field is empty.
        let base_url = if base_url.is_empty() {
            provider.selected_base_url.clone()
        } else {
            base_url.to_string()
        };
        // Resolve the default auth method from the registry for this provider.
        let auth_str = agentik_sdk::provider::registry::default_auth_method(&provider.provider_type)
            .storage_tag();

        // Check if a row for this provider name already exists.
        let existing: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM providers WHERE name = ?1",
                [provider_name],
                |row| row.get(0),
            )
            .ok();

        let result = if let Some(id) = existing {
            self.conn.execute(
                "UPDATE providers SET api_key = ?1, base_url = ?2, auth_method = ?3 WHERE id = ?4",
                rusqlite::params![api_key, &base_url, auth_str, id],
            )
        } else {
            self.conn.execute(
                "INSERT INTO providers (name, provider_type, base_url, api_key, auth_method)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![provider_name, &provider_type, &base_url, api_key, auth_str],
            )
        };

        match &result {
            Ok(_) => tracing::info!("saved provider config: {provider_name}"),
            Err(e) => tracing::error!("failed to save provider config: {e}"),
        }
    }

    /// 写入 ChatGPT 登录产物：openai 行 api_key = token blob JSON、
    /// auth_method = "chatgpt"、base_url = 默认 ChatGPT 后端。
    pub(super) fn save_chatgpt_provider(
        &self,
        blob: &agentik_sdk::provider::openai::oauth::TokenBlob,
    ) -> Result<(), String> {
        let json = blob.to_json()?;
        let base_url = agentik_sdk::provider::registry::default_base_url(&ProviderType::Openai)
            .unwrap_or_default()
            .to_string();
        let existing: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM providers WHERE name = 'openai'",
                ["openai"],
                |row| row.get(0),
            )
            .ok();
        let result = if let Some(id) = existing {
            self.conn.execute(
                "UPDATE providers SET api_key = ?1, base_url = ?2, auth_method = 'chatgpt' WHERE id = ?3",
                rusqlite::params![json, base_url, id],
            )
        } else {
            self.conn.execute(
                "INSERT INTO providers (name, provider_type, base_url, api_key, auth_method)
                 VALUES ('openai', 'openai', ?1, ?2, 'chatgpt')",
                rusqlite::params![base_url, json],
            )
        };
        result.map(|_| ()).map_err(|e| e.to_string())
    }

    /// 发起 ChatGPT 订阅 OAuth 登录：绑定本机回调端口 → 发
    /// [`AppEvent::ChatgptLoginUrl`]（剪贴板 + 浏览器）→ 后台等待浏览器
    /// 授权并换 token → [`AppEvent::ChatgptLoginCompleted`] 写库重载。
    /// 仿 `fetch_remote_model_catalog` 的异步派发模式。
    fn start_chatgpt_login(&mut self, provider_name: &str) {
        use agentik_sdk::provider::openai::oauth;

        let (url, waiter) = match oauth::login_flow() {
            Ok(pair) => pair,
            Err(e) => {
                tracing::warn!(provider = provider_name, error = %e, "chatgpt login failed to start");
                self.state.toasts.error("登录启动失败", Some(e));
                return;
            }
        };
        tracing::info!(provider = provider_name, "chatgpt login flow started");
        let tx = self.app_event_tx.clone();
        tx.send(crate::app_event::AppEvent::ChatgptLoginUrl(url));
        agentik_core::supervise::spawn_safe_on_drop(
            &self.runtime_handle,
            "chatgpt_login",
            async move {
                let result = waiter.wait().await;
                tx.send(crate::app_event::AppEvent::ChatgptLoginCompleted { result });
            },
        );
    }

    /// 默认模型槽指向 openai 时重建之（登录/刷新前 openai 模型可能不可
    /// 构建）。`RuntimeHost` 与 `AppState` 共享同一 `ArcSwapOption` 槽，
    /// store 即对双方生效。
    pub(super) fn rebuild_openai_default_model(&mut self) {
        let Ok(spec) = self.conn.query_row(
            "SELECT value FROM settings WHERE key = 'active_model'",
            [],
            |row| row.get::<_, String>(0),
        ) else {
            return;
        };
        if !spec.starts_with("openai:") {
            return;
        }
        match Self::build_model_from_spec(&self.conn, &spec) {
            Some(model) => {
                let model = self.attach_chatgpt_refresh_callback(Arc::new(model));
                self.state.active_model.store(Some(model));
                tracing::info!(model = %spec, "openai default model rebuilt after login/refresh");
            }
            None => {
                tracing::warn!(model = %spec, "openai default model still unbuildable");
            }
        }
    }

    /// 给 ChatGPT OAuth 模型挂 token 刷新回调（新 blob JSON → 事件 →
    /// 主循环写库）。非 OAuth 模型原样返回。
    pub(super) fn attach_chatgpt_refresh_callback(
        &self,
        model: Arc<agentik_sdk::model::Model>,
    ) -> Arc<agentik_sdk::model::Model> {
        let tx = self.app_event_tx.clone();
        Arc::new(
            (*model)
                .clone()
                .with_token_refreshed(move |blob_json| {
                    tx.send(crate::app_event::AppEvent::ChatgptTokenRefreshed(blob_json));
                }),
        )
    }

    /// 启动收尾：openai 默认模型补挂 token 刷新回调（App::new 构建模型
    /// 时事件通道尚未就绪），随后做启动期主动刷新检查。
    pub(super) fn on_startup_chatgpt_setup(&mut self) {
        if let Some(model) = self.state.active_model.load_full() {
            if model.is_chatgpt() {
                let model = self.attach_chatgpt_refresh_callback(model);
                self.state.active_model.store(Some(model));
            }
        }
        self.spawn_chatgpt_ensure_fresh();
    }

    /// 启动期/选择期主动刷新：读 openai blob，满足 8 天/24h 条件则后台
    /// `refresh_blob` → [`AppEvent::ChatgptTokenRefreshed`] 落库并重建。
    pub(super) fn spawn_chatgpt_ensure_fresh(&self) {
        let Some(blob) = Self::chatgpt_token_blob(&self.conn, "openai") else {
            return;
        };
        if !blob.should_refresh() {
            return;
        }
        let tx = self.app_event_tx.clone();
        agentik_core::supervise::spawn_safe_on_drop(
            &self.runtime_handle,
            "chatgpt_ensure_fresh",
            async move {
                match agentik_sdk::provider::openai::oauth::refresh_blob(&blob).await {
                    Ok(refreshed) => match refreshed.to_json() {
                        Ok(json) => {
                            tx.send(crate::app_event::AppEvent::ChatgptTokenRefreshed(json));
                        }
                        Err(e) => tracing::warn!(error = %e, "chatgpt ensure-fresh serialize failed"),
                    },
                    Err(e) => {
                        // 非致命：401 自愈与下次启动会再试。
                        tracing::warn!(error = %e, "chatgpt ensure-fresh refresh failed");
                    }
                }
            },
        );
    }

    /// 尽力用系统默认浏览器打开 URL。失败仅返回 `false`（不报错——TUI
    /// 全屏场景的主通路是剪贴板里的授权链接）。子进程 stdio 一律置
    /// null：浏览器从终端拉起时会向继承的 stdout/stderr 狂刷输出，
    /// 把全屏 TUI 刷成乱码。
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
    /// (OpenRouter's public `/v1/models`; openai 的 ChatGPT 后端
    /// `/backend-api/codex/models`，需登录态). The result lands back
    /// on the event loop as [`AppEvent::RemoteCatalogFetched`], which
    /// persists the rows and reloads the catalogue widget.
    pub(super) fn fetch_remote_model_catalog(&mut self, provider_name: &str, base_url: &str) {
        use agentik_sdk::provider::registry;

        let provider_type = ProviderType::from(provider_name);
        if !registry::supports_remote_catalog(&provider_type) {
            tracing::warn!(provider = provider_name, "no remote catalogue support");
            return;
        }
        let url = if base_url.is_empty() {
            registry::default_base_url(&provider_type)
                .unwrap_or("")
                .to_string()
        } else {
            base_url.to_string()
        };
        tracing::info!(provider = provider_name, url = %url, "fetching remote catalogue");

        let name = provider_name.to_string();
        let tx = self.app_event_tx.clone();
        // ChatGPT 订阅的目录端点需要登录态（Bearer + account id）；blob
        // 从库里读，登录/401 自愈后总是最新。
        if matches!(provider_type, ProviderType::Openai) {
            let Some(blob) = Self::chatgpt_token_blob(&self.conn, &name) else {
                self.state.toasts.error(
                    "请先登录",
                    Some("openai 模型目录需要 ChatGPT 登录态".into()),
                );
                return;
            };
            let token = blob.access_token.clone();
            let account = blob.account_id.clone();
            agentik_core::supervise::spawn_safe_on_drop(
                &self.runtime_handle,
                "fetch_remote_catalog",
                async move {
                    let result =
                        agentik_sdk::provider::openai::OpenaiProvider::fetch_remote_catalog(
                            &url, &token, &account,
                        )
                        .await;
                    tx.send(crate::app_event::AppEvent::RemoteCatalogFetched {
                        provider_name: name,
                        result,
                    });
                },
            );
            return;
        }
        agentik_core::supervise::spawn_safe_on_drop(
            &self.runtime_handle,
            "fetch_remote_catalog",
            async move {
                let result =
                    agentik_sdk::provider::openrouter::OpenrouterProvider::fetch_remote_catalog(
                        &url,
                    )
                    .await;
                tx.send(crate::app_event::AppEvent::RemoteCatalogFetched {
                    provider_name: name,
                    result,
                });
            },
        );
    }

    /// Replace the provider's rows in the `models` table with a freshly
    /// fetched remote catalogue. Returns the persisted model count.
    pub(super) fn persist_remote_models(
        &self,
        provider_name: &str,
        models: &[agentik_sdk::model::ModelInfo],
    ) -> rusqlite::Result<usize> {
        let provider_id: i64 = self.conn.query_row(
            "SELECT id FROM providers WHERE name = ?1",
            [provider_name],
            |r| r.get(0),
        )?;
        self.conn
            .execute("DELETE FROM models WHERE provider_id = ?1", [provider_id])?;
        for m in models {
            self.conn.execute(
                "INSERT OR REPLACE INTO models (model_name, provider_id, context_length,
                 max_output_tokens, vision_ability, supports_function_calling, supports_streaming,
                 supports_thinking, thinking_enabled, input_token_price, output_token_price)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
                rusqlite::params![
                    m.model_name,
                    provider_id,
                    m.context_length as i64,
                    m.max_output_tokens as i64,
                    m.vision_ability as i64,
                    m.supports_function_calling as i64,
                    m.supports_streaming as i64,
                    m.supports_thinking as i64,
                    m.thinking_enabled as i64,
                    m.input_token_price,
                    m.output_token_price,
                ],
            )?;
        }
        Ok(models.len())
    }
}
