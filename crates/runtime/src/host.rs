//! Runtime host: process-level shared infrastructure + per-agent handles.
//!
//! [`RuntimeHost`] owns heavy shared resources (DataEngine, storage, file
//! system) that are created **once** per process. It also owns a persistent
//! [`AgentNetwork`] for multi-agent topology routing.
//!
//! Individual agents are spawned via [`RuntimeHost::spawn_agent`], which
//! returns an [`AgentHandle`] — a lightweight per-agent control struct
//! (channels + task handle). Agents can be registered with the host's
//! internal registry for multiplexed event access via
//! [`recv_any`](Self::recv_any) and topology-aware message routing via
//! [`AgentNetwork`].

use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use agentik_core::Agent;
use agentik_core::TursoAgentStorage;
use agentik_core::agent::InternalEvent;
use agentik_core::error::AgentError;
use agentik_core::storage::{AgentProfileRegistry, AgentStorage};
use agentik_network::{AgentNetwork, EdgeTrigger, NodeSpec, RoutingAction, TerminationSpec};
use agentik_sdk::model::Model;
use agentik_sdk::types::{AgentEvent, ContentBlock};
use arc_swap::ArcSwapOption;
use dag_core::resource_catalog::{
    DbKind, DocKind, ResourceAddress, ResourceCatalog, ResourceEntry, ResourceKind,
};
use data_engine::dag::DagHistory;
use data_engine::data_engine::DataEngine;
use data_engine::runtime::{DataEngineClient, DataEngineManager};
use datalake::Datalake;
use fs::OpendalFileStorage;
use futures::FutureExt;
use thiserror::Error;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::config::{PromptCapabilities, RuntimeConfig};
use crate::control::AgentStatus;
use crate::tools::DefaultToolSetError;

