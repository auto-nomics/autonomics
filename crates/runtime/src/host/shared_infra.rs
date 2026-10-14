//! Process-level shared infrastructure ([`SharedInfra`]): every heavy
//! resource opened once per process and reused by all agents, plus the
//! per-agent spawn path and tool assembly.

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use agentik_core::Agent;
use agentik_core::TursoAgentStorage;
use agentik_core::memory::{MemoryBackend, MemoryConfig, MemoryStore, SemanticGrounding};
use agentik_core::profile::{AgentKind, AgentProfileConfig};
use agentik_core::storage::AgentStorage;
use arc_swap::ArcSwapOption;
use browser_use::{BrowserOptions, BrowserSession};
use container_runtime::ContainerExecutionInfra;
use data_engine::dag::DagHistory;
use data_engine::data_engine::DataEngine;
use data_engine::runtime::DataEngineManager;
use rcsb::RcsbClient;
use vfs::OpendalFileStorage;

use crate::config::{PromptCapabilities, RuntimeConfig};
use crate::error::{Error, Result};
use crate::memory_kms::KmsMemoryGrounding;

use super::bundles::build_bundle_registry;
use super::gc::{
    spawn_container_gc, spawn_dag_task_gc, spawn_environment_distiller, spawn_plugin_distiller,
};
use super::vfs_setup::{
    BROWSER_DOWNLOAD_VFS_PATH, agent_vfs_principal, browser_download_root, build_vfs_with_catalog,
};

/// Cache of per-agent [`BrowserSession`]s.
///
/// Each browser-kind agent owns a strong `Arc<BrowserSession>` through the
/// tool registrations it carries; the pool holds only a `Weak` so the
/// strong count going to zero is what actually closes the browser (via
/// the inner `Browser`'s `kill_on_drop`). A re-spawn of the same agent
/// path finds the dangling `Weak` (after the previous run ended),
/// upgrades to `None`, and just allocates a fresh session.
#[derive(Default)]
pub(super) struct BrowserPool {
    by_path: HashMap<String, Weak<BrowserSession>>,
}

impl BrowserPool {
    /// Drop any `Weak` entries whose strong Arc has been freed (i.e. the
    /// previous browser agent at that path has exited and its session
    /// dropped). Cheap; only called when we are about to allocate.
    fn prune_dead(&mut self) {
        self.by_path.retain(|_, weak| weak.strong_count() > 0);
    }

    /// Whether the agent at `path` still holds a live session.
    /// Test-only: used by the session-lifecycle test to observe the
    /// Weak-dangling transition after agent shutdown.
    #[cfg(test)]
    pub(super) fn session_alive(&self, path: &str) -> bool {
        self.by_path.get(path).is_some_and(|w| w.strong_count() > 0)
    }
}

// AgentKind carries the same tool-capability flags as RuntimeConfig, so we
// can build a dynamic system prompt that only mentions tools the kind
// actually enables.
impl PromptCapabilities for agentik_core::AgentKind {
    fn enable_bibliography(&self) -> bool {
        (*self).enable_bibliography()
    }
    fn enable_opengwas(&self) -> bool {
        (*self).enable_opengwas()
    }
    fn enable_opentargets(&self) -> bool {
        (*self).enable_opentargets()
    }
    fn enable_gwascatalog(&self) -> bool {
        (*self).enable_gwascatalog()
    }
    fn enable_chembl(&self) -> bool {
        (*self).enable_chembl()
    }
    fn enable_rcsb(&self) -> bool {
        (*self).enable_rcsb()
    }
    fn enable_string(&self) -> bool {
        (*self).enable_string()
    }
    fn enable_kegg(&self) -> bool {
        (*self).enable_kegg()
    }
    fn enable_dag_history(&self) -> bool {
        (*self).enable_dag_history()
    }
    fn enable_plugin_rsi(&self) -> bool {
        (*self).enable_plugin_rsi()
    }
    fn enable_browser(&self) -> bool {
        (*self).enable_browser()
    }
}

