//! Daemon-side owner of the app database (`app.db`): provider
//! credentials, model catalogue, and settings. In the gateway model this
//! rusqlite connection exists in exactly one process — the daemon — and
//! frontends read/write it through `GET/PUT /api/v1/model-config`.
//!
//! Also hosts the ChatGPT OAuth flows (login callback listener, startup
//! ensure-fresh, token-rotation persistence) that previously lived inside
//! the TUI: the token refresh callback attached to chatgpt models must
//! write to this DB, so the models it refreshes must be built here.

use std::sync::{Arc, Mutex};

use agentik_sdk::model::Model;
use runtime::model_bootstrap::{
    self, ModelRow, ProviderRow, ensure_app_schema, resolve_active_model, resolve_model_spec,
};
use tokio::sync::Mutex as AsyncMutex;

use crate::hub::{EventHub, EventKind};
use crate::proto::{
    ChatgptLoginInfo, DisplaySettings, GatewayNotice, ModelCatalog, OpenaiTokenState,
    SaveProviderRequest,
};

/// Errors surfaced by the model store.
#[derive(Debug, thiserror::Error)]
pub enum ModelStoreError {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("{0}")]
    Message(String),
}

pub type Result<T> = std::result::Result<T, ModelStoreError>;

/// The ChatGPT subscription provider row name.
const OPENAI_PROVIDER: &str = "openai";

pub struct ModelStore {
    conn: Mutex<rusqlite::Connection>,
    /// Serializes ChatGPT OAuth login flows (the flow binds a local
    /// callback listener, so at most one may be in flight). Held as an
    /// `Arc` so the background completion task can hold an owned guard.
    login_lock: Arc<AsyncMutex<()>>,
}

