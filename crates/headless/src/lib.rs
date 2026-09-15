//! Headless (non-interactive) execution of agentik agents.
//!
//! This crate is the UI-free sibling of the TUI: both entry points sit on
//! the same library layer (`runtime::RuntimeHost`, `agentik-core`) and
//! never share UI objects. It provides
//!
//! - [`event`]: the stable external run-event contract (`RunEvent`),
//!   emitted as JSONL on stdout in `--json` mode;
//! - [`processor`]: pluggable output rendering (human-readable vs JSONL)
//!   over the same translated event stream;
//! - [`run_task`]: the run loop — open a [`RuntimeHost`], spawn one agent
//!   from a profile, deliver a single prompt, translate the agent's event
//!   stream into [`RunEvent`]s until the turn reaches a terminal state,
//!   then shut down cleanly.
//!
//! # Termination semantics
//!
//! The authoritative signal is `AgentEvent::TurnCompleted` for the
//! top-level turn (no delegation): `Completed`, `Failed`, or
//! `Interrupted` (mapped to a cancelled run). The compatibility `Done` /
//! `Error` events are cross-checks only, never triggers.
//!
//! # stdout discipline
//!
//! Borrowed from codex's `exec`: in human mode the only bytes written to
//! stdout are the final agent message (if any); in `--json` mode stdout is
//! strictly JSONL, one event per line. Everything else — progress, tool
//! output, warnings — goes to the progress stream (stderr). Processors
//! therefore write through injected [`std::io::Write`] targets instead of
//! printing, which also keeps them unit-testable against buffers.

#![deny(clippy::print_stdout)]

pub mod event;
pub mod gateway_runner;
pub mod manifest;
pub mod processor;

use std::collections::VecDeque;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use agentik_core::{AgentProfile, AgentRuntimeOverrides};
use agentik_sdk::model::Model;
use agentik_types::{AgentEvent, CompactEvent, TurnExecutionStatus};
use arc_swap::ArcSwapOption;
use processor::OutputProcessor;
use runtime::{HostEvent, RuntimeConfig, RuntimeHost};
use serde_json::Value;
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use vfs::{BackendConfig, BackendDefinition, MountDefinition, VfsManifest};

use event::{
    AgentMessageItem, ItemEvent, NoticeEvent, NoticeKind, ReasoningItem, RunEndedEvent, RunEvent,
    RunItem, RunItemDetails, RunStartedEvent, RunStatus, ToolCallItem, TurnCompletedEvent,
    TurnFailedEvent, TurnStartedEvent, Usage,
};

/// How long `recv_any` may block before the loop pumps the host command
/// channel again, so agent tools issuing [`runtime::HostControl`] calls
/// never starve.
const COMMAND_PUMP_INTERVAL: Duration = Duration::from_millis(50);

/// Upper bound on the final agent shutdown wait before the host is
/// dropped regardless.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(30);

/// Implementation selected for an ephemeral run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EphemeralBackend {
    /// Open `RuntimeHost` in the calling process. This is the Phase 1
    /// default.
    #[default]
    InProcess,
    /// Start a one-shot gateway process. Reserved for a later phase.
    Gateway,
}

/// One host source mapped into the ephemeral virtual filesystem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EphemeralMount {
    pub source: PathBuf,
    pub target: String,
    pub read_only: bool,
}

impl EphemeralMount {
    pub fn new(
        source: impl Into<PathBuf>,
        target: impl Into<String>,
        read_only: bool,
    ) -> Result<Self, EphemeralSetupError> {
        let source = source.into();
        let target = target.into();
        let target = normalize_virtual_target(&target)?;
        Ok(Self {
            source,
            target,
            read_only,
        })
    }
}

/// Declarative inputs used to build one isolated run's state and VFS.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EphemeralRunSpec {
    pub backend: EphemeralBackend,
    pub workspace: Option<EphemeralMount>,
    pub data_mounts: Vec<EphemeralMount>,
    /// Reuse a workspace that already contains entries. Inputs remain
    /// read-only, but this permits explicit benchmark retries.
    pub resume_workspace: bool,
}