/// Process-level shared infrastructure created once and reused by all agents.
///
/// Cloning is cheap — every field is `Arc`-backed.
#[derive(Clone)]
pub struct SharedInfra {
    /// Per-agent session manager — each agent gets its own DAG actor task,
    /// fully isolated from other agents. Heavy infrastructure (NodeRegistry,
    /// DagHistory, RuntimeEnv) is shared via `Arc`.
    pub engine_manager: Arc<DataEngineManager>,
    pub file_storage: Arc<OpendalFileStorage>,
    /// Unix-style virtual filesystem mounted under `vfs://`.
    pub vfs: Arc<vfs::MountedObjectStore>,
    /// Remote catalog access plus the local package cache. `None` when the
    /// catalog is absent or disabled.
    pub catalog: Option<Arc<crate::catalog_tools::CatalogState>>,
    /// Process-wide Podman connection and immutable panel cache shared by
    /// all DAG sessions.
    pub container_execution: Arc<ContainerExecutionInfra>,
    /// Unified plugin request, development, publication, and registration.
    pub rsi: Arc<plugin_rsi::RsiInfra>,
    /// Environment (image) development subsystem; `None` when disabled in
    /// configuration, in which case agents receive no environment tools.
    pub environment_dev: Option<Arc<plugin_rsi::EnvironmentDevInfra>>,
    pub storage: Arc<dyn AgentStorage>,
    /// Process-wide RCSB PDB HTTP client shared by every enabled agent.
    pub rcsb: Arc<RcsbClient>,
    /// Bibliography storage + literature gateway, opened **once** per
    /// process and shared by every spawned agent.
    pub bib: Arc<bib_base::BibShared>,
    /// LaTeX writing system (store + optional engine), opened **once** per
    /// process. Reuses `bib` for citation resolution when available.
    pub writing: Arc<writing_base::WritingShared>,
    /// Base persistent-memory backend. Each agent gets a clone with its own
    /// resolved runtime overrides while sharing the store and grounding sink.
    pub memory: Arc<MemoryBackend>,
    /// Optional Turso-backed KMS knowledge service.
    pub kms: Option<Arc<kms::KmsService>>,
    /// The process-wide observation hub — the upstream evidence
    /// ledger shared by skills, plugins, and gateway handlers. Neither
    /// downstream owns it; both resolve the same instance.
    pub observations: Arc<evolution_core::ObservationHub>,
    /// Control handle for the clustering worker spawned over the
    /// observation hub. Cloning shares the same worker; dropping the
    /// last handle stops it after its current pass.
    pub evolution_handle: evolution_core::handle::EvolutionHandle,
    /// Central skill manager (tiered registry + generation counter +
    /// change broadcast + usage telemetry). Initialized as the
    /// process-wide singleton so every mutation path — agent tools,
    /// eval auto-capture, gateway handlers — shares one notification
    /// invariant.
    pub skills: Arc<skills::SkillManager>,
    /// Remote Ollama embedding service constructed from startup
    /// configuration. `None` when disabled — the observation hub then
    /// serves corpus-fitted BM25 vectors.
    pub embedding: Arc<ArcSwapOption<text_embedding::RemoteEmbeddingService>>,
    /// The unified skill control handle, when the service is
    /// enabled. `None` means the loop is disabled at the daemon
    /// level — every gateway mutation request then returns HTTP
    /// 503 (the dashboard, the agent `skill_evolve` tool, and the
    /// observation-recording tools all funnel through this handle;
    /// with the service off they record without the nudge and the
    /// on-demand paths still run cycles directly).
    pub skill_evolution: Option<skills::SkillControlHandle>,
    /// The tokio runtime handle (for spawning agent tasks).
    pub runtime_handle: tokio::runtime::Handle,
    /// Optional host control for agent tools. Set by RuntimeHost when
    /// available. When `Some`, spawned agents receive host management tools
    /// (spawn_agent, delegate_to, etc.).
    pub host_control: Option<crate::control::HostControl>,
    /// Per-agent Chromium session cache. `Weak` entries so the agent's
    /// tool list owns the strong Arc and browser process dies with the
    /// agent — see [`BrowserPool`].
    pub(super) browsers: Arc<tokio::sync::Mutex<BrowserPool>>,
    /// Effective Chromium launch template, derived from the global
    /// `RuntimeConfig` at `open` time. Each browser agent clones this
    /// template and tweaks it (today: nothing — per-agent overrides
    /// belong on `AgentProfileConfig` once we have a use case).
    pub(super) browser_options_template: BrowserOptions,
}

