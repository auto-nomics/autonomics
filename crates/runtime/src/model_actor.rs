//! Model-management actor: the daemon-side owner of the app database,
//! the default model slot, and the ChatGPT OAuth flows.
//!
//! One dedicated worker task owns the [`ModelStorage`] handle (SQLite
//! today — the trait keeps the backend swappable); every consumer talks
//! to it through [`ModelHandle`], a cheap clonable command sender, built
//! by [`start`] on the worker side (same shape as the skill crate's
//! evolution control service: `start` → worker task → handle).
//!
//! Consequences of the mailbox:
//!
//! - the rusqlite connection lives on exactly one task — no shared
//!   mutex, no `block_in_place` at call sites (the storage's own sync
//!   work is bounded point queries inside this dedicated task);
//! - multi-step operations (resolve → persist → swap slot → publish)
//!   are atomic per mailbox turn, so `set_active_model` can no longer
//!   interleave with itself;
//! - ChatGPT token writes all funnel back through the mailbox
//!   (`PersistChatgptBlob`), including the SDK's 401-self-heal callback,
//!   so the worker remains the single writer;
//! - network I/O (OAuth login wait, token refresh, remote catalogue
//!   fetch) never blocks the loop: the worker spawns a task and the task
//!   reports back as a command.
//!
//! Outbound [`ModelEvent`]s (model changed, login finished, catalogue
//! fetched) flow through the unbounded channel the caller hands to
//! [`start`] — this module stays hub-agnostic; the daemon bridges them
//! onto its event hub.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use agentik_sdk::model::{Model, ModelInfo, ProviderType};
use agentik_sdk::provider::ollama::OllamaProvider;
use agentik_sdk::provider::openai::OpenaiProvider;
use agentik_sdk::provider::openai::oauth::{self, TokenBlob};
use agentik_sdk::provider::openrouter::OpenrouterProvider;
use arc_swap::ArcSwapOption;
use tokio::sync::{mpsc, oneshot};

use crate::model_bootstrap::{
    DeleteModelParams, ModelStorage, ModelStorageError, SaveModelParams, SaveProviderParams,
    StorageResult, resolve_from_parts,
};

// ─────────────────────────────────────────────────────────────────────
// Events (outbound)
// ─────────────────────────────────────────────────────────────────────

/// Daemon-side model events. The embedder bridges these onto its own
/// notification surface (the gateway maps them 1:1 onto `GatewayNotice`).
#[derive(Debug, Clone)]
pub enum ModelEvent {
    /// The default model changed (activated, re-set, or token-refreshed).
    ModelChanged {
        spec: Option<String>,
        reason: String,
    },
    /// A ChatGPT OAuth login flow finished.
    ChatgptLogin {
        result: std::result::Result<LoginSuccess, String>,
    },
    /// A remote catalogue fetch + persist finished.
    CatalogFetched {
        provider: String,
        result: std::result::Result<usize, String>,
    },
}

/// Successful ChatGPT login details (email/plan for the UI).
#[derive(Debug, Clone)]
pub struct LoginSuccess {
    pub email: Option<String>,
    pub plan_type: Option<String>,
}

/// Daemon-side view types (the wire mirrors live in the gateway's proto).
#[derive(Debug, Clone, Default)]
pub struct ModelCatalogData {
    pub providers: Vec<crate::model_bootstrap::ProviderRow>,
    pub models: Vec<crate::model_bootstrap::ModelRow>,
    /// Active default model as `provider_name:model_name`.
    pub active_model: Option<String>,
    /// Health of the openai ChatGPT subscription token, when present.
    pub openai_token: Option<OpenaiTokenHealth>,
}

/// Application-owned Ollama embedding settings. The persisted JSON is
/// opaque to the settings table; this is its typed read/write view.
#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddingConfigData(pub text_embedding::EmbeddingConfig);

/// Health of the stored ChatGPT token.
#[derive(Debug, Clone, Copy)]
pub struct OpenaiTokenHealth {
    pub present: bool,
    pub expired: bool,
}