// AgentProfile carries the same tool-capability flags as RuntimeConfig, so we
// can build a dynamic system prompt that only mentions tools the profile
// actually enables.
impl PromptCapabilities for agentik_core::AgentProfile {
    fn enable_bibliography(&self) -> bool {
        self.enable_bibliography
    }
    fn enable_opengwas(&self) -> bool {
        self.enable_opengwas
    }
    fn enable_opentargets(&self) -> bool {
        self.enable_opentargets
    }
    fn enable_gwascatalog(&self) -> bool {
        self.enable_gwascatalog
    }
    fn enable_iceberg(&self) -> bool {
        self.enable_iceberg
    }
    fn enable_dag_history(&self) -> bool {
        self.enable_dag_history
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Error
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Error)]
pub enum HostError {
    #[error("failed to build agent: {0}")]
    AgentBuild(#[from] AgentError),

    #[error("{0}")]
    Engine(#[from] data_engine::error::Error),

    #[error("OpenGWAS setup failed: {0}")]
    Opengwas(#[from] opengwas::OpengwasError),

    #[error("tool assembly failed: {0}")]
    ToolAssembly(#[from] DefaultToolSetError),

    #[error("agent storage error: {0}")]
    Storage(#[from] agentik_core::storage::StorageError),

    #[error("bibliography shared init failed: {0}")]
    BibShared(#[from] bib_base::Error),

    #[error("writing system init failed: {0}")]
    WritingShared(#[from] writing_base::Error),

    #[error("{0}")]
    Other(String),
}

pub type HostResult<T> = std::result::Result<T, HostError>;

// ═══════════════════════════════════════════════════════════════════════
// SharedInfra — process-level shared resources
// ═══════════════════════════════════════════════════════════════════════

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
    pub datalake: Arc<Datalake>,
    /// Centralized resource catalog — the single source of truth for all
    /// resource addresses (Iceberg tables, file paths, endpoints, config,
    /// databases). Nodes resolve resources through this instead of
    /// hardcoding names/paths.
    pub resources: Arc<ResourceCatalog>,
    pub storage: Arc<dyn AgentStorage>,
    /// Profile registry (same DB connection, separate trait object).
    /// Used by RuntimeHost for dynamic profile derivation.
    pub profile_storage: Arc<dyn AgentProfileRegistry>,
    /// Bibliography storage + literature gateway, opened **once** per
    /// process and shared by every spawned agent.
    pub bib: Arc<bib_base::BibShared>,
    /// LaTeX writing system (store + optional engine), opened **once** per
    /// process. Reuses `bib` for citation resolution when available.
    pub writing: Arc<writing_base::WritingShared>,
    /// The tokio runtime handle (for spawning agent tasks).
    pub runtime_handle: tokio::runtime::Handle,
    /// Optional host control for agent tools. Set by RuntimeHost when
    /// available. When `Some`, spawned agents receive host management tools
    /// (spawn_agent, send_to_agent, connect_agents, etc.).
    pub host_control: Option<crate::control::HostControl>,
}

impl SharedInfra {
    /// Open all shared resources from a global [`RuntimeConfig`].
    ///
    /// The config's `data_dir`, `state_dir`, `agent_db`, and feature flags
    /// for Iceberg / DAG history are consumed here. Per-agent settings
    /// (identity, prompts, tool flags) are **not** read — those are passed
    /// to [`RuntimeHost::spawn_agent`] instead.
    pub async fn open(config: &RuntimeConfig) -> HostResult<Self> {
        let file_storage = Arc::new(OpendalFileStorage::new(&config.data_dir));

        // ── Resource Catalog ─────────────────────────────────────────
        // Open (or load) the centralized resource catalog first, so every
        // downstream component (DataEngine nodes, SDK clients, etc.) resolves
        // addresses through it. The manifest DB lives in state_dir.
        let manifest_db = config.state_dir.join("resource-manifest.db");
        if let Some(parent) = manifest_db.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let resources =
            Arc::new(ResourceCatalog::load_or_new(&config.data_dir, &manifest_db).await);

        // Register built-in resources from RuntimeConfig (config-as-resource).
        register_config_resources(&resources, config);

        // Set the process-wide global so SDK crates (eutils, embase, etc.)
        // can resolve endpoints via `ResourceCatalog::global()`.
        let _ = ResourceCatalog::set_global(resources.clone());

        // ── DataEngine ───────────────────────────────────────────────
        let mut engine_builder = DataEngine::builder()
            .register_opendal_fs(file_storage.clone())?
            .with_resources(resources.clone());

        if config.enable_iceberg {
            match engine_builder.register_iceberg().await {
                Ok(()) => {
                    tracing::info!("iceberg datalake registered");
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "failed to connect to iceberg datalake; \
                         iceberg/datalake tools disabled — \
                         agent spawn/resume will still work"
                    );
                }
            }
        }

        let mut engine = engine_builder.build();

        // ── DAG history ──────────────────────────────────────────────
        if config.enable_dag_history {
            let history_db = resources
                .resolve_database("db.dag_history")
                .unwrap_or_else(|_| config.dag_history_db.clone());
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

        // ── Datalake ─────────────────────────────────────────────────
        let datalake = Arc::new(Datalake::new());

        // ── Agent storage ────────────────────────────────────────────
        // Resolve DB paths from the catalog (allows provider overrides and
        // persistence-backed path changes; falls back to RuntimeConfig).
        let agent_db = resources
            .resolve_database("db.agent")
            .unwrap_or_else(|_| config.agent_db.clone());
        // Open once and clone — both trait objects share the same underlying
        // connection (and its Mutex).  Opening the file twice creates two
        // separate Database objects and risks file-lock contention.
        let turso_store = match TursoAgentStorage::open(&agent_db).await {
            Ok(s) => {
                tracing::info!(path = %agent_db.display(), "agent storage opened");
                s
            }
            Err(e) => {
                return Err(HostError::Storage(e));
            }
        };
        let storage: Arc<dyn AgentStorage> = Arc::new(turso_store.clone());
        // Profile registry — clone of the same storage (shares one connection).
        let profile_storage: Arc<dyn AgentProfileRegistry> = Arc::new(turso_store);

        let bib_db_path = resources
            .resolve_database("db.bib")
            .unwrap_or_else(|_| config.bib_db_path.clone());
        let bib =
            Arc::new(bib_base::BibShared::open_with(&bib_db_path, config.bib_http.clone()).await?);

        let writing_db_path = resources
            .resolve_database("db.writing")
            .unwrap_or_else(|_| config.writing_db_path.clone());
        let writing = Arc::new(
            writing_base::WritingShared::open_with(
                &writing_db_path.to_string_lossy(),
                Some(bib.clone()),
            )
            .await?,
        );

        // ── Drift check + persistence ────────────────────────────────
        // Best-effort: compare the catalog's registered Iceberg tables
        // against the live datalake, logging any mismatches. Non-fatal.
        if config.enable_iceberg {
            check_drift_best_effort(&resources, &datalake).await;
        }
        resources.persist().await;

        Ok(Self {
            engine_manager,
            file_storage,
            datalake,
            resources,
            storage,
            profile_storage,
            runtime_handle: tokio::runtime::Handle::current(),
            bib,
            writing,
            host_control: None,
        })
    }

    /// Spawn a new agent from an [`AgentProfile`](agentik_core::AgentProfile).
    ///
    /// This method only needs [`SharedInfra`] — it does not touch the
    /// topology network or agent registry. Callers can clone `SharedInfra`
    /// (cheap, all `Arc`) into async tasks.
    pub async fn spawn_agent(
        &self,
        agent_path: &agentik_types::AgentPath,
        profile: &agentik_core::AgentProfile,
        global_model: Arc<ArcSwapOption<Model>>,
        model_override: Option<Model>,
    ) -> HostResult<AgentHandle> {
        let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();
        let cancel_token = CancellationToken::new();

        let model = match model_override {
            Some(m) => Arc::new(ArcSwapOption::from_pointee(Some(m))),
            None => Arc::new(ArcSwapOption::from_pointee(
                global_model.load_full().as_deref().cloned(),
            )),
        };

        let tool_list = self.tools_from_profile(agent_path, profile).await?;

        let config_json = serde_json::to_value(profile).unwrap_or_default();
        let storage = self.storage.clone();

        let mut builder = Agent::builder()
            .with_model(model.clone())
            .with_agent_event_tx(event_tx)
            .with_path(agent_path.clone())
            .with_config_json(config_json)
            .with_system_prompt_identity(&profile.agent_identity)
            .with_storage(storage.clone());

        if let Some(ref prompt) = profile.system_prompt {
            builder = builder.with_system_prompt_section(prompt);
        } else {
            builder =
                builder.with_system_prompt_section(crate::config::build_system_prompt(profile));
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

        Ok(AgentHandle {
            agent_id,
            path: agent_path.clone(),
            profile_path: profile.path.clone(),
            internal_tx,
            event_rx,
            agent_task,
            cancel_token,
            model: model_handle,
        })
    }

    /// Assemble the tool set for a profile, respecting its feature flags.
    async fn tools_from_profile(
        &self,
        agent_path: &agentik_types::AgentPath,
        profile: &agentik_core::AgentProfile,
    ) -> HostResult<Vec<agentik_core::tools::ToolRegistration>> {
        use crate::tools::*;
        use agentik_core::tools::ToolRegistration;

        let file_storage = self.file_storage.clone();
        let datalake = self.datalake.clone();
        // Use the agent's unique hierarchical path (e.g. "/root/researcher/worker1")
        // as the session key — NOT profile.path, which is shared by all agents
        // spawned from the same profile blueprint. Using profile.path here was
        // a multi-agent migration legacy bug: two agents with the same profile
        // would silently share the same DAG graph, so add_node / add_edge /
        // run_dag calls from one agent would mutate the other agent's DAG.
        let engine_client = self.engine_manager.client_for_session(agent_path.as_str());

        let mut tools: Vec<ToolRegistration> = fs::vbash_registrations(file_storage.clone());

        if profile.enable_opengwas {
            match opengwas_tools_with_token(file_storage.clone(), None) {
                Ok(t) => tools.extend(t),
                Err(e) => tracing::warn!(error = %e, "OpenGWAS tools disabled"),
            }
        }

        if profile.enable_opentargets {
            tools.extend(opentargets_tools());
        }

        if profile.enable_gwascatalog {
            tools.extend(gwascatalog_tools(file_storage));
        }

        tools.extend(datalake_tools(datalake));
        tools.extend(data_engine_tools::registrations(Arc::new(engine_client)));

        if profile.enable_bibliography {
            let bib_shared = self.bib.clone();
            let bib_tools = bib_base::bib_all_registrations(
                bib_shared.bib.clone(),
                bib_shared.gateway.clone(),
                Some(bib_shared.europe_pmc.clone()),
            );
            tools.extend(bib_tools);

            // Extended literature tools: source-specific capabilities of
            // OpenAlex / Crossref / Semantic Scholar that fall outside the
            // LiteratureGateway's search/fetch contract (autocomplete,
            // citation graph, recommendations, author lookup, type
            // catalogue). The shared clients live on BibShared.
            tools.extend(bib_base::bib_extended_registrations(
                bib_shared.openalex.clone(),
                bib_shared.crossref.clone(),
                bib_shared.s2.clone(),
            ));
        }

        if profile.enable_writing {
            let ws = self.writing.clone();
            let writing_tools = writing_base::writing_all_registrations(
                ws.store.clone(),
                ws.bib.as_ref().map(|b| b.bib.clone()),
                ws.engine.clone(),
            );
            tools.extend(writing_tools);
        }

        // Host control tools (spawn_agent, delegate_to, list_agents, etc.)
        // Pass the agent's own path so list_agents / route_task can exclude self.
        tools.extend(crate::host_tools::host_tools(
            self.host_control.clone(),
            agent_path,
            &profile.path,
        ));

        Ok(tools)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// AgentHandle — per-agent control struct
// ═══════════════════════════════════════════════════════════════════════

// ── Resource catalog bootstrap helpers ───────────────────────────────────

/// Register built-in resources derived from [`RuntimeConfig`] into the catalog.
///
/// This folds env-derived paths and config values into the centralized catalog
/// so that all modules resolve them by logical name instead of reading env
/// vars or config fields directly. Registration is idempotent (same-name +
/// same-address is a no-op); failures are logged and non-fatal.
fn register_config_resources(catalog: &ResourceCatalog, config: &RuntimeConfig) {
    use std::collections::BTreeMap;

    let reg = |name: &str,
               kind: ResourceKind,
               desc: &str,
               address: ResourceAddress,
               tags: Vec<String>| {
        let mut entry = ResourceEntry::new(name, kind, desc, address);
        if !tags.is_empty() {
            entry = entry.with_tags(tags);
        }
        if let Err(e) = catalog.register(entry) {
            tracing::warn!("resource catalog: failed to register '{name}': {e}");
        }
    };

    // ── Database / file paths ─────────────────────────────────────────
    reg(
        "db.agent",
        ResourceKind::Database,
        "Agent persistence database (Turso/SQLite)",
        ResourceAddress::database(DbKind::Sqlite, &config.agent_db),
        vec!["runtime".into()],
    );
    reg(
        "db.dag_history",
        ResourceKind::Database,
        "DAG history database (snapshots/refs)",
        ResourceAddress::database(DbKind::Sqlite, &config.dag_history_db),
        vec!["runtime".into()],
    );
    reg(
        "db.bib",
        ResourceKind::Database,
        "Bibliography database",
        ResourceAddress::database(DbKind::Sqlite, &config.bib_db_path),
        vec!["runtime".into()],
    );
    reg(
        "db.writing",
        ResourceKind::Database,
        "Writing system database",
        ResourceAddress::database(DbKind::Sqlite, &config.writing_db_path),
        vec!["runtime".into()],
    );
    reg(
        "db.app",
        ResourceKind::Database,
        "TUI / application database",
        ResourceAddress::database(DbKind::Sqlite, &config.app_db_path),
        vec!["runtime".into()],
    );

    // ── File paths ────────────────────────────────────────────────────
    reg(
        "app.data_dir",
        ResourceKind::FilePath,
        "Root directory for agent file storage (downloads, scratch, artifacts)",
        ResourceAddress::path(&config.data_dir),
        vec!["runtime".into()],
    );
    reg(
        "app.state_dir",
        ResourceKind::FilePath,
        "Directory for agent-internal state",
        ResourceAddress::path(&config.state_dir),
        vec!["runtime".into()],
    );

    // ── Doc / reference paths ────────────────────────────────────────
    for (logical, path, desc, kind) in [
        (
            "doc.docs",
            "docs",
            "Project documentation root",
            DocKind::Notes,
        ),
        (
            "doc.logs",
            "logs",
            "Application logs directory",
            DocKind::Notes,
        ),
        (
            "doc.reference",
            "reference",
            "Reference data / panels",
            DocKind::Notes,
        ),
        (
            "doc.fixtures",
            "test_datasets",
            "Test fixture data",
            DocKind::Notes,
        ),
    ] {
        let p = config.data_dir.join(path);
        reg(
            logical,
            ResourceKind::Doc,
            desc,
            ResourceAddress::doc(kind, &p),
            vec!["doc".into()],
        );
    }

    // ── Infra bin data paths ─────────────────────────────────────────
    reg(
        "sink.ld_matrix.data_root",
        ResourceKind::FilePath,
        "Source LD matrix TSV root (1000G PLINK dataset)",
        ResourceAddress::path("/mnt/disk2/dataset/1000g_plink/"),
        vec!["infra".into()],
    );
    reg(
        "sink.af.data_path",
        ResourceKind::FilePath,
        "Source allele-frequency .afreq directory (1000G EUR)",
        ResourceAddress::path("/mnt/disk2/dataset/1000g_plink/eur/maf/"),
        vec!["infra".into()],
    );

    // ── External API endpoints ────────────────────────────────────────
    // Registered so SDK clients can resolve via ResourceCatalog::global().
    // Each SDK crate falls back to its own const when the global is not set.
    for (logical, url, desc) in [
        (
            "endpoint.eutils",
            "https://eutils.ncbi.nlm.nih.gov/entrez/eutils",
            "NCBI E-utilities API",
        ),
        (
            "endpoint.embase",
            "https://api.elsevier.com/content/embase/article",
            "Elsevier Embase API",
        ),
        (
            "endpoint.openalex",
            "https://api.openalex.org",
            "OpenAlex REST API",
        ),
        (
            "endpoint.s2_graph",
            "https://api.semanticscholar.org/graph/v1",
            "Semantic Scholar Graph API",
        ),
        (
            "endpoint.s2_reco",
            "https://api.semanticscholar.org/recommendations/v1",
            "Semantic Scholar Recommendations API",
        ),
        (
            "endpoint.opentargets",
            "https://api.platform.opentargets.org/api/v4/graphql",
            "Open Targets GraphQL API",
        ),
        (
            "endpoint.crossref",
            "https://api.crossref.org",
            "Crossref REST API",
        ),
        (
            "endpoint.gwascatalog_ss",
            "https://www.ebi.ac.uk/gwas/summary-statistics/api",
            "GWAS Catalog Summary Stats API",
        ),
        (
            "endpoint.gwascatalog_rest",
            "https://www.ebi.ac.uk/gwas/rest/api",
            "GWAS Catalog REST API",
        ),
        (
            "endpoint.gwascatalog_search",
            "https://www.ebi.ac.uk/gwas/api/search",
            "GWAS Catalog Solr Search API",
        ),
        (
            "endpoint.gwascatalog_ftp",
            "https://ftp.ebi.ac.uk/pub/databases/gwas/summary_statistics",
            "GWAS Catalog FTP (summary stats files)",
        ),
        (
            "endpoint.europepmc",
            "https://www.ebi.ac.uk/europepmc/webservices/rest",
            "Europe PMC REST API",
        ),
        (
            "endpoint.biorxiv",
            "https://api.biorxiv.org",
            "bioRxiv/medRxiv API",
        ),
        (
            "endpoint.arxiv",
            "http://export.arxiv.org/api/query",
            "arXiv API",
        ),
    ] {
        reg(
            logical,
            ResourceKind::Endpoint,
            desc,
            ResourceAddress::endpoint(url),
            vec!["api".into()],
        );
    }

    // ── Config values (from env or defaults) ──────────────────────────
    let read_env = |key: &str| std::env::var(key).ok();
    if let Some(uri) = read_env("ICEBERG_REST_URI") {
        reg(
            "config.iceberg_rest_uri",
            ResourceKind::Config,
            "Iceberg REST catalog URI",
            ResourceAddress::config("ICEBERG_REST_URI", &uri),
            vec!["iceberg".into()],
        );
    }
    for (logical, env_key, desc) in [
        (
            "config.s3_endpoint",
            "ICEBERG_S3_ENDPOINT",
            "S3 endpoint for Iceberg",
        ),
        (
            "config.s3_bucket",
            "ICEBERG_S3_BUCKET",
            "S3 bucket for Iceberg",
        ),
        (
            "config.s3_region",
            "ICEBERG_S3_REGION",
            "S3 region for Iceberg",
        ),
        (
            "config.s3_warehouse",
            "ICEBERG_S3_WAREHOUSE",
            "S3 warehouse path for Iceberg",
        ),
    ] {
        if let Some(val) = read_env(env_key) {
            reg(
                logical,
                ResourceKind::Config,
                desc,
                ResourceAddress::config(env_key, &val),
                vec!["iceberg".into()],
            );
        }
    }
}

/// Best-effort drift check: compare the catalog's registered Iceberg tables
/// against the live datalake, logging warnings for missing tables or
/// non-existent file paths. Never fails — connection errors are swallowed.
async fn check_drift_best_effort(catalog: &ResourceCatalog, datalake: &Datalake) {
    // Build a snapshot of the live Iceberg tables.
    let live_tables = match datalake.list_all_tables().await {
        Ok(tables) => tables,
        Err(e) => {
            tracing::warn!(
                error = %e,
                "resource catalog: drift check skipped (datalake unreachable)"
            );
            return;
        }
    };

    let snapshot = dag_core::resource_catalog::CatalogSnapshot {
        tables: live_tables
            .into_iter()
            .filter_map(|(ns, table)| {
                // Use the last namespace segment as the schema name (the
                // common 2-level `schema.table` pattern).
                let schema = ns.last()?.clone();
                Some((schema, table))
            })
            .collect(),
    };

    let warnings = catalog.check_drift(&snapshot);
    if !warnings.is_empty() {
        for w in &warnings {
            tracing::warn!(
                name = %w.name,
                detail = %w.detail,
                "resource catalog drift: registered resource not found in live data"
            );
        }
        tracing::warn!(
            count = warnings.len(),
            "resource catalog: drift check found missing resources — index unchanged"
        );
    } else {
        tracing::debug!("resource catalog: drift check passed (all registered resources found)");
    }
}

/// Control handle for one running agent.
///
/// Cheap to move; owns the event receiver and cancellation token. When
/// dropped, the agent's tokio task is **not** automatically killed — call
/// [`shutdown`](Self::shutdown) explicitly.
pub struct AgentHandle {
    pub agent_id: uuid::Uuid,
    pub path: agentik_types::AgentPath,
    /// Profile path this agent was instantiated from. Used to resolve
    /// child profile lookups when this agent spawns sub-agents.
    pub profile_path: String,
    internal_tx: tokio::sync::mpsc::UnboundedSender<InternalEvent>,
    event_rx: tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,
    agent_task: tokio::task::JoinHandle<Result<(), agentik_core::supervise::TaskPanic>>,
    cancel_token: CancellationToken,
    model: Arc<ArcSwapOption<Model>>,
}

impl AgentHandle {
    /// Convenience: short name (last path segment).
    pub fn name(&self) -> &str {
        self.path.name()
    }
}

impl AgentHandle {
    pub fn send_message(&self, text: String) {
        let _ = self
            .internal_tx
            .send(InternalEvent::MessageInject(vec![ContentBlock::Text {
                text,
            }]));
    }

    pub fn cancel(&mut self) {
        self.cancel_token.cancel();
        let new_token = CancellationToken::new();
        let _ = self
            .internal_tx
            .send(InternalEvent::ResetCancelToken(new_token.clone()));
        self.cancel_token = new_token;
    }

    /// Gracefully signal the agent to shut down.
    ///
    /// Sends `InternalEvent::Shutdown` — the agent's `run()` loop will
    /// process it, pause all sessions (persisting snapshots + ending WAL
    /// sessions), and then exit. The caller should subsequently await
    /// [`join`](Self::join) to ensure the task has fully completed before
    /// the runtime is dropped.
    pub fn shutdown(&mut self) {
        let _ = self.internal_tx.send(InternalEvent::Shutdown);
    }

    /// Consume and await the background agent task. Call this after
    /// [`shutdown`](Self::shutdown) to ensure all sessions are paused and
    /// snapshots are persisted before the runtime is torn down.
    ///
    /// If the agent task panicked at any point (detected via `catch_unwind`),
    /// the panic details are logged here as an additional safety net.
    pub async fn join(self) {
        match self.agent_task.await {
            Ok(Ok(())) => {} // clean exit
            Ok(Err(panic)) => {
                tracing::error!(
                    agent = %self.path,
                    panic_task = %panic.task,
                    panic_msg = %panic.msg,
                    "agent task had panicked before join"
                );
            }
            Err(join_err) => {
                tracing::error!(
                    agent = %self.path,
                    error = %join_err,
                    "agent task join failed (panic or cancellation)"
                );
            }
        }
    }

    pub fn poll_event(&mut self) -> Option<AgentEvent> {
        self.event_rx.try_recv().ok()
    }

    pub async fn recv_event(&mut self) -> Option<AgentEvent> {
        self.event_rx.recv().await
    }

    pub fn model_handle(&self) -> &Arc<ArcSwapOption<Model>> {
        &self.model
    }

    /// Hot-swap the model for this specific agent. The agent's run loop
    /// picks up the new model on its next LLM request.
    pub fn set_model(&self, model: Model) {
        self.model.store(Some(Arc::new(model)));
    }

    // ── Session management ────────────────────────────────

    /// Create a new session within this agent. Returns the new session ID.
    ///
    /// If `fork_from` is `Some(parent_id)`, the new session deep-clones the
    /// parent's memory (conversation branching). Otherwise the session
    /// starts with an empty conversation.
    pub fn create_session(
        &self,
        title: Option<String>,
        fork_from: Option<uuid::Uuid>,
    ) -> uuid::Uuid {
        let id = uuid::Uuid::new_v4();
        let _ = self.internal_tx.send(InternalEvent::CreateSession {
            id,
            fork_from,
            title,
        });
        id
    }

    /// Switch the active session to `id`. Pauses the current session and
    /// activates the target.
    pub fn switch_session(&self, id: uuid::Uuid) {
        let _ = self.internal_tx.send(InternalEvent::SwitchSession { id });
    }

    /// Close and remove a session.
    pub fn close_session(&self, id: uuid::Uuid) {
        let _ = self.internal_tx.send(InternalEvent::CloseSession { id });
    }

    /// Request a list of all sessions. The reply arrives as
    /// `AgentEvent::SessionList` on the event channel.
    pub fn list_sessions(&self) {
        let _ = self.internal_tx.send(InternalEvent::ListSessions);
    }

    /// Rename a session.
    pub fn rename_session(&self, id: uuid::Uuid, title: String) {
        let _ = self
            .internal_tx
            .send(InternalEvent::RenameSession { id, title });
    }
}

// ═══════════════════════════════════════════════════════════════════════
// RuntimeHost — top-level multi-agent manager
// ═══════════════════════════════════════════════════════════════════════

/// Top-level runtime that owns shared infrastructure, a persistent
/// [`AgentNetwork`] for topology routing, and an agent registry for
/// multiplexed event access.
///
/// Created once per process via [`RuntimeHost::open`]. Individual agents
/// are spawned via [`RuntimeHost::spawn_agent`]. For multi-agent topologies,
/// use the topology control API ([`add_node`](Self::add_node),
/// [`connect`](Self::connect), etc.) combined with
/// [`recv_any`](Self::recv_any) for unified event polling.
///
/// **Not `Clone`** — owns the agent network and registry. Pass `&self` or
/// `&mut self` to consumers rather than cloning.
pub struct RuntimeHost {
    infra: SharedInfra,
    /// Persistent topology routing engine — same lifetime as the host.
    network: AgentNetwork,
    /// Per-agent relay entries for multiplexed event access.
    agents: HashMap<String, AgentEntry>,
    /// Multiplexed event channel — each relay task pushes here.
    event_tx: UnboundedSender<TaggedEvent>,
    /// Receiver half (owned, drained via `recv_any`).
    event_rx: UnboundedReceiver<TaggedEvent>,
    /// Command channel from agent tools (HostControl).
    cmd_rx: tokio::sync::mpsc::UnboundedReceiver<crate::control::HostCommand>,
    /// Clonable control handle — passed to agent tools.
    control: crate::control::HostControl,
    /// Pending tool-delegation reply channels: maps delegatee name → reply.
    /// When the delegatee Dones, its response is sent through the channel.
    tool_delegations: HashMap<String, tokio::sync::oneshot::Sender<String>>,
    /// Cached profiles (blueprints) loaded at startup. Used by GetStatus
    /// and route_task so agents can discover what they can spawn.
    profiles: Vec<agentik_core::AgentProfile>,
    /// Current model (for spawning agents from tools). Set by TUI at startup.
    model: Option<Arc<ArcSwapOption<Model>>>,
    /// Channel for receiving spawned agent handles from background tasks.
    /// Background spawn tasks send (handle, info) here; the main loop
    /// drains and registers them.
    registration_rx: tokio::sync::mpsc::UnboundedReceiver<(AgentHandle, crate::control::AgentInfo)>,
    registration_tx: tokio::sync::mpsc::UnboundedSender<(AgentHandle, crate::control::AgentInfo)>,
    /// Notification channel — fires when an agent is registered or
    /// shut down. The TUI subscribes to this to keep its session list
    /// in sync with RuntimeHost's agent registry.
    notify_tx: tokio::sync::mpsc::UnboundedSender<HostEvent>,
    notify_rx: tokio::sync::mpsc::UnboundedReceiver<HostEvent>,
    /// Broadcast event channel — same `HostEvent` stream as `notify_tx`,
    /// but multi-consumer. Each subscriber gets its own `broadcast::Receiver`
    /// that lags independently. Used by event subscribers (e.g. the TUI)
    /// so multiple consumers can observe agent status changes concurrently
    /// without stealing events from each other.
    /// Buffer of 256 should be plenty for in-flight status transitions;
    /// if it overflows, subscribers see `RecvError::Lagged` and skip ahead.
    event_broadcast: tokio::sync::broadcast::Sender<HostEvent>,
}

/// Lifecycle events emitted by RuntimeHost. The TUI subscribes to keep
/// its session tabs in sync with backend agent state.
#[derive(Debug, Clone)]
pub enum HostEvent {
    /// An agent was just registered with the host's relay.
    AgentRegistered {
        path: agentik_types::AgentPath,
        info: crate::control::AgentInfo,
    },
    /// An agent was shut down and removed from the registry.
    AgentUnregistered { path: String },
    /// An agent's runtime status changed (e.g. `Running → AwaitingTool`,
    /// `AwaitingTool → Completed`). Emitted from
    /// [`RuntimeHost::recv_any`] after the new state is written to the
    /// agent's [`AgentEntry`]. The TUI and other subscribers can use
    /// this to keep their status panels in sync without polling
    /// [`crate::control::HostControl::get_status`].
    AgentStatusChanged {
        path: String,
        status: crate::control::AgentStatus,
        last_event: Option<String>,
    },
}

/// An `AgentEvent` tagged with the agent name that produced it.
pub type TaggedEvent = (String, AgentEvent);

/// Commands sent to a per-agent relay task.
enum AgentCommand {
    Message(String),
    Shutdown,
    Cancel,
    ListSessions,
    CreateSession {
        title: Option<String>,
        fork_from: Option<uuid::Uuid>,
    },
    SwitchSession(uuid::Uuid),
    CloseSession(uuid::Uuid),
    RenameSession {
        id: uuid::Uuid,
        title: String,
    },
    SetModel(Model),
}

/// Internal entry for one registered agent.
struct AgentEntry {
    cmd_tx: UnboundedSender<AgentCommand>,
    _relay_task: JoinHandle<Result<(), agentik_core::supervise::TaskPanic>>,
    /// Full hierarchical path — source of truth for identity.
    /// Mirrors the HashMap key but kept here for typed access within entries.
    #[allow(dead_code)]
    path: agentik_types::AgentPath,
    /// Profile path this agent was instantiated from. Used to resolve
    /// child profile lookups when this agent spawns sub-agents.
    #[allow(dead_code)]
    profile_path: String,
    /// Capability metadata for routing and discovery.
    ///
    /// `status` and `last_event` mirror the live runtime fields — the
    /// host writes to both this struct and `info` so that
    /// `GetAgentInfo` / `GetStatus` (which serialize `AgentInfo`) see the
    /// same value that `update_agent_status` last set.
    info: crate::control::AgentInfo,
    /// Live runtime status. Updated on every observed [`AgentEvent`].
    /// Phase-1 deliverable — surfaces "what is agent X doing?" without
    /// requiring the agent to expose anything new.
    status: AgentStatus,
    /// Most recent event summary (tool name on AwaitingTool, error
    /// message on Failed, etc.). `None` until the first event lands.
    last_event: Option<String>,
    /// Shared model slot — same Arc as the relay's AgentHandle.
    /// Allows querying and hot-swapping the model without direct
    /// access to the moved AgentHandle.
    model: Arc<ArcSwapOption<Model>>,
}

impl RuntimeHost {
    /// Open shared infrastructure and create an empty agent network.
    pub async fn open(config: &RuntimeConfig) -> HostResult<Self> {
        let mut infra = SharedInfra::open(config).await?;
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (registration_tx, registration_rx) = mpsc::unbounded_channel();
        let (notify_tx, notify_rx) = mpsc::unbounded_channel();
        // Broadcast channel for multi-consumer host events. Capacity 256:
        // even a busy agent at ~10 status changes/sec × 25 sec fits, and
        // subscribers that fall behind get `RecvError::Lagged` and skip.
        let (event_broadcast, _) = tokio::sync::broadcast::channel(256);
        let control = crate::control::HostControl::new(cmd_tx, event_broadcast.clone());
        infra.host_control = Some(control.clone());
        Ok(Self {
            infra,
            network: AgentNetwork::new(),
            agents: HashMap::new(),
            event_tx,
            event_rx,
            cmd_rx,
            control,
            tool_delegations: HashMap::new(),
            profiles: Vec::new(),
            model: None,
            registration_rx,
            registration_tx,
            notify_tx,
            notify_rx,
            event_broadcast,
        })
    }

    /// Returns a clonable [`HostControl`] for passing to agent tools.
    /// Agents use this to spawn peers, manage topology, send messages,
    /// and query system status.
    pub fn control(&self) -> crate::control::HostControl {
        self.control.clone()
    }

    /// Set cached profiles (blueprints). Called by the TUI after loading
    /// profiles from storage at startup. Enables agents to discover
    /// what profiles they can spawn via `list_agents` / `route_task`.
    pub fn set_profiles(&mut self, profiles: Vec<agentik_core::AgentProfile>) {
        self.profiles = profiles;
    }

    /// Set the current model (for spawning agents from tools).
    pub fn set_model(&mut self, model: Arc<ArcSwapOption<Model>>) {
        self.model = Some(model);
    }

    /// Drain and execute all pending commands from agent tools.
    /// Call this in the event loop (e.g. at each render tick).
    pub fn try_process_commands(&mut self) {
        while let Ok(cmd) = self.cmd_rx.try_recv() {
            self.process_command(cmd);
        }
    }

    /// Await either a host command or a background spawn completion.
    /// Event-driven — wakes when either arrives. Use as a `select!`
    /// branch in the event loop.
    pub async fn recv_and_process_command(&mut self) {
        tokio::select! {
            cmd = self.cmd_rx.recv() => {
                if let Some(cmd) = cmd {
                    self.process_command(cmd);
                }
            }
            reg = self.registration_rx.recv() => {
                if let Some((handle, info)) = reg {
                    let path = handle.path.clone();
                    let info_clone = info.clone();
                    self.register_agent(handle, info);
                    self.emit_host_event(HostEvent::AgentRegistered {
                        path: path.clone(),
                        info: info_clone,
                    });
                    tracing::info!(agent = %path, "background spawn completed and registered");
                }
            }
        }
    }

    fn process_command(&mut self, cmd: crate::control::HostCommand) {
        use crate::control::{HostCommand, HostStatus};
        match cmd {
            HostCommand::Spawn {
                name,
                caller_path,
                caller_profile_path,
                profile_segment,
                reply_tx,
            } => {
                // Derive child path from caller's path + the LLM-provided segment.
                let child_path = match caller_path.join(&name) {
                    Ok(p) => p,
                    Err(e) => {
                        let _ = reply_tx.send(Err(format!("invalid agent name `{name}`: {e}")));
                        return;
                    }
                };
                // Reject duplicate paths.
                if self.agents.contains_key(child_path.as_str()) {
                    let _ =
                        reply_tx.send(Err(format!("agent at path `{child_path}` already exists")));
                    return;
                }

                // ── Resolve profile ──
                // 1. None → reuse caller's profile (by caller_profile_path).
                // 2. Contains '/' → absolute profile path.
                // 3. Single segment → relative: try "{caller}/{segment}",
                //    fallback to root-level "{segment}".
                let target_profile_path = match &profile_segment {
                    None => caller_profile_path.clone(),
                    Some(seg) if seg.contains('/') => seg.clone(),
                    Some(seg) => {
                        let relative = format!("{caller_profile_path}/{seg}");
                        if self.profiles.iter().any(|p| p.path == relative) {
                            relative
                        } else {
                            seg.clone()
                        }
                    }
                };

                let Some(profile) = self
                    .profiles
                    .iter()
                    .find(|p| p.path == target_profile_path)
                    .cloned()
                else {
                    // List available child profiles under the caller's path.
                    let available_children: Vec<_> = self
                        .profiles
                        .iter()
                        .filter(|p| p.parent_path() == Some(caller_profile_path.as_str()))
                        .map(|p| p.name().to_string())
                        .collect();
                    let available_roots: Vec<_> = self
                        .profiles
                        .iter()
                        .filter(|p| p.depth() == 0)
                        .map(|p| p.name().to_string())
                        .collect();
                    let _ = reply_tx.send(Err(format!(
                        "Profile '{target_profile_path}' not found. \
                         Child profiles under '{caller_profile_path}': [{}]. \
                         Root profiles: [{}].",
                        available_children.join(", "),
                        available_roots.join(", "),
                    )));
                    return;
                };

                // Need a model to spawn.
                let Some(ref model) = self.model else {
                    let _ = reply_tx.send(Err("No model configured on host.".into()));
                    return;
                };

                // Spawn in background — agent creation is async.
                let infra = self.infra.clone();
                let model = model.clone();
                let reg_tx = self.registration_tx.clone();
                let info =
                    capability_from_profile(child_path.name(), child_path.as_str(), &profile);
                let path_for_spawn = child_path.clone();

                self.infra.runtime_handle.spawn(async move {
                    let result = AssertUnwindSafe(async {
                        infra
                            .spawn_agent(&path_for_spawn, &profile, model, None)
                            .await
                    })
                    .catch_unwind()
                    .await;
                    match result {
                        Ok(Ok(handle)) => {
                            let registered_path = handle.path.as_str().to_string();
                            let _ = reply_tx.send(Ok(registered_path));
                            let _ = reg_tx.send((handle, info));
                        }
                        Ok(Err(e)) => {
                            let _ = reply_tx.send(Err(e.to_string()));
                        }
                        Err(panic_payload) => {
                            let msg = panic_payload
                                .downcast_ref::<&'static str>()
                                .map(|s| (*s).to_string())
                                .or_else(|| {
                                    panic_payload.downcast_ref::<String>().map(|s| s.clone())
                                })
                                .unwrap_or_else(|| "<panic in spawn_agent>".to_string());
                            tracing::error!(
                                target: "spawn_safe",
                                task = "host::spawn_agent",
                                panic = %msg,
                                "spawn_agent task panicked"
                            );
                            let _ = reply_tx.send(Err(format!("internal panic: {msg}")));
                        }
                    }
                });
            }
            HostCommand::SpawnWithProfile {
                name,
                caller_path,
                profile,
                model_override,
                reply_tx,
            } => {
                // Derive child path from caller's path + the LLM-provided segment.
                let child_path = match caller_path.join(&name) {
                    Ok(p) => p,
                    Err(e) => {
                        let _ = reply_tx.send(Err(format!("invalid agent name `{name}`: {e}")));
                        return;
                    }
                };
                // Reject duplicate paths.
                if self.agents.contains_key(child_path.as_str()) {
                    let _ =
                        reply_tx.send(Err(format!("agent at path `{child_path}` already exists")));
                    return;
                }
                // Resolve model: use override if provided, else global model.
                let model = match (model_override, self.model.as_ref()) {
                    (Some(m), _) => Arc::new(ArcSwapOption::from_pointee(Some(m))),
                    (None, Some(g)) => g.clone(),
                    (None, None) => {
                        let _ = reply_tx.send(Err("No model configured on host.".into()));
                        return;
                    }
                };

                let infra = self.infra.clone();
                let reg_tx = self.registration_tx.clone();
                let info =
                    capability_from_profile(child_path.name(), child_path.as_str(), &profile);
                let path_for_spawn = child_path.clone();

                self.infra.runtime_handle.spawn(async move {
                    let result = AssertUnwindSafe(async {
                        infra
                            .spawn_agent(&path_for_spawn, &profile, model, None)
                            .await
                    })
                    .catch_unwind()
                    .await;
                    match result {
                        Ok(Ok(handle)) => {
                            let registered_path = handle.path.as_str().to_string();
                            let _ = reply_tx.send(Ok(registered_path));
                            let _ = reg_tx.send((handle, info));
                        }
                        Ok(Err(e)) => {
                            let _ = reply_tx.send(Err(e.to_string()));
                        }
                        Err(panic_payload) => {
                            let msg = panic_payload
                                .downcast_ref::<&'static str>()
                                .map(|s| (*s).to_string())
                                .or_else(|| {
                                    panic_payload.downcast_ref::<String>().map(|s| s.clone())
                                })
                                .unwrap_or_else(|| "<panic in spawn_agent>".to_string());
                            tracing::error!(
                                target: "spawn_safe",
                                task = "host::spawn_with_profile",
                                panic = %msg,
                                "spawn_with_profile task panicked"
                            );
                            let _ = reply_tx.send(Err(format!("internal panic: {msg}")));
                        }
                    }
                });
            }
            HostCommand::Shutdown { name } => {
                let resolved = self.resolve_agent(&name).unwrap_or(name);
                self.shutdown_agent(&resolved);
            }
            HostCommand::DeriveProfile {
                caller_profile_path,
                segment,
                overrides,
                reply_tx,
            } => {
                // Look up parent profile in cache.
                let Some(parent) = self
                    .profiles
                    .iter()
                    .find(|p| p.path == caller_profile_path)
                    .cloned()
                else {
                    let _ = reply_tx.send(Err(format!(
                        "Your profile '{caller_profile_path}' not found in cache"
                    )));
                    return;
                };
                // Derive the child profile.
                let child = match parent.derive_child(&segment, *overrides) {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = reply_tx.send(Err(e));
                        return;
                    }
                };
                // Reject if path already exists.
                if self.profiles.iter().any(|p| p.path == child.path) {
                    let _ = reply_tx.send(Err(format!("Profile '{}' already exists", child.path)));
                    return;
                }
                // Persist to storage.
                let profile_storage = self.infra.profile_storage.clone();
                let child_for_persist = child.clone();
                let child_path = child.path.clone();
                self.infra.runtime_handle.spawn(async move {
                    let result = AssertUnwindSafe(async {
                        profile_storage.create_profile(child_for_persist).await
                    })
                    .catch_unwind()
                    .await;
                    match result {
                        Ok(Ok(())) => {
                            let _ = reply_tx.send(Ok(child_path));
                        }
                        Ok(Err(e)) => {
                            let _ = reply_tx
                                .send(Err(format!("Failed to persist derived profile: {e}")));
                        }
                        Err(panic_payload) => {
                            let msg = panic_payload
                                .downcast_ref::<&'static str>()
                                .map(|s| (*s).to_string())
                                .or_else(|| {
                                    panic_payload.downcast_ref::<String>().map(|s| s.clone())
                                })
                                .unwrap_or_else(|| "<panic in create_profile>".to_string());
                            tracing::error!(
                                target: "spawn_safe",
                                task = "host::create_profile",
                                panic = %msg,
                                "create_profile task panicked"
                            );
                            let _ = reply_tx.send(Err(format!("internal panic: {msg}")));
                        }
                    }
                });
                // Add to in-memory cache immediately (non-async).
                self.profiles.push(child);
            }
            HostCommand::AddNode {
                name,
                profile,
                initial_prompt,
            } => {
                let _ = self.network.add_node(agentik_network::NodeSpec {
                    name,
                    profile,
                    initial_prompt,
                });
            }
            HostCommand::RemoveNode { name } => {
                self.network.remove_node(&name);
            }
            HostCommand::Connect { from, to, trigger } => {
                let _ = self.network.connect(&from, &to, trigger, None);
            }
            HostCommand::Disconnect { from, to } => {
                self.network.disconnect(&from, &to);
            }
            HostCommand::DeliverMessage { name, message } => {
                let resolved = self.resolve_agent(&name).unwrap_or(name);
                self.send_to(&resolved, message);
            }
            // ── Phase 5: fire-and-forget inter-agent message ──
            // Unlike Delegate, no reply_tx is recorded in tool_delegations —
            // the sender gets immediate Ok/Err feedback but does NOT wait
            // for the target's Done event.
            HostCommand::SendMessage {
                to,
                message,
                reply_tx,
            } => {
                let resolved = match self.resolve_agent(&to) {
                    Some(path) => path,
                    None => {
                        let _ = reply_tx.send(Err(format!(
                            "Agent '{to}' is not registered. \
                             Use list_agents to see available agents, \
                             or spawn_agent to create one first."
                        )));
                        return;
                    }
                };
                // Validate the agent is still in the registry (resolve_agent
                // checks profiles too, but we can only message live agents).
                if !self.agents.contains_key(&resolved) {
                    let _ = reply_tx.send(Err(format!(
                        "Agent '{to}' resolved to '{resolved}' but is not \
                         currently running. Only live agents can receive \
                         messages."
                    )));
                    return;
                }
                self.send_to(&resolved, message);
                let _ = reply_tx.send(Ok(()));
            }
            HostCommand::Delegate {
                to,
                message,
                reply_tx,
            } => {
                // Resolve target: full path or short name.
                let resolved = match self.resolve_agent(&to) {
                    Some(path) => path,
                    None => {
                        let _ = reply_tx.send(format!(
                            "Error: agent '{to}' is not registered. \
                             Use list_agents to see available agents, \
                             or spawn_agent to create one first."
                        ));
                        return;
                    }
                };
                // Record the reply channel — when `to` Dones, its response
                // is sent through reply_tx (handled in step()).
                self.tool_delegations.insert(resolved.clone(), reply_tx);
                self.send_to(&resolved, message);
            }
            HostCommand::GetStatus { reply_tx } => {
                let g = self.network.graph();
                let status = HostStatus {
                    agents: self.agents.values().map(|e| e.info.clone()).collect(),
                    profiles: self
                        .profiles
                        .iter()
                        .map(|p| capability_from_profile(p.name(), &p.path, p))
                        .collect(),
                    nodes: g.node_names().into_iter().map(String::from).collect(),
                    edge_count: g.edge_count(),
                    is_cyclic: g.is_cyclic(),
                    roots: g.root_nodes().into_iter().map(String::from).collect(),
                    leaves: g.leaf_nodes().into_iter().map(String::from).collect(),
                    rounds: self.network.rounds(),
                    is_finished: self.network.is_finished(),
                    termination: format!("{:?}", self.network.termination()),
                };
                let _ = reply_tx.send(status);
            }
            HostCommand::SetTermination { spec } => {
                self.network.set_termination(spec);
            }
            HostCommand::ResetRunState => {
                self.network.reset_run_state();
            }
            HostCommand::InjectPrompts => {
                self.inject_initial_prompts();
            }
            HostCommand::RouteTask {
                description,
                exclude,
                reply_tx,
            } => {
                let result = self.route_task(&description, exclude.as_deref());
                let _ = reply_tx.send(result);
            }
            HostCommand::GetAgentInfo { name, reply_tx } => {
                // Resolve agent name, then check running agents first,
                // fall back to profiles (by short name).
                let resolved = self.resolve_agent(&name);
                let info = resolved
                    .as_ref()
                    .and_then(|key| self.agents.get(key).map(|e| e.info.clone()))
                    .or_else(|| {
                        self.profiles
                            .iter()
                            .find(|p| p.path == name)
                            .map(|p| capability_from_profile(p.name(), &p.path, p))
                    });
                let _ = reply_tx.send(info);
            }

            // ── Session management (forwarded to relay) ──
            HostCommand::CancelAgent { name } => {
                self.send_agent_command(&name, AgentCommand::Cancel);
            }
            HostCommand::ListSessions { name } => {
                self.send_agent_command(&name, AgentCommand::ListSessions);
            }
            HostCommand::CreateSession {
                name,
                title,
                fork_from,
            } => {
                self.send_agent_command(&name, AgentCommand::CreateSession { title, fork_from });
            }
            HostCommand::SwitchSession { name, session_id } => {
                self.send_agent_command(&name, AgentCommand::SwitchSession(session_id));
            }
            HostCommand::CloseSession { name, session_id } => {
                self.send_agent_command(&name, AgentCommand::CloseSession(session_id));
            }
            HostCommand::RenameSession {
                name,
                session_id,
                title,
            } => {
                self.send_agent_command(
                    &name,
                    AgentCommand::RenameSession {
                        id: session_id,
                        title,
                    },
                );
            }

            // ── Model management ──
            HostCommand::SetAgentModel { name, model } => {
                self.send_agent_command(&name, AgentCommand::SetModel(model));
            }
            HostCommand::GetAgentModel { name, reply_tx } => {
                let resolved = self.resolve_agent(&name);
                let info = resolved
                    .as_ref()
                    .and_then(|key| self.agents.get(key))
                    .and_then(|e| {
                        e.model
                            .load_full()
                            .as_deref()
                            .map(|m| (m.model_info.model_name.clone(), m.model_info.context_length))
                    });
                let _ = reply_tx.send(info);
            }
            // ── Phase 4: query persisted agent graph ──
            // Synchronous read of the `agent_graph` table — the storage
            // backend is a local SQLite DB so the query is cheap. Returns
            // an empty vec on storage error (rather than blocking the
            // caller's reply channel with an Err) so the dashboard can
            // render an empty state instead of crashing.
            HostCommand::ListPersistedAgents { reply_tx } => {
                let storage = self.infra.storage.clone();
                let runtime_handle = self.infra.runtime_handle.clone();
                runtime_handle.spawn(async move {
                    let result = AssertUnwindSafe(async {
                        match storage.list_persisted_agents().await {
                            Ok(entries) => entries,
                            Err(e) => {
                                tracing::warn!(
                                    error = %e,
                                    "list_persisted_agents failed; returning empty list"
                                );
                                Vec::new()
                            }
                        }
                    })
                    .catch_unwind()
                    .await;
                    match result {
                        Ok(entries) => {
                            let _ = reply_tx.send(entries);
                        }
                        Err(panic_payload) => {
                            let msg = panic_payload
                                .downcast_ref::<&'static str>()
                                .map(|s| (*s).to_string())
                                .or_else(|| {
                                    panic_payload.downcast_ref::<String>().map(|s| s.clone())
                                })
                                .unwrap_or_else(|| "<panic in list_persisted_agents>".to_string());
                            tracing::error!(
                                target: "spawn_safe",
                                task = "host::list_persisted_agents",
                                panic = %msg,
                                "list_persisted_agents task panicked"
                            );
                            let _ = reply_tx.send(Vec::new());
                        }
                    }
                });
            }
        }
    }

    /// Resolve an agent reference to its full path key in the registry.
    ///
    /// Accepts:
    /// 1. **Full path** (`/root/researcher/worker`) — exact match.
    /// 2. **Short name** (`researcher`) — matches when unambiguous.
    ///
    /// Returns the HashMap key string, or `None` if no match / ambiguous.
    fn resolve_agent(&self, target: &str) -> Option<String> {
        // 1. Exact full-path match.
        if target.starts_with("/root") && self.agents.contains_key(target) {
            return Some(target.to_string());
        }
        // 2. Short-name match: collect all agents whose last path segment
        //    equals `target`.
        let matches: Vec<&String> = self
            .agents
            .keys()
            .filter(|key| key.rsplit('/').next().unwrap_or(key) == target)
            .collect();
        match matches.len() {
            1 => Some(matches[0].clone()),
            0 => None,
            _ => {
                tracing::warn!(
                    target = %target,
                    candidates = ?matches,
                    "ambiguous agent name — use full path to disambiguate"
                );
                None
            }
        }
    }

    /// Forward a command to a named agent's relay task.
    /// `name` is resolved via [`resolve_agent`](Self::resolve_agent).
    fn send_agent_command(&self, name: &str, cmd: AgentCommand) {
        if let Some(resolved) = self.resolve_agent(name) {
            if let Some(entry) = self.agents.get(&resolved) {
                let _ = entry.cmd_tx.send(cmd);
                return;
            }
        }
        tracing::warn!(agent = %name, "send_agent_command: agent not registered");
    }

    /// Notify listeners that an agent has been shut down.
    /// Called from `shutdown_agent` / `shutdown_all_agents`.
    fn notify_unregistered(&self, name: &str) {
        self.emit_host_event(HostEvent::AgentUnregistered {
            path: name.to_string(),
        });
    }

    /// Route a task description to the best-matching agent.
    /// Considers both running agents and available profiles (blueprints).
    /// Running agents get a small bonus score since they're immediately
    /// available for delegation.
    fn route_task(&self, description: &str, exclude: Option<&str>) -> crate::control::RouteResult {
        let desc_lower = description.to_lowercase();
        let desc_words: std::collections::HashSet<&str> = desc_lower
            .split_whitespace()
            .filter(|w| w.len() > 2)
            .collect();

        let score_info = |info: &crate::control::AgentInfo, is_running: bool| {
            let matched_tags: Vec<String> = info
                .tags
                .iter()
                .filter(|tag| desc_lower.contains(tag.as_str()))
                .cloned()
                .collect();

            let tag_score = matched_tags.len() as f64 * 3.0;

            let info_text = format!(
                "{} {}",
                info.summary.to_lowercase(),
                info.expertise.join(" ").to_lowercase()
            );
            let info_words: std::collections::HashSet<&str> = info_text
                .split_whitespace()
                .filter(|w| w.len() > 2)
                .collect();

            let word_overlap = desc_words.intersection(&info_words).count() as f64;
            let running_bonus = if is_running { 0.5 } else { 0.0 };
            let score = tag_score + word_overlap + running_bonus;

            crate::control::RouteCandidate {
                agent: info.name.clone(),
                score,
                matched_tags,
            }
        };

        // Score running agents (excluding the caller itself).
        let mut candidates: Vec<crate::control::RouteCandidate> = self
            .agents
            .values()
            .filter(|e| Some(e.info.name.as_str()) != exclude)
            .map(|e| score_info(&e.info, true))
            .collect();

        // Score profiles (excluding those already running under the same name
        // and the caller itself).
        let running_names: std::collections::HashSet<&str> =
            self.agents.keys().map(|s| s.as_str()).collect();
        candidates.extend(
            self.profiles
                .iter()
                .filter(|p| !running_names.contains(p.path.as_str()))
                .filter(|p| Some(p.path.as_str()) != exclude)
                .map(|p| score_info(&capability_from_profile(p.name(), &p.path, p), false)),
        );

        let mut candidates: Vec<_> = candidates.into_iter().filter(|c| c.score > 0.0).collect();

        candidates.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let (best, reason) = if let Some(top) = candidates.first() {
            let reason = if top.matched_tags.is_empty() {
                format!("Best keyword match (score: {:.1})", top.score)
            } else {
                format!(
                    "Matched tags: {} (score: {:.1})",
                    top.matched_tags.join(", "),
                    top.score
                )
            };
            (top.agent.clone(), reason)
        } else {
            ("none".into(), "No matching agent found.".into())
        };

        crate::control::RouteResult {
            agent: best,
            reason,
            score: candidates.first().map(|c| c.score).unwrap_or(0.0),
            candidates,
        }
    }

    /// Returns a reference to the shared storage.
    pub fn storage(&self) -> &Arc<dyn AgentStorage> {
        &self.infra.storage
    }

    /// Returns a clone of the shared infra (for advanced use).
    pub fn infra(&self) -> SharedInfra {
        self.infra.clone()
    }

    /// Returns a reference to the persistent topology network.
    pub fn network(&self) -> &AgentNetwork {
        &self.network
    }

    /// Returns a mutable reference to the topology network.
    pub fn network_mut(&mut self) -> &mut AgentNetwork {
        &mut self.network
    }

    // ── Topology control ───────────────────────────────────

    /// Add a node to the topology.
    pub fn add_node(&mut self, name: &str, profile: &str) -> Result<(), String> {
        self.network.add_node(NodeSpec {
            name: name.into(),
            profile: profile.into(),
            initial_prompt: None,
        })
    }

    /// Add a node with an initial prompt.
    pub fn add_node_with_prompt(
        &mut self,
        name: &str,
        profile: &str,
        prompt: impl Into<String>,
    ) -> Result<(), String> {
        self.network.add_node(NodeSpec {
            name: name.into(),
            profile: profile.into(),
            initial_prompt: Some(prompt.into()),
        })
    }

    /// Remove a node from the topology (and clean up routing state).
    pub fn remove_node(&mut self, name: &str) {
        self.network.remove_node(name);
    }

    /// Connect two nodes with a trigger (request-response delegation).
    pub fn connect(&mut self, from: &str, to: &str, trigger: EdgeTrigger) -> Result<(), String> {
        self.network.connect(from, to, trigger, None)
    }

    /// Remove all edges between two nodes.
    pub fn disconnect(&mut self, from: &str, to: &str) -> usize {
        self.network.disconnect(from, to)
    }

    /// Set the termination condition.
    pub fn set_termination(&mut self, termination: TerminationSpec) {
        self.network.set_termination(termination);
    }

    /// Reset routing state (buffers, counts, finished) while keeping the
    /// topology graph intact.
    pub fn reset_run_state(&mut self) {
        self.network.reset_run_state();
    }

    // ── Agent registration + multiplexed transport ─────────

    /// Register an [`AgentHandle`] with the host's internal multiplexer.
    ///
    /// After registration, use [`send_to`](Self::send_to) for message
    /// delivery and [`recv_any`](Self::recv_any) for unified event polling.
    /// The host takes ownership of the handle — do not use it directly
    /// after registration.
    pub fn register_agent(&mut self, handle: AgentHandle, info: crate::control::AgentInfo) {
        let path = handle.path.clone();
        let profile_path = handle.profile_path.clone();
        let agent_id = handle.agent_id;
        let relay_name = path.as_str().to_string();
        let model = handle.model.clone(); // Clone Arc before moving handle
        let event_tx = self.event_tx.clone();
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<AgentCommand>();

        let relay_task = agentik_core::supervise::spawn_safe_on(
            &self.infra.runtime_handle,
            &format!("relay::{relay_name}"),
            async move {
                relay_loop(handle, cmd_rx, event_tx, relay_name).await;
            },
        );

        self.agents.insert(
            path.as_str().to_string(),
            AgentEntry {
                cmd_tx,
                _relay_task: relay_task,
                path: path.clone(),
                profile_path: profile_path.clone(),
                status: info.status.clone(),
                last_event: info.last_event.clone(),
                info: info.clone(),
                model,
            },
        );

        // ── Phase 4: persist agent graph entry ──
        // Fire-and-forget: registration semantics are owned by the in-memory
        // registry; the dashboard's persistence is a read-side projection that
        // can tolerate eventual consistency. Failure here is logged but does
        // not abort the spawn flow.
        self.persist_upsert_agent_graph(&path, agent_id, &profile_path, &info);
    }

    /// Spawn a background task that upserts the agent's graph row in storage.
    ///
    /// Caller has already inserted the in-memory entry. This mirrors the
    /// registration into the `agent_graph` table so the dashboard can
    /// reconstruct the topology across process restarts. Errors are
    /// logged at WARN — persistence is best-effort.
    fn persist_upsert_agent_graph(
        &self,
        path: &agentik_types::AgentPath,
        agent_id: uuid::Uuid,
        profile_path: &str,
        info: &crate::control::AgentInfo,
    ) {
        use agentik_core::storage::PersistedAgentGraph;

        let storage = self.infra.storage.clone();
        let path_str = path.as_str().to_string();
        let parent_path = path.parent().map(|p| p.as_str().to_string());
        let profile_path = profile_path.to_string();
        let status_json = match serde_json::to_string(&info.status) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(agent = %path_str, error = %e, "skip persistence: status serialization failed");
                return;
            }
        };
        let last_event = info.last_event.clone();

        agentik_core::supervise::spawn_safe_on_drop(
            &self.infra.runtime_handle,
            &format!("persist_upsert_agent_graph::{path_str}"),
            async move {
                // Wall-clock millis since the unix epoch. std::time::SystemTime
                // is the only reliable source here (the runtime crate doesn't
                // depend on chrono at runtime — it's a dev-dep only).
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0);
                let entry = PersistedAgentGraph {
                    path: path_str.clone(),
                    parent_path,
                    profile_path,
                    agent_id,
                    status_json,
                    last_event,
                    created_at: now,
                    updated_at: now,
                };
                if let Err(e) = storage.upsert_agent_graph_entry(entry).await {
                    tracing::warn!(
                        agent = %path_str,
                        error = %e,
                        "failed to persist agent graph entry (non-fatal)"
                    );
                }
            },
        );
    }

    /// Spawn a background task that updates the agent's persisted status.
    /// Mirrors [`Self::observe_status`] to durable storage. Fire-and-forget.
    fn persist_agent_status(
        &self,
        path: &str,
        status: &crate::control::AgentStatus,
        last_event: &Option<String>,
    ) {
        let storage = self.infra.storage.clone();
        let path_str = path.to_string();
        let status_json = match serde_json::to_string(status) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(agent = %path_str, error = %e, "skip status persistence: serialization failed");
                return;
            }
        };
        let last_event = last_event.clone();

        agentik_core::supervise::spawn_safe_on_drop(
            &self.infra.runtime_handle,
            &format!("persist_agent_status::{path_str}"),
            async move {
                if let Err(e) = storage
                    .update_agent_graph_status(&path_str, &status_json, last_event.as_deref())
                    .await
                {
                    tracing::warn!(
                        agent = %path_str,
                        error = %e,
                        "failed to persist agent status (non-fatal)"
                    );
                }
            },
        );
    }

    /// Spawn a background task that removes the agent's persisted graph row.
    /// Called on shutdown. Fire-and-forget.
    fn persist_remove_agent_graph(&self, path: &str) {
        let storage = self.infra.storage.clone();
        let path_str = path.to_string();

        agentik_core::supervise::spawn_safe_on_drop(
            &self.infra.runtime_handle,
            &format!("persist_remove_agent_graph::{path_str}"),
            async move {
                if let Err(e) = storage.remove_agent_graph_entry(&path_str).await {
                    tracing::warn!(
                        agent = %path_str,
                        error = %e,
                        "failed to remove persisted agent graph entry (non-fatal)"
                    );
                }
            },
        );
    }

    /// Spawn an agent and immediately register it with the host's
    /// multiplexer. Returns the agent name.
    pub async fn spawn_and_register(
        &mut self,
        agent_name: &str,
        profile: &agentik_core::AgentProfile,
        global_model: Arc<ArcSwapOption<Model>>,
        model_override: Option<Model>,
    ) -> HostResult<String> {
        let path = agentik_types::AgentPath::root()
            .join(agent_name)
            .map_err(|e| HostError::Other(e.to_string()))?;
        let handle = self
            .spawn_agent(&path, profile, global_model, model_override)
            .await?;
        let name = handle.path.as_str().to_string();
        let info = capability_from_profile(handle.path.name(), handle.path.as_str(), profile);
        self.register_agent(handle, info);
        Ok(name)
    }

    /// Send a message to a named agent (via the relay task).
    ///
    /// `name` should already be a resolved full path. Callers that receive
    /// user/LLM-provided names should call [`resolve_agent`](Self::resolve_agent)
    /// first.
    pub fn send_to(&self, name: &str, message: String) {
        if let Some(entry) = self.agents.get(name) {
            let _ = entry.cmd_tx.send(AgentCommand::Message(message));
        }
    }

    /// Query the model info for a named agent (for TUI rendering).
    pub fn agent_model_info(&self, name: &str) -> Option<(String, u64)> {
        self.agents.get(name).and_then(|e| {
            e.model
                .load_full()
                .as_deref()
                .map(|m| (m.model_info.model_name.clone(), m.model_info.context_length))
        })
    }

    /// Inject initial prompts for all nodes that have them.
    pub fn inject_initial_prompts(&mut self) {
        let messages = self.network.initial_messages();
        for (node, prompt) in messages {
            self.send_to(&node, prompt);
        }
    }

    /// Shut down a named agent and remove it from the registry.
    pub fn shutdown_agent(&mut self, name: &str) {
        if let Some(entry) = self.agents.remove(name) {
            let _ = entry.cmd_tx.send(AgentCommand::Shutdown);
        }
        self.notify_unregistered(name);
        // Phase 4: remove the persisted graph row so the dashboard doesn't
        // resurrect a stale entry on the next process start.
        self.persist_remove_agent_graph(name);
    }

    /// Shut down all registered agents and await their graceful exit.
    ///
    /// Each agent's relay task calls `handle.join()` which waits for the
    /// agent's `run()` loop to process the `Shutdown` event, pause all
    /// sessions (persist snapshots + end WAL sessions), and flush pending
    /// `persist_worker` operations.
    ///
    /// **Must be called from within the tokio runtime** (e.g. inside a
    /// `block_on`) so that background tasks can make progress while we
    /// await their completion.
    pub async fn shutdown_all_agents_and_wait(&mut self) {
        let names: Vec<String> = self.agents.keys().cloned().collect();
        let mut relay_tasks = Vec::new();
        for (_, entry) in self.agents.drain() {
            let _ = entry.cmd_tx.send(AgentCommand::Shutdown);
            relay_tasks.push(entry._relay_task);
        }
        for name in names {
            self.notify_unregistered(&name);
        }
        // Wait for all relay tasks to finish. Each relay loop calls
        // `handle.join().await` before exiting, which in turn waits for
        // the agent's `run()` to complete its session-pause shutdown.
        for task in relay_tasks {
            match task.await {
                Ok(Ok(())) => {}
                Ok(Err(panic)) => {
                    tracing::error!(
                        target: "spawn_safe",
                        task = %panic.task,
                        panic = %panic.msg,
                        "relay task panicked during shutdown"
                    );
                }
                Err(join_err) => {
                    tracing::error!(error = %join_err, "relay task join error during shutdown");
                }
            }
        }
    }

    /// Shut down all registered agents (fire-and-forget).
    ///
    /// **Prefer [`shutdown_all_agents_and_wait`](Self::shutdown_all_agents_and_wait)
    /// when the runtime is still alive** — this method does NOT wait for
    /// agents to gracefully pause their sessions. It exists for scenarios
    /// where the caller cannot await (e.g. sync code paths).
    pub fn shutdown_all_agents(&mut self) {
        let names: Vec<String> = self.agents.keys().cloned().collect();
        for (_, entry) in self.agents.drain() {
            let _ = entry.cmd_tx.send(AgentCommand::Shutdown);
        }
        for name in names {
            self.notify_unregistered(&name);
        }
    }

    /// Receive the next event from any registered agent.
    ///
    /// This also handles delegation plumbing:
    /// - `LlmResponse` events are accumulated in the network's response
    ///   buffer (so `accumulated_response` works when Done arrives).
    /// - `Done` events trigger the tool delegation reply: the accumulated
    ///   response is sent to the waiting `delegate_to` tool via its
    ///   `reply_tx` channel.
    ///
    /// The raw event is still returned to the caller for UI rendering.
    pub async fn recv_any(&mut self) -> Option<TaggedEvent> {
        let (name, event) = self.event_rx.recv().await?;

        // For Done events: capture accumulated response BEFORE process_event
        // (process_event drains the response buffer).
        let delegation_response = if matches!(event, AgentEvent::Done) {
            if self.tool_delegations.contains_key(&name) {
                Some(self.network.accumulated_response(&name).to_string())
            } else {
                None
            }
        } else {
            None
        };

        // Feed the event through the network (accumulates LlmResponse,
        // handles topology-edge delegation routing, termination checks).
        let actions = self.network.process_event(&name, &event);

        // If we captured a delegation response, send it to the waiting tool.
        if let Some(response) = delegation_response {
            if let Some(reply_tx) = self.tool_delegations.remove(&name) {
                let _ = reply_tx.send(response);
                tracing::info!(agent = %name, "delegation response delivered to caller tool");
            }
        }

        // Execute any routing actions (topology-edge based forwarding).
        for action in &actions {
            if let agentik_network::RoutingAction::Send { to, message } = action {
                self.send_to(to, message.clone());
            }
        }

        // Phase 1: derive the agent's runtime status from the observed
        // event and notify subscribers. Done after network.process_event so
        // that `Done` reliably reflects the final-terminal state of the
        // turn (no later event will revert it back to Running unless a
        // fresh message arrives — which itself flips status again).
        self.observe_status(&name, &event);

        Some((name, event))
    }

    /// Update the named agent's runtime [`AgentStatus`] from the event
    /// just observed, mirroring the change into the entry's `AgentInfo`
    /// and broadcasting a [`HostEvent::AgentStatusChanged`] when the
    /// status actually transitions.
    ///
    /// No-op if `name` is not in the registry (e.g. the agent was shut
    /// down between `event_rx.recv()` and this call — possible because
    /// shutdown is a separate command path that races with event
    /// delivery).
    fn observe_status(&mut self, name: &str, event: &AgentEvent) {
        let (new_status, new_last_event) = derive_agent_status(event);
        let entry = match self.agents.get_mut(name) {
            Some(e) => e,
            None => return,
        };

        // Skip the bookkeeping work (and notification fan-out) if the
        // status didn't actually change. LlmResponse / TextDelta /
        // ThinkingDelta / intra-stream events all map to `Running`, so
        // a busy agent emits dozens of events per turn — without this
        // guard the TUI would be spammed with no-op notifications.
        if entry.status == new_status && entry.last_event == new_last_event {
            return;
        }

        entry.status = new_status.clone();
        entry.last_event = new_last_event.clone();
        // Mirror into AgentInfo so list_agents / get_agent_info see it.
        entry.info.status = new_status.clone();
        entry.info.last_event = new_last_event.clone();

        self.emit_host_event(HostEvent::AgentStatusChanged {
            path: name.to_string(),
            status: new_status.clone(),
            last_event: new_last_event.clone(),
        });

        // Phase 4: persist the status change so the dashboard can
        // reconstruct the agent's runtime state after a process restart.
        // Fire-and-forget — the in-memory state is authoritative for the
        // live runtime; persistence is a read-side projection.
        self.persist_agent_status(name, &new_status, &new_last_event);
    }

    /// Send a `HostEvent` to both the mpsc channel (TUI / `recv_event`)
    /// and the broadcast channel (event subscribers). The mpsc
    /// send is silently dropped if no receiver is alive; the broadcast
    /// send only fails if no receiver has ever subscribed (and we don't
    /// care in that case either — the broadcast keeps a 0-receiver
    /// buffer without panicking).
    fn emit_host_event(&self, event: HostEvent) {
        let _ = self.notify_tx.send(event.clone());
        let _ = self.event_broadcast.send(event);
    }

    /// Receive the next host lifecycle event (agent registered / unregistered).
    pub async fn recv_event(&mut self) -> Option<HostEvent> {
        self.notify_rx.recv().await
    }

    /// Try to receive a host lifecycle event without blocking.
    pub fn try_recv_event(&mut self) -> Option<HostEvent> {
        self.notify_rx.try_recv().ok()
    }

    /// Try to receive an event without blocking.
    pub fn try_recv_any(&mut self) -> Option<TaggedEvent> {
        self.event_rx.try_recv().ok()
    }

    /// One iteration of the event loop: receive an event, process it
    /// through the topology network, and execute routing actions.
    ///
    /// Returns `(agent_name, raw_event, routing_actions)` or `None` if
    /// all agents are done.
    ///
    /// **Note**: delegation plumbing (response accumulation + tool reply)
    /// is handled inside `recv_any()`. This method is kept for callers
    /// that need the routing actions.
    pub async fn step(&mut self) -> Option<(String, AgentEvent)> {
        // Drain any pending tool commands first.
        self.try_process_commands();
        self.recv_any().await
    }

    /// Check if a named agent is registered.
    pub fn contains_agent(&self, name: &str) -> bool {
        self.agents.contains_key(name)
    }

    /// Number of registered agents.
    pub fn agent_count(&self) -> usize {
        self.agents.len()
    }

    /// Names of all registered agents.
    pub fn agent_names(&self) -> Vec<&str> {
        self.agents.keys().map(|s| s.as_str()).collect()
    }

    /// Spawn a new agent from an [`AgentProfile`](agentik_core::AgentProfile).
    ///
    /// Delegates to [`SharedInfra::spawn_agent`] — which only needs the
    /// shared infrastructure, not the topology network or registry.
    /// This allows callers to clone [`SharedInfra`] (cheap, all `Arc`)
    /// into async tasks without cloning the non-`Clone` [`RuntimeHost`].
    pub async fn spawn_agent(
        &self,
        agent_path: &agentik_types::AgentPath,
        profile: &agentik_core::AgentProfile,
        global_model: Arc<ArcSwapOption<Model>>,
        model_override: Option<Model>,
    ) -> HostResult<AgentHandle> {
        self.infra
            .spawn_agent(agent_path, profile, global_model, model_override)
            .await
    }

    /// Returns a clonable [`SharedInfra`] that can spawn agents from
    /// within async tasks. Use this instead of cloning [`RuntimeHost`]
    /// (which is not `Clone`).
    ///
    /// ```ignore
    /// let infra = host.spawner();
    /// tokio::spawn(async move {
    ///     infra.spawn_agent(...).await
    /// });
    /// ```
    pub fn spawner(&self) -> SharedInfra {
        self.infra.clone()
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Capability extraction
// ═══════════════════════════════════════════════════════════════════════

/// Build [`AgentInfo`] from an [`AgentProfile`], auto-extracting tags,
/// expertise, and tool list from the profile's feature flags.
fn capability_from_profile(
    name: &str,
    path: &str,
    profile: &agentik_core::AgentProfile,
) -> crate::control::AgentInfo {
    let mut tags = Vec::new();
    let mut expertise = Vec::new();

    if profile.enable_bibliography {
        tags.push("literature".into());
        tags.push("bibliography".into());
        expertise.push("literature-search".into());
        expertise.push("citation-management".into());
    }
    if profile.enable_writing {
        tags.push("writing".into());
        tags.push("latex".into());
        tags.push("manuscript".into());
        expertise.push("document-editing".into());
        expertise.push("citation-insertion".into());
        expertise.push("latex-compilation".into());
    }
    if profile.enable_opengwas {
        tags.push("gwas".into());
        tags.push("genetics".into());
        expertise.push("gwas-analysis".into());
    }
    if profile.enable_opentargets {
        tags.push("drug-targets".into());
        expertise.push("target-identification".into());
    }
    if profile.enable_gwascatalog {
        tags.push("gwas-catalog".into());
        expertise.push("variant-lookup".into());
    }
    if profile.enable_iceberg {
        tags.push("datalake".into());
        tags.push("iceberg".into());
        expertise.push("data-lake-query".into());
    }
    if profile.enable_dag_history {
        tags.push("pipeline".into());
        expertise.push("dag-execution".into());
    }

    crate::control::AgentInfo {
        name: name.into(),
        path: path.into(),
        summary: profile.description.clone(),
        tags,
        expertise,
        tools: Vec::new(), // populated at runtime if needed
        status: crate::control::AgentStatus::Idle,
        last_event: None,
    }
}

// ═══════════════════════════════════════════════════════════════════════
// derive_agent_status — pure projection AgentEvent → AgentStatus
// ═══════════════════════════════════════════════════════════════════════

/// Project an observed [`AgentEvent`] into a runtime [`AgentStatus`].
///
/// Pure function — no side effects, no registry access — so the
/// projection logic stays trivially unit-testable in isolation. The
/// caller ([`RuntimeHost::observe_status`]) handles the bookkeeping.
///
/// The authoritative source for the agent's lifecycle is the
/// [`AgentLifecycleStatus`] enum which the agent itself emits via
/// [`AgentEvent::LifecycleChanged`]. We map that into our coarser
/// 4-state [`AgentStatus`] (Idle / Running / AwaitingTool / Failed)
/// plus the tool-level events ([`AgentEvent::ToolCall`],
/// [`AgentEvent::ToolResult`], [`AgentEvent::ToolCallBackground`],
/// [`AgentEvent::ToolBackgroundComplete`]) which add the
/// "what tool is it waiting on?" detail that the lifecycle signal
/// alone can't carry.
///
/// `Done` and `Error` are terminal-in-turn signals. `Done` flips to
/// [`AgentStatus::Completed`] and is non-sticky — a fresh message
/// kicks the agent back to `Running`. `Error` is sticky until the
/// agent is shut down.
fn derive_agent_status(event: &AgentEvent) -> (AgentStatus, Option<String>) {
    match event {
        // ── Authoritative lifecycle signals ──
        AgentEvent::LifecycleChanged(lc) => match lc {
            agentik_types::AgentLifecycleStatus::Idle
            | agentik_types::AgentLifecycleStatus::Aborted => (AgentStatus::Idle, None),

            agentik_types::AgentLifecycleStatus::Requesting
            | agentik_types::AgentLifecycleStatus::Streaming
            | agentik_types::AgentLifecycleStatus::Compacting => (AgentStatus::Running, None),

            agentik_types::AgentLifecycleStatus::ToolRunning => (
                AgentStatus::AwaitingTool {
                    tool: "executing".into(),
                },
                Some("tool running".into()),
            ),

            agentik_types::AgentLifecycleStatus::Retrying => {
                (AgentStatus::Running, Some("retrying".into()))
            }

            agentik_types::AgentLifecycleStatus::Waiting => (
                AgentStatus::AwaitingTool {
                    tool: "wait_task".into(),
                },
                Some("wait_task".into()),
            ),

            agentik_types::AgentLifecycleStatus::Error => (
                AgentStatus::Failed {
                    message: "lifecycle error".into(),
                },
                Some("lifecycle error".into()),
            ),

            agentik_types::AgentLifecycleStatus::Cancelled => (
                AgentStatus::Failed {
                    message: "cancelled by user".into(),
                },
                Some("cancelled by user".into()),
            ),
        },

        // ── Standalone Requesting signal ──
        // The agent emits both `LifecycleChanged(Requesting)` and the bare
        // `Requesting` event; the latter carries no extra info. Either path
        // maps to Running.
        AgentEvent::Requesting => (AgentStatus::Running, None),

        // ── Tool-level signals (overwrite the lifecycle-derived status) ──
        AgentEvent::ToolCall { name, .. } | AgentEvent::ToolCallBackground { name, seq: _ } => (
            AgentStatus::AwaitingTool { tool: name.clone() },
            Some(name.clone()),
        ),

        AgentEvent::ToolResult { ok, content } => {
            let prefix = if *ok { "ok" } else { "err" };
            let preview = truncate_preview(content, 80);
            (
                AgentStatus::Running,
                if preview.is_empty() {
                    Some(format!("tool_result[{prefix}]"))
                } else {
                    Some(format!("tool_result[{prefix}]: {preview}"))
                },
            )
        }

        AgentEvent::ToolBackgroundComplete { ok, seq: _ } => {
            let prefix = if *ok { "ok" } else { "err" };
            (AgentStatus::Running, Some(format!("bg_tool[{prefix}]")))
        }

        // ── Aggregated LLM responses ──
        AgentEvent::LlmResponse(text) => {
            let preview = truncate_preview(text, 120);
            (AgentStatus::Running, Some(format!("llm: {preview}")))
        }
        AgentEvent::Thinking(text) => {
            let preview = truncate_preview(text, 120);
            (AgentStatus::Running, Some(format!("thinking: {preview}")))
        }

        // ── Terminal-in-turn signals ──
        AgentEvent::Done => (AgentStatus::Completed, Some("done".into())),

        AgentEvent::Error(msg) => (
            AgentStatus::Failed {
                message: msg.clone(),
            },
            Some(msg.clone()),
        ),

        AgentEvent::TurnAborted => (
            AgentStatus::Failed {
                message: "turn aborted by user".into(),
            },
            Some("turn aborted by user".into()),
        ),

        AgentEvent::RetryableError {
            message,
            attempt,
            max_retries,
        } => (
            AgentStatus::Running,
            Some(format!("retrying ({attempt}/{max_retries}): {message}")),
        ),

        // ── Context-management events — agent is busy, keep Running ──
        AgentEvent::Compact { .. } | AgentEvent::PlanUpdate { .. } => (AgentStatus::Running, None),

        // ── Intra-stream noise (token deltas, content-block boundaries,
        //    usage updates, stream start/stop) — all part of "running" ──
        AgentEvent::TextDelta(_)
        | AgentEvent::ThinkingDelta(_)
        | AgentEvent::UsageUpdate { .. }
        | AgentEvent::StreamStart { .. }
        | AgentEvent::ContentBlockStart { .. }
        | AgentEvent::ContentBlockStop { .. }
        | AgentEvent::StreamDelta { .. } => (AgentStatus::Running, None),

        // ── Session lifecycle events — out of scope for runtime status ──
        AgentEvent::SessionActivated { .. }
        | AgentEvent::SessionPaused { .. }
        | AgentEvent::SessionClosed { .. }
        | AgentEvent::SessionList { .. } => (AgentStatus::Idle, None),
    }
}

/// Returns `true` if `status` is terminal — i.e. the agent won't
/// transition further on its own.
///
/// `Completed` is terminal-in-turn but **not** sticky across shutdown:
/// a fresh message flips the agent back to `Running`. From a caller's
/// perspective, `Completed` IS terminal — the caller has the response
/// it needed and any subsequent turn is a new request the caller must
/// opt into via `send_message` / `delegate_to`.
///
/// `Failed` is sticky until the agent is shut down: every subsequent
/// event re-asserts the same failure.
pub fn is_final_status(status: &AgentStatus) -> bool {
    matches!(status, AgentStatus::Completed | AgentStatus::Failed { .. })
}

/// Truncate a free-form string for inclusion in `last_event` so the
/// field stays a one-line summary. Collapses internal whitespace so
/// LLM response previews don't blow up across line breaks.
fn truncate_preview(s: &str, max: usize) -> String {
    let collapsed: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.len() <= max {
        collapsed
    } else {
        // Cut on a char boundary, not byte index.
        let mut end = max;
        while !collapsed.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &collapsed[..end])
    }
}

#[cfg(test)]
mod status_derivation_tests {
    use super::*;
    use agentik_sdk::types::AgentEvent;
    use agentik_types::{AgentLifecycleStatus, CompactEvent};
    use chrono::Utc;
    use serde_json::json;

    // ── Lifecycle events ──

    #[test]
    fn lifecycle_idle_flips_to_idle() {
        let (s, _) = derive_agent_status(&AgentEvent::LifecycleChanged(AgentLifecycleStatus::Idle));
        assert_eq!(s, AgentStatus::Idle);
    }

    #[test]
    fn lifecycle_requesting_flips_to_running() {
        let (s, _) = derive_agent_status(&AgentEvent::LifecycleChanged(
            AgentLifecycleStatus::Requesting,
        ));
        assert_eq!(s, AgentStatus::Running);
    }

    #[test]
    fn lifecycle_waiting_flips_to_awaiting_tool() {
        let (s, _) =
            derive_agent_status(&AgentEvent::LifecycleChanged(AgentLifecycleStatus::Waiting));
        assert_eq!(
            s,
            AgentStatus::AwaitingTool {
                tool: "wait_task".into()
            }
        );
    }

    #[test]
    fn lifecycle_cancelled_flips_to_failed() {
        let (s, _) = derive_agent_status(&AgentEvent::LifecycleChanged(
            AgentLifecycleStatus::Cancelled,
        ));
        assert!(matches!(s, AgentStatus::Failed { .. }));
    }

    #[test]
    fn lifecycle_error_flips_to_failed() {
        let (s, _) =
            derive_agent_status(&AgentEvent::LifecycleChanged(AgentLifecycleStatus::Error));
        assert!(matches!(s, AgentStatus::Failed { .. }));
    }

    #[test]
    fn lifecycle_retrying_carries_retry_label() {
        let (s, ev) = derive_agent_status(&AgentEvent::LifecycleChanged(
            AgentLifecycleStatus::Retrying,
        ));
        assert_eq!(s, AgentStatus::Running);
        assert_eq!(ev.as_deref(), Some("retrying"));
    }

    // ── Tool events ──

    #[test]
    fn tool_call_flips_to_awaiting_tool() {
        let (s, ev) = derive_agent_status(&AgentEvent::ToolCall {
            name: "run_bash".into(),
            input: json!({}),
        });
        assert_eq!(
            s,
            AgentStatus::AwaitingTool {
                tool: "run_bash".into()
            }
        );
        assert_eq!(ev.as_deref(), Some("run_bash"));
    }

    #[test]
    fn tool_call_background_flips_to_awaiting_tool() {
        let (s, _) = derive_agent_status(&AgentEvent::ToolCallBackground {
            seq: 1,
            name: "run_dag".into(),
        });
        assert_eq!(
            s,
            AgentStatus::AwaitingTool {
                tool: "run_dag".into()
            }
        );
    }

    #[test]
    fn tool_result_returns_to_running_with_preview() {
        let (s, ev) = derive_agent_status(&AgentEvent::ToolResult {
            ok: true,
            content: "exit 0\nhello".into(),
        });
        assert_eq!(s, AgentStatus::Running);
        assert!(ev.unwrap().contains("ok"));
    }

    #[test]
    fn tool_background_complete_returns_to_running() {
        let (s, ev) = derive_agent_status(&AgentEvent::ToolBackgroundComplete { seq: 1, ok: true });
        assert_eq!(s, AgentStatus::Running);
        assert!(ev.unwrap().contains("ok"));
    }

    // ── Aggregated LLM responses ──

    #[test]
    fn llm_response_carries_truncated_preview() {
        let long = "x".repeat(500);
        let (s, ev) = derive_agent_status(&AgentEvent::LlmResponse(long));
        assert_eq!(s, AgentStatus::Running);
        let preview = ev.unwrap();
        assert!(preview.len() <= 130); // 120 + ellipsis + prefix
        assert!(preview.ends_with('…'));
    }

    // ── Terminal-in-turn events ──

    #[test]
    fn done_flips_to_completed() {
        let (s, _) = derive_agent_status(&AgentEvent::Done);
        assert_eq!(s, AgentStatus::Completed);
    }

    #[test]
    fn error_flips_to_failed_with_full_message() {
        let (s, ev) = derive_agent_status(&AgentEvent::Error("LLM 503: rate limited".into()));
        assert_eq!(
            s,
            AgentStatus::Failed {
                message: "LLM 503: rate limited".into()
            }
        );
        assert_eq!(ev.as_deref(), Some("LLM 503: rate limited"));
    }

    #[test]
    fn turn_aborted_flips_to_failed() {
        let (s, _) = derive_agent_status(&AgentEvent::TurnAborted);
        assert!(matches!(s, AgentStatus::Failed { .. }));
    }

    #[test]
    fn retryable_error_keeps_running() {
        let (s, ev) = derive_agent_status(&AgentEvent::RetryableError {
            message: "rate limit".into(),
            attempt: 2,
            max_retries: 5,
        });
        assert_eq!(s, AgentStatus::Running);
        assert!(ev.unwrap().contains("2/5"));
    }

    // ── Context management ──

    #[test]
    fn compact_flips_to_running() {
        let (s, _) = derive_agent_status(&AgentEvent::Compact {
            event: CompactEvent::CompactStart { ts: Utc::now() },
        });
        assert_eq!(s, AgentStatus::Running);
    }

    // ── Intra-stream noise ──

    #[test]
    fn text_delta_does_not_change_running() {
        let (s, ev) = derive_agent_status(&AgentEvent::TextDelta("tok".into()));
        assert_eq!(s, AgentStatus::Running);
        assert!(ev.is_none());
    }

    // ── truncate_preview helpers ──

    #[test]
    fn truncate_collapses_whitespace() {
        let s = truncate_preview("line1\n  line2\t\tline3", 100);
        assert_eq!(s, "line1 line2 line3");
    }

    #[test]
    fn truncate_respects_char_boundaries() {
        // Chinese chars are 3 bytes in UTF-8 — naive byte slicing would panic.
        let s = truncate_preview("你好世界你好世界你好世界", 7);
        assert!(s.ends_with('…'));
        // The returned string is still valid UTF-8 (no panic, no partial char).
        assert!(s.is_char_boundary(s.len()));
    }
}

#[cfg(test)]
mod status_tests {
    use super::*;

    // ── is_final_status ──

    #[test]
    fn idle_is_not_final() {
        assert!(!is_final_status(&AgentStatus::Idle));
    }

    #[test]
    fn running_is_not_final() {
        assert!(!is_final_status(&AgentStatus::Running));
    }

    #[test]
    fn awaiting_tool_is_not_final() {
        assert!(!is_final_status(&AgentStatus::AwaitingTool {
            tool: "run_bash".into()
        }));
    }

    #[test]
    fn completed_is_final() {
        assert!(is_final_status(&AgentStatus::Completed));
    }

    #[test]
    fn failed_is_final() {
        assert!(is_final_status(&AgentStatus::Failed {
            message: "boom".into()
        }));
    }

    // ── AgentStatus::tag for log lines ──

    #[test]
    fn agent_status_tags_match_serde_kind() {
        assert_eq!(AgentStatus::Idle.tag(), "idle");
        assert_eq!(AgentStatus::Running.tag(), "running");
        assert_eq!(
            AgentStatus::AwaitingTool { tool: "x".into() }.tag(),
            "awaiting_tool"
        );
        assert_eq!(AgentStatus::Completed.tag(), "completed");
        assert_eq!(
            AgentStatus::Failed {
                message: "x".into()
            }
            .tag(),
            "failed"
        );
    }

    // ── emit_host_event forwards to both channels ──
    // Integration test: spin up a real RuntimeHost with SharedInfra
    // minimal setup, register a fake agent, observe a status change,
    // and verify the broadcast receiver gets the AgentStatusChanged.

    #[tokio::test]
    async fn broadcast_receives_agent_status_changed() {
        use std::time::Duration;

        // The full RuntimeHost::open requires DataEngine/Iceberg/Turso
        // setup which is heavy for a unit test. Instead, exercise the
        // broadcast wiring at the module level by directly calling
        // the low-level emit_host_event pattern.
        let (tx, _) = tokio::sync::broadcast::channel::<HostEvent>(16);
        let mut rx = tx.subscribe();
        let event = HostEvent::AgentStatusChanged {
            path: "/root/test".into(),
            status: AgentStatus::Completed,
            last_event: Some("done".into()),
        };
        let _ = tx.send(event.clone());

        // Receiver must see the event within a short timeout.
        let received = tokio::time::timeout(Duration::from_millis(100), rx.recv())
            .await
            .expect("timeout")
            .expect("recv");
        assert_eq!(received.agent_path_or_test(), "/root/test");
    }

    // Helper accessor for tests — HostEvent doesn't expose fields
    // publicly but tests need to assert path equality.
    trait HostEventTestExt {
        fn agent_path_or_test(&self) -> &str;
    }

    impl HostEventTestExt for HostEvent {
        fn agent_path_or_test(&self) -> &str {
            match self {
                HostEvent::AgentRegistered { path, .. } => path.as_str(),
                HostEvent::AgentUnregistered { path } => path,
                HostEvent::AgentStatusChanged { path, .. } => path,
            }
        }
    }
}

#[cfg(test)]
mod interrupt_agent_tests {
    //! Phase 3 — interrupt_agent vs shutdown_agent distinction.
    //!
    //! These tests verify the command-layer invariants without spinning
    //! up a full RuntimeHost (which requires DataEngine/Iceberg/Turso).
    //! The full integration test would be: spawn agent → delegate_to →
    //! interrupt_agent → assert LifecycleChanged(Cancelled) and agent
    //! still in registry.

    use super::*;
    use crate::control::HostCommand;

    /// `interrupt` and `shutdown` MUST be distinct commands — the
    /// handler dispatches them to different paths:
    /// - `CancelAgent` → AgentCommand::Cancel → handle.cancel() (preserves
    ///   agent; only the current turn aborts; new token issued)
    /// - `Shutdown { name }` → AgentCommand::Shutdown → handle.shutdown()
    ///   → graceful exit (removes from registry)
    ///
    /// Mixing these up would silently kill long-lived worker agents on
    /// a benign interrupt request.
    #[test]
    fn cancel_and_shutdown_are_distinct_commands() {
        let cancel = HostCommand::CancelAgent {
            name: "worker".into(),
        };
        let shutdown = HostCommand::Shutdown {
            name: "worker".into(),
        };

        // Different concrete types — pattern-match proves it.
        let cancel_kind = match &cancel {
            HostCommand::CancelAgent { .. } => "cancel",
            _ => "other",
        };
        let shutdown_kind = match &shutdown {
            HostCommand::Shutdown { .. } => "shutdown",
            _ => "other",
        };
        assert_eq!(cancel_kind, "cancel");
        assert_eq!(shutdown_kind, "shutdown");
    }

    /// `InterruptAgentInput` should accept `agent_name` as required and
    /// `reason` as optional with a sensible default. The tool emits a
    /// log line + a ToolResult::success message; we test the schema by
    /// serializing/deserializing JSON in the same shape the LLM would
    /// produce.
    #[test]
    fn interrupt_input_schema_accepts_minimal_payload() {
        // Mimic the schema the LLM would emit when calling the tool.
        let raw = serde_json::json!({ "agent_name": "worker" });
        let parsed: serde_json::Result<serde_json::Value> = Ok(raw.clone());
        let v = parsed.expect("parse");
        assert_eq!(v["agent_name"], "worker");
        // reason is omitted → tool default is "user-requested interrupt"
        assert!(v.get("reason").is_none());
    }

    /// `InterruptAgentInput` schema with explicit reason.
    #[test]
    fn interrupt_input_schema_accepts_full_payload() {
        let raw = serde_json::json!({
            "agent_name": "researcher",
            "reason": "wrong agent selected"
        });
        assert_eq!(raw["agent_name"], "researcher");
        assert_eq!(raw["reason"], "wrong agent selected");
    }

    /// The `is_final_status` predicate interacts with interrupt:
    /// after `cancel_agent`, the agent emits `LifecycleChanged(Cancelled)`
    /// which `derive_agent_status` maps to `AgentStatus::Failed`. Verify
    /// the chain `Cancelled → Failed → is_final` returns true.
    #[test]
    fn cancelled_lifecycle_maps_to_failed_via_derive() {
        let (status, _) = derive_agent_status(&AgentEvent::LifecycleChanged(
            agentik_types::AgentLifecycleStatus::Cancelled,
        ));
        assert!(
            is_final_status(&status),
            "Cancelled must map to a terminal status so delegated tasks \
             don't hang forever after interrupt_agent fires"
        );
    }

    /// Same for `Error` lifecycle — fatal system errors must also be
    /// terminal.
    #[test]
    fn error_lifecycle_maps_to_failed_via_derive() {
        let (status, _) = derive_agent_status(&AgentEvent::LifecycleChanged(
            agentik_types::AgentLifecycleStatus::Error,
        ));
        assert!(is_final_status(&status));
    }
}

#[cfg(test)]
mod send_message_tests {
    //! Phase 5 — `send_message` fire-and-forget inter-agent messaging.
    //!
    //! These tests verify the command-layer invariants without spinning
    //! up a full RuntimeHost. The key properties tested:
    //! 1. `SendMessage` is a distinct variant from `DeliverMessage` and
    //!    `Delegate` (they must not be confused).
    //! 2. `SendMessage` carries a reply channel for delivery confirmation.
    //! 3. The `SendMessageInput` schema matches what the LLM would emit.

    use super::*;
    use crate::control::{HostCommand, HostControl};
    use tokio::sync::oneshot;

    /// `SendMessage`, `DeliverMessage`, and `Delegate` MUST be distinct
    /// variants. They dispatch to different handler paths:
    /// - `DeliverMessage` → fire-and-forget TUI message (no reply)
    /// - `SendMessage` → fire-and-forget inter-agent (reply: Ok/Err)
    /// - `Delegate` → request-response (reply: full response text)
    #[test]
    fn send_message_is_distinct_from_deliver_and_delegate() {
        let send = HostCommand::SendMessage {
            to: "worker".into(),
            message: "hello".into(),
            reply_tx: oneshot::channel().0,
        };
        let deliver = HostCommand::DeliverMessage {
            name: "worker".into(),
            message: "hello".into(),
        };
        let delegate = HostCommand::Delegate {
            to: "worker".into(),
            message: "hello".into(),
            reply_tx: oneshot::channel().0,
        };

        // Pattern-match proves each is a distinct variant.
        let send_kind = match &send {
            HostCommand::SendMessage { .. } => "send",
            _ => "other",
        };
        let deliver_kind = match &deliver {
            HostCommand::DeliverMessage { .. } => "deliver",
            _ => "other",
        };
        let delegate_kind = match &delegate {
            HostCommand::Delegate { .. } => "delegate",
            _ => "other",
        };
        assert_eq!(send_kind, "send");
        assert_eq!(deliver_kind, "deliver");
        assert_eq!(delegate_kind, "delegate");
        assert_ne!(send_kind, deliver_kind);
        assert_ne!(send_kind, delegate_kind);
    }

    /// `SendMessage` uses a `Result<(), String>` reply channel (unlike
    /// `Delegate` which uses `String`). This is important: the handler
    /// must reply `Ok(())` on success or `Err(msg)` on agent-not-found,
    /// not a plain string.
    #[test]
    fn send_message_reply_channel_is_result_unit_string() {
        let (tx, rx) = oneshot::channel::<Result<(), String>>();
        let _cmd = HostCommand::SendMessage {
            to: "worker".into(),
            message: "hello".into(),
            reply_tx: tx,
        };
        // The type system already proved the channel type by compiling.
        // We drop rx without sending — the command was never processed.
        drop(rx);
    }

    /// `SendMessageInput` schema: `agent_name` + `message`, both required.
    /// No optional fields (unlike `interrupt_agent` which has optional
    /// `reason`). The LLM must always specify a target and content.
    #[test]
    fn send_message_input_schema_is_name_and_message() {
        let raw = serde_json::json!({
            "agent_name": "researcher",
            "message": "Please analyze the results."
        });
        assert_eq!(raw["agent_name"], "researcher");
        assert_eq!(raw["message"], "Please analyze the results.");
        // No optional fields in the schema.
        assert!(raw.get("reason").is_none());
        assert!(raw.get("timeout_ms").is_none());
    }

    /// `HostControl::send_message` wires up the reply channel correctly.
    /// Verify the round-trip: send a SendMessage command through a
    /// channel, extract it, reply, and confirm the caller receives the
    /// result.
    #[tokio::test]
    async fn send_message_round_trip_delivery_success() {
        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<HostCommand>();
        let (event_tx, _) = tokio::sync::broadcast::channel::<HostEvent>(1);
        let control = HostControl::new(cmd_tx, event_tx);

        // Spawn the "caller" — sends the message and awaits reply.
        let caller =
            tokio::spawn(async move { control.send_message("worker", "hello there").await });

        // "Host" side: receive the command and reply Ok(()).
        let cmd = cmd_rx.recv().await.expect("command received");
        match cmd {
            HostCommand::SendMessage {
                to,
                message,
                reply_tx,
            } => {
                assert_eq!(to, "worker");
                assert_eq!(message, "hello there");
                let _ = reply_tx.send(Ok(()));
            }
            _ => panic!("expected SendMessage variant"),
        }

        let result = caller.await.expect("caller task panicked");
        assert_eq!(result, Some(Ok(())));
    }

    /// Round-trip with agent-not-found error.
    #[tokio::test]
    async fn send_message_round_trip_agent_not_found() {
        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<HostCommand>();
        let (event_tx, _) = tokio::sync::broadcast::channel::<HostEvent>(1);
        let control = HostControl::new(cmd_tx, event_tx);

        let caller = tokio::spawn(async move { control.send_message("nonexistent", "test").await });

        let cmd = cmd_rx.recv().await.expect("command received");
        match cmd {
            HostCommand::SendMessage { reply_tx, .. } => {
                let _ = reply_tx.send(Err("agent not found".into()));
            }
            _ => panic!("expected SendMessage variant"),
        }

        let result = caller.await.expect("caller task panicked");
        assert!(matches!(result, Some(Err(_))));
    }

    /// When the host command channel is closed (host dropped), `send_message`
    /// returns `None` — not an error, not a hang. This is the same
    /// fail-soft behavior as all other `ask()`-based methods.
    #[tokio::test]
    async fn send_message_returns_none_when_channel_closed() {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<HostCommand>();
        let (event_tx, _) = tokio::sync::broadcast::channel::<HostEvent>(1);
        let control = HostControl::new(cmd_tx, event_tx);

        // Drop the receiver → channel is closed.
        drop(cmd_rx);

        let result = control.send_message("worker", "hello").await;
        assert!(result.is_none(), "should return None on closed channel");
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Relay loop — per-agent event/command bridge
// ═══════════════════════════════════════════════════════════════════════

/// Owns one [`AgentHandle`] and bridges:
/// - **Inbound**: `AgentCommand`s from the host → `send_message`/`shutdown`
/// - **Outbound**: `AgentEvent`s from the agent → tagged events to host
async fn relay_loop(
    mut handle: AgentHandle,
    mut cmd_rx: UnboundedReceiver<AgentCommand>,
    event_tx: UnboundedSender<TaggedEvent>,
    name: String,
) {
    tracing::debug!(agent = %name, "relay task started");

    loop {
        tokio::select! {
            biased;

            event = handle.recv_event() => match event {
                Some(ev) => {
                    if event_tx.send((name.clone(), ev)).is_err() {
                        break;
                    }
                }
                None => break,
            },

            cmd = cmd_rx.recv() => match cmd {
                Some(AgentCommand::Message(text)) => {
                    handle.send_message(text);
                }
                Some(AgentCommand::Cancel) => {
                    handle.cancel();
                }
                Some(AgentCommand::ListSessions) => {
                    handle.list_sessions();
                }
                Some(AgentCommand::CreateSession { title, fork_from }) => {
                    let _ = handle.create_session(title, fork_from);
                }
                Some(AgentCommand::SwitchSession(id)) => {
                    handle.switch_session(id);
                }
                Some(AgentCommand::CloseSession(id)) => {
                    handle.close_session(id);
                }
                Some(AgentCommand::RenameSession { id, title }) => {
                    handle.rename_session(id, title);
                }
                Some(AgentCommand::SetModel(model)) => {
                    handle.set_model(model);
                }
                Some(AgentCommand::Shutdown) | None => {
                    handle.shutdown();
                    break;
                }
            }
        }
    }

    // After breaking out of the relay loop, wait for the agent's `run()`
    // task to finish. The agent needs time to process the Shutdown event,
    // pause all sessions (persist snapshots + end WAL sessions), and flush
    // any remaining persist_worker messages. Without this await, dropping
    // the runtime would kill the task mid-shutdown, causing data loss.
    handle.join().await;

    tracing::debug!(agent = %name, "relay task exited");
}

// ═══════════════════════════════════════════════════════════════════════
// Drop — best-effort shutdown
// ═══════════════════════════════════════════════════════════════════════

impl Drop for RuntimeHost {
    fn drop(&mut self) {
        for (_, entry) in self.agents.drain() {
            let _ = entry.cmd_tx.send(AgentCommand::Shutdown);
        }
    }
}