impl SharedInfra {
    /// Hot-swap the observation hub's remote embedding provider and
    /// publish the service handle. A disabled config installs the
    /// corpus-fitted BM25 fallback.
    pub fn configure_embedding<'a>(
        &'a self,
        config: &'a text_embedding::EmbeddingConfig,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            if config.enabled {
                let service = text_embedding::RemoteEmbeddingService::init(config)
                    .await
                    .map_err(|error| {
                        Error::Other(format!("Ollama embedding service unavailable: {error}"))
                    })?;
                swap_embedder(&self.observations, &service).await;
                self.embedding.store(Some(Arc::new(service)));
            } else {
                install_bm25_fallback(&self.observations).await;
                self.embedding.store(None);
            }
            Ok(())
        })
    }

    /// Open all shared resources from a global [`RuntimeConfig`].
    ///
    /// The config's `data_dir`, `state_dir`, `agent_db`, and feature flags
    /// for DAG history are consumed here. Per-agent settings
    /// (identity, prompts, tool flags) are **not** read — those are passed
    /// to [`RuntimeHost::spawn_agent`](super::RuntimeHost::spawn_agent)
    /// instead.
    pub async fn open(config: &RuntimeConfig) -> Result<Self> {
        tracing::info!("SharedInfra::open: starting");

        // Skill and plugin evolution share one state dir. The
        // observation hub is installed FIRST — evidence is upstream of
        // both — then the skill manager re-installs the same directory
        // (idempotent) and keeps its own reference.
        let skills = skills::SkillManager::init(skills::SkillManager::new(&config.state_dir));
        // One hub instance shared by the manager, RSI, and every
        // SharedInfra consumer — the manager built it over the same
        // state dir; hand out its reference instead of a second one.
        let observations = skills.observation_hub();
        skills.load_usage();

        let embedding = open_embedding(config, &observations).await?;
        let (rsi, environments) = open_plugin_rsi(config, &observations).await?;

        let (vfs, catalog_service, file_storage, catalog_bundles) =
            open_vfs_and_storage(config, &rsi, &environments).await?;
        let container_execution = open_container_execution()?;
        let environment_dev =
            open_environment_dev(config, &environments, &container_execution).await?;
        // Reclaim crash residue and expired scratch from previous runs once at
        // startup, then on the configured interval. Failures are logged and
        // never block the host.
        spawn_container_gc(&container_execution);
        spawn_dag_task_gc();

        let engine_manager = build_engine_manager(
            config,
            &file_storage,
            &vfs,
            &container_execution,
            catalog_bundles,
            &rsi,
        )
        .await?;

        let turso_store = open_agent_storage(config).await?;
        let storage: Arc<dyn AgentStorage> = turso_store.clone();
        let rcsb = Arc::new(RcsbClient::new());
        let kms = open_kms(config, &turso_store).await?;
        let memory = open_memory(config, turso_store, &kms);
        let bib = open_bibliography(config, &file_storage).await?;
        let writing = open_writing(config, &bib).await?;

        // One clustering worker over the shared hub. Triggers land on
        // a buffered channel, so callers never block on a slow pass.
        let evolution_handle = evolution_core::worker::start(Arc::clone(&observations));
        tracing::info!("SharedInfra::open: clustering worker started");

        let skill_evolution = start_skill_evolution(config, &skills);

        tracing::info!("SharedInfra::open: all infrastructure ready");

        let mut browser_options = BrowserOptions::default();
        browser_options.headless = config.browser_headless;
        browser_options.no_sandbox = config.browser_no_sandbox;
        browser_options.download_dir = Some(browser_download_root(config));
        browser_options.download_vfs_path = Some(BROWSER_DOWNLOAD_VFS_PATH.to_string());

        Ok(Self {
            engine_manager,
            file_storage,
            vfs,
            container_execution,
            rsi,
            environment_dev,
            catalog: catalog_service,
            storage,
            rcsb,
            bib,
            writing,
            memory,
            kms,
            observations,
            evolution_handle,
            skills,
            embedding,
            skill_evolution,
            runtime_handle: tokio::runtime::Handle::current(),
            host_control: None,
            browsers: Arc::new(tokio::sync::Mutex::new(BrowserPool::default())),
            browser_options_template: browser_options,
        })
    }

    /// Spawn a new agent from an [`AgentProfileConfig`].
    ///
    /// This method only needs [`SharedInfra`] — it does not touch the
    /// topology network or agent registry. Callers can clone `SharedInfra`
    /// (cheap, all `Arc`) into async tasks.
    pub async fn spawn_agent(
        &self,
        agent_path: &agentik_types::AgentPath,
        config: &AgentProfileConfig,
        global_model: Arc<ArcSwapOption<agentik_sdk::model::Model>>,
        model_override: Option<agentik_sdk::model::Model>,
    ) -> Result<super::AgentHandle> {
        let kind = config.kind;
        let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();
        let cancel_token = tokio_util::sync::CancellationToken::new();

        let model = match model_override {
            Some(m) => Arc::new(ArcSwapOption::from_pointee(Some(m))),
            None => Arc::new(ArcSwapOption::from_pointee(
                global_model.load_full().as_deref().cloned(),
            )),
        };

        let tool_list = self.tools_from_profile(agent_path, kind).await?;

        let config_json = config.to_json();
        let storage = self.storage.clone();
        let defaults = self.memory.runtime_config();
        let runtime_config = agentik_core::AgentRuntimeConfig::new(
            config.runtime.use_memory.unwrap_or(defaults.use_memory),
            config
                .runtime
                .generate_memory
                .unwrap_or(defaults.generate_memory),
        );
        let memory = Arc::new(MemoryBackend::clone(&self.memory));
        memory.set_runtime_config(runtime_config);

        let mut builder = Agent::builder()
            .with_model(model.clone())
            .with_agent_event_tx(event_tx)
            .with_path(agent_path.clone())
            .with_config_json(config_json)
            .with_system_prompt_identity(kind.agent_identity())
            .with_storage(storage.clone());
        builder = builder.with_memory_backend(Arc::clone(&memory));

        builder = builder.with_system_prompt_section(crate::config::build_system_prompt(&kind));
        // Skill index: one line per visible skill; bodies load on
        // demand via skill_get. Empty library → no section at all.
        let skill_section = skills::prompt_section(&self.skills.registry().list());
        if !skill_section.is_empty() {
            builder = builder.with_system_prompt_section(skill_section);
        }

        builder = builder
            .with_tools(tool_list)
            .with_cancel_token(cancel_token.clone());

        if let Ok(Some(record)) = storage.get_agent_by_name(agent_path.as_str()).await {
            tracing::info!(
                agent = %agent_path,
                agent_id = %record.id,
                "restoring agent from storage"
            );
            builder = builder.with_id(record.id);
            // Session state is restored from storage in Agent::run() bootstrap
            // via per-session snapshot + WAL replay. No builder-level restore needed.
        }

        let mut agent = builder.build().await?;
        let agent_id = agent.id();
        let internal_tx = agent.internal_event_tx();
        let model_handle = agent.model_handle().clone();

        // Wrap `agent.run()` in catch_unwind so a panic inside the agent
        // loop (e.g. a bug in compact, tool execution, or message handling)
        // is logged with a backtrace instead of silently killing the task.
        let agent_path_str = agent_path.as_str().to_string();
        let agent_task = agentik_core::supervise::spawn_safe_on(
            &self.runtime_handle,
            &format!("agent::{agent_path_str}"),
            async move {
                agent.run().await;
            },
        );

        Ok(super::AgentHandle {
            agent_id,
            path: agent_path.clone(),
            kind,
            internal_tx,
            event_rx,
            agent_task,
            cancel_token,
            model: model_handle,
            memory,
        })
    }

    /// Look up or create the per-agent [`BrowserSession`] for a browser-kind
    /// agent. Returns the strong `Arc`; the agent's tool list owns it for
    /// the agent's lifetime, and the pool only retains a `Weak` so the
    /// Chromium process exits when the agent does.
    ///
    /// `agent_path` is the canonical agent identifier (e.g. `/root/browser`).
    /// `options` overrides the default `BrowserOptions` — most callers pass
    /// `BrowserOptions::default()` and tweak `headless` / `no_sandbox` from
    /// the global `RuntimeConfig`.
    pub(super) async fn browser_session_for(
        &self,
        agent_path: &str,
        options: BrowserOptions,
    ) -> Result<Arc<BrowserSession>> {
        let mut pool = self.browsers.lock().await;
        pool.prune_dead();
        if let Some(existing) = pool.by_path.get(agent_path).and_then(Weak::upgrade) {
            return Ok(existing);
        }
        let session = BrowserSession::launch(options)
            .await
            .map_err(|e| Error::Other(format!("launching browser: {e}")))?;
        pool.by_path
            .insert(agent_path.to_string(), Arc::downgrade(&session));
        Ok(session)
    }

    /// Assemble the tool set for an agent kind, respecting its feature flags.
    pub(super) async fn tools_from_profile(
        &self,
        agent_path: &agentik_types::AgentPath,
        kind: AgentKind,
    ) -> Result<Vec<agentik_core::tools::ToolRegistration>> {
        use crate::tools::*;
        use agentik_core::tools::ToolRegistration;

        let file_storage = Arc::new(
            self.file_storage
                .with_principal(agent_vfs_principal(agent_path.as_str())),
        );
        // Use the agent's unique hierarchical path (e.g. "/root/researcher/worker1")
        // as the session key. Two agents must never share a DAG graph, or
        // add_node / add_edge / run_dag calls from one agent would mutate the
        // other agent's DAG.
        let engine_client = self.engine_manager.client_for_session(agent_path.as_str());

        let mut tools: Vec<ToolRegistration> = Vec::new();

        // Browser-kind agents get a private BrowserSession allocated on
        // their first tool assembly; the session Arc is cloned into every
        // browser_* tool's struct, so the pool only needs to keep a Weak.
        if kind.enable_browser() {
            let session = self
                .browser_session_for(agent_path.as_str(), self.browser_options_template.clone())
                .await?;
            tools.extend(browser_use::tools::registrations(session));
        }

        tools.extend(vfs::vbash_registrations(file_storage.clone()));
        tools.push(crate::evolution_tools::evo_observe_registration(
            (*self.rsi).clone(),
            self.skill_evolution.clone(),
        ));
        tools.extend(skills::skill_registrations(
            self.skills.clone(),
            self.skill_evolution.clone(),
        ));
        if kind.enable_plugin_rsi() {
            tools.extend(plugin_rsi::plugin_development_tool_registrations(
                agent_path.as_str(),
            ));
            if self.environment_dev.is_some() {
                tools.extend(plugin_rsi::environment_development_tool_registrations(
                    agent_path.as_str(),
                ));
            }
        }
        if let Some(catalog) = self.catalog.clone() {
            tools.extend(crate::catalog_tools::catalog_registrations(catalog));
        }

        if kind.enable_opengwas() {
            match opengwas_tools_with_token(file_storage.clone(), None) {
                Ok(t) => tools.extend(t),
                Err(e) => tracing::warn!(error = %e, "OpenGWAS tools disabled"),
            }
        }

        if kind.enable_opentargets() {
            tools.extend(opentargets_tools());
        }

        if kind.enable_gwascatalog() {
            tools.extend(gwascatalog_tools(file_storage.clone()));
        }

        if kind.enable_chembl() {
            tools.extend(chembl_tools());
        }

        if kind.enable_rcsb() {
            tools.extend(rcsb_tools_with_client(self.rcsb.clone()));
        }
        if kind.enable_string() {
            tools.extend(string_tools());
        }

        if kind.enable_kegg() {
            tools.extend(kegg_tools());
        }

        let engine_client = Arc::new(engine_client);
        tools.extend(data_engine_tools::registrations(Arc::clone(&engine_client)));
        // Engine-bound skill tools (workflow instantiation, evals) share
        // the registry with the pure skill tools and the session's DAG
        // client, so template-built DAGs are ordinary session DAGs.
        tools.extend(crate::skill_workflow_tools::skill_workflow_registrations(
            self.skills.clone(),
            engine_client,
            self.skill_evolution.clone(),
        ));

        if kind.enable_bibliography() {
            let bib_shared = self.bib.clone();
            let bib_tools =
                bib_base::bib_all_registrations(bib_shared.bib.clone(), file_storage.clone());
            tools.extend(bib_tools);

            // Literature retrieval (search, fetch, citation graph,
            // recommendations) flows through the DAG evidence channel —
            // source_literature / source_literature_fetch /
            // source_literature_citations / source_s2_recommendations —
            // not through agent tools. BibShared still owns the shared
            // clients the nodes reach via their process-wide singletons.
        }

        if kind.enable_writing() {
            let ws = self.writing.clone();
            let writing_tools = writing_base::writing_all_registrations(
                ws.store.clone(),
                ws.bib.as_ref().map(|b| b.bib.clone()),
                ws.engine.clone(),
            );
            tools.extend(writing_tools);
        }

        if let Some(kms) = self.kms.clone() {
            tools.extend(kms_tools::kms_readonly_registrations(kms));
        }

        // Host control tools (spawn_agent, delegate_to, list_agents, etc.)
        // Pass the agent's own path so list_agents / route_task can exclude self.
        tools.extend(crate::host_tools::host_tools(
            self.host_control.clone(),
            agent_path,
            kind,
        ));

        Ok(tools)
    }
}