/// Display toggle snapshot read from the `settings` table.
#[derive(Debug, Clone, Copy, Default)]
pub struct DisplaySettings {
    pub collapse_thinking: bool,
    pub collapse_tool_calls: bool,
    pub collapse_tool_results: bool,
}

/// Settings-table key for the Ollama embedding JSON document.
pub const EMBEDDING_CONFIG_KEY: &str = "embedding_config";

fn validate_embedding_config(config: &text_embedding::EmbeddingConfig) -> Result<(), String> {
    if config.enabled && (config.base_url.trim().is_empty() || config.model.trim().is_empty()) {
        return Err("Ollama base URL and model must not be empty".into());
    }
    if config.batch_size == 0 {
        return Err("Ollama batch size must be greater than zero".into());
    }
    if config.timeout_secs == 0 {
        return Err("Ollama timeout must be greater than zero".into());
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────
// Commands (inbound, private)
// ─────────────────────────────────────────────────────────────────────

/// One mailbox message. Storage-touching requests carry a oneshot reply;
/// background completions (refresh, login, catalogue fetch, OAuth
/// callback) are reply-less.
enum ModelCommand {
    Catalog {
        reply: oneshot::Sender<StorageResult<ModelCatalogData>>,
    },
    Resolve {
        spec: String,
        reply: oneshot::Sender<std::result::Result<Model, String>>,
    },
    SetActiveModel {
        spec: String,
        reply: oneshot::Sender<std::result::Result<Model, String>>,
    },
    ActivateDefault {
        reply: oneshot::Sender<Option<Model>>,
    },
    SaveProvider {
        params: SaveProviderParams,
        reply: oneshot::Sender<StorageResult<()>>,
    },
    SaveModel {
        params: SaveModelParams,
        reply: oneshot::Sender<StorageResult<()>>,
    },
    DeleteModel {
        params: DeleteModelParams,
        reply: oneshot::Sender<StorageResult<()>>,
    },
    PutSetting {
        key: String,
        value: String,
        reply: oneshot::Sender<StorageResult<()>>,
    },
    DisplaySettings {
        reply: oneshot::Sender<DisplaySettings>,
    },
    EmbeddingConfig {
        reply: oneshot::Sender<std::result::Result<EmbeddingConfigData, String>>,
    },
    SaveEmbeddingConfig {
        config: EmbeddingConfigData,
        reply: oneshot::Sender<std::result::Result<EmbeddingConfigData, String>>,
    },
    StartLogin {
        reply: oneshot::Sender<std::result::Result<String, String>>,
    },
    RefreshNow {
        reply: oneshot::Sender<std::result::Result<bool, String>>,
    },
    EnsureFresh,
    FetchRemoteCatalog {
        provider: String,
        base_url: String,
    },
    /// From the SDK's 401-self-heal callback: persist a refreshed blob.
    PersistChatgptBlob {
        json: String,
        reason: &'static str,
    },
    /// Background OAuth login waiter finished.
    LoginCompleted {
        result: std::result::Result<TokenBlob, String>,
    },
    /// Background token refresh finished.
    RefreshCompleted {
        result: std::result::Result<TokenBlob, String>,
        reply: Option<oneshot::Sender<std::result::Result<bool, String>>>,
    },
    /// Background remote-catalogue fetch finished.
    RemoteCatalogFetched {
        provider: String,
        result: std::result::Result<Vec<ModelInfo>, String>,
    },
}

// ─────────────────────────────────────────────────────────────────────
// Handle
// ─────────────────────────────────────────────────────────────────────

/// Cheap, cloneable handle for every model-management operation in the
/// daemon. Built by [`start`] — the worker side owns the state; the
/// handle is a typed sender plus the default-model slot for lock-free
/// reads.
#[derive(Clone)]
pub struct ModelHandle {
    tx: mpsc::Sender<ModelCommand>,
    dropped: Arc<AtomicU64>,
    /// The daemon-wide default model slot. The worker is the sole
    /// writer; everyone else reads it lock-free (every LLM request
    /// snapshots it).
    slot: Arc<ArcSwapOption<Model>>,
}

impl ModelHandle {
    fn new(
        tx: mpsc::Sender<ModelCommand>,
        dropped: Arc<AtomicU64>,
        slot: Arc<ArcSwapOption<Model>>,
    ) -> Self {
        Self { tx, dropped, slot }
    }
    /// The default model slot shared with every agent spawned without a
    /// model-specific override.
    pub fn model_slot(&self) -> Arc<ArcSwapOption<Model>> {
        Arc::clone(&self.slot)
    }

    /// Fire-and-forget commands dropped so far because the channel was
    /// full or the worker had exited — an observability signal, not an
    /// error.
    pub fn dropped_commands(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Full catalogue view for `GET /model-config` / hydration.
    pub async fn catalog(&self) -> StorageResult<ModelCatalogData> {
        self.call(|reply| ModelCommand::Catalog { reply })
            .await
            .ok_or_else(|| ModelStorageError::Message(WORKER_GONE.to_string()))?
    }

    /// Resolve `provider:model` against the storage. `Err` carries a
    /// human-readable reason for the HTTP layer. The returned model has
    /// the ChatGPT token-rotation callback attached when applicable.
    pub async fn resolve(&self, spec: &str) -> std::result::Result<Model, String> {
        self.call(|reply| ModelCommand::Resolve {
            spec: spec.to_string(),
            reply,
        })
        .await
        .ok_or_else(|| WORKER_GONE.to_string())?
    }

    /// Validate, persist, and activate the default model for newly
    /// spawned agents — atomically in one mailbox turn. Publishes a
    /// `ModelChanged` event on success.
    pub async fn set_active_model(&self, spec: &str) -> std::result::Result<Model, String> {
        self.call(|reply| ModelCommand::SetActiveModel {
            spec: spec.to_string(),
            reply,
        })
        .await
        .ok_or_else(|| WORKER_GONE.to_string())?
    }

    /// Resolve and install the daemon-startup default model from the
    /// settings table. Returns the built model, or `None` when no model
    /// is configured / resolvable (callers treat that as "run without a
    /// model"). No event is published — this runs before the HTTP
    /// server exists.
    pub async fn activate_default(&self) -> Option<Model> {
        self.call(|reply| ModelCommand::ActivateDefault { reply })
            .await
            .flatten()
    }

    /// Insert or update a provider row (credentials + base URL).
    pub async fn save_provider(&self, params: &SaveProviderParams) -> StorageResult<()> {
        self.call(|reply| ModelCommand::SaveProvider {
            params: params.clone(),
            reply,
        })
        .await
        .ok_or_else(|| ModelStorageError::Message(WORKER_GONE.to_string()))?
    }

    /// Insert or update a custom model row. Built-in SDK presets are never
    /// accepted here: edits must address an existing storage row.
    pub async fn save_model(&self, params: &SaveModelParams) -> StorageResult<()> {
        self.call(|reply| ModelCommand::SaveModel {
            params: params.clone(),
            reply,
        })
        .await
        .ok_or_else(|| ModelStorageError::Message(WORKER_GONE.to_string()))?
    }

    /// Remove a custom model row. The storage layer refuses built-in
    /// presets (`is_custom = 0`) so this method is safe to expose to any
    /// caller; the frontend still gates by `model.custom` to avoid an
    /// extra round trip.
    pub async fn delete_model(&self, params: &DeleteModelParams) -> StorageResult<()> {
        self.call(|reply| ModelCommand::DeleteModel {
            params: params.clone(),
            reply,
        })
        .await
        .ok_or_else(|| ModelStorageError::Message(WORKER_GONE.to_string()))?
    }

    pub async fn put_setting(&self, key: &str, value: &str) -> StorageResult<()> {
        self.call(|reply| ModelCommand::PutSetting {
            key: key.to_string(),
            value: value.to_string(),
            reply,
        })
        .await
        .ok_or_else(|| ModelStorageError::Message(WORKER_GONE.to_string()))?
    }

    /// Display settings (`settings` table) for hydration.
    pub async fn display_settings(&self) -> DisplaySettings {
        self.call(|reply| ModelCommand::DisplaySettings { reply })
            .await
            .unwrap_or_default()
    }

    /// Typed Ollama embedding configuration from `settings`.
    pub async fn embedding_config(&self) -> std::result::Result<EmbeddingConfigData, String> {
        self.call(|reply| ModelCommand::EmbeddingConfig { reply })
            .await
            .ok_or_else(|| WORKER_GONE.to_string())?
    }

    /// Persist and return Ollama embedding configuration.
    pub async fn save_embedding_config(
        &self,
        config: text_embedding::EmbeddingConfig,
    ) -> std::result::Result<EmbeddingConfigData, String> {
        self.call(|reply| ModelCommand::SaveEmbeddingConfig {
            config: EmbeddingConfigData(config),
            reply,
        })
        .await
        .ok_or_else(|| WORKER_GONE.to_string())?
    }

    /// Start a ChatGPT OAuth login: binds the local callback listener and
    /// returns the authorize URL immediately; the flow completes in the
    /// background (persist + event). One flow at a time — the mailbox
    /// serializes this against `login_in_flight`.
    pub async fn start_chatgpt_login(&self) -> std::result::Result<String, String> {
        self.call(|reply| ModelCommand::StartLogin { reply })
            .await
            .ok_or_else(|| WORKER_GONE.to_string())?
    }

    /// Manual refresh (`POST chatgpt/refresh`): refresh now if a blob
    /// exists, regardless of the should_refresh heuristic.
    pub async fn refresh_chatgpt_now(&self) -> std::result::Result<bool, String> {
        self.call(|reply| ModelCommand::RefreshNow { reply })
            .await
            .ok_or_else(|| WORKER_GONE.to_string())?
    }

    /// Startup proactive refresh (8-day/24h rule). Fire-and-forget —
    /// returns false when the command was dropped (worker busy/gone;
    /// counted in [`Self::dropped_commands`]).
    pub fn ensure_fresh(&self) -> bool {
        self.fire(ModelCommand::EnsureFresh)
    }

    /// Fetch a provider's remote catalogue and persist it — fire-and-
    /// forget (the HTTP handler answers 202); completion arrives as a
    /// `CatalogFetched` event. Returns false when the command was
    /// dropped.
    pub fn fetch_remote_catalog(&self, provider: String, base_url: String) -> bool {
        self.fire(ModelCommand::FetchRemoteCatalog { provider, base_url })
    }

    async fn call<R>(&self, build: impl FnOnce(oneshot::Sender<R>) -> ModelCommand) -> Option<R> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx.send(build(reply_tx)).await.ok()?;
        reply_rx.await.ok()
    }

    fn fire(&self, cmd: ModelCommand) -> bool {
        match self.tx.try_send(cmd) {
            Ok(()) => true,
            Err(_) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    }
}

/// Reply-carrying call failed because the worker task is gone.
const WORKER_GONE: &str = "model worker is not running";

// ─────────────────────────────────────────────────────────────────────
// Worker
// ─────────────────────────────────────────────────────────────────────

/// Start the model worker over `storage`: builds the mailbox, spawns the
/// worker task, and returns the control handle (skill-crate evolution
/// service shape). `ModelEvent`s flow out through `events`.
///
/// The startup default model is NOT loaded here — call
/// [`ModelHandle::activate_default`] once, before any consumer observes
/// the slot.
pub fn start(
    storage: Arc<dyn ModelStorage>,
    events: mpsc::UnboundedSender<ModelEvent>,
) -> ModelHandle {
    let slot = Arc::new(ArcSwapOption::empty());
    start_with_model_slot(storage, events, slot)
}

/// Start the model worker while publishing to a caller-owned slot. The KMS
/// worker owns such a slot before the model actor exists, so both must share
/// one installation point.
pub fn start_with_model_slot(
    storage: Arc<dyn ModelStorage>,
    events: mpsc::UnboundedSender<ModelEvent>,
    slot: Arc<ArcSwapOption<Model>>,
) -> ModelHandle {
    // Capacity 64: model-config traffic is human-paced; a bounded
    // channel surfaces a stuck worker as dropped commands instead of
    // unbounded memory.
    let (tx, rx) = mpsc::channel::<ModelCommand>(64);
    let worker = ModelWorker {
        tx: tx.clone(),
        storage,
        events,
        slot: Arc::clone(&slot),
        login_in_flight: false,
    };
    tokio::spawn(worker.run(rx));
    ModelHandle::new(tx, Arc::new(AtomicU64::new(0)), slot)
}

/// The worker itself. All fields are owned by the worker task.
struct ModelWorker {
    tx: mpsc::Sender<ModelCommand>,
    storage: Arc<dyn ModelStorage>,
    events: mpsc::UnboundedSender<ModelEvent>,
    slot: Arc<ArcSwapOption<Model>>,
    login_in_flight: bool,
}

impl ModelWorker {
    async fn run(mut self, mut rx: mpsc::Receiver<ModelCommand>) {
        while let Some(cmd) = rx.recv().await {
            self.handle(cmd).await;
        }
        tracing::debug!("model worker stopped: last handle dropped");
    }

    async fn handle(&mut self, cmd: ModelCommand) {
        match cmd {
            ModelCommand::Catalog { reply } => {
                let _ = reply.send(self.catalog().await);
            }
            ModelCommand::Resolve { spec, reply } => {
                let _ = reply.send(self.resolve(&spec).await);
            }
            ModelCommand::SetActiveModel { spec, reply } => {
                let _ = reply.send(self.set_active_model(&spec).await);
            }
            ModelCommand::ActivateDefault { reply } => {
                let _ = reply.send(self.activate_default().await);
            }
            ModelCommand::SaveProvider { params, reply } => {
                let _ = reply.send(self.storage.save_provider(&params).await);
            }
            ModelCommand::SaveModel { params, reply } => {
                let _ = reply.send(self.storage.save_model(&params).await);
            }
            ModelCommand::DeleteModel { params, reply } => {
                let _ = reply.send(self.storage.delete_model(&params).await);
            }
            ModelCommand::PutSetting { key, value, reply } => {
                let _ = reply.send(self.storage.put_setting(&key, &value).await);
            }
            ModelCommand::DisplaySettings { reply } => {
                let _ = reply.send(self.display_settings().await);
            }
            ModelCommand::EmbeddingConfig { reply } => {
                let _ = reply.send(self.embedding_config().await);
            }
            ModelCommand::SaveEmbeddingConfig { config, reply } => {
                let _ = reply.send(self.save_embedding_config(config.0).await);
            }
            ModelCommand::StartLogin { reply } => {
                let _ = reply.send(self.start_login().await);
            }
            ModelCommand::RefreshNow { reply } => {
                self.refresh_now(reply).await;
            }
            ModelCommand::EnsureFresh => self.ensure_fresh().await,
            ModelCommand::FetchRemoteCatalog { provider, base_url } => {
                self.fetch_remote_catalog(provider, base_url).await;
            }
            ModelCommand::PersistChatgptBlob { json, reason } => {
                self.persist_chatgpt_blob(&json, reason).await;
            }
            ModelCommand::LoginCompleted { result } => {
                self.login_completed(result).await;
            }
            ModelCommand::RefreshCompleted { result, reply } => {
                self.refresh_completed(result, reply).await;
            }
            ModelCommand::RemoteCatalogFetched { provider, result } => {
                self.remote_catalog_fetched(provider, result).await;
            }
        }
    }

    fn publish(&self, event: ModelEvent) {
        // Unbounded: the embedder's bridge is a cheap re-map onto its
        // own hub; never block the worker on observability fan-out.
        let _ = self.events.send(event);
    }

    // ── resolution ──────────────────────────────────────────────────

    fn unavailable(spec: &str) -> String {
        format!("model `{spec}` unavailable (unknown provider/model or missing API key)")
    }

    /// Resolve a spec against storage and attach the token-rotation
    /// callback. Uniform across every path (startup default, spawn
    /// override, per-agent hot-swap) so OAuth-backed models always
    /// persist refreshed tokens.
    async fn resolve(&self, spec: &str) -> std::result::Result<Model, String> {
        // Model names may contain colons; the provider name must not, so
        // split on the LAST colon.
        let Some((provider_name, model_name)) = spec.rsplit_once(':') else {
            return Err(Self::unavailable(spec));
        };
        let auth = self
            .storage
            .provider_auth(provider_name)
            .await
            .map_err(|e: ModelStorageError| e.to_string())?
            .ok_or_else(|| Self::unavailable(spec))?;
        let db_model = self
            .storage
            .find_model(provider_name, model_name)
            .await
            .map_err(|e: ModelStorageError| e.to_string())?;
        resolve_from_parts(provider_name, model_name, &auth, db_model)
            .map(|model| self.attach_refresh_callback(model))
            .ok_or_else(|| Self::unavailable(spec))
    }

    /// Attach the ChatGPT token-rotation callback: a refreshed blob JSON
    /// is routed back through the mailbox for persistence + event (a
    /// no-op on non-OAuth models).
    fn attach_refresh_callback(&self, model: Model) -> Model {
        let tx = self.tx.clone();
        model.with_token_refreshed(move |blob_json| {
            if let Err(e) = tx.try_send(ModelCommand::PersistChatgptBlob {
                json: blob_json,
                reason: "token_refresh",
            }) {
                tracing::error!(
                    error = %e,
                    "model worker busy or gone; chatgpt token refresh not persisted"
                );
            }
        })
    }

    async fn set_active_model(&self, spec: &str) -> std::result::Result<Model, String> {
        let model = self.resolve(spec).await?;
        self.storage
            .set_active_model(spec)
            .await
            .map_err(|e: ModelStorageError| e.to_string())?;
        // Slot first, event second: a frontend reloading on the event
        // always observes the new default already installed.
        self.slot.store(Some(Arc::new(model.clone())));
        self.publish(ModelEvent::ModelChanged {
            spec: Some(spec.to_string()),
            reason: "set_default".to_string(),
        });
        Ok(model)
    }

    async fn activate_default(&self) -> Option<Model> {
        let spec = self.storage.active_model_spec().await.ok()??;
        match self.resolve(&spec).await {
            Ok(model) => {
                self.slot.store(Some(Arc::new(model.clone())));
                Some(model)
            }
            // Unresolvable startup model: run without a model, as before.
            Err(e) => {
                tracing::warn!(spec = %spec, error = %e, "startup default model unresolvable");
                None
            }
        }
    }

    // ── catalogue + settings ────────────────────────────────────────

    async fn catalog(&self) -> StorageResult<ModelCatalogData> {
        let providers = self.storage.providers().await?;
        let models = self.storage.models().await?;
        // Thin clients (GUI) have no SDK registry; fold the built-in preset
        // catalogues in here so every consumer sees them. DB rows win.
        let (providers, models) =
            crate::model_bootstrap::catalog_with_sdk_presets(providers, models);
        let active_model = self.storage.active_model_spec().await?;
        let openai_token = match self.storage.chatgpt_blob_json().await? {
            Some(json) => TokenBlob::from_json(&json)
                .ok()
                .map(|blob| OpenaiTokenHealth {
                    present: true,
                    expired: blob.access_token_expired(),
                }),
            None => None,
        };
        Ok(ModelCatalogData {
            providers,
            models,
            active_model,
            openai_token,
        })
    }

    async fn display_settings(&self) -> DisplaySettings {
        let get = |key: &str| {
            let key = key.to_string();
            async move {
                matches!(
                    self.storage.get_setting(&key).await,
                    Ok(Some(value)) if value == "1"
                )
            }
        };
        DisplaySettings {
            collapse_thinking: get("collapse_thinking").await,
            collapse_tool_calls: get("collapse_tool_calls").await,
            collapse_tool_results: get("collapse_tool_results").await,
        }
    }

    async fn embedding_config(&self) -> std::result::Result<EmbeddingConfigData, String> {
        match self.storage.get_setting(EMBEDDING_CONFIG_KEY).await {
            Ok(Some(json)) => serde_json::from_str(&json)
                .map(EmbeddingConfigData)
                .map_err(|e| format!("invalid embedding settings: {e}")),
            Ok(None) => Ok(EmbeddingConfigData(
                text_embedding::EmbeddingConfig::default(),
            )),
            Err(e) => Err(e.to_string()),
        }
    }

    async fn save_embedding_config(
        &self,
        config: text_embedding::EmbeddingConfig,
    ) -> std::result::Result<EmbeddingConfigData, String> {
        validate_embedding_config(&config)?;
        let json = serde_json::to_string(&config).map_err(|e| e.to_string())?;
        self.storage
            .put_setting(EMBEDDING_CONFIG_KEY, &json)
            .await
            .map_err(|e| e.to_string())?;
        Ok(EmbeddingConfigData(config))
    }

    // ── ChatGPT OAuth ───────────────────────────────────────────────

    /// The stored ChatGPT token blob, if any.
    async fn chatgpt_blob(&self) -> Option<TokenBlob> {
        let json = self.storage.chatgpt_blob_json().await.ok()??;
        TokenBlob::from_json(&json).ok()
    }

    async fn start_login(&mut self) -> std::result::Result<String, String> {
        if self.login_in_flight {
            return Err(
                "a ChatGPT login flow is already in progress (one callback listener at a time)"
                    .to_string(),
            );
        }
        let (url, waiter) = oauth::login_flow()?;
        self.login_in_flight = true;
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = waiter.wait().await;
            let _ = tx.try_send(ModelCommand::LoginCompleted { result });
        });
        Ok(url)
    }

    async fn login_completed(&mut self, result: std::result::Result<TokenBlob, String>) {
        self.login_in_flight = false;
        let event = match result {
            Ok(blob) => {
                let success = LoginSuccess {
                    email: blob.email.clone(),
                    plan_type: blob.plan_type.clone(),
                };
                match blob.to_json() {
                    Ok(json) => match self.storage.save_chatgpt_blob_json(&json).await {
                        Ok(()) => {
                            self.publish(ModelEvent::ModelChanged {
                                spec: None,
                                reason: "chatgpt_login".to_string(),
                            });
                            ModelEvent::ChatgptLogin {
                                result: Ok(success),
                            }
                        }
                        Err(e) => ModelEvent::ChatgptLogin {
                            result: Err(e.to_string()),
                        },
                    },
                    Err(e) => ModelEvent::ChatgptLogin { result: Err(e) },
                }
            }
            Err(e) => ModelEvent::ChatgptLogin { result: Err(e) },
        };
        self.publish(event);
    }

    async fn refresh_now(&self, reply: oneshot::Sender<std::result::Result<bool, String>>) {
        let Some(blob) = self.chatgpt_blob().await else {
            let _ = reply.send(Ok(false));
            return;
        };
        self.spawn_refresh(blob, Some(reply));
    }

    async fn ensure_fresh(&self) {
        let Some(blob) = self.chatgpt_blob().await else {
            return;
        };
        if !blob.should_refresh() {
            return;
        }
        self.spawn_refresh(blob, None);
    }

    /// Network work happens off the mailbox; the result reports back as
    /// a [`ModelCommand::RefreshCompleted`].
    fn spawn_refresh(
        &self,
        blob: TokenBlob,
        reply: Option<oneshot::Sender<std::result::Result<bool, String>>>,
    ) {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = oauth::refresh_blob(&blob).await;
            let _ = tx.try_send(ModelCommand::RefreshCompleted { result, reply });
        });
    }

    async fn refresh_completed(
        &self,
        result: std::result::Result<TokenBlob, String>,
        reply: Option<oneshot::Sender<std::result::Result<bool, String>>>,
    ) {
        let outcome: std::result::Result<(), String> = async {
            let blob = result?;
            let json = blob.to_json()?;
            self.storage
                .save_chatgpt_blob_json(&json)
                .await
                .map_err(|e: ModelStorageError| e.to_string())
        }
        .await;
        match outcome {
            Ok(()) => {
                self.publish(ModelEvent::ModelChanged {
                    spec: None,
                    reason: "token_refresh".to_string(),
                });
                if let Some(reply) = reply {
                    let _ = reply.send(Ok(true));
                }
            }
            Err(e) => {
                if let Some(reply) = reply {
                    let _ = reply.send(Err(e));
                } else {
                    // Non-fatal: 401 self-healing retries on next startup.
                    tracing::warn!(error = %e, "chatgpt ensure-fresh refresh failed");
                }
            }
        }
    }

    async fn persist_chatgpt_blob(&self, json: &str, reason: &'static str) {
        match self.storage.save_chatgpt_blob_json(json).await {
            Ok(()) => {
                tracing::info!("chatgpt token refreshed and persisted by daemon");
                self.publish(ModelEvent::ModelChanged {
                    spec: None,
                    reason: reason.to_string(),
                });
            }
            Err(e) => {
                tracing::error!(error = %e, "chatgpt token refresh persist failed");
            }
        }
    }

    // ── remote catalogue ────────────────────────────────────────────

    async fn fetch_remote_catalog(&self, provider: String, base_url: String) {
        let provider_type = ProviderType::from(provider.as_str());
        // The openai catalogue authenticates with the stored ChatGPT
        // login — read it here (inside the worker) and hand the snapshot
        // to the network task.
        let openai_blob = if matches!(provider_type, ProviderType::Openai) {
            self.chatgpt_blob().await
        } else {
            None
        };
        let ollama_key = if matches!(provider_type, ProviderType::Ollama) {
            self.storage
                .provider_auth(&provider)
                .await
                .ok()
                .flatten()
                .map(|auth| auth.api_key)
                .unwrap_or_default()
        } else {
            String::new()
        };
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = if matches!(provider_type, ProviderType::Openai) {
                match openai_blob {
                    Some(blob) => {
                        OpenaiProvider::fetch_remote_catalog(
                            &base_url,
                            &blob.access_token,
                            &blob.account_id,
                        )
                        .await
                    }
                    None => Err("openai model catalogue requires a ChatGPT login".to_string()),
                }
            } else if matches!(provider_type, ProviderType::Ollama) {
                OllamaProvider::fetch_remote_catalog_with_auth(&base_url, &ollama_key).await
            } else {
                OpenrouterProvider::fetch_remote_catalog(&base_url).await
            };
            let _ = tx.try_send(ModelCommand::RemoteCatalogFetched { provider, result });
        });
    }

    async fn remote_catalog_fetched(
        &self,
        provider: String,
        result: std::result::Result<Vec<ModelInfo>, String>,
    ) {
        let event = match result {
            Ok(models_info) => match self
                .storage
                .replace_provider_models(&provider, &models_info)
                .await
            {
                Ok(count) => ModelEvent::CatalogFetched {
                    provider: provider.clone(),
                    result: Ok(count),
                },
                Err(e) => ModelEvent::CatalogFetched {
                    provider: provider.clone(),
                    result: Err(e.to_string()),
                },
            },
            Err(e) => ModelEvent::CatalogFetched {
                provider,
                result: Err(e),
            },
        };
        self.publish(event);
    }
}