#[derive(Debug, Error)]
pub enum EphemeralSetupError {
    #[error("the gateway ephemeral backend is not implemented yet")]
    UnsupportedBackend,
    #[error("mount source must be absolute: {0}")]
    RelativeSource(PathBuf),
    #[error("invalid {kind} target `{target}`: {reason}")]
    InvalidTarget {
        kind: &'static str,
        target: String,
        reason: &'static str,
    },
    #[error("mount target `{target}` overlaps reserved target `{reserved}`")]
    ReservedTargetOverlap { target: String, reserved: String },
    #[error("mount target `{target}` overlaps `{other}`")]
    MountOverlap { target: String, other: String },
    #[error("workspace `{path}` is not empty; pass --resume-workspace to reuse it")]
    NonEmptyWorkspace { path: PathBuf },
    #[error("source {kind} `{path}`: {source}")]
    SourceIo {
        kind: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("create state directory `{path}`: {source}")]
    StateIo {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("serialize VFS manifest `{path}`: {source}")]
    ManifestSerialize {
        path: PathBuf,
        source: toml::ser::Error,
    },
    #[error("write VFS manifest `{path}`: {source}")]
    ManifestWrite {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid generated VFS manifest: {0}")]
    InvalidManifest(String),
}

/// Everything `run_task` needs to execute one prompt.
#[derive(Clone)]
pub struct RunTaskConfig {
    /// Invocation id used by the external event stream and run manifest.
    pub run_id: Uuid,
    /// The prompt delivered as the single user message of the run.
    pub prompt: String,
    /// Profile path to spawn from; `None` picks the first stored profile.
    pub profile: Option<String>,
    /// Runtime overrides layered onto the selected profile before spawn.
    pub agent_runtime: AgentRuntimeOverrides,
    /// Agent name segment, mounted directly under the root path.
    pub agent_name: String,
    /// The model the agent runs with. `None` leaves the host without a
    /// global model — spawning then fails, so callers normally resolve
    /// one via `runtime::model_bootstrap` (or inject a mock in tests).
    pub model: Option<Model>,
    /// Model name reported in `run.started`. `Model` exposes no name
    /// accessor, so the caller passes the name it resolved (or a hint
    /// like `"mock"` in tests).
    pub model_name: Option<String>,
    /// Resume this session instead of continuing the agent's active
    /// one. The agent must already know it (same agent path, persisted
    /// in the state dir); scripts obtain the id from `turn.started`.
    pub session: Option<Uuid>,
    /// Wall-clock budget for the whole run. When it elapses the run is
    /// abandoned as cancelled (exit code 2 territory): a
    /// `turn.failed` event with the timeout message is emitted, then
    /// `run.ended{status: cancelled}`, and the agent is shut down.
    pub timeout: Option<Duration>,
    /// Fully resolved runtime configuration (state dir, feature flags, …).
    pub runtime_config: RuntimeConfig,
    /// Cooperative cancellation requested by the embedding frontend.
    pub cancel: CancellationToken,
}

/// Keeps the ephemeral scratch directories alive for the duration of a
/// run. The root is removed on drop unless [`EphemeralState::keep`] was
/// called.
pub struct EphemeralState {
    root: PathBuf,
    cleanup: bool,
}

impl EphemeralState {
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Preserve the ephemeral root after the run (debugging support).
    pub fn keep(&mut self) -> &Path {
        self.cleanup = false;
        self.root.as_path()
    }