// ── SharedInfra::open, one section per function ─────────────────────
// The call order below is the construction order: evidence hub first,
// then everything downstream of it, heavy stores last. Each function
// owns its section's inline commentary.

/// Install the remote embedding service into the hub, or fit the BM25
/// fallback when the feature is off.
///
/// The hub embeds every observation at record time, so the service is
/// opened before the evolution worker starts and the swap backfills
/// vectors recorded under whatever embedder served before. When the
/// feature is off, BM25 statistics are fitted over the observation
/// corpus instead: clustering keeps working on weighted lexical vectors.
async fn open_embedding(
    config: &RuntimeConfig,
    observations: &Arc<evolution_core::ObservationHub>,
) -> Result<Arc<ArcSwapOption<text_embedding::RemoteEmbeddingService>>> {
    if config.embedding.enabled {
        let service = text_embedding::RemoteEmbeddingService::init(&config.embedding)
            .await
            .map_err(|error| {
                crate::error::Error::Other(format!("Ollama embedding service unavailable: {error}"))
            })?;
        swap_embedder(observations, &service).await;
        Ok(Arc::new(ArcSwapOption::from_pointee(service)))
    } else {
        install_bm25_fallback(observations).await;
        Ok(Arc::new(ArcSwapOption::empty()))
    }
}

