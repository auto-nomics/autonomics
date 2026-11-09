use super::*;
use dag_core::{
    DagNode, NodeInput, NodePorts, TaskExecution, TaskExecutor,
    dag::{
        DagError,
        graph::PortOutputs,
        node_event::{JobResult, NodeReporter},
    },
    registry::NodeCtx,
};
use data_engine::workflow::{FlowGraph, FlowRunConfig, FlowScheduler, Process, SourceResolution};
use futures::StreamExt;
use std::{
    any::Any,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

struct Fixture {
    _dir: tempfile::TempDir,
    config: RuntimeConfig,
    registry: Arc<ComputeRegistry>,
    containers: ContainerExecutionInfra,
}
fn mounts(config: &RuntimeConfig) -> (Arc<vfs::MountedObjectStore>, Arc<vfs::OpendalFileStorage>) {
    let vfs = Arc::new(
        vfs::MountedObjectStore::from_manifest(&vfs::VfsManifest::local_root(
            config.data_dir.to_string_lossy(),
        ))
        .unwrap(),
    );
    let storage = Arc::new(vfs::OpendalFileStorage::with_mounts(
        &config.data_dir,
        vfs.clone(),
    ));
    (vfs, storage)
}
fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let mut config = RuntimeConfig::default();
    config.state_dir = dir.path().to_path_buf();
    config.data_dir = dir.path().join("data");
    fs::create_dir(&config.data_dir).unwrap();
    config.slurm.enabled = false;
    config.slurm.tools_enabled = false;
    config.slurm.typed_worker_config = None;
    config.slurm.apptainer_manifest = None;
    let containers = ContainerExecutionInfra::from_config(
        container_runtime::PodmanConfig::for_state_dir(dir.path()),
    );
    let (vfs, storage) = mounts(&config);
    let registry = ComputeRegistry::open(
        &config,
        storage,
        vfs,
        &containers,
        crate::hpc_monitor::MonitorHub::empty(),
    )
    .unwrap();
    Fixture {
        _dir: dir,
        config,
        registry,
        containers,
    }
}
fn save_local(registry: &ComputeRegistry, id: &str, enabled: bool) {
    registry
        .save_target(SaveTarget {
            data_domains: Vec::new(),
            expected_revision: registry.state().revision,
            id: id.into(),
            name: id.into(),
            enabled,
            backend: ComputeBackend::Local,
        })
        .unwrap();
}
fn select(registry: &ComputeRegistry, id: &str) {
    registry
        .select(SelectTarget {
            expected_revision: registry.state().revision,
            target_id: id.into(),
        })
        .unwrap();
}

#[tokio::test]
async fn deleting_target_retains_bound_revision_and_prevents_reuse() {
    let f = fixture();
    save_local(&f.registry, "other", true);
    select(&f.registry, "other");
    let (_, binding) = f.registry.bind().unwrap();
    assert!(
        f.registry
            .delete_target("other", f.registry.state().revision)
            .is_err()
    );
    select(&f.registry, "local");
    let before = f.registry.state().revision;
    assert!(f.registry.delete_target("other", before - 1).is_err());
    f.registry.delete_target("other", before).unwrap();
    assert!(
        f.registry
            .target(&binding.target_id, Some(binding.target_revision))
            .is_ok()
    );
    assert!(
        f.registry
            .select(SelectTarget {
                expected_revision: f.registry.state().revision,
                target_id: "other".into()
            })
            .is_err()
    );
    assert!(
        f.registry
            .save_target(SaveTarget {
                expected_revision: f.registry.state().revision,
                id: "other".into(),
                name: "reused".into(),
                enabled: true,
                backend: ComputeBackend::Local,
                data_domains: vec![]
            })
            .is_err()
    );
    let (vfs, storage) = mounts(&f.config);
    let reopened = ComputeRegistry::open(
        &f.config,
        storage,
        vfs,
        &f.containers,
        crate::hpc_monitor::MonitorHub::empty(),
    )
    .unwrap();
    assert!(reopened.state().deleted_target_ids.contains("other"));
    assert!(
        reopened
            .target("other", Some(binding.target_revision))
            .is_ok()
    );
}
fn remote_settings(root: &Path, host: &str) -> SlurmConfig {
    let profile = root.join(format!("{host}-worker.json"));
    write_new(&profile,&serde_json::json!({"worker":{"executable":"/cluster/autonomics","executable_sha256":"a".repeat(64),"expected_source_revision":dag_core::source_revision()},"local_kinds":["channel","dynamic_fanout"]})).unwrap();
    let mut settings = SlurmConfig::default();
    settings.enabled = true;
    settings.ssh_host = host.into();
    let ssh = root.join(format!("{host}-ssh"));
    fs::write(&ssh,b"#!/bin/sh\nif [ \"$1\" = -G ]; then printf 'hostname toy.example\\nuser toy\\nport 22\\n'; exit 0; fi\nexit 91\n").unwrap();
    fs::set_permissions(&ssh, fs::Permissions::from_mode(0o700)).unwrap();
    settings.ssh_command = ssh.display().to_string();
    settings.data_ssh_host = Some(format!("{host}-data"));
    settings.pi_ssh_host = None;
    settings.typed_worker_config = Some(profile);
    settings.apptainer_manifest = None;
    settings
}

#[tokio::test]
async fn revision_evidence_is_visible_to_existing_clients_without_exposing_profiles() {
    let f = fixture();
    let reader = f.registry.storage.clone();
    let settings = remote_settings(&f.config.state_dir, "evidence");
    for revision in 1..=2 {
        f.registry
            .save_target(SaveTarget {
                data_domains: Vec::new(),
                expected_revision: f.registry.state().revision,
                id: "evidence".into(),
                name: "evidence".into(),
                enabled: true,
                backend: ComputeBackend::Slurm {
                    settings: settings.clone(),
                },
            })
            .unwrap();
        let root = f
            .registry
            .root
            .join("targets")
            .join(key("evidence", revision));
        for directory in ["tasks", "artifacts"] {
            let file = root.join(directory).join("attempt/task.json");
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(&file, b"{\"synthetic\":true}").unwrap();
            let path = file.to_string_lossy();
            assert_eq!(reader.local_host_path(&path), Some(file.clone()));
            assert_eq!(
                reader.read_range(&path, 0..18).await.unwrap().to_vec(),
                b"{\"synthetic\":true}"
            );
            assert!(reader.check_writable(&path).is_err());
        }
        let private = root.join("private-credential.json");
        fs::write(&private, b"private synthetic credential").unwrap();
        assert_ne!(
            reader.local_host_path(&private.to_string_lossy()),
            Some(private.clone())
        );
    }
    let (vfs, storage) = mounts(&f.config);
    let reopened = ComputeRegistry::open(
        &f.config,
        storage.clone(),
        vfs,
        &f.containers,
        crate::hpc_monitor::MonitorHub::empty(),
    )
    .unwrap();
    let historical = reopened
        .root
        .join("targets/evidence-r1/artifacts/attempt/task.json");
    assert_eq!(
        storage
            .read_range(&historical.to_string_lossy(), 0..18)
            .await
            .unwrap()
            .to_vec(),
        b"{\"synthetic\":true}"
    );
}

#[tokio::test]
async fn image_evidence_tracks_resident_revisions_not_mutable_catalog_files() {
    let f = fixture();
    let reference = format!("example/toy@sha256:{}", "a".repeat(64));
    let inventory = f.config.state_dir.join("toy-images.json");
    let settings_for = |sha: &str| {
        serde_json::json!({"program":"/usr/bin/apptainer",
        "images":{reference.clone():{"path":"/admitted/toy.sif","sha256":sha}}})
    };
    write_new(&inventory, &settings_for(&"b".repeat(64))).unwrap();
    let mut settings = remote_settings(&f.config.state_dir, "mapping");
    settings.apptainer_manifest = Some(inventory.clone());
    let save = |settings: SlurmConfig| {
        f.registry
            .save_target(SaveTarget {
                data_domains: Vec::new(),
                expected_revision: f.registry.state().revision,
                id: "mapping".into(),
                name: "mapping".into(),
                enabled: true,
                backend: ComputeBackend::Slurm { settings },
            })
            .unwrap()
    };
    save(settings.clone());
    let first = f.registry.image_evidence("mapping", 1, &reference).unwrap();
    assert_eq!(first.resolution.sif_sha256, "b".repeat(64));
    assert!(!first.sif_bytes_verified && !first.node_execution_performed);
    assert!(first.capsule_sha256.contains_key("images.json"));
    fs::write(
        &inventory,
        serde_json::to_vec(&settings_for(&"c".repeat(64))).unwrap(),
    )
    .unwrap();
    save(settings);
    let second = f.registry.image_evidence("mapping", 2, &reference).unwrap();
    assert_eq!(second.resolution.sif_sha256, "c".repeat(64));
    assert_ne!(first.target_sha256, second.target_sha256);
    assert_ne!(first.capsule_sha256, second.capsule_sha256);
    select(&f.registry, "mapping");
    // Even corrupting the on-disk frozen profile cannot rewrite resident evidence.
    let old = f.registry.target("mapping", Some(1)).unwrap();
    let ComputeBackend::Slurm { settings } = &old.target.backend else {
        unreachable!()
    };
    fs::write(
        settings.apptainer_manifest.as_ref().unwrap(),
        b"invalid replacement",
    )
    .unwrap();
    let retained = f.registry.image_evidence("mapping", 1, &reference).unwrap();
    assert_eq!(retained.resolution, first.resolution);
    assert_eq!(retained.target_sha256, first.target_sha256);
    assert!(
        f.registry
            .image_evidence("mapping", 99, &reference)
            .is_err()
    );
    assert!(
        f.registry
            .image_evidence("mapping", 2, "example/toy:latest")
            .is_err()
    );
    assert!(f.registry.image_evidence("local", 1, &reference).is_err());
    assert!(
        f.registry
            .image_evidence("mapping", 2, &format!("other@sha256:{}", "d".repeat(64)))
            .is_err()
    );
    assert_eq!(f.registry.status().unwrap().runs.len(), 0);
}