    /// Remove the root explicitly so callers can surface cleanup failures.
    pub fn cleanup(mut self) -> Result<PathBuf, std::io::Error> {
        self.cleanup = false;
        let root = self.root.clone();
        std::fs::remove_dir_all(&root).map(|()| root)
    }
}

impl Drop for EphemeralState {
    fn drop(&mut self) {
        if self.cleanup {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
}

impl RunTaskConfig {
    /// A run whose data/state directories live under a fresh temp dir,
    /// removed when the returned [`EphemeralState`] drops. The model is
    /// still resolved by the caller (normally from the real app DB) and
    /// set afterwards — ephemerality scopes *conversation state*, not
    /// credentials.
    pub fn ephemeral(prompt: impl Into<String>) -> (Self, EphemeralState) {
        Self::ephemeral_with_mounts(prompt, EphemeralRunSpec::default())
            .expect("default ephemeral setup is valid")
    }

    /// Create an isolated run and materialize its VFS manifest from
    /// host-mounted benchmark inputs and workspace.
    pub fn ephemeral_with_mounts(
        prompt: impl Into<String>,
        mut spec: EphemeralRunSpec,
    ) -> Result<(Self, EphemeralState), EphemeralSetupError> {
        if spec.backend == EphemeralBackend::Gateway {
            return Err(EphemeralSetupError::UnsupportedBackend);
        }
        validate_mount_targets(&mut spec)?;

        let dir = tempfile::tempdir().expect("create ephemeral dir");
        restrict_directory_to_owner(dir.path());
        let mut runtime_config = RuntimeConfig::default();
        // Every persistent path must move under the temp dir —
        // `RuntimeConfig::default()` bakes absolute `~/.autonomics`
        // paths for the databases at resolution time, and an ephemeral
        // run that still writes sessions to the real agent DB is not
        // ephemeral.
        runtime_config.data_dir = dir.path().join("data");
        runtime_config.state_dir = dir.path().join("state");
        runtime_config.agent_db = dir.path().join("state/agents.db");
        runtime_config.dag_history_db = dir.path().join("state/dag_history.db");
        runtime_config.bib_db_path = dir.path().join("state/bib.db");
        runtime_config.writing_db_path = dir.path().join("state/writing.db");
        runtime_config.app_db_path = dir.path().join("state/app.db");
        let opengwas_cache_dir = dir.path().join("cache/opengwas");
        runtime_config.opengwas_cache_dir = Some(opengwas_cache_dir.clone());
        prepare_workspace(&mut spec)?;
        std::fs::create_dir_all(&opengwas_cache_dir).map_err(|source| {
            EphemeralSetupError::StateIo {
                path: opengwas_cache_dir,
                source,
            }
        })?;
        std::fs::create_dir_all(&runtime_config.data_dir).map_err(|source| {
            EphemeralSetupError::StateIo {
                path: runtime_config.data_dir.clone(),
                source,
            }
        })?;
        let manifest = ephemeral_vfs_manifest(&runtime_config, &spec)?;
        vfs::MountedObjectStore::from_manifest(&manifest)
            .map_err(|error| EphemeralSetupError::InvalidManifest(error.to_string()))?;
        write_ephemeral_vfs_manifest(&dir.path().join("state/vfs.toml"), &manifest)?;
        let config = Self::new(prompt, runtime_config);
        Ok((
            config,
            EphemeralState {
                root: dir.keep(),
                cleanup: true,
            },
        ))
    }

    /// Parse the TOML form accepted by `autonomics run --mount-manifest`.
    ///
    /// The file describes mount intent, not the generated VFS manifest:
    /// read-only mode is supplied by the field used (`data_mounts` or
    /// `workspace`) and cannot be overridden by the file.
    pub fn parse_mount_manifest(source: &str) -> Result<EphemeralRunSpec, EphemeralSetupError> {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawMount {
            source: PathBuf,
            target: String,
        }

        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawManifest {
            #[serde(default)]
            workspace: Option<RawMount>,
            #[serde(default)]
            data_mounts: Vec<RawMount>,
        }

        let raw: RawManifest = toml::from_str(source)
            .map_err(|error| EphemeralSetupError::InvalidManifest(error.to_string()))?;
        let mut spec = EphemeralRunSpec::default();
        if let Some(workspace) = raw.workspace {
            spec.workspace = Some(EphemeralMount::new(
                workspace.source,
                workspace.target,
                false,
            )?);
        }
        for mount in raw.data_mounts {
            spec.data_mounts
                .push(EphemeralMount::new(mount.source, mount.target, true)?);
        }
        Ok(spec)
    }

    pub fn new(prompt: impl Into<String>, runtime_config: RuntimeConfig) -> Self {
        Self {
            run_id: Uuid::new_v4(),
            prompt: prompt.into(),
            profile: None,
            agent_runtime: AgentRuntimeOverrides::default(),
            agent_name: "headless".to_string(),
            model: None,
            model_name: None,
            session: None,
            timeout: None,
            runtime_config,
            cancel: CancellationToken::new(),
        }
    }
}

fn normalize_virtual_target(target: impl AsRef<str>) -> Result<String, EphemeralSetupError> {
    let target = target.as_ref();
    if target.contains('\0') {
        return Err(invalid_target(target, "must not contain a NUL byte"));
    }

    let path = Path::new(target);
    if !path.is_absolute() {
        return Err(invalid_target(target, "must use an absolute virtual path"));
    }

    let mut components = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::Normal(part) => components.push(part.to_os_string()),
            std::path::Component::CurDir => {}
            std::path::Component::RootDir => {}
            std::path::Component::ParentDir => {
                return Err(invalid_target(target, "must not contain `..`"));
            }
            std::path::Component::Prefix(_) => {
                return Err(invalid_target(target, "must not contain a Windows prefix"));
            }
        }
    }
    if components.is_empty() {
        return Err(invalid_target(target, "must name a path below `/`"));
    }

    let mut normalized = PathBuf::from("/");
    for component in components {
        normalized.push(component);
    }
    normalized
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| invalid_target(target, "must be valid UTF-8"))
}

fn invalid_target(target: &str, reason: &'static str) -> EphemeralSetupError {
    EphemeralSetupError::InvalidTarget {
        kind: "mount",
        target: target.to_owned(),
        reason,
    }
}

fn validate_mount_targets(spec: &mut EphemeralRunSpec) -> Result<(), EphemeralSetupError> {
    let mut targets = Vec::new();
    let mut validate = |mount: &mut EphemeralMount| -> Result<(), EphemeralSetupError> {
        mount.target = normalize_virtual_target(&mount.target)?;
        if mount.target == "/literature" || mount.target.starts_with("/literature/") {
            return Err(EphemeralSetupError::ReservedTargetOverlap {
                target: mount.target.clone(),
                reserved: "/literature".to_owned(),
            });
        }
        if let Some(previous) = targets
            .iter()
            .find(|previous| {
                Path::new(previous).starts_with(&mount.target)
                    || Path::new(&mount.target).starts_with(previous)
            })
            .cloned()
        {
            return Err(EphemeralSetupError::MountOverlap {
                target: mount.target.clone(),
                other: previous,
            });
        }
        targets.push(mount.target.clone());
        Ok(())
    };

    if let Some(workspace) = spec.workspace.as_mut() {
        workspace.read_only = false;
        validate(workspace)?;
    }
    for mount in spec.data_mounts.iter_mut() {
        mount.read_only = true;
        validate(mount)?;
    }
    Ok(())
}