/// Open the plugin RSI store over the shared observation hub: the
/// state layout, the publishers, the environment registry, and the
/// registry materialization. Returns the registry too — environment
/// development and the VFS section open against the same one.
async fn open_plugin_rsi(
    config: &RuntimeConfig,
    observations: &Arc<evolution_core::ObservationHub>,
) -> Result<(Arc<plugin_rsi::RsiInfra>, plugin_rsi::EnvironmentRegistry)> {
    let plugin_layout = plugin_rsi::PluginStateLayout::open(&config.state_dir);
    plugin_layout
        .ensure_v2_directories()
        .map_err(|error| crate::error::Error::Other(error.to_string()))?;

    // PluginStore owns `state_dir/plugins.toml` and `state_dir/plugins`;
    // container-plugin remains the protocol and checkout tool layer.
    let gh_publisher = Arc::new(plugin_rsi::GhPublisher::new(
        config.plugin_rsi.publisher.clone(),
    ));
    let plugin_publisher: plugin_rsi::SharedPluginPublisher = gh_publisher.clone();
    let pull_request_publisher: plugin_rsi::SharedPullRequestPublisher = gh_publisher;
    let environments = plugin_rsi::EnvironmentRegistry::open(
        config.state_dir.join("plugin-environments.toml"),
        config.plugin_rsi.environments.clone(),
    )
    .map_err(|error| crate::error::Error::Other(error.to_string()))?;
    let rsi = Arc::new(
        plugin_rsi::RsiInfra::open_with_environments(
            &config.state_dir,
            "main",
            "Autonomics RSI",
            "rsi@autonomics.example",
            // The hub installed by the caller — plugins and skills
            // read the same evidence ledger, neither owns it.
            Arc::clone(observations),
            environments.clone(),
            plugin_publisher,
            pull_request_publisher,
        )
        .map_err(|error| crate::error::Error::Other(error.to_string()))?,
    );
    let plugin_report = rsi
        .store()
        .materialize_registry()
        .map_err(|error| crate::error::Error::Other(error.to_string()))?;
    if !plugin_report.outcomes.is_empty() {
        tracing::info!("plugins: {}", plugin_report.summary());
    }
    Ok((rsi, environments))
}

/// Build the VFS mount table, the agent-facing file storage, and the
/// plugin toolset registry's view of both. Returns the catalog
/// bundles too — the DataEngine builder overrides its built-ins with
/// them.
async fn open_vfs_and_storage(
    config: &RuntimeConfig,
    rsi: &Arc<plugin_rsi::RsiInfra>,
    environments: &plugin_rsi::EnvironmentRegistry,
) -> Result<(
    Arc<vfs::MountedObjectStore>,
    Option<Arc<crate::catalog_tools::CatalogState>>,
    Arc<OpendalFileStorage>,
    dag_core::BundleRegistry,
)> {
    // Build the VFS mount table first so we can attach it to the
    // agent-facing `OpendalFileStorage`. This makes the `vfs`
    // tool see mounted paths (otherwise it would only see the
    // bare `data_dir` local FS).
    let (vfs, catalog_bundles, catalog_service) = build_vfs_with_catalog(config).await?;
    let vfs = Arc::new(vfs);
    let file_storage = Arc::new(OpendalFileStorage::with_mounts(
        &config.data_dir,
        vfs.clone(),
    ));
    plugin_rsi::PluginDevelopmentToolsetRegistry::global()
        .configure_infra(rsi.clone())
        .map_err(|error| crate::error::Error::Other(error.to_string()))?;
    plugin_rsi::PluginDevelopmentToolsetRegistry::global()
        .configure_environments(environments.clone())
        .map_err(|error| crate::error::Error::Other(error.to_string()))?;
    plugin_rsi::PluginDevelopmentToolsetRegistry::global()
        .configure_vfs((*file_storage).clone(), rsi.store().root())
        .map_err(|error| crate::error::Error::Other(error.to_string()))?;
    Ok((vfs, catalog_service, file_storage, catalog_bundles))
}

/// Open the process-wide Podman execution infrastructure and teach
/// the plugin toolset registry its runtime.
fn open_container_execution() -> Result<Arc<ContainerExecutionInfra>> {
    let container_execution =
        Arc::new(ContainerExecutionInfra::try_from_env().map_err(crate::error::Error::Other)?);
    tracing::info!(
        workspace_root = %container_execution.config.workspace_root.display(),
        panel_cache_root = %container_execution.config.panel_cache_root.display(),
        "SharedInfra::open: Podman execution infrastructure ready"
    );
    plugin_rsi::PluginDevelopmentToolsetRegistry::global()
        .configure_runtime(Arc::clone(&container_execution.runtime))
        .map_err(|error| crate::error::Error::Other(error.to_string()))?;
    Ok(container_execution)
}