impl ModelStore {
    /// Open (and schema-ensure) the app DB. The daemon must hold the
    /// instance lock before calling this.
    pub fn open(path: &std::path::Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                ModelStoreError::Message(format!("create {}: {e}", parent.display()))
            })?;
        }
        let conn = rusqlite::Connection::open(path)?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        ensure_app_schema(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            login_lock: Arc::new(AsyncMutex::new(())),
        })
    }

    fn with_conn<T>(&self, f: impl FnOnce(&rusqlite::Connection) -> Result<T>) -> Result<T> {
        let conn = self.conn.lock().expect("model store mutex poisoned");
        f(&conn)
    }

    /// Full catalogue view for `GET /model-config` / hydration.
    pub fn catalog(&self) -> Result<ModelCatalog> {
        self.with_conn(|conn| {
            let providers = ProviderRow::all(conn)?;
            let models = ModelRow::all(conn)?;
            let active_model = model_bootstrap::active_model_spec(conn);
            let openai_token = chatgpt_blob(conn, OPENAI_PROVIDER).map(|blob| OpenaiTokenState {
                present: true,
                expired: blob.access_token_expired(),
            });
            Ok(ModelCatalog {
                providers,
                models,
                active_model,
                openai_token: openai_token.or(Some(OpenaiTokenState {
                    present: false,
                    expired: false,
                })),
            })
        })
    }

    /// Resolve `provider:model` against the DB. `Err` carries a
    /// human-readable reason for the HTTP layer.
    pub fn resolve(&self, spec: &str) -> std::result::Result<Model, String> {
        self.with_conn(|conn| {
            Ok(resolve_model_spec(conn, spec).ok_or_else(|| {
                format!("model `{spec}` unavailable (unknown provider/model or missing API key)")
            }))
        })
        .map_err(|e: ModelStoreError| e.to_string())?
        .map_err(|message: String| message)
    }

    /// Build a model with the ChatGPT token-rotation callback attached:
    /// a refreshed blob JSON is persisted to the openai provider row and a
    /// `ModelChanged` notice is broadcast so frontends reload.
    pub fn resolve_with_refresh_callback(
        self: &Arc<Self>,
        spec: &str,
        hub: &EventHub,
    ) -> std::result::Result<Model, String> {
        let model = self.resolve(spec)?;
        Ok(self.attach_refresh_callback(model, hub))
    }

    /// Attach the token-rotation callback (fires only for OAuth-backed
    /// chatgpt models).
    fn attach_refresh_callback(self: &Arc<Self>, model: Model, hub: &EventHub) -> Model {
        let store = Arc::clone(self);
        let hub = hub.clone();
        model.with_token_refreshed(move |blob_json| {
            match store.save_chatgpt_blob_json(&blob_json) {
                Ok(()) => {
                    tracing::info!("chatgpt token refreshed and persisted by daemon");
                    hub.publish(EventKind::Notice(GatewayNotice::ModelChanged {
                        spec: None,
                        reason: "token_refresh".to_string(),
                    }));
                }
                Err(e) => {
                    tracing::error!(error = %e, "chatgpt token refresh persist failed");
                }
            }
        })
    }

    /// The daemon-startup default model, with the refresh callback
    /// attached when it is a ChatGPT subscription model.
    pub fn active_model(self: &Arc<Self>, hub: &EventHub) -> Option<Model> {
        let built = self
            .with_conn(|conn| Ok(resolve_active_model(conn)))
            .ok()
            .flatten()?;
        if built.is_chatgpt() {
            Some(self.attach_refresh_callback(built, hub))
        } else {
            Some(built)
        }
    }

    /// Validate, persist, and activate the default model for newly
    /// spawned agents. Returns the built model (callback attached) for
    /// the caller to store in the shared model slot; publishes a
    /// `ModelChanged` notice on success.
    pub fn set_active_model(
        self: &Arc<Self>,
        spec: &str,
        hub: &EventHub,
    ) -> std::result::Result<Model, String> {
        let model = self.resolve_with_refresh_callback(spec, hub)?;
        self.with_conn(|conn| {
            conn.execute(
                "INSERT OR REPLACE INTO settings (key, value) VALUES ('active_model', ?1)",
                rusqlite::params![spec],
            )
            .map(|_| ())
            .map_err(ModelStoreError::Db)
        })
        .map_err(|e| e.to_string())?;
        hub.publish(EventKind::Notice(GatewayNotice::ModelChanged {
            spec: Some(spec.to_string()),
            reason: "set_default".to_string(),
        }));
        Ok(model)
    }

    /// Insert or update a provider row (credentials + base URL).
    pub fn save_provider(&self, req: &SaveProviderRequest) -> Result<()> {
        let auth_method = agentik_sdk::provider::registry::default_auth_method(
            &agentik_sdk::model::ProviderType::from(req.provider_type.as_str()),
        )
        .storage_tag()
        .to_string();
        self.with_conn(|conn| {
            let existing: Option<i64> = conn
                .query_row(
                    "SELECT id FROM providers WHERE name = ?1",
                    [&req.name],
                    |row| row.get(0),
                )
                .ok();
            let result = if let Some(id) = existing {
                conn.execute(
                    "UPDATE providers SET api_key = ?1, base_url = ?2, auth_method = ?3 WHERE id = ?4",
                    rusqlite::params![req.api_key, req.base_url, auth_method, id],
                )
            } else {
                conn.execute(
                    "INSERT INTO providers (name, provider_type, base_url, api_key, auth_method)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![req.name, req.provider_type, req.base_url, req.api_key, auth_method],
                )
            };
            result.map(|_| ()).map_err(ModelStoreError::Db)
        })
    }

    /// Persist a ChatGPT login's token blob to the openai provider row.
    pub fn save_chatgpt_blob(
        &self,
        blob: &agentik_sdk::provider::openai::oauth::TokenBlob,
    ) -> std::result::Result<(), String> {
        let json = blob.to_json()?;
        self.save_chatgpt_blob_json(&json)
    }

    pub(crate) fn save_chatgpt_blob_json(
        &self,
        blob_json: &str,
    ) -> std::result::Result<(), String> {
        let base_url = agentik_sdk::provider::registry::default_base_url(
            &agentik_sdk::model::ProviderType::Openai,
        )
        .unwrap_or_default()
        .to_string();
        self.with_conn(|conn| {
            let existing: Option<i64> = conn
                .query_row(
                    "SELECT id FROM providers WHERE name = ?1",
                    [OPENAI_PROVIDER],
                    |row| row.get(0),
                )
                .ok();
            let result = if let Some(id) = existing {
                conn.execute(
                    "UPDATE providers SET api_key = ?1, base_url = ?2, auth_method = 'chatgpt' WHERE id = ?3",
                    rusqlite::params![blob_json, base_url, id],
                )
            } else {
                conn.execute(
                    "INSERT INTO providers (name, provider_type, base_url, api_key, auth_method)
                     VALUES ('openai', 'openai', ?1, ?2, 'chatgpt')",
                    rusqlite::params![base_url, blob_json],
                )
            };
            result.map(|_| ()).map_err(ModelStoreError::Db)
        })
        .map_err(|e: ModelStoreError| e.to_string())
    }

    /// Read the openai ChatGPT token blob, if any.
    pub fn chatgpt_blob(&self) -> Option<agentik_sdk::provider::openai::oauth::TokenBlob> {
        self.with_conn(|conn| Ok(chatgpt_blob(conn, OPENAI_PROVIDER)))
            .ok()
            .flatten()
    }

    /// Startup proactive refresh (8-day/24h rule from the TUI, now living
    /// in the daemon that owns the DB). Fire-and-forget.
    pub fn spawn_ensure_fresh(self: &Arc<Self>, hub: EventHub) {
        let Some(blob) = self.chatgpt_blob() else {
            return;
        };
        if !blob.should_refresh() {
            return;
        }
        let store = Arc::clone(self);
        tokio::spawn(async move {
            match agentik_sdk::provider::openai::oauth::refresh_blob(&blob).await {
                Ok(refreshed) => match store.save_chatgpt_blob(&refreshed) {
                    Ok(()) => {
                        hub.publish(EventKind::Notice(GatewayNotice::ModelChanged {
                            spec: None,
                            reason: "token_refresh".to_string(),
                        }));
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "chatgpt ensure-fresh persist failed");
                    }
                },
                Err(e) => {
                    // Non-fatal: 401 self-healing retries on next startup.
                    tracing::warn!(error = %e, "chatgpt ensure-fresh refresh failed");
                }
            }
        });
    }

    /// Manual refresh (`POST chatgpt/refresh`): refresh now if a blob
    /// exists, regardless of the should_refresh heuristic.
    pub async fn refresh_chatgpt_now(
        self: &Arc<Self>,
        hub: EventHub,
    ) -> std::result::Result<bool, String> {
        let Some(blob) = self.chatgpt_blob() else {
            return Ok(false);
        };
        let refreshed = agentik_sdk::provider::openai::oauth::refresh_blob(&blob).await?;
        self.save_chatgpt_blob(&refreshed)?;
        hub.publish(EventKind::Notice(GatewayNotice::ModelChanged {
            spec: None,
            reason: "token_refresh".to_string(),
        }));
        Ok(true)
    }

    /// Start a ChatGPT OAuth login: binds the local callback listener,
    /// returns the authorize URL immediately, and completes the flow in
    /// the background (persist + notice). The login lock is held for the
    /// whole flow duration by the background task, enforcing
    /// one-listener-at-a-time.
    pub fn start_chatgpt_login(
        self: &Arc<Self>,
        hub: EventHub,
    ) -> std::result::Result<String, String> {
        use agentik_sdk::provider::openai::oauth;

        let guard =
            tokio::sync::Mutex::try_lock_owned(Arc::clone(&self.login_lock)).map_err(|_| {
                "a ChatGPT login flow is already in progress (one callback listener at a time)"
                    .to_string()
            })?;

        let (url, waiter) = oauth::login_flow()?;
        let store = Arc::clone(self);
        let hub = hub.clone();
        tokio::spawn(async move {
            let _guard = guard;
            let result = waiter.wait().await;
            let notice = match result {
                Ok(blob) => {
                    let info = ChatgptLoginInfo {
                        email: blob.email.clone(),
                        plan_type: blob.plan_type.clone(),
                    };
                    match store.save_chatgpt_blob(&blob) {
                        Ok(()) => {
                            hub.publish(EventKind::Notice(GatewayNotice::ModelChanged {
                                spec: None,
                                reason: "chatgpt_login".to_string(),
                            }));
                            GatewayNotice::ChatgptLogin { result: Ok(info) }
                        }
                        Err(e) => GatewayNotice::ChatgptLogin { result: Err(e) },
                    }
                }
                Err(e) => GatewayNotice::ChatgptLogin { result: Err(e) },
            };
            hub.publish(EventKind::Notice(notice));
        });
        Ok(url)
    }

    /// Display settings (`settings` table) for hydration.
    pub fn display_settings(&self) -> DisplaySettings {
        let get = |key: &str| self.get_setting(key).as_deref() == Some("1");
        DisplaySettings {
            collapse_thinking: get("collapse_thinking"),
            collapse_tool_calls: get("collapse_tool_calls"),
            collapse_tool_results: get("collapse_tool_results"),
        }
    }

    pub fn get_setting(&self, key: &str) -> Option<String> {
        self.with_conn(|conn| {
            conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
                row.get::<_, String>(0)
            })
            .map_err(|_| ModelStoreError::Message(String::new()))
        })
        .ok()
    }

    pub fn put_setting(&self, key: &str, value: &str) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
                rusqlite::params![key, value],
            )
            .map(|_| ())
            .map_err(ModelStoreError::Db)
        })
    }

    /// Replace a provider's model rows with a freshly fetched catalogue.
    pub fn persist_remote_models(
        &self,
        provider_name: &str,
        models: &[agentik_sdk::model::ModelInfo],
    ) -> Result<usize> {
        self.with_conn(|conn| {
            let provider_id: i64 = conn.query_row(
                "SELECT id FROM providers WHERE name = ?1",
                [provider_name],
                |r| r.get(0),
            )?;
            conn.execute("DELETE FROM models WHERE provider_id = ?1", [provider_id])?;
            for m in models {
                conn.execute(
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
        })
    }
}

fn chatgpt_blob(
    conn: &rusqlite::Connection,
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