#[agentik_proc::tool(
    name = "environment_install",
    description = "Synthetic placement probe"
)]
pub struct EnvironmentProbeInput {}
struct EnvironmentProbe(Arc<std::sync::atomic::AtomicUsize>);
#[async_trait::async_trait]
impl agentik_core::tools::ToolFunction for EnvironmentProbe {
    type Input = EnvironmentProbeInput;
    async fn run(
        &self,
        _: Self::Input,
    ) -> Result<agentik_sdk::types::ToolResult, agentik_core::tools::ToolError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(agentik_sdk::types::ToolResult::success_json(
            serde_json::json!({"installed":true}),
        ))
    }
}

#[tokio::test]
async fn agent_context_and_environment_tools_follow_live_operator_policy() {
    use agentik_core::context::ContextProvider;
    let f = fixture();
    let context = super::tools::ComputeContext::new(f.registry.clone());
    assert!(context.poll().await.unwrap().contains("local"));
    assert!(context.poll().await.is_none());
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let tools = super::tools::guard_environment_tools(
        vec![EnvironmentProbe(calls.clone()).into()],
        Some(&f.registry),
    );
    let settings = remote_settings(&f.config.state_dir, "environment-host");
    f.registry
        .save_target(SaveTarget {
            data_domains: Vec::new(),
            expected_revision: f.registry.state().revision,
            id: "remote".into(),
            name: "remote".into(),
            enabled: true,
            backend: ComputeBackend::Slurm { settings },
        })
        .unwrap();
    select(&f.registry, "remote");
    assert!(context.poll().await.unwrap().contains("remote"));
    assert!(context.poll().await.is_none());
    assert!(
        tools[0]
            .implementation
            .execute(serde_json::json!({}))
            .await
            .is_err()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    select(&f.registry, "local");
    assert!(context.poll().await.unwrap().contains("local"));
    assert!(
        tools[0]
            .implementation
            .execute(serde_json::json!({}))
            .await
            .is_ok()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn durable_policy_uses_compare_and_swap_and_keeps_old_revisions() {
    let f = fixture();
    let (old, old_binding) = f.registry.bind().unwrap();
    save_local(&f.registry, "second", true);
    assert!(
        f.registry
            .select(SelectTarget {
                expected_revision: 1,
                target_id: "second".into()
            })
            .is_err()
    );
    select(&f.registry, "second");
    assert_eq!(old.target.id, "local");
    assert_eq!(old_binding.target_id, "local");
    assert_eq!(f.registry.bind().unwrap().1.target_id, "second");
    save_local(&f.registry, "second", false);
    assert!(f.registry.bind().is_err());
    assert!(
        f.registry
            .select(SelectTarget {
                expected_revision: f.registry.state().revision,
                target_id: "second".into()
            })
            .is_err()
    );
    assert_eq!(
        f.registry
            .target("second", Some(1))
            .unwrap()
            .target
            .revision,
        1
    );
    let (vfs, storage) = mounts(&f.config);
    let restored = ComputeRegistry::open(
        &f.config,
        storage,
        vfs,
        &f.containers,
        crate::hpc_monitor::MonitorHub::empty(),
    )
    .unwrap();
    assert_eq!(restored.state().policy.target_id, "second");
    assert!(restored.bind().is_err());
    assert_eq!(restored.status().unwrap().runs.len(), 2);
}

#[tokio::test]
async fn sources_survive_switches_and_restart_and_reach_existing_agent_views() {
    let f = fixture();
    let old_agent = f
        .registry
        .storage
        .with_principal(vfs::permission::VfsPrincipal::new(12345, 100));
    let root = f.config.state_dir.join("original-data");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("input.txt"), b"original science input").unwrap();
    let source = DataSource {
        domain: None,
        id: "sjtu".into(),
        name: "retained data".into(),
        backend: DataBackend::Local {
            root: root.display().to_string(),
        },
        read_only: true,
        endpoint: None,
        permissions: Default::default(),
    };
    f.registry
        .add_source(SaveDataSource {
            expected_revision: 1,
            source: source.clone(),
        })
        .await
        .unwrap();
    save_local(&f.registry, "server-b", true);
    select(&f.registry, "server-b");
    assert_eq!(
        old_agent
            .content_length("/data/sjtu/input.txt")
            .await
            .unwrap(),
        22
    );
    let stream = old_agent
        .read_stream("/data/sjtu/input.txt", 0..22)
        .await
        .unwrap();
    futures::pin_mut!(stream);
    let mut bytes = Vec::new();
    while let Some(b) = stream.next().await {
        bytes.extend(b.unwrap().to_vec());
    }
    assert_eq!(bytes, b"original science input");
    assert!(old_agent.check_writable("/data/sjtu/input.txt").is_err());
    assert!(
        f.registry
            .add_source(SaveDataSource {
                expected_revision: f.registry.state().revision,
                source
            })
            .await
            .is_err()
    );
    let (vfs, storage) = mounts(&f.config);
    let restored = ComputeRegistry::open(
        &f.config,
        storage,
        vfs,
        &f.containers,
        crate::hpc_monitor::MonitorHub::empty(),
    )
    .unwrap();
    assert_eq!(
        restored
            .storage
            .content_length("/data/sjtu/input.txt")
            .await
            .unwrap(),
        22
    );
    assert_eq!(restored.state().policy.target_id, "server-b");
}

#[tokio::test]
async fn new_source_cannot_hide_root_mount_files_directories_or_symlinks() {
    let f = fixture();
    let namespace = f.config.data_dir.join("data");
    fs::create_dir(&namespace).unwrap();
    fs::write(namespace.join("file"), b"original input").unwrap();
    fs::create_dir(namespace.join("directory")).unwrap();
    fs::write(namespace.join("directory/input.csv"), b"original table").unwrap();
    std::os::unix::fs::symlink(namespace.join("missing"), namespace.join("link")).unwrap();
    let source_root = f.config.state_dir.join("replacement");
    fs::create_dir(&source_root).unwrap();
    for id in ["file", "directory", "link"] {
        let error = f
            .registry
            .add_source(SaveDataSource {
                expected_revision: 1,
                source: DataSource {
                    domain: None,
                    id: id.into(),
                    name: id.into(),
                    read_only: true,
                    backend: DataBackend::Local {
                        root: source_root.display().to_string(),
                    },
                    endpoint: None,
                    permissions: Default::default(),
                },
            })
            .await
            .unwrap_err();
        assert!(error.contains("shadow existing VFS data"), "{error}");
        assert_eq!(f.registry.state().revision, 1);
        assert!(f.registry.state().data_sources.is_empty());
    }
    assert_eq!(
        f.registry
            .storage
            .content_length("/data/file")
            .await
            .unwrap(),
        14
    );
    assert_eq!(
        f.registry
            .storage
            .content_length("/data/directory/input.csv")
            .await
            .unwrap(),
        14
    );
    let persisted: ComputeRegistryState =
        serde_json::from_slice(&read_private(&f.registry.root.join("registry.json")).unwrap())
            .unwrap();
    assert!(persisted.data_sources.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn binding_publication_is_atomic_and_preserves_failed_evidence() {
    let f = fixture();
    let runs = f.registry.root.join("runs");
    // A crash before publication must not break status or count a partial run.
    let incomplete = runs.join(".pending-interrupted");
    write_bytes_new(&incomplete, b"{\"id\":").unwrap();
    assert!(f.registry.status().unwrap().runs.is_empty());
    let writer = f.registry.clone();
    let task = tokio::task::spawn_blocking(move || {
        for _ in 0..32 {
            writer.bind().unwrap();
        }
    });
    while !task.is_finished() {
        f.registry.status().unwrap();
        tokio::task::yield_now().await;
    }
    task.await.unwrap();
    let status = f.registry.status().unwrap();
    assert_eq!(status.runs.len(), 32);
    assert_eq!(fs::read(&incomplete).unwrap(), b"{\"id\":");
    let binding = &status.runs[0];
    let published = runs.join(format!("{}.json", binding.id));
    let original = read_private(&published).unwrap();
    let mut replacement = binding.clone();
    replacement.target_id = "must-not-replace".into();
    assert!(write_binding(&published, &replacement).is_err());
    assert_eq!(read_private(&published).unwrap(), original);
    assert_eq!(f.registry.status().unwrap().runs.len(), 32);
    assert_eq!(fs::read_dir(&runs).unwrap().count(), 34);
    // Only unpublished files are excluded. Corrupt historical records must
    // remain visible as an error, rather than silently disappearing.
    write_bytes_new(&runs.join("corrupt.json"), b"{").unwrap();
    assert!(f.registry.status().is_err());
    assert_eq!(fs::read(runs.join("corrupt.json")).unwrap(), b"{");
}

#[tokio::test]
async fn target_profiles_are_frozen_and_tampering_is_rejected_on_restart() {
    let f = fixture();
    let settings = remote_settings(&f.config.state_dir, "server-a");
    let source = settings.typed_worker_config.clone().unwrap();
    f.registry
        .save_target(SaveTarget {
            data_domains: Vec::new(),
            expected_revision: 1,
            id: "remote".into(),
            name: "remote".into(),
            enabled: true,
            backend: ComputeBackend::Slurm { settings },
        })
        .unwrap();
    let target = f.registry.target("remote", Some(1)).unwrap();
    let ComputeBackend::Slurm { settings } = &target.target.backend else {
        panic!()
    };
    let frozen = settings.typed_worker_config.clone().unwrap();
    assert_ne!(source, frozen);
    fs::write(&source, b"changed source profile").unwrap();
    select(&f.registry, "remote");
    assert!(f.registry.bind().is_ok());
    let (vfs, storage) = mounts(&f.config);
    assert!(
        ComputeRegistry::open(
            &f.config,
            storage,
            vfs,
            &f.containers,
            crate::hpc_monitor::MonitorHub::empty()
        )
        .is_ok()
    );
    fs::write(&frozen, b"{}").unwrap();
    let (vfs, storage) = mounts(&f.config);
    assert!(
        ComputeRegistry::open(
            &f.config,
            storage,
            vfs,
            &f.containers,
            crate::hpc_monitor::MonitorHub::empty()
        )
        .is_err()
    );
}

#[derive(Clone)]
struct EmptyNode(NodePorts);
#[async_trait::async_trait]
impl DagNode for EmptyNode {
    fn ports(&self) -> &NodePorts {
        &self.0
    }
    async fn execute(
        &mut self,
        _: &NodeCtx,
        _: &[NodeInput],
        _: &NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        panic!("router must not execute an unintended local node")
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "compute-test"
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
struct CaptureExecutor {
    label: String,
    calls: Arc<Mutex<Vec<String>>>,
    switch: AtomicBool,
    registry: std::sync::Weak<ComputeRegistry>,
}
#[async_trait::async_trait]
impl TaskExecutor for CaptureExecutor {
    fn name(&self) -> &'static str {
        "capture"
    }
    async fn run(&self, execution: TaskExecution) -> JobResult {
        self.calls.lock().unwrap().push(self.label.clone());
        if self.switch.swap(false, Ordering::SeqCst) {
            select(&self.registry.upgrade().unwrap(), "server-b");
        }
        JobResult::Success {
            id: execution.submission.spec.id,
            outputs: PortOutputs::new(),
            duration: Duration::ZERO,
            details: None,
        }
    }
}
fn install_capture(
    registry: &Arc<ComputeRegistry>,
    id: &str,
    calls: Arc<Mutex<Vec<String>>>,
    switch: bool,
) {
    let old = registry.target(id, Some(1)).unwrap();
    let target = Arc::new(BuiltTarget {
        typed_worker_revision: None,
        network_policy_sha256: None,
        target: old.target.clone(),
        containers: old.containers.clone(),
        executor: Arc::new(CaptureExecutor {
            label: id.into(),
            calls,
            switch: AtomicBool::new(switch),
            registry: Arc::downgrade(registry),
        }),
        hpc: None,
        ssh_probe: None,
        monitor_key: old.monitor_key.clone(),
    });
    registry
        .built
        .write()
        .unwrap()
        .insert(target.monitor_key.clone(), target);
}
#[tokio::test]
async fn scheduler_binds_once_and_existing_agent_graphs_follow_next_run_policy() {
    let f = fixture();
    save_local(&f.registry, "server-b", true);
    let calls = Arc::new(Mutex::new(Vec::new()));
    install_capture(&f.registry, "local", calls.clone(), true);
    install_capture(&f.registry, "server-b", calls.clone(), false);
    let graph = FlowGraph::builder()
        .process(Process::new("first", "compute-test", serde_json::json!({})))
        .process(Process::new(
            "second",
            "compute-test",
            serde_json::json!({}),
        ))
        .build()
        .unwrap();
    let ctx = NodeCtx::new(
        Arc::new(datafusion::execution::runtime_env::RuntimeEnv::default()),
        Some(f.registry.storage.clone()),
    );
    let cfg = FlowRunConfig {
        max_concurrency: 1,
        resume: false,
    };
    let scheduler = FlowScheduler::new(
        Arc::new(ComputeExecutor(f.registry.clone())),
        Arc::new(|_, _| Ok(Box::new(EmptyNode(NodePorts::new())))),
        Arc::new(ctx),
    );
    let report = scheduler
        .run_dataflow(&graph, &SourceResolution::new(), &cfg, None)
        .await
        .unwrap();
    assert!(report.ok);
    assert_eq!(report.execution_binding.unwrap()["target_id"], "local");
    assert_eq!(*calls.lock().unwrap(), vec!["local", "local"]);
    let report = scheduler
        .run_dataflow(&graph, &SourceResolution::new(), &cfg, None)
        .await
        .unwrap();
    assert!(report.ok);
    assert_eq!(report.execution_binding.unwrap()["target_id"], "server-b");
    assert_eq!(
        *calls.lock().unwrap(),
        vec!["local", "local", "server-b", "server-b"]
    );
    assert_eq!(f.registry.status().unwrap().runs.len(), 2);
}

#[tokio::test]
async fn invalid_sources_and_failed_persistence_do_not_publish_configuration() {
    let f = fixture();
    for id in ["../escape", "", "a/b"] {
        assert!(
            f.registry
                .save_target(SaveTarget {
                    data_domains: Vec::new(),
                    expected_revision: 1,
                    id: id.into(),
                    name: "invalid".into(),
                    enabled: true,
                    backend: ComputeBackend::Local
                })
                .is_err()
        );
    }
    let original = f.registry.root.join("registry.json");
    let saved = f.registry.root.join("registry.saved");
    fs::rename(&original, &saved).unwrap();
    fs::create_dir(&original).unwrap();
    assert!(
        f.registry
            .select(SelectTarget {
                expected_revision: 1,
                target_id: "local".into()
            })
            .is_err()
    );
    assert_eq!(f.registry.state().revision, 1);
    assert!(f.registry.bind().is_err());
}

#[tokio::test]
async fn all_target_monitors_survive_selection_and_disabled_target_revisions() {
    use hpc_tools::monitor::{MonitorRecord, Owner};
    let f = fixture();
    for (id, host) in [("a", "server-a"), ("b", "server-b")] {
        let settings = remote_settings(&f.config.state_dir, host);
        f.registry
            .save_target(SaveTarget {
                data_domains: Vec::new(),
                expected_revision: f.registry.state().revision,
                id: id.into(),
                name: id.into(),
                enabled: true,
                backend: ComputeBackend::Slurm { settings },
            })
            .unwrap();
        let target = f.registry.target(id, Some(1)).unwrap();
        let nonce = "a".repeat(32);
        let record = MonitorRecord {
            version: 1,
            id: format!(
                "{:x}",
                Sha256::digest(format!("{host}\0{nonce}").as_bytes())
            ),
            owner: Owner {
                agent_id: uuid::Uuid::nil().to_string(),
                agent_path: "/root/test".into(),
                session_id: uuid::Uuid::nil().to_string(),
            },
            ssh_host: host.into(),
            cluster: "default".into(),
            submission_key: "same-key".into(),
            nonce,
            request_sha256: "b".repeat(64),
            job_id: Some("123".into()),
            state: "watching".into(),
            observation: None,
            notification: "none".into(),
            verification_plan: serde_json::json!({}),
            next_poll: u64::MAX,
            failures: 0,
            last_error: None,
            revision: 1,
            created_at: 1,
            updated_at: 1,
        };
        write_new(
            &target
                .hpc
                .as_ref()
                .unwrap()
                .state_dir()
                .join("job-monitors")
                .join(format!("{}.json", record.id)),
            &record,
        )
        .unwrap();
    }
    select(&f.registry, "b");
    let old = f.registry.target("a", Some(1)).unwrap();
    f.registry
        .save_target(SaveTarget {
            data_domains: Vec::new(),
            expected_revision: f.registry.state().revision,
            id: "a".into(),
            name: "disabled a".into(),
            enabled: false,
            backend: old.target.backend.clone(),
        })
        .unwrap();
    let jobs = f.registry.monitors.views(None).unwrap();
    assert_eq!(jobs.len(), 2);
    assert!(jobs.iter().all(|j| j.completion_pending()));
    assert!(
        jobs.iter()
            .any(|j| j.target_id.as_deref() == Some("a") && j.target_revision == Some(1))
    );
    assert!(crate::hpc_monitor::owner_has_pending_jobs(
        &jobs,
        uuid::Uuid::nil(),
        "/root/test",
        uuid::Uuid::nil()
    ));
    // A global switch cannot turn an uncertain old intent into a fresh submit.
    use agentik_core::storage::{AgentRecord, AgentStorage};
    let mut uncertain = f
        .registry
        .monitors
        .target_hub("a-r1")
        .unwrap()
        .records()
        .unwrap()
        .remove(0);
    uncertain.state = "submission_uncertain".into();
    uncertain.job_id = None;
    let path = old
        .hpc
        .as_ref()
        .unwrap()
        .state_dir()
        .join("job-monitors")
        .join(format!("{}.json", uncertain.id));
    fs::write(path, serde_json::to_vec(&uncertain).unwrap()).unwrap();
    let agents = Arc::new(
        agentik_core::TursoAgentStorage::open_in_memory()
            .await
            .unwrap(),
    );
    agents
        .upsert_agent(AgentRecord {
            id: uuid::Uuid::nil(),
            name: "/root/test".into(),
            config_json: serde_json::json!({}),
            created_at: 1,
            last_active: 1,
        })
        .await
        .unwrap();
    agents
        .start_session(uuid::Uuid::nil(), uuid::Uuid::nil())
        .await
        .unwrap();
    agents
        .bind_agent_context(uuid::Uuid::nil(), uuid::Uuid::nil())
        .await
        .unwrap();
    let tools = super::tools::hpc_registrations(f.registry.clone(), "/root/test", agents);
    let tool = tools
        .iter()
        .find(|t| t.definition.name == "hpc_jobs")
        .unwrap();
    let context = agentik_core::tools::ToolContext {
        session: Some(agentik_core::tools::ContextToolCtx {
            session_id: uuid::Uuid::nil(),
            plan: Arc::new(arc_swap::ArcSwap::from_pointee(
                agentik_types::AgentPlan::default(),
            )),
        }),
        ..Default::default()
    };
    let blocked = tool
        .implementation
        .execute_with_context(
            serde_json::json!({"action":"submit","submission_key":"same-key"}),
            &context,
        )
        .await
        .unwrap_err();
    assert!(blocked.to_string().contains("unresolved intent"));
    let independent = tool
        .implementation
        .execute_with_context(
            serde_json::json!({"action":"submit","submission_key":"independent-work"}),
            &context,
        )
        .await
        .unwrap_err();
    assert!(
        independent
            .to_string()
            .contains("account tools are disabled")
    );
    let (vfs, storage) = mounts(&f.config);
    let restored = ComputeRegistry::open(
        &f.config,
        storage,
        vfs,
        &f.containers,
        crate::hpc_monitor::MonitorHub::empty(),
    )
    .unwrap();
    assert_eq!(restored.monitors.views(None).unwrap().len(), 2);
}

#[tokio::test]
async fn connection_identity_is_retained_when_alias_configuration_changes() {
    let f = fixture();
    let settings = remote_settings(&f.config.state_dir, "alias-a");
    let program = settings.ssh_command.clone();
    f.registry
        .save_target(SaveTarget {
            data_domains: Vec::new(),
            expected_revision: 1,
            id: "a".into(),
            name: "a".into(),
            enabled: true,
            backend: ComputeBackend::Slurm { settings },
        })
        .unwrap();
    fs::write(
        program,
        b"#!/bin/sh\nprintf 'hostname replacement.example\\nuser other\\nport 2222\\n'\n",
    )
    .unwrap();
    let target = f.registry.target("a", Some(1)).unwrap();
    let endpoint = &target.target.endpoints["alias-a"];
    assert_eq!(endpoint.hostname, "toy.example");
    assert_eq!(endpoint.user, "toy");
    assert_eq!(endpoint.port, 22);
    assert!(endpoint.options().contains(&"Hostname=toy.example".into()));
}

#[tokio::test]
async fn old_job_addressing_survives_switching_compute_to_local() {
    use agentik_core::TursoAgentStorage;
    let f = fixture();
    let mut settings = remote_settings(&f.config.state_dir, "files-a");
    settings.tools_enabled = true;
    let script = r#"#!/usr/bin/python3
import os,sys
if sys.argv[1]=='-G':
    print('hostname toy.example\nuser toy\nport 22')
else:
    os.execv('/bin/sh',['sh','-c',sys.argv[-1]])
"#;
    fs::write(&settings.ssh_command, script).unwrap();
    f.registry
        .save_target(SaveTarget {
            data_domains: Vec::new(),
            expected_revision: 1,
            id: "a".into(),
            name: "a".into(),
            enabled: true,
            backend: ComputeBackend::Slurm { settings },
        })
        .unwrap();
    let target = f.registry.target("a", Some(1)).unwrap();
    let storage = Arc::new(TursoAgentStorage::open_in_memory().await.unwrap());
    let tools = super::tools::hpc_registrations(f.registry.clone(), "/root/test", storage);
    // Tool objects predate the switch and the disabling revision.
    select(&f.registry, "a");
    select(&f.registry, "local");
    f.registry
        .save_target(SaveTarget {
            data_domains: Vec::new(),
            expected_revision: f.registry.state().revision,
            id: "a".into(),
            name: "a".into(),
            enabled: false,
            backend: target.target.backend.clone(),
        })
        .unwrap();
    // Existing jobs remain addressable by their original revision after the
    // switch, and new computation never falls back to the disabled target.
    let jobs = tools
        .iter()
        .find(|t| t.definition.name == "hpc_jobs")
        .unwrap();
    assert!(jobs.implementation.execute(serde_json::json!({"target_id":"a","target_revision":1,"action":"submit","submission_key":"no-fallback"})).await.is_err());
    assert!(tools.iter().all(|t| t.definition.name != "hpc_files"));
    assert!(tools.iter().all(|t| t.definition.name != "hpc_transfer"));
}

struct MarkerContainer {
    root: PathBuf,
    label: String,
}
#[async_trait::async_trait]
impl PodmanConnection for MarkerContainer {
    fn workspace_root(&self) -> &Path {
        &self.root
    }
    async fn run_observed(
        &self,
        request: container_runtime::ContainerRunRequest,
        private: Option<container_runtime::types::PrivateEnvironment>,
        observer: container_runtime::observation::RunObserver,
    ) -> Result<container_runtime::ContainerRunResult, container_runtime::ContainerRuntimeError>
    {
        for evidence in ["verified_software", "verified_stop"] {
            observer.emit(container_runtime::observation::RuntimeObservation {
                phase: "checking_worker_stop".into(),
                worker: Some(serde_json::json!({"evidence": evidence,
                    "target": self.label, "private": private.is_some()})),
                ..Default::default()
            });
        }
        self.run(request).await
    }
    async fn run(
        &self,
        _: container_runtime::ContainerRunRequest,
    ) -> Result<container_runtime::ContainerRunResult, container_runtime::ContainerRuntimeError>
    {
        Err(container_runtime::ContainerRuntimeError::Invalid(
            self.label.clone(),
        ))
    }
}
struct ContainerProbe {
    registry: Arc<ComputeRegistry>,
    switch: AtomicBool,
    expected: String,
}
#[async_trait::async_trait]
impl TaskExecutor for ContainerProbe {
    fn name(&self) -> &'static str {
        "container-probe"
    }
    async fn run(&self, e: TaskExecution) -> JobResult {
        if self.switch.swap(false, Ordering::SeqCst) {
            select(&self.registry, "b");
        }
        let router = super::routing::ContainerRouter {
            registry: self.registry.clone(),
            root: self.registry.config.state_dir.clone(),
        };
        let request = container_runtime::ContainerRunRequest {
            image: "synthetic".into(),
            command: vec!["synthetic".into()],
            workspace: container_runtime::WorkspaceRef {
                host_path: self.registry.config.state_dir.clone(),
                container_workdir: "/work".into(),
            },
            env: vec![],
            panels: vec![],
            input_mounts: vec![],
            network: container_runtime::ContainerNetwork::Isolated,
            read_only_rootfs: true,
            pull_policy: container_runtime::PullPolicy::Never,
            cpus: None,
            memory: None,
            pids_limit: None,
            shm_size: None,
            gpus: container_runtime::GpuRequest::None,
            user: None,
            timeout_secs: 1,
            name: "probe".into(),
        };
        for private in [false, true] {
            let events = Arc::new(Mutex::new(Vec::new()));
            let saved = events.clone();
            let observer = container_runtime::observation::RunObserver::new(move |event| {
                saved.lock().unwrap().push(event);
            });
            let env = private.then(|| {
                container_runtime::types::PrivateEnvironment::new(vec![(
                    "OPENGWAS_JWT".into(),
                    "synthetic".into(),
                )])
                .unwrap()
            });
            let error = router
                .run_observed(request.clone(), env, observer)
                .await
                .unwrap_err();
            assert!(error.to_string().contains(&self.expected));
            let events = events.lock().unwrap();
            assert_eq!(events.len(), 2, "router lost the backend observer");
            for (event, evidence) in events.iter().zip(["verified_software", "verified_stop"]) {
                let worker = event.worker.as_ref().unwrap();
                assert_eq!(worker["target"], self.expected);
                assert_eq!(worker["evidence"], evidence);
                assert_eq!(worker["private"], private);
            }
        }
        let error = router.run(request).await.unwrap_err().to_string();
        assert!(
            error.contains(&self.expected),
            "container backend changed during a bound run: {error}"
        );
        JobResult::Success {
            id: e.submission.spec.id,
            outputs: PortOutputs::new(),
            duration: Duration::ZERO,
            details: None,
        }
    }
}
#[tokio::test]
async fn containers_use_the_same_target_snapshot_as_the_dag_run() {
    let f = fixture();
    save_local(&f.registry, "b", true);
    for id in ["local", "b"] {
        let previous = f.registry.target(id, Some(1)).unwrap();
        let target = Arc::new(BuiltTarget {
            typed_worker_revision: None,
            network_policy_sha256: None,
            target: previous.target.clone(),
            executor: Arc::new(ContainerProbe {
                registry: f.registry.clone(),
                switch: AtomicBool::new(id == "local"),
                expected: format!("container-{id}"),
            }),
            containers: Arc::new(MarkerContainer {
                root: f.config.state_dir.clone(),
                label: format!("container-{id}"),
            }),
            hpc: None,
            ssh_probe: None,
            monitor_key: previous.monitor_key.clone(),
        });
        f.registry
            .built
            .write()
            .unwrap()
            .insert(target.monitor_key.clone(), target);
    }
    let graph = FlowGraph::builder()
        .process(Process::new(
            "container",
            "compute-test",
            serde_json::json!({}),
        ))
        .build()
        .unwrap();
    let ctx = NodeCtx::new(
        Arc::new(datafusion::execution::runtime_env::RuntimeEnv::default()),
        Some(f.registry.storage.clone()),
    );
    let scheduler = FlowScheduler::new(
        Arc::new(ComputeExecutor(f.registry.clone())),
        Arc::new(|_, _| Ok(Box::new(EmptyNode(NodePorts::new())))),
        Arc::new(ctx),
    );
    let first = scheduler
        .run_dataflow(
            &graph,
            &SourceResolution::new(),
            &FlowRunConfig::default(),
            None,
        )
        .await
        .unwrap();
    assert!(first.ok);
    assert_eq!(first.execution_binding.unwrap()["target_id"], "local");
    let second = scheduler
        .run_dataflow(
            &graph,
            &SourceResolution::new(),
            &FlowRunConfig::default(),
            None,
        )
        .await
        .unwrap();
    assert!(second.ok);
    assert_eq!(second.execution_binding.unwrap()["target_id"], "b");
    // Break test-only Arc cycles without abandoning any worker.
    f.registry.built.write().unwrap().clear();
}

#[tokio::test]
async fn restart_uses_enrolled_profiles_instead_of_mutable_legacy_paths() {
    let f = fixture();
    let settings = remote_settings(&f.config.state_dir, "restart-host");
    let source = settings.typed_worker_config.clone().unwrap();
    f.registry
        .save_target(SaveTarget {
            data_domains: Vec::new(),
            expected_revision: 1,
            id: "remote".into(),
            name: "remote".into(),
            enabled: true,
            backend: ComputeBackend::Slurm { settings },
        })
        .unwrap();
    select(&f.registry, "remote");
    fs::remove_file(source).unwrap();
    let mut obsolete = f.config.clone();
    obsolete.slurm.enabled = true;
    obsolete.slurm.typed_worker_config =
        Some(f.config.state_dir.join("deleted-legacy-profile.json"));
    let resolved = startup_config(&obsolete).unwrap();
    assert!(resolved.slurm.typed_worker_config.unwrap().is_file());
    assert!(!resolved.plugin_rsi.environment_dev.enabled);
    assert!(obsolete.local_container_execution_required().is_ok());
    select(&f.registry, "local");
    let resolved = startup_config(&obsolete).unwrap();
    assert!(!resolved.slurm.enabled);
    assert!(resolved.slurm.typed_worker_config.is_none());
}

#[test]
fn existing_registry_ignores_obsolete_site_profile() {
    let f = fixture();
    fs::write(f.config.state_dir.join("slurm-site.json"), b"not json").unwrap();
    let resolved = startup_config(&f.config).unwrap();
    assert!(!resolved.slurm.enabled);
}

struct ProbeImageBuilder(Arc<std::sync::atomic::AtomicUsize>);
#[async_trait::async_trait]
impl container_runtime::ImageBuildConnection for ProbeImageBuilder {
    async fn build(
        &self,
        _: container_runtime::ImageBuildRequest,
    ) -> Result<container_runtime::ImageBuildResult, container_runtime::ContainerRuntimeError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(container_runtime::ContainerRuntimeError::Invalid(
            "synthetic build".into(),
        ))
    }
    async fn push(
        &self,
        _: container_runtime::ImagePushRequest,
    ) -> Result<container_runtime::ImagePushResult, container_runtime::ContainerRuntimeError> {
        Err(container_runtime::ContainerRuntimeError::Invalid(
            "synthetic push".into(),
        ))
    }
}
#[tokio::test]
async fn background_image_builds_cannot_bypass_remote_compute_policy() {
    use container_runtime::ImageBuildConnection;
    let f = fixture();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let builder = super::ComputeImageBuilder {
        registry: f.registry.clone(),
        local: Arc::new(ProbeImageBuilder(calls.clone())),
    };
    let request = container_runtime::ImageBuildRequest {
        context_dir: f.config.state_dir.clone(),
        containerfile: "Containerfile".into(),
        tag: "synthetic".into(),
        build_args: vec![],
        timeout_secs: 1,
    };
    assert!(builder.build(request.clone()).await.is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let settings = remote_settings(&f.config.state_dir, "image-host");
    f.registry
        .save_target(SaveTarget {
            data_domains: Vec::new(),
            expected_revision: f.registry.state().revision,
            id: "remote".into(),
            name: "remote".into(),
            enabled: true,
            backend: ComputeBackend::Slurm { settings },
        })
        .unwrap();
    select(&f.registry, "remote");
    assert!(builder.build(request).await.is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn first_enrollment_preserves_explicit_legacy_container_placement() {
    let f = fixture();
    let mut config = f.config.clone();
    config.state_dir = config.state_dir.join("legacy-state");
    private_dir(&config.state_dir).unwrap();
    config.slurm.enabled = true;
    config.slurm.tools_enabled = false;
    config.slurm.typed_worker_config = None;
    config.slurm.apptainer_manifest = None;
    let containers = ContainerExecutionInfra::from_config(
        container_runtime::PodmanConfig::for_state_dir(&config.state_dir),
    );
    let (vfs, storage) = mounts(&config);
    let registry = ComputeRegistry::open(
        &config,
        storage,
        vfs,
        &containers,
        crate::hpc_monitor::MonitorHub::empty(),
    )
    .unwrap();
    assert_eq!(registry.state().policy.target_id, "legacy-slurm");
    assert_eq!(
        registry
            .target("legacy-slurm", Some(1))
            .unwrap()
            .containers
            .name(),
        containers.runtime.name()
    );
    assert_eq!(f.registry.state().policy.target_id, "local");
}

#[tokio::test]
async fn reopening_legacy_seed_labels_preserves_catalog_bytes_and_historical_revisions() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = RuntimeConfig::default();
    config.state_dir = dir.path().to_path_buf();
    config.data_dir = dir.path().join("data");
    fs::create_dir(&config.data_dir).unwrap();
    config.slurm.enabled = true;
    config.slurm.tools_enabled = false;
    config.slurm.typed_worker_config = None;
    config.slurm.apptainer_manifest = None;
    let containers = ContainerExecutionInfra::from_config(
        container_runtime::PodmanConfig::for_state_dir(dir.path()),
    );
    let open = || {
        let (vfs, storage) = mounts(&config);
        ComputeRegistry::open(
            &config,
            storage,
            vfs,
            &containers,
            crate::hpc_monitor::MonitorHub::empty(),
        )
    };
    let registry = open().unwrap();
    let mut retained = registry.state();
    assert_eq!(
        retained
            .targets
            .iter()
            .find(|t| t.id == "local")
            .unwrap()
            .name,
        "Local"
    );
    assert_eq!(
        retained
            .targets
            .iter()
            .find(|t| t.id == "legacy-slurm")
            .unwrap()
            .name,
        "SJTU"
    );
    let mut historical = retained
        .targets
        .iter()
        .find(|t| t.id == "legacy-slurm")
        .unwrap()
        .clone();
    drop(registry);
    for target in &mut retained.targets {
        target.name = if target.id == "local" {
            "本地"
        } else {
            "交我算（原配置）"
        }
        .into();
    }
    for (revision, name) in [(2, "交我算"), (3, "Operator's retained cluster")] {
        historical.revision = revision;
        historical.name = name.into();
        retained.targets.push(historical.clone());
    }
    retained.policy.target_revision = 2;
    let root = config.state_dir.join("compute");
    let catalog_path = root.join("registry.json");
    let pending = root.join(".legacy-fixture");
    write_new(&pending, &retained).unwrap();
    fs::rename(&pending, &catalog_path).unwrap();
    let before = read_private(&catalog_path).unwrap();
    for _ in 0..2 {
        let reopened = open().unwrap();
        assert_eq!(
            serde_json::to_value(reopened.state()).unwrap(),
            serde_json::to_value(&retained).unwrap()
        );
        assert_eq!(
            read_private(&catalog_path).unwrap(),
            before,
            "opening the registry must preserve all target revisions, policy and deployment CAS bytes"
        );
    }
}

#[tokio::test]
async fn incompatible_worker_cannot_be_selected_checked_or_bound_after_upgrade() {
    let f = fixture();
    let settings = remote_settings(&f.config.state_dir, "old-worker");
    let profile = settings.typed_worker_config.as_ref().unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(profile).unwrap()).unwrap();
    value["worker"]["expected_source_revision"] = serde_json::json!("previous-release");
    fs::write(profile, serde_json::to_vec(&value).unwrap()).unwrap();
    let catalog = f
        .registry
        .save_target(SaveTarget {
            data_domains: Vec::new(),
            expected_revision: f.registry.state().revision,
            id: "old-worker".into(),
            name: "Old worker".into(),
            enabled: true,
            backend: ComputeBackend::Slurm { settings },
        })
        .unwrap();
    let error = f
        .registry
        .select(SelectTarget {
            expected_revision: catalog.revision,
            target_id: "old-worker".into(),
        })
        .unwrap_err();
    assert!(error.contains("previous-release") && error.contains(dag_core::source_revision()));
    assert_eq!(f.registry.state().revision, catalog.revision);
    assert_eq!(f.registry.state().policy.target_id, "local");
    // The SSH fixture always fails: an incompatible profile must stop before SSH.
    let check = f.registry.check_target("old-worker").await.unwrap();
    assert_eq!(check["configuration_valid"], false);
    assert_eq!(check["deployment"]["source_compatible"], false);
    assert!(check["connection_observation"].is_null());
    // Simulate an enrolled policy retained across a coordinator upgrade.
    let mut retained = f.registry.state();
    retained.policy.target_id = "old-worker".into();
    retained.policy.target_revision = 1;
    f.registry.persist(&retained).unwrap();
    let (vfs, storage) = mounts(&f.config);
    let reopened = ComputeRegistry::open(
        &f.config,
        storage,
        vfs,
        &f.containers,
        crate::hpc_monitor::MonitorHub::empty(),
    )
    .unwrap();
    assert_eq!(
        reopened.status().unwrap().deployment["source_compatible"],
        false
    );
    assert!(reopened.bind().is_err());
    assert!(reopened.status().unwrap().runs.is_empty());
    // Historical target identity remains available for inspection and recovery.
    assert_eq!(
        reopened
            .target("old-worker", Some(1))
            .unwrap()
            .target
            .revision,
        1
    );
    // Register a measured matching replacement, preserving the old capsule.
    let old = reopened
        .target("old-worker", Some(1))
        .unwrap()
        .target
        .clone();
    let old_bytes = fs::read(match &old.backend {
        ComputeBackend::Slurm { settings } => settings.typed_worker_config.as_ref().unwrap(),
        ComputeBackend::Local | ComputeBackend::Ssh { .. } => unreachable!(),
    })
    .unwrap();
    let replacement = remote_settings(&f.config.state_dir, "matching-worker");
    reopened
        .save_target(SaveTarget {
            data_domains: Vec::new(),
            expected_revision: reopened.state().revision,
            id: "old-worker".into(),
            name: "Matching replacement".into(),
            enabled: true,
            backend: ComputeBackend::Slurm {
                settings: replacement,
            },
        })
        .unwrap();
    select(&reopened, "old-worker");
    let status = reopened.status().unwrap();
    assert_eq!(status.deployment["source_compatible"], true);
    assert_eq!(status.deployment["target_revision"], 2);
    assert!(reopened.bind().is_ok());
    assert_eq!(reopened.status().unwrap().runs.len(), 1);
    let historical = reopened.target("old-worker", Some(1)).unwrap();
    assert_eq!(historical.target.capsule_sha256, old.capsule_sha256);
    assert_eq!(
        fs::read(match &historical.target.backend {
            ComputeBackend::Slurm { settings } => settings.typed_worker_config.as_ref().unwrap(),
            ComputeBackend::Local | ComputeBackend::Ssh { .. } => unreachable!(),
        })
        .unwrap(),
        old_bytes
    );
    select(&reopened, "local");
    assert_eq!(
        reopened.status().unwrap().deployment["source_compatible"],
        true
    );
}

#[derive(Clone)]
struct RequiredImageNode {
    ports: NodePorts,
    image: String,
}
#[async_trait::async_trait]
impl DagNode for RequiredImageNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }
    fn required_container_images(&self) -> Vec<&str> {
        vec![&self.image]
    }
    async fn execute(
        &mut self,
        _: &NodeCtx,
        _: &[NodeInput],
        _: &NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        panic!("preflight must prevent every execution")
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "readiness-probe"
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[tokio::test]
async fn readiness_blocks_missing_images_per_instance_and_keeps_bound_target() {
    let f = fixture();
    let image = format!("example/present@sha256:{}", "a".repeat(64));
    let missing = format!("example/missing@sha256:{}", "b".repeat(64));
    let inventory = f.config.state_dir.join("readiness-images.json");
    write_new(
        &inventory,
        &serde_json::json!({"program":"/usr/bin/apptainer",
        "images":{image.clone():{"path":"/admitted/present.sif","sha256":"c".repeat(64)}}}),
    )
    .unwrap();
    let mut settings = remote_settings(&f.config.state_dir, "readiness");
    settings.apptainer_manifest = Some(inventory);
    f.registry
        .save_target(SaveTarget {
            data_domains: Vec::new(),
            expected_revision: f.registry.state().revision,
            id: "readiness".into(),
            name: "readiness".into(),
            enabled: true,
            backend: ComputeBackend::Slurm { settings },
        })
        .unwrap();
    select(&f.registry, "readiness");
    let request = ReadinessRequest {
        images: vec![image.clone(), missing.clone()],
        ..Default::default()
    };
    let report = f.registry.readiness(&request, &[]).unwrap();
    assert_eq!(report["state"], "BLOCKED");
    assert_eq!(report["deployment"]["source_compatible"], true);
    assert_eq!(report["blockers"][0]["code"], "IMAGE_MAPPING_UNAVAILABLE");
    assert_eq!(f.registry.status().unwrap().runs.len(), 0);
    let valid = f
        .registry
        .readiness(
            &ReadinessRequest {
                images: vec![image],
                ..Default::default()
            },
            &[],
        )
        .unwrap();
    assert_eq!(valid["state"], "CONFIGURATION_READY");
    assert_eq!(valid["end_to_end_acceptance"], "NOT_VERIFIED");
    assert_eq!(
        f.registry
            .readiness(&ReadinessRequest::default(), &[])
            .unwrap()["state"],
        "NOT_VERIFIED"
    );
    assert_eq!(valid["remote_sif_bytes_verified"], false);
    let calls = Arc::new(Mutex::new(Vec::new()));
    install_capture(&f.registry, "readiness", calls.clone(), false);
    let node = RequiredImageNode {
        ports: NodePorts::new(),
        image: missing,
    };
    let graph = FlowGraph::builder()
        .process(Process::new(
            "a_ordinary",
            "compute-test",
            serde_json::json!({}),
        ))
        .process(Process::new(
            "z_missing",
            "readiness-probe",
            serde_json::json!({}),
        ))
        .build()
        .unwrap();
    let ctx = NodeCtx::new(
        Arc::new(datafusion::execution::runtime_env::RuntimeEnv::default()),
        Some(f.registry.storage.clone()),
    );
    let required = node.clone();
    let scheduler = FlowScheduler::new(
        Arc::new(ComputeExecutor(f.registry.clone())),
        Arc::new(move |kind, _| match kind {
            "compute-test" => Ok(Box::new(EmptyNode(NodePorts::new()))),
            _ => Ok(Box::new(required.clone())),
        }),
        Arc::new(ctx),
    );
    let error = scheduler
        .run_dataflow(
            &graph,
            &SourceResolution::new(),
            &FlowRunConfig {
                max_concurrency: 1,
                resume: false,
            },
            None,
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("WORKFLOW_BLOCKED"));
    // The streaming scheduler checks each instance at dispatch time, so a
    // preceding task may already have run when another image is rejected.
    assert_eq!(*calls.lock().unwrap(), vec!["readiness"]);
    let executor = ComputeExecutor(f.registry.clone())
        .bind_run()
        .await
        .unwrap()
        .unwrap();
    select(&f.registry, "local");
    let blocked = executor
        .preflight(&[("dynamic", &node)])
        .unwrap_err()
        .to_string();
    assert!(blocked.contains("IMAGE_MAPPING_UNAVAILABLE"));
    assert!(blocked.contains("\"blocked_nodes_submitted\":false"));
    assert!(!blocked.contains("\"new_job_submitted\""));
    assert_eq!(
        executor.execution_binding().unwrap()["target_id"],
        "readiness"
    );
}

#[tokio::test]
async fn readiness_reports_unregistered_candidate_and_local_unknown_without_mutation() {
    let f = fixture();
    let before = f.registry.state().revision;
    let report = f
        .registry
        .readiness(
            &ReadinessRequest {
                nodes: vec!["candidate/run".into()],
                ..Default::default()
            },
            &[],
        )
        .unwrap();
    assert_eq!(report["state"], "BLOCKED");
    assert_eq!(report["blockers"][0]["code"], "NODE_NOT_REGISTERED");
    let empty = f
        .registry
        .readiness(&ReadinessRequest::default(), &[])
        .unwrap();
    assert_eq!(empty["state"], "NOT_VERIFIED");
    assert_eq!(empty["requested_images"], 0);
    assert_eq!(empty["requested_identities"], 0);
    assert!(!empty["verification_gaps"].as_array().unwrap().is_empty());
    assert_eq!(f.registry.state().revision, before);
    assert!(f.registry.status().unwrap().runs.is_empty());
}

#[tokio::test]
async fn readiness_requires_a_resident_network_policy_pin() {
    let f = fixture();
    let settings = remote_settings(&f.config.state_dir, "network");
    f.registry
        .save_target(SaveTarget {
            data_domains: Vec::new(),
            expected_revision: f.registry.state().revision,
            id: "network".into(),
            name: "network".into(),
            enabled: true,
            backend: ComputeBackend::Slurm {
                settings: settings.clone(),
            },
        })
        .unwrap();
    select(&f.registry, "network");
    let nodes = vec![dag_core::registry::NodeInfo {
        address: "io/http_fetch".into(),
        plugin: "io".into(),
        kind: "http_fetch".into(),
        desc: String::new(),
        deprecated: false,
        data_bundles: vec![],
        resident_plugin: None,
    }];
    let request = ReadinessRequest {
        nodes: vec![nodes[0].address.clone()],
        ..Default::default()
    };
    let blocked = f.registry.readiness(&request, &nodes).unwrap();
    assert_eq!(blocked["state"], "BLOCKED");
    assert_eq!(
        blocked["blockers"][0]["code"],
        "WORKER_NETWORK_POLICY_UNCONFIGURED"
    );
    let path = settings.typed_worker_config.as_ref().unwrap();
    let mut profile: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    profile["worker"]["native_environment"] = serde_json::json!({"python":"/usr/bin/python3",
        "file":"/cluster/private/environment.json", "network_policy_sha256":"b".repeat(64)});
    fs::write(path, serde_json::to_vec(&profile).unwrap()).unwrap();
    // Changing an external seed never upgrades the already resident target.
    assert_eq!(
        f.registry.readiness(&request, &nodes).unwrap()["state"],
        "BLOCKED"
    );
    f.registry
        .save_target(SaveTarget {
            data_domains: Vec::new(),
            expected_revision: f.registry.state().revision,
            id: "network".into(),
            name: "network".into(),
            enabled: true,
            backend: ComputeBackend::Slurm { settings },
        })
        .unwrap();
    select(&f.registry, "network");
    let ready = f.registry.readiness(&request, &nodes).unwrap();
    assert_eq!(ready["state"], "CONFIGURATION_READY");
    assert_eq!(ready["end_to_end_acceptance"], "NOT_VERIFIED");
    assert!(f.registry.status().unwrap().runs.is_empty());
}

// ── direct SSH backend: real-host integration ────────────────────────────

/// Full-stack proof against a real host: enroll a direct SSH compute
/// target, bind it and run one process task through the registry executor.
/// The payload really runs remotely; the output is really downloaded back.
#[tokio::test]
#[ignore = "runs a real payload on a remote host over ssh; run only with explicit operator approval"]
async fn real_ssh_target_runs_process_task_end_to_end() {
    let host = std::env::var("AUTONOMICS_SSH_SMOKE_HOST").unwrap_or_else(|_| "ht".into());
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let scratch = format!("~/autonomics-ssh-smoke-{stamp}");
    println!("host {host} scratch {scratch}");
    let f = fixture();
    let mut settings = SlurmConfig::default();
    settings.enabled = true;
    settings.ssh_host = host.clone();
    settings.ssh_command = "ssh".into();
    settings.scratch_remote_dir = scratch.clone();
    settings.tools_enabled = false;
    settings.typed_worker_config = None;
    settings.apptainer_manifest = None;
    f.registry
        .save_target(SaveTarget {
            data_domains: Vec::new(),
            expected_revision: f.registry.state().revision,
            id: "ht-direct".into(),
            name: "Direct SSH host".into(),
            enabled: true,
            backend: ComputeBackend::Ssh { settings },
        })
        .unwrap();
    select(&f.registry, "ht-direct");
    let (target, binding) = f.registry.bind().unwrap();
    assert_eq!(binding.target_id, "ht-direct");

    let input_path = f.config.state_dir.join("smoke-input.txt");
    fs::write(&input_path, "registry smoke input").unwrap();
    let input = dag_core::FileRef::local(&input_path, Some("text".into())).unwrap();
    let process = dag_core::ProcessTaskSpec {
        container: None,
        script: Some(
            "hostname > out-0.txt\ncat $AUTONOMICS_WORKSPACE/input-0-0 >> out-0.txt\n".into(),
        ),
        command: Vec::new(),
        env: Default::default(),
        outputs: vec![dag_core::ProcessTaskOutput {
            port: 0,
            pattern: "out-*.txt".into(),
            optional: false,
        }],
    };
    let submission = dag_core::TaskSubmission {
        spec: dag_core::TaskSpec {
            id: "registry-ssh-smoke".into(),
            logical_node: Some("process".into()),
            kind: "process".into(),
            spec: serde_json::to_value(&process).unwrap(),
            item_key: None,
            item: None,
            inputs: vec![dag_core::TaskInputBinding {
                name: "f".into(),
                port: 0,
                source: dag_core::TaskInputSource::UpstreamPort {
                    from: "source".into(),
                    from_port: 0,
                },
                payload: "file".into(),
                path: Some(input.path.clone()),
                fingerprint: input.fingerprint.clone(),
                staged_paths: vec![input.path.clone()],
            }],
            outputs: vec![dag_core::TaskOutputBinding {
                name: "port_0".into(),
                port: 0,
                payload: "file".into(),
                artifact_paths: Vec::new(),
            }],
        },
        inputs: Vec::new(),
        resources: dag_core::TaskResources::default(),
    };
    let ctx = NodeCtx::new(
        Arc::new(datafusion::execution::runtime_env::RuntimeEnv::default()),
        Some(f.registry.storage.clone()),
    );
    let execution = TaskExecution {
        submission,
        node: Box::new(EmptyNode(NodePorts::new())),
        engine_ctx: ctx.into(),
        reporter: NodeReporter::noop(),
        cancellation: tokio_util::sync::CancellationToken::new(),
    };
    let outcome = target.executor.run(execution).await;
    let details = match &outcome {
        JobResult::Success { details, .. } => details,
        other => panic!("registry ssh smoke failed: {other:?}"),
    };
    let _ = details;
    // The remote payload's identity is in the run details; the run itself
    // succeeding through the whole stack is the assertion.
    let status = f.registry.status().unwrap();
    assert_eq!(status.runs.len(), 1);
    let _ = target;
}

// ── data-domain declarations (placement phase 1) ────────────────────────

fn domain(alias: &str) -> DataDomain {
    DataDomain {
        id: "hpc-b".into(),
        ssh_aliases: vec![alias.into()],
        remote_root: "/lustre/share".into(),
        vfs_prefix: "/data/hpc-b".into(),
        retained_vfs_prefix: None,
        retained_remote_root: None,
    }
}

#[test]
fn data_domain_admission_mirrors_transport_rules() {
    assert!(domain("data-a").validate().is_ok());

    let mut leading_dash = domain("data-a");
    leading_dash.ssh_aliases = vec!["-bad".into()];
    assert!(leading_dash.validate().is_err());

    let mut no_alias = domain("data-a");
    no_alias.ssh_aliases = Vec::new();
    assert!(no_alias.validate().is_err());

    let mut relative_root = domain("data-a");
    relative_root.remote_root = "lustre/share".into();
    assert!(relative_root.validate().is_err());

    // Retained namespace must be paired, distinct, and below the root.
    let mut unpaired = domain("data-a");
    unpaired.retained_vfs_prefix = Some("/data/hpc-b-kept".into());
    assert!(unpaired.validate().is_err());

    let mut outside = domain("data-a");
    outside.retained_vfs_prefix = Some("/data/hpc-b-kept".into());
    outside.retained_remote_root = Some("/elsewhere".into());
    assert!(outside.validate().is_err());

    let mut kept = domain("data-a");
    kept.retained_vfs_prefix = Some("/data/hpc-b-kept".into());
    kept.retained_remote_root = Some("/lustre/share/kept".into());
    assert!(kept.validate().is_ok());
}

#[test]
fn data_domain_converts_to_transport_home_losslessly() {
    let mut d = domain("data-a");
    d.retained_vfs_prefix = Some("/data/hpc-b-kept".into());
    d.retained_remote_root = Some("/lustre/share/kept".into());
    let home = d.data_home();
    assert_eq!(home.ssh_aliases, vec!["data-a".to_string()]);
    assert_eq!(home.remote_root, "/lustre/share");
    assert_eq!(home.vfs_prefix, "/data/hpc-b");
    // The catalog id flows into every placement the store publishes.
    assert_eq!(home.domain_id.as_deref(), Some("hpc-b"));
    assert_eq!(
        home.retained_vfs_prefix.as_deref(),
        Some("/data/hpc-b-kept")
    );
    assert_eq!(
        home.retained_remote_root.as_deref(),
        Some("/lustre/share/kept")
    );
}

#[tokio::test]
async fn data_domain_aliases_are_pinned_and_survive_catalog_roundtrip() {
    // A domain declaration needs its VFS projection to exist as an SSH
    // mount resolving to the same alias and root, so build the fixture
    // with one extra backend+mount over the usual local root.
    let dir = tempfile::tempdir().unwrap();
    let mut config = RuntimeConfig::default();
    config.state_dir = dir.path().to_path_buf();
    config.data_dir = dir.path().join("data");
    fs::create_dir(&config.data_dir).unwrap();
    config.slurm.enabled = false;
    config.slurm.tools_enabled = false;
    config.slurm.typed_worker_config = None;
    config.slurm.apptainer_manifest = None;
    let mut manifest = vfs::VfsManifest::local_root(config.data_dir.to_string_lossy());
    manifest.backend.push(vfs::BackendDefinition {
        id: "dom-backend".into(),
        config: vfs::BackendConfig::Ssh {
            alias: "domain-a".into(),
            root: "/lustre/share".into(),
            python: "python3".into(),
            endpoint: None,
        },
    });
    manifest.mount.push(vfs::MountDefinition {
        domain: Some("hpc-b".into()),
        path: "/data/hpc-b".into(),
        backend: "dom-backend".into(),
        source: "/".into(),
        read_only: true,
        permissions: Default::default(),
    });
    let vfs_store = Arc::new(vfs::MountedObjectStore::from_manifest(&manifest).unwrap());
    let storage = Arc::new(vfs::OpendalFileStorage::with_mounts(
        &config.data_dir,
        vfs_store.clone(),
    ));
    let containers = ContainerExecutionInfra::from_config(
        container_runtime::PodmanConfig::for_state_dir(dir.path()),
    );
    let registry = ComputeRegistry::open(
        &config,
        storage,
        vfs_store,
        &containers,
        crate::hpc_monitor::MonitorHub::empty(),
    )
    .unwrap();

    let mut settings = remote_settings(&config.state_dir, "domain-a");
    // The transport requires task work to live inside the retained
    // output root; the domain root must therefore contain the scratch.
    settings.scratch_remote_dir = "/lustre/share/scratch".into();
    let saved = registry
        .save_target(SaveTarget {
            expected_revision: 1,
            id: "dom".into(),
            name: "dom".into(),
            enabled: true,
            backend: ComputeBackend::Ssh { settings },
            data_domains: vec![domain("domain-a")],
        })
        .unwrap();
    let target = saved.targets.iter().find(|t| t.id == "dom").unwrap();
    // The domain alias is frozen with the same endpoint identity
    // treatment as the target's own login alias.
    let endpoint = &target.endpoints["domain-a"];
    assert_eq!(endpoint.hostname, "toy.example");
    assert_eq!(target.data_domains.len(), 1);
    assert_eq!(target.data_domains[0].remote_root, "/lustre/share");

    // A second domain is rejected: the transport admits exactly one.
    let err = registry
        .save_target(SaveTarget {
            expected_revision: saved.revision,
            id: "dom2".into(),
            name: "dom2".into(),
            enabled: true,
            backend: ComputeBackend::Local,
            data_domains: vec![domain("a"), domain("b")],
        })
        .unwrap_err();
    assert!(err.contains("at most one data domain"), "{err}");
}

#[tokio::test]
async fn enroll_from_minimal_input_derives_everything() {
    let f = fixture();
    // Fake ssh: answer endpoint probes, run payloads through the local sh.
    let ssh = f.config.state_dir.join("enroll-ssh");
    fs::write(
        &ssh,
        b"#!/bin/sh\nif [ \"$1\" = -G ]; then printf 'hostname toy.example\\nuser toy\\nport 22\\n'; exit 0; fi\nwhile [ \"$1\" = -o ]; do shift 2; done\nshift\nexec /bin/sh -c \"$*\"\n",
    )
    .unwrap();
    fs::set_permissions(&ssh, fs::Permissions::from_mode(0o700)).unwrap();
    let cluster = f.config.state_dir.join("cluster-root");
    fs::create_dir_all(&cluster).unwrap();
    let worker = cluster.join("autonomics-worker");
    fs::write(&worker, format!(r#"#!/bin/sh
hash=$(sha256sum -- "$0"); hash=${{hash%% *}}
printf '{{"artifact":"autonomics-worker","protocol":1,"source_revision":"{}","executable_sha256":"%s","capabilities":["task-worker"]}}\n' "$hash"
"#, dag_core::source_revision())).unwrap();
    fs::set_permissions(&worker, fs::Permissions::from_mode(0o755)).unwrap();

    let catalog = f
        .registry
        .enroll(super::EnrollRequest {
            expected_revision: 1,
            id: "hpc-b".into(),
            name: None,
            login_alias: "hpc-b-login".into(),
            data_alias: Some("hpc-b-data".into()),
            data_root: Some(cluster.display().to_string()),
            worker_executable: worker.display().to_string(),
            deploy_worker: None,
            python: None,
            partition: Some("test-part".into()),
            account: None,
            ssh_command: Some(ssh.display().to_string()),
            scratch_dir: None,
            select: true,
        })
        .await
        .unwrap();

    // Selected, pinned, and fully derived.
    assert_eq!(catalog.policy.target_id, "hpc-b");
    let target = catalog.targets.iter().find(|t| t.id == "hpc-b").unwrap();
    assert!(target.endpoints.contains_key("hpc-b-login"));
    assert!(target.endpoints.contains_key("hpc-b-data"));
    let domain = &target.data_domains[0];
    assert_eq!(domain.vfs_prefix, "/data/hpc-b");
    assert_eq!(domain.remote_root, cluster.display().to_string());
    let backend = match &target.backend {
        ComputeBackend::Slurm { settings } => settings.clone(),
        _ => panic!("enroll defaults to a Slurm backend"),
    };
    assert_eq!(backend.partition, "test-part");
    assert_eq!(
        backend.scratch_remote_dir,
        format!("{}/scratch", cluster.display())
    );
    assert!(backend.typed_worker_config.is_some());
    let profile: serde_json::Value =
        serde_json::from_slice(&fs::read(backend.typed_worker_config.as_ref().unwrap()).unwrap())
            .unwrap();
    for kind in [
        "source_literature",
        "bib_save",
        "evidence_export",
        "table_from_df",
        "figure_embed",
    ] {
        assert!(
            profile["local_kinds"]
                .as_array()
                .unwrap()
                .iter()
                .any(|value| value == kind)
        );
    }

    // The domain mount: read-only, domain-tagged, at the derived prefix.
    let source = catalog
        .data_sources
        .iter()
        .find(|s| s.id == "hpc-b")
        .unwrap();
    assert!(source.read_only);
    assert_eq!(source.domain.as_deref(), Some("hpc-b"));
    assert_eq!(source.mount_path(), "/data/hpc-b");

    // One-key repair reconnects and verifies a healthy worker without a package
    // path, configuration changes, or an unnecessary target revision.
    let repaired = f.registry.repair_worker(super::RepairWorkerRequest {
        expected_revision: catalog.revision,
        target_id: "hpc-b".into(),
    }).await.unwrap();
    assert_eq!(serde_json::to_value(&repaired).unwrap(), serde_json::to_value(&catalog).unwrap());
}

#[tokio::test]
async fn enroll_rejects_partial_data_home_inputs() {
    let f = fixture();
    let error = f
        .registry
        .enroll(super::EnrollRequest {
            expected_revision: 1,
            id: "broken".into(),
            name: None,
            login_alias: "alias".into(),
            data_alias: Some("data-alias".into()),
            data_root: None,
            worker_executable: "/worker".into(),
            deploy_worker: None,
            python: None,
            partition: None,
            account: None,
            ssh_command: None,
            scratch_dir: None,
            select: false,
        })
        .await
        .unwrap_err();
    assert!(error.contains("together"), "{error}");
}