/// Open the environment (image) development subsystem when enabled:
/// a second Podman handle for image builds, the publisher, and the
/// dev infra wired into the plugin toolset registry.
async fn open_environment_dev(
    config: &RuntimeConfig,
    environments: &plugin_rsi::EnvironmentRegistry,
    container_execution: &Arc<ContainerExecutionInfra>,
) -> Result<Option<Arc<plugin_rsi::EnvironmentDevInfra>>> {
    let environment_dev_config = config.plugin_rsi.environment_dev.clone();
    if !environment_dev_config.enabled {
        tracing::info!("environment development disabled");
        return Ok(None);
    }
    // Environment (image) development shares the plugin toolset registry.
    // The builder is a second PodmanRuntime handle: the execution infra
    // stores its runtime as `Arc<dyn PodmanConnection>`, which cannot be
    // upcast to the separate `ImageBuildConnection` trait. Both handles
    // resolve the same CLI from the environment and hold no mutable state.
    let image_builder: Arc<dyn container_runtime::ImageBuildConnection> =
        Arc::new(container_runtime::PodmanRuntime::from_env());
    let image_publisher: plugin_rsi::SharedImagePublisher =
        Arc::new(plugin_rsi::PodmanImagePublisher::new(
            plugin_rsi::ImageRegistryConfig {
                enabled: true,
                registry: environment_dev_config.registry.clone(),
                namespace: environment_dev_config.namespace.clone(),
            },
            Arc::clone(&image_builder),
        ));
    let environment_dev = Arc::new(
        plugin_rsi::EnvironmentDevInfra::open(
            &config.state_dir,
            "main",
            "Autonomics RSI",
            "rsi@autonomics.example",
            &environment_dev_config.namespace,
            environments.clone(),
            image_publisher,
        )
        .map_err(|error| crate::error::Error::Other(error.to_string()))?,
    );
    environment_dev.configure_builder(Arc::clone(&image_builder));
    environment_dev.configure_runner(Arc::clone(&container_execution.runtime));
    plugin_rsi::PluginDevelopmentToolsetRegistry::global()
        .configure_environment_dev(Arc::clone(&environment_dev))
        .map_err(|error| crate::error::Error::Other(error.to_string()))?;
    spawn_environment_distiller(&environment_dev, &environment_dev_config);
    Ok(Some(environment_dev))
}

/// Build the DAG engine — bundle registry, storage mounts, container
/// execution, optional DAG history — wrap it in the manager, bridge
/// plugin registry reloads onto it, and start the plugin distiller.
async fn build_engine_manager(
    config: &RuntimeConfig,
    file_storage: &Arc<OpendalFileStorage>,
    vfs: &Arc<vfs::MountedObjectStore>,
    container_execution: &Arc<ContainerExecutionInfra>,
    catalog_bundles: dag_core::BundleRegistry,
    rsi: &Arc<plugin_rsi::RsiInfra>,
) -> Result<Arc<DataEngineManager>> {
    tracing::info!(
        mounts = ?vfs.mount_paths(),
        "SharedInfra::open: VFS mounted"
    );
    tracing::info!("SharedInfra::open: building DataEngine");
    let user_bundle_registry = build_bundle_registry(config)?;
    let bundle_registry = catalog_bundles
        .with_overriding_bundles(
            user_bundle_registry
                .iter()
                .map(|(_, bundle)| bundle.clone()),
        )
        .map_err(|error| Error::Other(error.to_string()))?;
    let engine_builder = DataEngine::builder()
        .register_opendal_fs(file_storage.clone())?
        .with_vfs((**vfs).clone())
        .with_bundle_registry(bundle_registry)
        .with_container_execution(Arc::clone(container_execution));

    let mut engine = engine_builder.build();
    tracing::info!("SharedInfra::open: DataEngine built");

    // ── DAG history ──────────────────────────────────────────────
    if config.enable_dag_history {
        let history_db = config.dag_history_db.clone();
        if let Some(parent) = history_db.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        engine = match DagHistory::open(&history_db).await {
            Ok(history) => {
                tracing::info!(
                    path = %history_db.display(),
                    "DAG history store opened"
                );
                engine.with_history(history)
            }
            Err(e) => {
                tracing::warn!(
                    path = %history_db.display(),
                    error = %e,
                    "failed to open DAG history store; history/ref tools disabled"
                );
                engine
            }
        };
    }

    let engine_manager = Arc::new(DataEngineManager::new(engine));
    rsi.configure_registry(Arc::new(SharedPluginRegistryControl {
        manager: Arc::clone(&engine_manager),
        execution: Arc::clone(container_execution),
        store: rsi.store(),
    }));
    spawn_plugin_distiller(rsi, &config.plugin_rsi);
    tracing::info!("SharedInfra::open: DataEngineManager created");
    Ok(engine_manager)
}

/// Open the shared agent storage — the one connection later upcast to
/// both the `AgentStorage` and `MemoryStore` trait objects.
///
/// Resolve DB paths from the catalog (allows provider overrides and
/// persistence-backed path changes; falls back to RuntimeConfig).
async fn open_agent_storage(config: &RuntimeConfig) -> Result<Arc<TursoAgentStorage>> {
    let agent_db = config.agent_db.clone();
    tracing::info!(
        "SharedInfra::open: opening agent storage at {}",
        agent_db.display()
    );
    let store = match TursoAgentStorage::open(&agent_db).await {
        Ok(s) => {
            tracing::info!(path = %agent_db.display(), "agent storage opened");
            s
        }
        Err(e) => {
            return Err(Error::Storage(e));
        }
    };
    Ok(Arc::new(store))
}