fn canonicalize_source(kind: &'static str, source: &Path) -> Result<PathBuf, EphemeralSetupError> {
    if !source.is_absolute() {
        return Err(EphemeralSetupError::RelativeSource(source.to_owned()));
    }
    source
        .canonicalize()
        .map_err(|source_error| EphemeralSetupError::SourceIo {
            kind,
            path: source.to_owned(),
            source: source_error,
        })
}

fn prepare_workspace(spec: &mut EphemeralRunSpec) -> Result<(), EphemeralSetupError> {
    for mount in spec.data_mounts.iter_mut() {
        mount.source = canonicalize_source("data mount", &mount.source)?;
    }

    if let Some(workspace) = spec.workspace.as_mut() {
        if !workspace.source.is_absolute() {
            return Err(EphemeralSetupError::RelativeSource(
                workspace.source.clone(),
            ));
        }
        if workspace.source.exists() {
            workspace.source = canonicalize_source("workspace", &workspace.source)?;
            if !workspace.source.is_dir() {
                return Err(EphemeralSetupError::SourceIo {
                    kind: "workspace",
                    path: workspace.source.clone(),
                    source: std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "workspace source must be a directory",
                    ),
                });
            }
            let is_empty = workspace
                .source
                .read_dir()
                .map_err(|source| EphemeralSetupError::SourceIo {
                    kind: "workspace",
                    path: workspace.source.clone(),
                    source,
                })?
                .next()
                .is_none();
            if !is_empty && !spec.resume_workspace {
                return Err(EphemeralSetupError::NonEmptyWorkspace {
                    path: workspace.source.clone(),
                });
            }
        } else {
            std::fs::create_dir_all(&workspace.source).map_err(|source| {
                EphemeralSetupError::SourceIo {
                    kind: "workspace",
                    path: workspace.source.clone(),
                    source,
                }
            })?;
            workspace.source = canonicalize_source("workspace", &workspace.source)?;
        }
    }
    Ok(())
}

fn ephemeral_vfs_manifest(
    config: &RuntimeConfig,
    spec: &EphemeralRunSpec,
) -> Result<VfsManifest, EphemeralSetupError> {
    let mut backend = vec![BackendDefinition {
        id: "scratch".to_owned(),
        config: BackendConfig::local("/"),
    }];
    let mut mount = vec![MountDefinition {
        path: "/".to_owned(),
        backend: "scratch".to_owned(),
        source: config.data_dir.to_string_lossy().into_owned(),
        read_only: false,
    }];

    let literature_root = config.state_dir.join("literature");
    backend.push(BackendDefinition {
        id: "literature".to_owned(),
        config: BackendConfig::local(literature_root.to_string_lossy().into_owned()),
    });
    mount.push(MountDefinition {
        path: "/literature".to_owned(),
        backend: "literature".to_owned(),
        source: "/".to_owned(),
        read_only: false,
    });

    let add_local_mount = |id: &str,
                           mount_spec: &EphemeralMount,
                           backend: &mut Vec<BackendDefinition>,
                           mount: &mut Vec<MountDefinition>| {
        let source = &mount_spec.source;
        let (root, source_key) = if source.is_dir() {
            (source.to_string_lossy().into_owned(), "/".to_owned())
        } else {
            let root = source.parent().unwrap_or(Path::new("/"));
            let key = source
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "/".to_owned());
            (root.to_string_lossy().into_owned(), key)
        };
        backend.push(BackendDefinition {
            id: id.to_owned(),
            config: BackendConfig::local(root),
        });
        mount.push(MountDefinition {
            path: mount_spec.target.clone(),
            backend: id.to_owned(),
            source: source_key,
            read_only: mount_spec.read_only,
        });
    };

    for (index, data_mount) in spec.data_mounts.iter().enumerate() {
        add_local_mount(
            &format!("benchmark-data-{index}"),
            data_mount,
            &mut backend,
            &mut mount,
        );
    }
    if let Some(workspace) = spec.workspace.as_ref() {
        add_local_mount("benchmark-workspace", workspace, &mut backend, &mut mount);
    }

    Ok(VfsManifest { backend, mount })
}

