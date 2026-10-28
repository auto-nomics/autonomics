//! Explicitly enabled bibliography embeddings. Configuration writes, canonical
//! reads/indexing, and query embeddings use independent tasks/connections.
//! No model request runs on the storage or lexical-search actor.
use crate::{
    Error, Result,
    semantic_index::{SemanticIndex, SemanticProfile, SemanticProgress, SemanticSnapshot},
    semantic_limits::{self, SemanticLimits, SemanticResourceUsage},
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};
use text_embedding::{OllamaInferenceOptions, PinnedOllamaEmbedder, TextEmbedder};
use tokio::sync::{Semaphore, mpsc, oneshot, watch};
use turso::Connection;
mod maintenance;

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SemanticModelDiscovery {
    pub origin: String,
    pub allow_remote: bool,
}
impl Default for SemanticModelDiscovery {
    fn default() -> Self {
        Self {
            origin: SemanticConfig::default().origin,
            allow_remote: false,
        }
    }
}
#[derive(Debug, Serialize, Deserialize)]
pub struct InstalledSemanticModel {
    pub name: String,
    pub digest: String,
}

/// Only model metadata is requested; this neither enables indexing nor sends
/// bibliography text, pulls a model, or creates a pin alias.
pub async fn installed_semantic_models(
    request: SemanticModelDiscovery,
) -> Result<Vec<InstalledSemanticModel>> {
    let config = SemanticConfig {
        origin: request.origin,
        allow_remote: request.allow_remote,
        ..Default::default()
    };
    config.validate()?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| Error::SemanticUnavailable("cannot construct model metadata client".into()))?;
    let mut response = client
        .get(format!("{}/api/tags", config.origin.trim_end_matches('/')))
        .send()
        .await
        .map_err(|_| Error::SemanticUnavailable("cannot reach model server".into()))?;
    if !response.status().is_success() {
        return Err(Error::SemanticUnavailable(
            "model server rejected metadata request".into(),
        ));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| Error::SemanticUnavailable("cannot read model metadata".into()))?
    {
        if bytes.len() + chunk.len() > 1024 * 1024 {
            return Err(Error::SemanticUnavailable(
                "model metadata exceeds 1 MiB".into(),
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    #[derive(Deserialize)]
    struct Tags {
        models: Vec<InstalledSemanticModel>,
    }
    let mut result: Tags = serde_json::from_slice(&bytes)
        .map_err(|_| Error::SemanticUnavailable("invalid model metadata".into()))?;
    if result.models.len() > 4096
        || result.models.iter().any(|model| {
            model.name.is_empty()
                || model.name.len() > 256
                || model.digest.len() != 64
                || !model
                    .digest
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        })
    {
        return Err(Error::SemanticUnavailable(
            "invalid installed model identity".into(),
        ));
    }
    result.models.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(result.models)
}

impl crate::BibBase {
    pub fn semantic_search_status(&self) -> Result<SemanticStatus> {
        Ok(self
            .semantic
            .as_ref()
            .ok_or_else(|| invalid("semantic search requires the Tantivy engine"))?
            .status())
    }
    pub async fn configure_semantic_search(&self, config: SemanticConfig) -> Result<String> {
        self.semantic
            .as_ref()
            .ok_or_else(|| invalid("semantic search requires the Tantivy engine"))?
            .configure(config)
            .await
    }
    pub async fn maintain_semantic_search(
        &self,
        request: SemanticMaintenanceRequest,
    ) -> Result<SemanticMaintenance> {
        self.semantic
            .as_ref()
            .ok_or_else(|| invalid("semantic search requires the Tantivy engine"))?
            .maintain(request)
            .await
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SemanticConfig {
    pub enabled: bool,
    pub origin: String,
    /// Explicit consent to sending the configured document fields off-host.
    pub allow_remote: bool,
    pub model: String,
    pub model_digest: String,
    pub profile: SemanticProfile,
    pub batch_size: usize,
    pub inference: OllamaInferenceOptions,
    pub limits: SemanticLimits,
    pub timeout_seconds: u64,
}
impl Default for SemanticConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            origin: "http://127.0.0.1:11434".into(),
            allow_remote: false,
            model: "qwen3-embedding:0.6b".into(),
            model_digest: String::new(),
            profile: SemanticProfile::default(),
            batch_size: 16,
            inference: OllamaInferenceOptions::default(),
            limits: SemanticLimits::default(),
            timeout_seconds: 120,
        }
    }
}
impl SemanticConfig {
    pub fn validate(&self) -> Result<()> {
        self.profile.validate()?;
        self.limits.validate()?;
        self.inference
            .validate()
            .map_err(|_| invalid("invalid semantic context or token batch size"))?;
        if self.origin.len() > 2048 {
            return Err(invalid("semantic origin exceeds 2048 bytes"));
        }
        let origin =
            reqwest::Url::parse(&self.origin).map_err(|_| invalid("invalid semantic origin"))?;
        if !matches!(origin.scheme(), "http" | "https")
            || !origin.username().is_empty()
            || origin.password().is_some()
            || origin.query().is_some()
            || origin.fragment().is_some()
            || !matches!(origin.path(), "" | "/")
            || origin.host_str().is_none()
        {
            return Err(invalid(
                "semantic origin must be an HTTP origin without credentials",
            ));
        }
        let local = origin.host_str().is_some_and(|host| {
            host == "localhost"
                || host
                    .trim_matches(['[', ']'])
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        });
        if !local && !self.allow_remote {
            return Err(invalid(
                "remote semantic origin requires explicit allow_remote",
            ));
        }
        if !(1..=64).contains(&self.batch_size)
            || !(1..=600).contains(&self.timeout_seconds)
            || self.model.is_empty()
            || self.model.len() > 256
            || self.model.chars().any(char::is_whitespace)
            || (self.enabled
                && (self.model_digest.len() != 64
                    || !self
                        .model_digest
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())))
        {
            return Err(invalid(
                "invalid semantic model digest, name, batch size or timeout",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticPhase {
    Disabled,
    Loading,
    Indexing,
    Ready,
    Failed,
    Paused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticMaintenanceAction {
    Rebuild,
    Repair,
    Resume,
    Cancel,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticMaintenanceRequest {
    pub action: SemanticMaintenanceAction,
    pub expected_configuration_generation: String,
    pub request_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticMaintenance {
    pub request_id: String,
    pub action: SemanticMaintenanceAction,
    pub base_configuration_generation: String,
    pub configuration_generation: String,
    /// queued, running, complete, failed, cancelled
    pub state: String,
    pub requested_at: String,
    pub finished_at: Option<String>,
    pub progress: Option<SemanticProgress>,
    pub last_error: Option<String>,
    /// Private repair store retained across pause, failure and lease recovery.
    pub store_generation: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticStatus {
    pub config: SemanticConfig,
    /// Changes on configuration replacement, including disable/re-enable.
    pub configuration_generation: String,
    pub phase: SemanticPhase,
    pub generation: Option<String>,
    pub signature: Option<String>,
    pub indexed_revision: Option<u64>,
    pub indexed_chunks: usize,
    #[serde(default)]
    pub resources: Option<SemanticResourceUsage>,
    pub last_error: Option<String>,
    pub progress: Option<SemanticProgress>,
    pub maintenance: Option<SemanticMaintenance>,
    pub store_generation: String,
}

#[derive(Clone)]
struct Ready {
    provider: Arc<dyn TextEmbedder>,
    profile: SemanticProfile,
    snapshot: Arc<SemanticSnapshot>,
    store_generation: String,
}
#[derive(Clone)]
struct State {
    status: SemanticStatus,
    ready: Option<Ready>,
}
#[derive(Clone)]
struct VersionedConfig {
    config: SemanticConfig,
    generation: String,
    maintenance: Option<SemanticMaintenance>,
}
enum Control {
    Configure {
        config: SemanticConfig,
        reply: oneshot::Sender<Result<String>>,
    },
    Maintain {
        request: SemanticMaintenanceRequest,
        reply: oneshot::Sender<Result<SemanticMaintenance>>,
    },
}

#[derive(Clone)]
pub struct SemanticService {
    controls: mpsc::Sender<Control>,
    state: watch::Receiver<State>,
    query_slots: Arc<Semaphore>,
}
/// Internal prepared input; the search actor must validate its source revision,
/// commit token and configuration generation before returning results.
pub struct PreparedSemanticQuery {
    pub query: String,
    pub configuration: SemanticConfig,
    pub configuration_generation: String,
    pub snapshot: Arc<SemanticSnapshot>,
    pub store_generation: String,
    pub vector: Vec<f32>,
}

#[async_trait]
trait Connector: Send + Sync {
    async fn connect(&self, config: &SemanticConfig) -> Result<Arc<dyn TextEmbedder>>;
}
struct OllamaConnector;
#[async_trait]
impl Connector for OllamaConnector {
    async fn connect(&self, config: &SemanticConfig) -> Result<Arc<dyn TextEmbedder>> {
        let provider = PinnedOllamaEmbedder::connect_with_inference(
            &config.origin,
            &config.model,
            &config.model_digest,
            config.profile.dimensions,
            config.batch_size,
            Duration::from_secs(config.timeout_seconds),
            config.inference.clone(),
        )
        .await
        .map_err(|_| {
            invalid(
                "semantic model unavailable or digest mismatch; check installed model and origin",
            )
        })?;
        Ok(Arc::new(provider))
    }
}
fn invalid(message: &str) -> Error {
    Error::InvalidSearchQuery(message.into())
}
fn initial(config: &VersionedConfig) -> State {
    State {
        status: SemanticStatus {
            config: config.config.clone(),
            configuration_generation: config.generation.clone(),
            phase: if config.config.enabled {
                SemanticPhase::Loading
            } else {
                SemanticPhase::Disabled
            },
            generation: None,
            signature: None,
            indexed_revision: None,
            indexed_chunks: 0,
            resources: None,
            last_error: None,
            progress: None,
            maintenance: config.maintenance.clone(),
            store_generation: String::new(),
        },
        ready: None,
    }
}

async fn read_configuration(connection: &Connection) -> Result<Option<VersionedConfig>> {
    let mut rows = connection
        .query(
            "SELECT value,generation FROM bibliographySemanticConfig WHERE id=1",
            (),
        )
        .await?;
    let Some(row) = rows.next().await? else {
        return Ok(None);
    };
    let config: SemanticConfig = serde_json::from_str(&row.get::<String>(0)?)
        .map_err(|_| invalid("stored semantic configuration is invalid; replace it in settings"))?;
    config.validate()?;
    let generation: String = row.get(1)?;
    drop(rows);
    let maintenance = read_maintenance(connection, &generation).await?;
    Ok(Some(VersionedConfig {
        config,
        generation,
        maintenance,
    }))
}

async fn read_maintenance(
    connection: &Connection,
    generation: &str,
) -> Result<Option<SemanticMaintenance>> {
    let mut rows=connection.query("SELECT value FROM bibliographySemanticMaintenance WHERE id=1 AND configuration_generation=?1",turso::params![generation]).await?;
    rows.next()
        .await?
        .map(|row| Ok(serde_json::from_str(&row.get::<String>(0)?)?))
        .transpose()
}

pub(crate) async fn active_store(connection: &Connection, signature: &str) -> Result<String> {
    let mut rows = connection
        .query(
            "SELECT store_id FROM bibliographySemanticStores WHERE signature=?1",
            turso::params![signature],
        )
        .await?;
    let id = rows
        .next()
        .await?
        .map(|row| row.get::<String>(0))
        .transpose()?
        .unwrap_or_default();
    if !id.is_empty() && uuid::Uuid::parse_str(&id).is_err() {
        return Err(invalid("invalid semantic store generation"));
    }
    Ok(id)
}
fn store_path(base: &str, id: &str) -> String {
    if base == ":memory:" || id.is_empty() {
        base.into()
    } else {
        format!("{base}.repair-{id}")
    }
}

impl SemanticService {
    pub(crate) async fn open(
        config_connection: Connection,
        source: Connection,
        progress_connection: Connection,
        path: String,
    ) -> Result<Self> {
        Self::open_with_connector(
            config_connection,
            source,
            progress_connection,
            path,
            Arc::new(OllamaConnector),
        )
        .await
    }
    async fn open_with_connector(
        config_connection: Connection,
        source: Connection,
        progress_connection: Connection,
        path: String,
        connector: Arc<dyn Connector>,
    ) -> Result<Self> {
        config_connection
            .pragma_update("busy_timeout", 5000)
            .await?;
        source.pragma_update("busy_timeout", 5000).await?;
        progress_connection
            .pragma_update("busy_timeout", 5000)
            .await?;
        config_connection.execute_batch("CREATE TABLE IF NOT EXISTS bibliographySemanticConfig (id INTEGER PRIMARY KEY CHECK(id=1), value TEXT NOT NULL, generation TEXT NOT NULL);").await?;
        config_connection.execute_batch(maintenance::SCHEMA).await?;
        let (config, load_error) = match read_configuration(&config_connection).await {
            Ok(Some(config)) => (config, None),
            result => (
                VersionedConfig {
                    config: SemanticConfig::default(),
                    generation: uuid::Uuid::new_v4().to_string(),
                    maintenance: None,
                },
                result.err().map(|error| error.to_string()),
            ),
        };
        let mut initial_state = initial(&config);
        if let Some(error) = load_error {
            initial_state.status.phase = SemanticPhase::Failed;
            initial_state.status.last_error = Some(error);
        }
        let (state_tx, state) = watch::channel(initial_state);
        let (config_tx, config_rx) = watch::channel(config);
        let (controls, mut rx) = mpsc::channel::<Control>(16);
        let worker_state = state_tx.clone();
        let cleanup_path = path.clone();
        tokio::spawn(async move {
            let mut poll = tokio::time::interval(Duration::from_secs(2));
            poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                let command = tokio::select! {
                    biased;
                    command=rx.recv()=>match command {Some(command)=>command,None=>break},
                    _=poll.tick()=>{
                        if let Err(error)=maintenance::cleanup(&config_connection,&cleanup_path).await {tracing::warn!(%error,"semantic staging cleanup will retry");}
                        match read_configuration(&config_connection).await {
                            Ok(Some(next))=>{
                                if next.generation!=config_tx.borrow().generation {
                                    activate_configuration(&state_tx,&next);
                                    config_tx.send_replace(next);
                                }
                            },
                            Ok(None)=>{
                                if config_tx.borrow().config.enabled {
                                    let next=VersionedConfig {config:SemanticConfig::default(),generation:uuid::Uuid::new_v4().to_string(),maintenance:None};
                                    activate_configuration(&state_tx,&next);config_tx.send_replace(next);
                                }
                            },
                            Err(error)=>{
                                let next=VersionedConfig {config:SemanticConfig::default(),generation:uuid::Uuid::new_v4().to_string(),maintenance:None};
                                let mut failed=initial(&next);failed.status.phase=SemanticPhase::Failed;failed.status.last_error=Some(error.to_string());
                                state_tx.send_replace(failed);config_tx.send_replace(next);
                            },
                        }
                        continue;
                    },
                };
                match command {
                    Control::Configure { config, reply } => {
                        let result = async {
                            let next = maintenance::configure(&config_connection, config).await?;
                            let generation = next.generation.clone();
                            activate_configuration(&state_tx, &next);
                            config_tx.send_replace(next);
                            Ok(generation)
                        }
                        .await;
                        let _ = reply.send(result);
                    }
                    Control::Maintain { request, reply } => {
                        let result = maintenance::request(&config_connection, request).await;
                        let result = result.map(|(next, job)| {
                            if next.generation != config_tx.borrow().generation {
                                activate_configuration(&state_tx, &next);
                                config_tx.send_replace(next);
                            }
                            job
                        });
                        let _ = reply.send(result);
                    }
                }
            }
        });
        tokio::spawn(run_worker(
            source,
            progress_connection,
            path,
            connector,
            config_rx,
            worker_state,
        ));
        Ok(Self {
            controls,
            state,
            query_slots: Arc::new(Semaphore::new(4)),
        })
    }

    pub fn status(&self) -> SemanticStatus {
        self.state.borrow().status.clone()
    }
    pub async fn configure(&self, config: SemanticConfig) -> Result<String> {
        config.validate()?;
        let (reply, result) = oneshot::channel();
        self.controls
            .send(Control::Configure { config, reply })
            .await
            .map_err(|_| Error::ActorStopped)?;
        result.await.map_err(|_| Error::ActorStopped)?
    }
    pub async fn maintain(
        &self,
        request: SemanticMaintenanceRequest,
    ) -> Result<SemanticMaintenance> {
        let (reply, result) = oneshot::channel();
        self.controls
            .send(Control::Maintain { request, reply })
            .await
            .map_err(|_| Error::ActorStopped)?;
        result.await.map_err(|_| Error::ActorStopped)?
    }
    pub async fn prepare(&self, text: &str) -> Result<PreparedSemanticQuery> {
        let state = self.state.borrow().clone();
        let ready = state.ready.ok_or_else(|| {
            Error::SemanticUnavailable("disabled or index not ready; check semantic status".into())
        })?;
        let input = ready.profile.query_input(text)?;
        semantic_limits::check(
            "model query input bytes",
            input.text.len() as u64,
            state.status.config.limits.max_input_bytes as u64,
        )?;
        let generation = state.status.configuration_generation;
        let timeout = Duration::from_secs(state.status.config.timeout_seconds);
        let mut changes = self.state.clone();
        changes.borrow_and_update();
        if changes.borrow().status.configuration_generation != generation {
            return Err(Error::SearchConflict(
                "semantic configuration changed; retry query".into(),
            ));
        }
        let embedding = tokio::time::timeout(timeout, async {
            let _slot = self
                .query_slots
                .acquire()
                .await
                .map_err(|_| Error::ActorStopped)?;
            ready
                .provider
                .embed(&input)
                .await
                .map_err(|_| Error::SemanticUnavailable("query embedding failed".into()))
        });
        tokio::pin!(embedding);
        let output = loop {
            tokio::select! {
                biased;
                changed=changes.changed()=>{
                    changed.map_err(|_|Error::ActorStopped)?;
                    if changes.borrow().status.configuration_generation!=generation {
                        return Err(Error::SearchConflict("semantic configuration changed; retry query".into()));
                    }
                }
                output=&mut embedding=>break output.map_err(|_|Error::SemanticUnavailable("query embedding timed out".into()))??,
            }
        };
        if self.state.borrow().status.configuration_generation != generation {
            return Err(Error::SearchConflict(
                "semantic configuration changed; retry query".into(),
            ));
        }
        if output.model_signature != ready.provider.signature()
            || ready.profile.signature(ready.provider.as_ref())? != ready.snapshot.signature
        {
            return Err(invalid("semantic query model signature changed"));
        }
        // Validate geometry before the prepared query can enter a search queue.
        crate::semantic_index::validate_query_vector(&output.vector, ready.profile.dimensions)?;
        Ok(PreparedSemanticQuery {
            query: text.into(),
            configuration: state.status.config,
            configuration_generation: generation,
            snapshot: ready.snapshot,
            store_generation: ready.store_generation,
            vector: output.vector,
        })
    }
}

#[cfg(test)]
mod tests;

fn publish(state: &watch::Sender<State>, generation: &str, update: impl FnOnce(&mut State)) {
    state.send_if_modified(|state| {
        if state.status.configuration_generation != generation {
            return false;
        }
        update(state);
        true
    });
}

fn activate_configuration(state: &watch::Sender<State>, next: &VersionedConfig) {
    let old = state.borrow().clone();
    let mut replacement = initial(next);
    if old.status.config == next.config && next.config.enabled {
        replacement.ready = old.ready;
        replacement.status.generation = old.status.generation;
        replacement.status.signature = old.status.signature;
        replacement.status.indexed_revision = old.status.indexed_revision;
        replacement.status.indexed_chunks = old.status.indexed_chunks;
        replacement.status.resources = old.status.resources;
        replacement.status.store_generation = old.status.store_generation;
    }
    if next
        .maintenance
        .as_ref()
        .is_some_and(|job| job.action == SemanticMaintenanceAction::Cancel)
    {
        replacement.status.phase = SemanticPhase::Paused;
    }
    state.send_replace(replacement);
}

fn ready(
    state: &watch::Sender<State>,
    config: &VersionedConfig,
    provider: Arc<dyn TextEmbedder>,
    snapshot: Arc<SemanticSnapshot>,
    store: String,
    resources: SemanticResourceUsage,
    cleanup_warning: Option<String>,
) {
    publish(state, &config.generation, |state| {
        state.status.phase = SemanticPhase::Ready;
        state.status.generation = Some(snapshot.generation.clone());
        state.status.signature = Some(snapshot.signature.clone());
        state.status.indexed_revision = Some(snapshot.revision);
        state.status.indexed_chunks = snapshot.chunks.len();
        state.status.resources = Some(resources);
        state.status.store_generation = store.clone();
        state.status.last_error = cleanup_warning;
        state.ready = Some(Ready {
            provider,
            profile: config.config.profile.clone(),
            snapshot,
            store_generation: store,
        });
    });
}
fn progress_callback(state: &watch::Sender<State>, generation: &str, counters: SemanticProgress) {
    publish(state, generation, |state| {
        if matches!(
            counters.stage.as_str(),
            "staging" | "copying_bindings" | "embedding" | "publishing"
        ) || (state.ready.is_none() && counters.stage != "complete")
        {
            state.status.phase = SemanticPhase::Indexing;
        }
        state.status.progress = Some(counters.clone());
        if let Some(job) = &mut state.status.maintenance
            && job.state == "running"
        {
            job.progress = Some(counters);
        }
    });
}

async fn run_worker(
    source: Connection,
    progress_connection: Connection,
    path: String,
    connector: Arc<dyn Connector>,
    mut configs: watch::Receiver<VersionedConfig>,
    state: watch::Sender<State>,
) {
    loop {
        let config = configs.borrow_and_update().clone();
        if !config.config.enabled {
            if configs.changed().await.is_err() {
                return;
            }
            continue;
        }
        let work = run_generation(
            &source,
            &progress_connection,
            &path,
            connector.as_ref(),
            &config,
            &state,
        );
        tokio::select! {
            biased;
            changed=configs.changed()=>{if changed.is_err(){return;}},
            result=work=>{
                if let Err(error)=result {
                    publish(&state,&config.generation,|state|{state.status.phase=SemanticPhase::Failed;state.status.last_error=Some(error.to_string());});
                }
                tokio::select! {
                    changed=configs.changed()=>{if changed.is_err(){return;}},
                    _=tokio::time::sleep(Duration::from_secs(2))=>{},
                }
            },
        }
    }
}

async fn run_generation(
    source: &Connection,
    control: &Connection,
    path: &str,
    connector: &dyn Connector,
    config: &VersionedConfig,
    state: &watch::Sender<State>,
) -> Result<()> {
    let mut provider: Option<Arc<dyn TextEmbedder>> = None;
    let mut index: Option<SemanticIndex> = None;
    let mut current_store = String::new();
    let mut resource_wait: Option<((u64, String), SemanticResourceUsage, String)> = None;
    loop {
        if let Some((version, usage, reason)) = &resource_wait {
            let mut unchanged = crate::semantic_index::source_version(source).await? == *version;
            if unchanged
                && (reason == "vector payload bytes per store"
                    || reason == "semantic cache disk bytes")
            {
                index
                    .as_mut()
                    .expect("resource-limited index")
                    .cleanup_bounded()
                    .await;
                let current = index
                    .as_ref()
                    .expect("resource-limited index")
                    .resource_usage()
                    .await?;
                // Retry only when this resource has more room. Unrelated WAL
                // growth must not make multiple limited workers wake each other.
                unchanged = if reason == "vector payload bytes per store" {
                    current.vector_bytes >= usage.vector_bytes
                } else {
                    current.disk_bytes >= usage.disk_bytes
                };
            }
            if unchanged {
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
            resource_wait = None;
        }
        let job = read_maintenance(source, &config.generation).await?;
        publish(state, &config.generation, |state| {
            state.status.maintenance = job.clone()
        });
        if let Some(job) = &job {
            if job.action == SemanticMaintenanceAction::Cancel || job.state == "failed" {
                publish(state, &config.generation, |state| {
                    state.status.phase = if job.action == SemanticMaintenanceAction::Cancel {
                        SemanticPhase::Paused
                    } else {
                        SemanticPhase::Failed
                    };
                    state.status.last_error = job.last_error.clone();
                });
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
        }
        if let Some(mut job) = job.filter(|job| job.state == "queued" || job.state == "running") {
            let owner = uuid::Uuid::new_v4().to_string();
            if !maintenance::claim(control, &mut job, &owner).await? {
                publish(state, &config.generation, |state| {
                    state.status.phase = SemanticPhase::Indexing
                });
                tokio::time::sleep(Duration::from_millis(500)).await;
                continue;
            }
            publish(state, &config.generation, |state| {
                state.status.phase = SemanticPhase::Indexing;
                state.status.maintenance = Some(job.clone());
            });
            let repair = job.store_generation.is_some();
            if repair && path != ":memory:" {
                control
                    .execute(
                        "INSERT INTO bibliographySemanticArtifacts VALUES(?1,?2) ON CONFLICT(store_id) DO UPDATE SET lease_until=excluded.lease_until",
                        turso::params![
                        job.store_generation.clone().expect("repair store"),
                            chrono::Utc::now().timestamp_millis() + maintenance::LEASE_MS
                        ],
                    )
                    .await?;
            }
            let build = async {
                // An over-budget library must not accumulate a new empty
                // private database on every repeated repair request.
                if repair
                    && let Some(bytes) = semantic_limits::disk_bytes(
                        (path != ":memory:").then(|| std::path::Path::new(path)),
                    )
                    .await?
                {
                    semantic_limits::check(
                        "semantic cache disk bytes",
                        bytes,
                        config.config.limits.max_disk_bytes,
                    )?;
                }
                let model = connector.connect(&config.config).await?;
                let signature = config.config.profile.signature(model.as_ref())?;
                let store = if repair {
                    job.store_generation.clone().expect("repair store")
                } else {
                    active_store(source, &signature).await?
                };
                let mut target = SemanticIndex::open(
                    source.clone(),
                    &store_path(path, &store),
                    config.config.profile.clone(),
                    model.clone(),
                )
                .await?
                .with_limits(config.config.limits.clone())?
                .with_disk_family(path);
                let result = target
                    .sync_with_progress(
                        repair || job.action != SemanticMaintenanceAction::Resume,
                        |progress| progress_callback(state, &config.generation, progress),
                    )
                    .await?;
                let resources = target.resource_usage().await?;
                Ok::<_, Error>((model, target, store, result.0, resources))
            };
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            tokio::pin!(build);
            let result = loop {
                tokio::select! {
                    biased;
                    _=tick.tick()=>{
                        job.progress=state.borrow().status.progress.clone();
                        if let Err(error)=maintenance::heartbeat(control,&job,&owner).await {break Err(error);}
                    },
                    result=&mut build=>break result,
                }
            };
            job.progress = state.borrow().status.progress.clone();
            job.finished_at = Some(chrono::Utc::now().to_rfc3339());
            match result {
                Ok((model, target, store, snapshot, resources)) => {
                    job.state = "complete".into();
                    maintenance::finish(
                        control,
                        &job,
                        &owner,
                        if repair {
                            Some((&snapshot.signature, &store))
                        } else {
                            None
                        },
                    )
                    .await?;
                    publish(state, &config.generation, |state| {
                        state.status.maintenance = Some(job.clone())
                    });
                    ready(
                        state,
                        config,
                        model.clone(),
                        snapshot,
                        store.clone(),
                        resources,
                        target.cleanup_warning(),
                    );
                    provider = Some(model);
                    index = Some(target);
                    current_store = store;
                }
                Err(error) => {
                    job.state = "failed".into();
                    job.last_error = Some(error.to_string());
                    maintenance::finish(control, &job, &owner, None).await?;
                    publish(state, &config.generation, |state| {
                        state.status.phase = SemanticPhase::Failed;
                        state.status.last_error = job.last_error.clone();
                        state.status.maintenance = Some(job.clone());
                    });
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
            continue;
        }
        let model = match &provider {
            Some(model) => model.clone(),
            None => {
                let model = connector.connect(&config.config).await?;
                provider = Some(model.clone());
                model
            }
        };
        let signature = config.config.profile.signature(model.as_ref())?;
        let store = active_store(source, &signature).await?;
        if index.is_none() || store != current_store {
            index = Some(
                SemanticIndex::open(
                    source.clone(),
                    &store_path(path, &store),
                    config.config.profile.clone(),
                    model.clone(),
                )
                .await?
                .with_limits(config.config.limits.clone())?
                .with_disk_family(path),
            );
            current_store = store;
        }
        let attempted_version = crate::semantic_index::source_version(source).await?;
        let result = async {
            let target = index.as_mut().expect("opened index");
            let (snapshot, _) = target
                .sync_with_progress(false, |progress| {
                    progress_callback(state, &config.generation, progress)
                })
                .await?;
            let resources = target.resource_usage().await?;
            Ok::<_, Error>((snapshot, resources, target.cleanup_warning()))
        }
        .await;
        match result {
            Ok((snapshot, resources, cleanup_warning)) => ready(
                state,
                config,
                model,
                snapshot,
                current_store.clone(),
                resources,
                cleanup_warning,
            ),
            Err(error) => {
                if let Error::SemanticResourceLimit { resource, .. } = &error {
                    resource_wait = Some((
                        attempted_version,
                        index
                            .as_ref()
                            .expect("opened index")
                            .resource_usage()
                            .await?,
                        resource.clone(),
                    ));
                }
                publish(state, &config.generation, |state| {
                    state.status.phase = SemanticPhase::Failed;
                    state.status.last_error = Some(error.to_string());
                });
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}