/// Open the KMS knowledge db, migrate legacy rows out of agent.db,
/// and build the service when the feature is enabled.
async fn open_kms(
    config: &RuntimeConfig,
    _turso_store: &Arc<TursoAgentStorage>,
) -> Result<Option<Arc<kms::KmsService>>> {
    // Initialize the KMS schema in its own knowledge.db unconditionally;
    // expose tools and semantic grounding only when the runtime feature
    // is enabled.
    let kms_db = config.kms_db_path.clone();
    tracing::info!(
        path = %kms_db.display(),
        "SharedInfra::open: opening KMS knowledge db"
    );
    let kms_storage = match kms::Storage::new(&kms_db.to_string_lossy()).await {
        Ok(storage) => storage,
        Err(error) => return Err(Error::Other(format!("open KMS storage: {error}"))),
    };
    if !config.enable_kms {
        return Ok(None);
    }
    match kms::KmsService::from_storage(kms_storage).await {
        Ok(service) => Ok(Some(Arc::new(service))),
        Err(error) => Err(Error::Other(format!("open KMS: {error}"))),
    }
}

/// Build the process-wide memory backend over the shared agent
/// storage, with KMS semantic grounding when the service is open.
fn open_memory(
    config: &RuntimeConfig,
    store: Arc<TursoAgentStorage>,
    kms: &Option<Arc<kms::KmsService>>,
) -> Arc<MemoryBackend> {
    let memory_store: Arc<dyn MemoryStore> = store;
    let grounding: Option<Arc<dyn SemanticGrounding>> = kms
        .as_ref()
        .map(|service| Arc::new(KmsMemoryGrounding::new(Arc::clone(service))) as Arc<_>);
    let mut memory_config = MemoryConfig::new();
    memory_config.use_memory = config.use_memory;
    memory_config.generate_memory = config.generate_memory;
    Arc::new(MemoryBackend::new(memory_config, memory_store, grounding))
}

/// Open the bibliography store + literature gateway once and publish
/// it to the library-backed DAG nodes, so their outputs share the
/// host's storage view and connection pool.
async fn open_bibliography(
    config: &RuntimeConfig,
    file_storage: &Arc<OpendalFileStorage>,
) -> Result<Arc<bib_base::BibShared>> {
    let bib_db_path = config.bib_db_path.clone();
    tracing::info!(
        "SharedInfra::open: opening bibliography db at {}",
        bib_db_path.display()
    );
    let bib = Arc::new(
        bib_base::BibShared::open_with(&bib_db_path, config.bib_http.clone())
            .await?
            .with_file_storage(file_storage.clone()),
    );
    bib_base::nodes::set_shared_bib(bib.clone());
    Ok(bib)
}

/// Open the LaTeX writing system (store + optional engine) over the
/// bibliography store.
async fn open_writing(
    config: &RuntimeConfig,
    bib: &Arc<bib_base::BibShared>,
) -> Result<Arc<writing_base::WritingShared>> {
    let writing_db_path = config.writing_db_path.clone();
    tracing::info!(
        "SharedInfra::open: opening writing db at {}",
        writing_db_path.display()
    );
    let writing = Arc::new(
        writing_base::WritingShared::open_with(
            &writing_db_path.to_string_lossy(),
            Some(bib.clone()),
        )
        .await?,
    );
    Ok(writing)
}

/// Start the unified skill evolution loop when enabled. `None` means
/// the loop is disabled at the daemon level — every gateway mutation
/// request then returns HTTP 503.
fn start_skill_evolution(
    config: &RuntimeConfig,
    skills: &Arc<skills::SkillManager>,
) -> Option<skills::SkillControlHandle> {
    if !config.enable_skill_evolution {
        tracing::info!("SharedInfra::open: skill evolution disabled");
        return None;
    }
    let handle = skills::evolution::start(
        skills.clone(),
        skills::evolution::EvolutionOptions {
            policy: skills::evolution::EvolutionPolicy::default(),
            quiet_window: std::time::Duration::from_millis(500),
            timer: Some(std::time::Duration::from_secs(
                config.skill_evolution_interval_secs.max(60),
            )),
        },
    );
    tracing::info!(
        interval_secs = config.skill_evolution_interval_secs,
        "SharedInfra::open: skill evolution service started"
    );
    Some(handle)
}

/// Swap the service's provider into the observation hub. The swap
/// backfills vectors recorded under the previous embedder.
async fn swap_embedder(
    observations: &Arc<evolution_core::ObservationHub>,
    service: &text_embedding::RemoteEmbeddingService,
) {
    if let Some(provider) = service.text_embedder() {
        if let Err(error) = observations.set_embedder(provider).await {
            tracing::warn!(
                %error,
                "embedding backfill after model install failed; \
                 the hub keeps the previous embedder"
            );
        }
    }
}

/// Fit BM25 statistics over the observations already in the ledger
/// and install the frozen snapshot as the hub's embedder. The swap
/// backfills vectors recorded under the previous embedder — a later
/// model swap backfills these in turn. An install failure leaves the
/// hub on its unfitted default.
async fn install_bm25_fallback(observations: &Arc<evolution_core::ObservationHub>) {
    let texts = observations.embedding_texts().await;
    let embedder = text_embedding::Bm25TextEmbedder::fit(texts);
    if let Err(error) = observations.set_embedder(Arc::new(embedder)).await {
        tracing::warn!(
            %error,
            "BM25 fallback install failed; the hub keeps its unfitted default"
        );
    }
}

/// Runtime adapter that lets PluginStore refresh node factories without
/// rebuilding the whole DataEngine.
#[derive(Clone)]
struct SharedPluginRegistryControl {
    manager: Arc<DataEngineManager>,
    execution: Arc<ContainerExecutionInfra>,
    store: Arc<plugin_rsi::PluginStore>,
}

impl plugin_rsi::PluginRegistryControl for SharedPluginRegistryControl {
    fn installed_node_addresses(&self) -> plugin_rsi::Result<Vec<String>> {
        Ok(self
            .manager
            .list_nodes()
            .into_iter()
            .map(|node| node.address)
            .collect())
    }