fn write_ephemeral_vfs_manifest(
    path: &Path,
    manifest: &VfsManifest,
) -> Result<(), EphemeralSetupError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| EphemeralSetupError::StateIo {
            path: parent.to_owned(),
            source,
        })?;
    }
    let contents = toml::to_string_pretty(manifest).map_err(|source| {
        EphemeralSetupError::ManifestSerialize {
            path: path.to_owned(),
            source,
        }
    })?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|source| EphemeralSetupError::ManifestWrite {
            path: path.to_owned(),
            source,
        })?;
    file.write_all(contents.as_bytes())
        .map_err(|source| EphemeralSetupError::ManifestWrite {
            path: path.to_owned(),
            source,
        })
}

fn restrict_directory_to_owner(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

/// Terminal outcome of a completed `run_task` invocation.
#[derive(Debug, Clone)]
pub struct RunSummary {
    pub run_id: Uuid,
    pub outcome: processor::Outcome,
    pub agent_path: String,
    /// Resolved profile path the agent ran with.
    pub profile: String,
    pub last_message: Option<String>,
    pub wall_time_secs: f64,
    /// Cumulative usage across all turns, when any was reported.
    pub usage: Option<Usage>,
    pub turns: u64,
    /// Tool calls that completed during the run.
    pub tool_calls: u64,
}

/// Startup-phase failures — distinct from a failed *turn*, which is a
/// measured result of the run, not an infrastructure error. The CLI maps
/// these to a different exit code (3) than turn failure (1).
#[derive(Debug, Error)]
pub enum RunError {
    #[error("ephemeral setup failed: {0}")]
    EphemeralSetup(#[from] EphemeralSetupError),
    #[error("failed to open runtime host: {0}")]
    HostOpen(#[from] runtime::Error),
    #[error("gateway error: {0}")]
    Gateway(String),
    #[error("no agent profile available (requested: {requested:?})")]
    NoProfile { requested: Option<String> },
    #[error("agent spawn failed: {message}")]
    Spawn { message: String },
    #[error("prompt delivery failed: {message}")]
    Send { message: String },
    #[error("session switch failed for {session}: {message}")]
    SessionSwitch { session: Uuid, message: String },
    #[error("run cancelled")]
    Cancelled,
}

/// Run one prompt headlessly, streaming translated events into `processor`.
///
/// Owns the full lifecycle: host open → profile bootstrap → agent spawn →
/// prompt delivery → event loop → clean shutdown. See the crate docs for
/// termination semantics and the stdout discipline contract.
pub async fn run_task<P: OutputProcessor>(
    config: RunTaskConfig,
    processor: &mut P,
) -> Result<RunSummary, RunError> {
    let started = Instant::now();
    if config.cancel.is_cancelled() {
        return Err(RunError::Cancelled);
    }
    let mut host = RuntimeHost::open(&config.runtime_config).await?;

    // ── Profile bootstrap (same sequence as the TUI startup) ────────
    let profile_storage = host.infra().profile_storage.clone();
    let _ = profile_storage.seed_defaults_if_empty().await;
    let profiles = profile_storage.list_profiles().await.unwrap_or_default();
    let mut profile =
        pick_profile(&profiles, config.profile.as_deref()).ok_or(RunError::NoProfile {
            requested: config.profile.clone(),
        })?;
    profile.apply_runtime_overrides(config.agent_runtime);
    host.set_profiles(profiles);
    host.set_model(Arc::new(ArcSwapOption::from_pointee(config.model)));

    // ── Spawn ────────────────────────────────────────────────────────
    // The control call needs the host command loop pumping while its
    // oneshot reply is pending, so it runs as a task and this side keeps
    // processing commands (same pattern as the TUI and the host tests).
    let control = host.control();
    let profile_for_spawn = profile.clone();
    let agent_name = config.agent_name.clone();
    let mut spawn = tokio::spawn(async move {
        control
            .spawn_with_profile(
                &agent_name,
                &agentik_types::AgentPath::root(),
                profile_for_spawn,
                None,
            )
            .await
    });
    let agent_path = loop {
        tokio::select! {
            result = &mut spawn => {
                break result.expect("spawn task panicked").map_err(|message| RunError::Spawn { message })?;
            }
            _ = host.recv_and_process_command() => {}
        }
    };

    // The registration event (carrying the agent id) is queued on the
    // host-event channel during command processing; recover it, best
    // effort with a deadline.
    let agent_id = match tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Some(HostEvent::AgentRegistered { info, .. }) = host.try_recv_event() {
                return info.agent_id.unwrap_or_default();
            }
            host.recv_and_process_command().await;
        }
    })
    .await
    {
        Ok(id) => id,
        Err(_) => Uuid::nil(),
    };

    // ── Optional session resume ──────────────────────────────────────
    // The switch is fire-and-forget and the agent applies it
    // asynchronously, so wait for its `SessionActivated` confirmation
    // before delivering the prompt — otherwise a fast prompt can beat
    // the switch and land in a fresh session. Events observed while
    // waiting are buffered and replayed into the translation loop.
    let mut pre_events: VecDeque<(String, AgentEvent)> = VecDeque::new();
    if let Some(session) = config.session {
        host.control().switch_session(&agent_path, session);
        host.try_process_commands();
        let mut activated = false;
        let switch_deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < switch_deadline {
            host.try_process_commands();
            match tokio::time::timeout(Duration::from_millis(20), host.recv_any()).await {
                Ok(Some(tagged)) => {
                    activated = matches!(
                        &tagged.1,
                        AgentEvent::SessionActivated { id, .. } if *id == session
                    );
                    if activated {
                        break;
                    }
                    pre_events.push_back(tagged);
                }
                Ok(None) => break,
                Err(_) => {}
            }
        }
        if !activated {
            return Err(RunError::SessionSwitch {
                session,
                message: "SessionActivated was not observed before delivery deadline".into(),
            });
        }
    }

    // ── Prompt delivery ──────────────────────────────────────────────
    let control = host.control();
    let path_for_send = agent_path.clone();
    let prompt = config.prompt.clone();
    let mut send = tokio::spawn(async move {
        control
            .send_message(&path_for_send, prompt)
            .await
            .ok_or_else(|| "host command channel closed".to_string())
            .and_then(|r| r)
    });
    loop {
        tokio::select! {
            result = &mut send => {
                result.expect("send task panicked").map_err(|message| RunError::Send { message })?;
                break;
            }
            _ = host.recv_and_process_command() => {}
        }
    }

    // ── Event loop ───────────────────────────────────────────────────
    processor.process(&RunEvent::RunStarted(RunStartedEvent {
        run_id: config.run_id,
        agent_id,
        // Bound at the first turn.started; nil until then (single-turn
        // runs have exactly one session, created by the agent itself).
        session_id: Uuid::nil(),
        profile: profile.path.clone(),
        model: config.model_name.clone(),
    }));

    let mut translation = TranslationState::default();
    let deadline = config.timeout.map(|budget| started + budget);
    let terminal = loop {
        if config.cancel.is_cancelled() {
            host.control().cancel_agent(&agent_path);
            host.try_process_commands();
            translation.terminal = Some(Terminal::Cancelled);
            if !processor.is_broken() {
                processor.process(&RunEvent::TurnFailed(TurnFailedEvent {
                    turn_id: translation.current_turn_id,
                    message: "run cancelled".to_string(),
                }));
            }
            break Terminal::Cancelled;
        }
        if let Some(deadline) = deadline
            && Instant::now() >= deadline
        {
            let budget = config.timeout.expect("deadline implies timeout");
            // Cooperative cancel first: aborts the in-flight turn —
            // interrupts retry backoff and running tools — so the
            // shutdown below doesn't wait on them. `cancel_agent` is
            // fire-and-forget, so drain the host command queue here to
            // actually dispatch it.
            host.control().cancel_agent(&agent_path);
            host.try_process_commands();
            translation.terminal = Some(Terminal::Cancelled);
            if !processor.is_broken() {
                processor.process(&RunEvent::TurnFailed(TurnFailedEvent {
                    turn_id: translation.current_turn_id,
                    message: format!("run timed out after {:.1}s", budget.as_secs_f64()),
                }));
            }
            break Terminal::Cancelled;
        }
        host.try_process_commands();
        let tagged = match pre_events.pop_front() {
            Some(tagged) => Ok(Some(tagged)),
            None => tokio::time::timeout(COMMAND_PUMP_INTERVAL, host.recv_any()).await,
        };
        match tagged {
            Ok(Some((name, event))) => {
                for run_event in translation.translate(&name, event) {
                    processor.process(&run_event);
                    if processor.is_broken() {
                        config.cancel.cancel();
                    }
                }
                if translation.terminal.is_some() {
                    // Drain events already queued behind the terminal one
                    // (e.g. the compatibility Done/Error) without blocking.
                    while let Some((name, event)) = host.try_recv_any() {
                        for run_event in translation.translate(&name, event) {
                            processor.process(&run_event);
                            if processor.is_broken() {
                                config.cancel.cancel();
                            }
                        }
                    }
                    break translation.terminal.take().expect("checked above");
                }
            }
            // Every agent task has ended and its event channel closed.
            Ok(None) => {
                break translation.terminal.take().unwrap_or(Terminal::Failed);
            }
            // Poll timeout with no event pending: loop back around to
            // pump host commands and re-check the deadline.
            Err(_) => continue,
        }
    };

    let wall_time_secs = started.elapsed().as_secs_f64();
    let status = match &terminal {
        Terminal::Completed => RunStatus::Completed,
        Terminal::Failed => RunStatus::Failed,
        Terminal::Cancelled => RunStatus::Cancelled,
    };
    let usage = (translation.turns > 0).then_some(translation.run_usage);
    let turns = translation.turns;
    let tool_calls = translation.tool_calls;
    if !processor.is_broken() {
        processor.process(&RunEvent::RunEnded(RunEndedEvent {
            run_id: config.run_id,
            status,
            wall_time_secs,
            usage,
            turns,
            tool_calls,
        }));
    }
    processor.finish();

    // Belt and braces: the cooperative cancel (timeout path) and the
    // terminal turn state (normal path) should make shutdown prompt, but
    // a wedged tool must not hang the process — drop the host instead.
    if tokio::time::timeout(SHUTDOWN_GRACE, host.shutdown_all_agents_and_wait())
        .await
        .is_err()
    {
        tracing::warn!("agents did not shut down within grace; dropping host");
    }

    Ok(RunSummary {
        run_id: config.run_id,
        profile: profile.path.clone(),
        outcome: match terminal {
            Terminal::Completed => processor::Outcome::Completed,
            Terminal::Failed => processor::Outcome::Failed,
            Terminal::Cancelled => processor::Outcome::Cancelled,
        },
        agent_path,
        last_message: processor.last_message().map(str::to_owned),
        wall_time_secs,
        usage,
        turns,
        tool_calls,
    })
}

