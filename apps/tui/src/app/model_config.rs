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
        let preset_models = registry::preset_models(&provider_type)?;
        let mut model_info = preset_models
            .into_iter()
            .find(|m| m.model_name == model_name)?;

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

        // Build catalogue from SDK registry + DB credentials.
        *state = crate::widgets::model_config_widget::build_catalog(&db_tuples);

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
                    let agent_name = self
                        .state
                        .sessions
                        .get(self.state.active_agent_idx)
                        .map(|s| s.name.clone());
                    if let Some(an) = agent_name {
                        if let Some(host) = self.host.as_ref() {
                            host.control().set_agent_model(&an, model);
                            tracing::info!(
                                agent = %an,
                                model = %spec,
                                "model hot-swapped for active agent"
                            );
                            self.persist_agent_model(&an, &spec);
                        }
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
        let auth_str =
            match agentik_sdk::provider::registry::default_auth_method(&provider.provider_type) {
                AuthMethod::Bearer => "Bearer",
                AuthMethod::Anthropic => "Anthropic",
            };

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
}