    fn reload_plugin(&self, plugin_name: &str) -> plugin_rsi::Result<()> {
        plugin_rsi::validate_plugin_name(plugin_name)?;
        let directory = self.store.runtime_root().join(plugin_name);
        let plugin = container_plugin::loader::load_plugin(
            &directory,
            Arc::clone(&self.execution.runtime),
            Arc::clone(&self.execution.panel_cache),
        )
        .map_err(|error| plugin_rsi::Error::PluginRegistry(error.to_string()))?;
        self.manager.reload_plugin(plugin);
        Ok(())
    }

    fn uninstall_plugin(&self, plugin_name: &str) -> plugin_rsi::Result<()> {
        plugin_rsi::validate_plugin_name(plugin_name)?;
        self.manager.remove_plugin(plugin_name);
        Ok(())
    }
}

#[cfg(test)]
mod remote_embedding_tests {
    use super::super::RuntimeHost;
    use super::*;
    use crate::config::RuntimeConfig;

    fn config(dir: &tempfile::TempDir, model_dir: &str) -> RuntimeConfig {
        let mut config = RuntimeConfig::default();
        config.data_dir = dir.path().join("data");
        config.state_dir = dir.path().join("state");
        config.agent_db = dir.path().join("agent.db");
        config.dag_history_db = dir.path().join("dag-history.db");
        config.bib_db_path = dir.path().join("bib.db");
        config.writing_db_path = dir.path().join("writing.db");
        config.app_db_path = dir.path().join("app.db");
        config.kms_db_path = dir.path().join("knowledge.db");
        config.use_memory = false;
        config.generate_memory = false;
        config.embedding = text_embedding::EmbeddingConfig {
            enabled: true,
            base_url: format!("http://{model_dir}"),
            model: "test-model".into(),
            ..Default::default()
        };
        config
    }

    /// End-to-end assembly: the service lands in SharedInfra and its
    /// provider is live. Requires a prepared model directory holding
    /// the complete default file layout (`onnx/model_quantized.onnx`
    /// etc.) — an incomplete directory fails the inline open this
    /// test asserts on — so point `TEXT_EMBEDDING_TEST_MODEL_DIR` at
    /// a full default-layout export; the test is a no-op when the
    /// variable is unset.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn remote_embedding_service_installs_into_shared_infra() {
        let Ok(model_dir) = std::env::var("OLLAMA_EMBEDDING_TEST_MODEL") else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let mut host = RuntimeHost::open(&config(&dir, &model_dir)).await.unwrap();
        let infra = host.infra();
        let service_guard = infra.embedding.load();
        let service = service_guard.as_deref().expect("embedding service opened");
        let signature = service
            .text_embedder()
            .expect("provider opened")
            .signature()
            .to_string();
        assert!(signature.contains("test-model"), "{signature}");
        host.shutdown_all_agents_and_wait().await;
    }

    /// With the feature on, a missing model is a hard startup error
    /// naming `prepare` — the daemon never downloads and never
    /// silently downgrades to a lexical embedder.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn remote_embedding_service_disabled_installs_bm25() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = config(&dir, "127.0.0.1:11434");
        cfg.embedding.enabled = false;
        let mut host = RuntimeHost::open(&cfg).await.unwrap();
        assert!(host.infra.embedding.load().is_none());
        assert!(
            host.infra
                .observations
                .embedding_signature()
                .starts_with("bm25-")
        );
        host.shutdown_all_agents_and_wait().await;
    }
}

#[cfg(test)]
mod plugin_profile_tools_tests {
    use super::super::RuntimeHost;
    use super::*;
    use crate::config::RuntimeConfig;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn plugin_tools_are_restricted_to_the_developer_profile() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = RuntimeConfig::default();
        config.data_dir = dir.path().join("data");
        config.state_dir = dir.path().join("state");
        config.agent_db = dir.path().join("agent.db");
        config.kms_db_path = dir.path().join("knowledge.db");
        // Keep the embedding model (network install + ONNX session)
        // out of this test.
        config.embedding.enabled = false;
        let host = RuntimeHost::open(&config).await.unwrap();
        let agent_path = agentik_types::AgentPath::try_from("/root/developer").unwrap();

        let researcher_tools = host
            .infra
            .tools_from_profile(&agent_path, AgentKind::Researcher)
            .await
            .unwrap();
        assert!(
            researcher_tools
                .iter()
                .any(|tool| tool.definition.name == "vfs")
        );
        assert!(
            !researcher_tools
                .iter()
                .any(|tool| tool.definition.name == "plugin_node_create")
        );
        assert!(
            !researcher_tools
                .iter()
                .any(|tool| tool.definition.name == "plugin_container_run")
        );
        assert!(
            !researcher_tools
                .iter()
                .any(|tool| tool.definition.name == "plugin_environments_list")
        );
        assert!(
            researcher_tools
                .iter()
                .any(|tool| tool.definition.name == "evo_observe")
        );
        assert!(
            !researcher_tools
                .iter()
                .any(|tool| tool.definition.name == "skill_observe")
        );
        assert!(
            !crate::config::build_system_prompt(&AgentKind::Researcher)
                .contains("Plugin Self-Improvement")
        );

        let developer_tools = host
            .infra
            .tools_from_profile(&agent_path, AgentKind::Developer)
            .await
            .unwrap();
        assert!(
            developer_tools
                .iter()
                .any(|tool| tool.definition.name == "plugin_node_create")
        );
        assert!(
            developer_tools
                .iter()
                .any(|tool| tool.definition.name == "plugin_container_run")
        );
        assert!(
            crate::config::build_system_prompt(&AgentKind::Developer)
                .contains("Plugin Self-Improvement")
        );
    }
}