/// Pick the profile to run: exact `path` match when requested, else the
/// first stored profile.
fn pick_profile(profiles: &[AgentProfile], requested: Option<&str>) -> Option<AgentProfile> {
    match requested {
        Some(path) => profiles
            .iter()
            .find(|p| p.path == path)
            .or_else(|| profiles.iter().find(|p| p.name() == path))
            .cloned(),
        None => profiles.first().cloned(),
    }
}

/// Terminal state extracted from the authoritative turn event.
#[derive(Debug, Clone)]
enum Terminal {
    Completed,
    /// The failure detail was already emitted as a `turn.failed` event;
    /// the variant itself only discriminates the outcome.
    Failed,
    Cancelled,
}

/// One-shot translation state: pairs tool calls with results, tracks
/// per-turn and cumulative usage, and captures the terminal turn status.
#[derive(Debug, Default)]
struct TranslationState {
    next_item: usize,
    pending_tools: VecDeque<(String, String, Value)>,
    turn_usage: Usage,
    run_usage: Usage,
    turns: u64,
    tool_calls: u64,
    terminal: Option<Terminal>,
    /// Turn id of the in-flight top-level turn, from `TurnStarted`.
    current_turn_id: Uuid,
    /// Most recent retryable-error message — used as the failure text
    /// when the turn then fails.
    error_hint: Option<String>,
}

impl TranslationState {
    fn next_id(&mut self, prefix: &str) -> String {
        self.next_item += 1;
        format!("{prefix}-{}", self.next_item)
    }

    /// Translate one agent event into zero or more run events. Events
    /// from agents other than the spawned one (delegated children) are
    /// translated too, but their turn boundaries never terminate the run.
    fn translate(&mut self, _agent: &str, event: AgentEvent) -> Vec<RunEvent> {
        match event {
            AgentEvent::TurnStarted {
                turn_id,
                session_id,
                delegation_id: None,
            } => {
                // A fresh top-level turn: reset per-turn usage.
                self.turn_usage = Usage::default();
                self.current_turn_id = turn_id;
                vec![RunEvent::TurnStarted(TurnStartedEvent {
                    turn_id,
                    session_id,
                })]
            }

            AgentEvent::ToolCall { name, input } => {
                let id = self.next_id("tool");
                self.pending_tools
                    .push_back((id.clone(), name.clone(), input.clone()));
                vec![RunEvent::ItemStarted(ItemEvent {
                    item: RunItem {
                        id,
                        details: RunItemDetails::ToolCall(ToolCallItem {
                            tool: name,
                            input,
                            result: None,
                            ok: None,
                        }),
                    },
                })]
            }

            AgentEvent::ToolResult { ok, content } => {
                // Pair with the oldest pending tool call. A result with no
                // pending call (e.g. stray background completion) is
                // dropped — background tools report through notices.
                let Some((id, tool, input)) = self.pending_tools.pop_front() else {
                    return vec![];
                };
                self.tool_calls += 1;
                vec![RunEvent::ItemCompleted(ItemEvent {
                    item: RunItem {
                        id,
                        details: RunItemDetails::ToolCall(ToolCallItem {
                            tool,
                            input,
                            result: Some(content),
                            ok: Some(ok),
                        }),
                    },
                })]
            }

            AgentEvent::LlmResponse(text) => {
                let id = self.next_id("msg");
                vec![RunEvent::ItemCompleted(ItemEvent {
                    item: RunItem {
                        id,
                        details: RunItemDetails::AgentMessage(AgentMessageItem { text }),
                    },
                })]
            }

            AgentEvent::Thinking(text) => {
                let id = self.next_id("think");
                vec![RunEvent::ItemCompleted(ItemEvent {
                    item: RunItem {
                        id,
                        details: RunItemDetails::Reasoning(ReasoningItem { text }),
                    },
                })]
            }

            AgentEvent::UsageUpdate {
                input_tokens,
                output_tokens,
                cache_creation_input_tokens,
                cache_read_input_tokens,
            } => {
                // Each delta carries stream-cumulative values; merge with
                // Option-awareness (input/cache fields are Some only on
                // the final delta of a stream).
                self.turn_usage.output_tokens = output_tokens;
                if let Some(v) = input_tokens {
                    self.turn_usage.input_tokens = Some(v);
                }
                if let Some(v) = cache_creation_input_tokens {
                    self.turn_usage.cache_creation_input_tokens = Some(v);
                }
                if let Some(v) = cache_read_input_tokens {
                    self.turn_usage.cache_read_input_tokens = Some(v);
                }
                vec![]
            }

            AgentEvent::TurnCompleted {
                turn_id,
                delegation_id: None,
                status,
                ..
            } => {
                self.turns += 1;
                self.fold_turn_usage();
                match status {
                    TurnExecutionStatus::Completed => {
                        self.terminal = Some(Terminal::Completed);
                        vec![RunEvent::TurnCompleted(TurnCompletedEvent {
                            turn_id,
                            usage: self.turn_usage,
                        })]
                    }
                    TurnExecutionStatus::Failed => {
                        let message = self
                            .error_hint
                            .clone()
                            .unwrap_or_else(|| "turn failed".to_string());
                        self.terminal = Some(Terminal::Failed);
                        vec![RunEvent::TurnFailed(TurnFailedEvent { turn_id, message })]
                    }
                    TurnExecutionStatus::Interrupted => {
                        self.terminal = Some(Terminal::Cancelled);
                        vec![RunEvent::TurnFailed(TurnFailedEvent {
                            turn_id,
                            message: "turn interrupted".to_string(),
                        })]
                    }
                }
            }

            AgentEvent::Error(message) => {
                // Contract: TurnCompleted(Failed) precedes Error. Treat a
                // bare Error without a prior terminal as a defensive
                // failure; otherwise it adds the detail the turn event
                // could not carry.
                let turn_id = self.current_turn_id;
                match self.terminal.take() {
                    Some(Terminal::Failed) => {
                        self.terminal = Some(Terminal::Failed);
                        vec![RunEvent::TurnFailed(TurnFailedEvent { turn_id, message })]
                    }
                    Some(other) => {
                        self.terminal = Some(other);
                        vec![]
                    }
                    None => {
                        self.turns += 1;
                        self.fold_turn_usage();
                        self.terminal = Some(Terminal::Failed);
                        vec![RunEvent::TurnFailed(TurnFailedEvent { turn_id, message })]
                    }
                }
            }

            AgentEvent::RetryableError { message, .. } => {
                self.error_hint = Some(message.clone());
                vec![notice(NoticeKind::RetryableError, message)]
            }

            AgentEvent::Compact { event } => {
                let message = match event {
                    CompactEvent::CompactStart { .. } => "context compaction started".to_string(),
                    CompactEvent::CompactFinish { .. } => "context compaction finished".to_string(),
                };
                vec![notice(NoticeKind::Compact, message)]
            }

            AgentEvent::PlanUpdate { revision, .. } => vec![notice(
                NoticeKind::PlanUpdate,
                format!("plan updated (revision {revision})"),
            )],

            AgentEvent::ToolCallBackground { seq, name } => vec![notice(
                NoticeKind::BackgroundTool,
                format!("tool {name} moved to background (#{seq})"),
            )],

            AgentEvent::TurnAborted => {
                if self.terminal.is_none() {
                    self.terminal = Some(Terminal::Cancelled);
                }
                vec![]
            }

            // Streaming deltas, lifecycle/session bookkeeping, delegated
            // child turns, and compatibility signals carry no run events.
            _ => vec![],
        }
    }

    /// Fold the finished turn's usage into the run totals: outputs sum
    /// across turns; input/cache fields reflect the latest known value.
    fn fold_turn_usage(&mut self) {
        self.run_usage.output_tokens += self.turn_usage.output_tokens;
        if self.turn_usage.input_tokens.is_some() {
            self.run_usage.input_tokens = self.turn_usage.input_tokens;
        }
        if self.turn_usage.cache_creation_input_tokens.is_some() {
            self.run_usage.cache_creation_input_tokens =
                self.turn_usage.cache_creation_input_tokens;
        }
        if self.turn_usage.cache_read_input_tokens.is_some() {
            self.run_usage.cache_read_input_tokens = self.turn_usage.cache_read_input_tokens;
        }
    }
}

fn notice(kind: NoticeKind, message: String) -> RunEvent {
    RunEvent::Notice(NoticeEvent { kind, message })
}

#[cfg(test)]
mod ephemeral_tests;
#[cfg(test)]
mod gateway_runner_tests;
#[cfg(test)]
mod tests;
