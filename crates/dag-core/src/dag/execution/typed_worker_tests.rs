use super::*;
use crate::dag::PortOutputs;
use crate::registry::error::Result as RegistryResult;
use crate::{
    NodeCtx, NodeFactory, NodePorts, PortType, TaskInputBinding, TaskInputSource,
    TaskOutputBinding, TaskResources, TaskSubmission,
};
use datafusion::prelude::SessionContext;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio_util::sync::CancellationToken;

fn settings() -> TypedWorkerSettings {
    TypedWorkerSettings {
        executable: "/cluster/autonomics".into(),
        executable_sha256: "a".repeat(64),
        expected_source_revision: crate::source_revision().into(),
        plugins_root: None,
        plugins_sha256: None,
        apptainer_catalog: None,
        apptainer_executable: None,
        podman: None,
        worker_vfs_manifest: None,
        worker_vfs_sha256: None,
        native_environment: None,
    }
}

#[derive(Clone)]
struct EchoNode {
    ports: NodePorts,
    omit_output: bool,
    path: Option<String>,
    container_dispatch: bool,
    host_service: bool,
}

#[async_trait::async_trait]
impl crate::DagNode for EchoNode {
    fn remote_execution_blocker(&self) -> Option<&'static str> {
        self.host_service.then_some("test coordinator service")
    }
    fn ports(&self) -> &NodePorts {
        &self.ports
    }
    fn delegates_to_container_runtime(&self) -> bool {
        self.container_dispatch
    }
    async fn execute(
        &mut self,
        _: &NodeCtx,
        inputs: &[NodeInput],
        _: &NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let mut outputs = PortOutputs::new();
        if self.omit_output {
            return Ok(outputs);
        }
        if let Some(path) = &self.path {
            assert!(inputs.is_empty(), "spec file must not become a node port");
            outputs.insert_file(
                0,
                FileRef::local(
                    path.strip_prefix("file://").unwrap_or(path),
                    Some("gz".into()),
                )?,
            );
        } else {
            outputs.insert(0, inputs[0].data.clone());
        }
        Ok(outputs)
    }
    fn file_input_spec_pointers(&self, _: &[NodeInput]) -> Vec<&'static str> {
        if self.path.is_some() {
            vec!["/path"]
        } else {
            vec![]
        }
    }
    fn clone_box(&self) -> Box<dyn crate::DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "worker_echo"
    }
    fn task_resources(&self) -> Result<TaskResources, DagError> {
        Ok(TaskResources {
            cpus: Some(2),
            memory_bytes: Some(128 * 1024 * 1024),
            max_duration_ms: Some(60_000),
        })
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

struct EchoFactory;
impl NodeFactory for EchoFactory {
    fn kind(&self) -> &'static str {
        "worker_echo"
    }
    fn desc(&self) -> &'static str {
        "test echo"
    }
    fn doc(&self) -> &'static str {
        "test echo"
    }
    fn spec_schema(&self) -> schemars::Schema {
        schemars::schema_for!(serde_json::Value)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _: NodeCtx,
    ) -> RegistryResult<Box<dyn crate::DagNode>> {
        let ty = match spec["type"].as_str().unwrap() {
            "dataframe" => PortType::DataFrame,
            "channel" => PortType::Channel,
            "file" => PortType::File,
            "file_set" => PortType::FileSet,
            _ => panic!("invalid test type"),
        };
        Ok(Box::new(EchoNode {
            omit_output: spec["omit_output"].as_bool().unwrap_or(false),
            container_dispatch: false,
            host_service: spec["host_service"].as_bool().unwrap_or(false),
            path: spec["path"].as_str().map(str::to_owned),
            ports: if spec["optional_output"].as_bool().unwrap_or(false) {
                NodePorts::new()
                    .add_input_port_of_type(None, ty.clone())
                    .add_output_port_of_type(None, ty)
                    .with_optional_output(0)
            } else {
                NodePorts::new()
                    .add_input_port_of_type(None, ty.clone())
                    .add_output_port_of_type(None, ty)
            },
        }))
    }
}

#[tokio::test]
async fn spec_file_is_staged_from_coordinator_vfs_and_rebound_without_a_node_port() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("方案.tar.gz");
    std::fs::write(&source, b"coordinator-only bytes").unwrap();
    let mut invocation = execution();
    invocation.submission.spec = task("file");
    invocation.submission.spec.inputs.clear();
    invocation.submission.spec.spec["path"] = serde_json::json!("vfs:///方案.tar.gz");
    invocation.submission.spec.spec["destination"] = serde_json::json!("unchanged-output");
    invocation.submission.inputs.clear();
    invocation.node = registry()
        .build_node(
            &invocation.submission.spec.kind,
            invocation.submission.spec.spec.clone(),
        )
        .unwrap();
    let storage = Arc::new(vfs::OpendalFileStorage::new(temp.path()));
    invocation.engine_ctx = Arc::new(NodeCtx::new(
        SessionContext::new().runtime_env(),
        Some(storage.clone()),
    ));
    let executor = TypedRemoteTaskExecutor::new(
        Arc::new(CaptureExecutor {
            calls: Arc::new(AtomicUsize::new(0)),
            capture: false,
        }),
        settings(),
    )
    .unwrap();
    executor.prepare(&mut invocation).unwrap();
    let envelope: TypedWorkerEnvelope =
        serde_json::from_value(invocation.submission.spec.spec["typed_dispatch"].clone()).unwrap();
    assert!(envelope.task.inputs.is_empty());
    assert_eq!(envelope.task.spec["path"], "vfs:///方案.tar.gz");
    let name = &envelope.spec_files["/path"];
    assert_eq!(invocation.submission.spec.inputs.len(), 1);
    let mut coordinator = super::super::workspace::LocalTaskWorkspace::create(
        &temp.path().join("coordinator"),
        &invocation.submission,
    )
    .unwrap();
    coordinator
        .stage_inputs_with_remote_observed(&mut invocation.submission, Some(&storage), None, None)
        .await
        .unwrap();
    let staged = &invocation.submission.spec.inputs[0].staged_paths[0];
    assert!(staged.ends_with(".tar.gz"));
    let worker = temp.path().join("worker");
    std::fs::create_dir_all(worker.join("typed-inputs")).unwrap();
    let snapshot = worker.join("typed-inputs/input.tar.gz");
    std::fs::copy(staged, &snapshot).unwrap();
    std::fs::remove_file(&source).unwrap();
    execute_typed_worker(
        &envelope,
        &registry(),
        [(name.clone(), vec![snapshot])].into(),
        &worker,
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read(worker.join("typed-outputs/port-0/00000000.gz")).unwrap(),
        b"coordinator-only bytes"
    );

    let mut forged_directory = envelope.clone();
    forged_directory
        .output_directories
        .insert("/destination".into(), 0);
    forged_directory.task.spec["destination"] = serde_json::json!("vfs:///work/forged");
    let error = execute_typed_worker(
        &forged_directory,
        &registry(),
        [(name.clone(), vec![worker.join("typed-inputs/input.tar.gz")])].into(),
        &worker,
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("output contract"));

    let mut forged = envelope.clone();
    forged
        .spec_files
        .insert("/destination".into(), "forged".into());
    let fake = worker.join("typed-inputs/fake.gz");
    std::fs::write(&fake, b"x").unwrap();
    let err = execute_typed_worker(
        &forged,
        &registry(),
        [
            (name.clone(), vec![fake.clone()]),
            ("forged".into(), vec![fake]),
        ]
        .into(),
        &worker,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("file input contract"));
    let outside = temp.path().join("outside.gz");
    std::fs::write(&outside, b"not an admitted snapshot").unwrap();
    let err = execute_typed_worker(
        &envelope,
        &registry(),
        [(
            name.clone(),
            vec![worker.join("typed-inputs/../../outside.gz")],
        )]
        .into(),
        &worker,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("inside this worker"));
}

fn registry() -> NodeRegistry {
    let mut registry = NodeRegistry::new(NodeCtx::new(SessionContext::new().runtime_env(), None));
    registry.register(Box::new(EchoFactory));
    registry
}

fn task(payload: &str) -> TaskSpec {
    TaskSpec {
        id: "toy-task".into(),
        logical_node: None,
        kind: "core/worker_echo".into(),
        spec: serde_json::json!({"type":payload}),
        item_key: None,
        item: None,
        inputs: vec![TaskInputBinding {
            name: "value".into(),
            port: 0,
            source: TaskInputSource::UpstreamPort {
                from: "source".into(),
                from_port: 0,
            },
            payload: payload.into(),
            path: None,
            fingerprint: None,
            staged_paths: vec![],
        }],
        outputs: vec![TaskOutputBinding {
            name: "value".into(),
            port: 0,
            payload: payload.into(),
            artifact_paths: vec![],
        }],
    }
}

fn execution() -> TaskExecution {
    let registry = registry();
    let spec = task("channel");
    TaskExecution {
        node: registry.build_node(&spec.kind, spec.spec.clone()).unwrap(),
        submission: TaskSubmission {
            spec,
            inputs: vec![NodeInput {
                port: 0,
                data: NodeValue::Channel(crate::ChannelValue {
                    items: vec![serde_json::json!({"item":7})],
                }),
            }],
            resources: TaskResources::default(),
        },
        engine_ctx: Arc::new(registry.ctx().clone()),
        reporter: NodeReporter::noop(),
        cancellation: CancellationToken::new(),
    }
}

struct CaptureExecutor {
    calls: Arc<AtomicUsize>,
    capture: bool,
}

#[tokio::test]
async fn host_service_node_fails_before_remote_submission() {
    let calls = Arc::new(AtomicUsize::new(0));
    let executor = TypedRemoteTaskExecutor::new(
        Arc::new(CaptureExecutor {
            calls: calls.clone(),
            capture: false,
        }),
        settings(),
    )
    .unwrap();
    let mut invocation = execution();
    invocation.submission.spec.spec["host_service"] = serde_json::json!(true);
    invocation.node = registry()
        .build_node(
            &invocation.submission.spec.kind,
            invocation.submission.spec.spec.clone(),
        )
        .unwrap();
    let result = executor.run(invocation).await;
    assert!(matches!(result, JobResult::Failed { .. }));
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "a known missing service must not stage or submit"
    );
}

#[tokio::test]
async fn worker_rejects_host_service_even_if_coordinator_admission_was_skipped() {
    let temp = tempfile::tempdir().unwrap();
    let mut spec = task("channel");
    spec.spec["host_service"] = serde_json::json!(true);
    let envelope = TypedWorkerEnvelope {
        protocol: 1,
        worker: settings(),
        task: spec,
        original_vfs_principal: None,
        spec_files: BTreeMap::new(),
        output_directories: BTreeMap::new(),
        input_files: BTreeMap::new(),
        bundles: vec![],
        plugin_identity: None,
    };
    let error = execute_typed_worker(&envelope, &registry(), BTreeMap::new(), temp.path())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("remote_service_unavailable"));
    assert!(!temp.path().join("typed-outputs").exists());
}

#[test]
fn host_service_admission_applies_to_shared_transport_lowering() {
    let mut invocation = execution();
    invocation.submission.spec.spec["host_service"] = serde_json::json!(true);
    invocation.node = registry()
        .build_node(
            &invocation.submission.spec.kind,
            invocation.submission.spec.spec.clone(),
        )
        .unwrap();
    let original = invocation.submission.spec.spec.clone();
    let error = lower_worker_task(&mut invocation, &settings(), None).unwrap_err();
    assert!(error.to_string().contains("remote_service_unavailable"));
    assert_eq!(invocation.submission.spec.spec, original);
}
#[async_trait::async_trait]
impl TaskExecutor for CaptureExecutor {
    fn name(&self) -> &'static str {
        "test-capture"
    }
    async fn run(&self, execution: TaskExecution) -> JobResult {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.capture {
            let dispatch = &execution.submission.spec;
            assert_eq!(dispatch.kind, "process");
            assert_eq!(dispatch.outputs[0].payload, "channel");
            let envelope: TypedWorkerEnvelope =
                serde_json::from_value(dispatch.spec["typed_dispatch"].clone()).unwrap();
            assert_eq!(envelope.task.kind, "core/worker_echo");
            assert!(
                dispatch.spec["script"]
                    .as_str()
                    .unwrap()
                    .contains("sha256sum")
            );
            assert_eq!(
                dispatch.spec["outputs"][0]["pattern"],
                "typed-outputs/port-0/*"
            );
        }
        JobResult::Success {
            id: execution.submission.spec.id,
            outputs: PortOutputs::new(),
            duration: Duration::ZERO,
            details: None,
        }
    }
}

#[tokio::test]
async fn typed_nodes_route_remotely_and_missing_transport_never_falls_back() {
    let local_calls = Arc::new(AtomicUsize::new(0));
    let remote_calls = Arc::new(AtomicUsize::new(0));
    let local = Arc::new(CaptureExecutor {
        calls: local_calls.clone(),
        capture: false,
    });
    let remote = Arc::new(
        TypedRemoteTaskExecutor::new(
            Arc::new(CaptureExecutor {
                calls: remote_calls.clone(),
                capture: true,
            }),
            settings(),
        )
        .unwrap(),
    );
    let placement = PlacementTaskExecutor::new(local.clone(), Some(remote), vec![]).unwrap();
    assert!(matches!(
        placement.run(execution()).await,
        JobResult::Success { .. }
    ));
    assert_eq!(local_calls.load(Ordering::SeqCst), 0);
    assert_eq!(remote_calls.load(Ordering::SeqCst), 1);
    let missing = PlacementTaskExecutor::new(local, None, vec![]).unwrap();
    assert!(matches!(
        missing.run(execution()).await,
        JobResult::Failed { .. }
    ));
    assert_eq!(local_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn new_container_plugins_use_bound_backend_only_when_enabled() {
    let local_calls = Arc::new(AtomicUsize::new(0));
    let remote_calls = Arc::new(AtomicUsize::new(0));
    let placement = || {
        PlacementTaskExecutor::new(
            Arc::new(CaptureExecutor {
                calls: local_calls.clone(),
                capture: false,
            }),
            Some(Arc::new(CaptureExecutor {
                calls: remote_calls.clone(),
                capture: false,
            })),
            vec![],
        )
        .unwrap()
    };
    let invocation = || {
        let mut task = execution();
        // A fresh plugin address has never appeared in a placement whitelist.
        task.submission.spec.kind = "new-plugin/fresh-container".into();
        let mut node = task
            .node
            .as_any()
            .downcast_ref::<EchoNode>()
            .unwrap()
            .clone();
        node.container_dispatch = true;
        task.node = Box::new(node);
        task
    };
    assert!(matches!(
        placement().run(invocation()).await,
        JobResult::Success { .. }
    ));
    assert_eq!(remote_calls.load(Ordering::SeqCst), 1);
    assert_eq!(local_calls.load(Ordering::SeqCst), 0);
    assert!(matches!(
        placement()
            .with_remote_container_dispatch()
            .run(invocation())
            .await,
        JobResult::Success { .. }
    ));
    assert_eq!(local_calls.load(Ordering::SeqCst), 1);
    // Setting a similarly named spec field cannot turn a native node into a
    // container adapter or bring native computation onto the coordinator.
    let mut native = execution();
    native.submission.spec.spec["delegates_to_container_runtime"] = serde_json::json!(true);
    assert!(matches!(
        placement()
            .with_remote_container_dispatch()
            .run(native)
            .await,
        JobResult::Success { .. }
    ));
    assert_eq!(remote_calls.load(Ordering::SeqCst), 2);
    assert_eq!(local_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn worker_roundtrips_channel_files_and_arrow_without_coordinator_memory() {
    for payload in ["channel", "file", "file_set", "dataframe"] {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("input");
        let paths = match payload {
            "channel" => {
                std::fs::write(&input, r#"[{"id":7},{"id":9}]"#).unwrap();
                vec![input]
            }
            "dataframe" => {
                let ctx = SessionContext::new();
                let frame = ctx
                    .sql("select 7 as id union all select 9 as id")
                    .await
                    .unwrap();
                let file = std::fs::File::create(&input).unwrap();
                let mut writer = FileWriter::try_new(file, frame.schema().as_ref()).unwrap();
                for batch in frame.collect().await.unwrap() {
                    writer.write(&batch).unwrap();
                }
                writer.finish().unwrap();
                vec![input]
            }
            _ => {
                std::fs::write(&input, b"complete input bytes").unwrap();
                vec![input]
            }
        };
        let mut task = task(payload);
        task.inputs[0].source = TaskInputSource::Channel {
            from: "reads".into(),
            from_port: 0,
            collected: false,
        };
        let envelope = TypedWorkerEnvelope {
            spec_files: BTreeMap::new(),
            output_directories: BTreeMap::new(),
            original_vfs_principal: None,
            protocol: 1,
            worker: settings(),
            task,
            input_files: BTreeMap::new(),
            bundles: vec![],
            plugin_identity: None,
        };
        let envelope: TypedWorkerEnvelope =
            serde_json::from_value(serde_json::to_value(envelope).unwrap()).unwrap();
        execute_typed_worker(
            &envelope,
            &registry(),
            [("value".into(), paths)].into_iter().collect(),
            temp.path(),
        )
        .await
        .unwrap();
        let root = temp.path().join("typed-outputs/port-0");
        match payload {
            "channel" => {
                let items: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(root.join("value.json")).unwrap())
                        .unwrap();
                assert_eq!(items, serde_json::json!([{"id":7},{"id":9}]));
            }
            "dataframe" => {
                let file = std::fs::File::open(root.join("value.arrow")).unwrap();
                let reader = arrow::ipc::reader::FileReader::try_new(file, None).unwrap();
                assert_eq!(
                    reader.map(|batch| batch.unwrap().num_rows()).sum::<usize>(),
                    2
                );
            }
            _ => {
                let output = std::fs::read_dir(root)
                    .unwrap()
                    .next()
                    .unwrap()
                    .unwrap()
                    .path();
                assert_eq!(std::fs::read(output).unwrap(), b"complete input bytes");
            }
        }
    }
}

#[tokio::test]
async fn unknown_remote_kind_is_rejected_before_any_output_publication() {
    let temp = tempfile::tempdir().unwrap();
    let mut spec = task("channel");
    spec.kind = "missing/plugin".into();
    let envelope = TypedWorkerEnvelope {
        spec_files: BTreeMap::new(),
        output_directories: BTreeMap::new(),
        original_vfs_principal: None,
        protocol: 1,
        worker: settings(),
        task: spec,
        input_files: BTreeMap::new(),
        bundles: vec![],
        plugin_identity: None,
    };
    let error = execute_typed_worker(&envelope, &registry(), BTreeMap::new(), temp.path())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("cannot build kind"));
    assert!(!temp.path().join("typed-outputs").exists());
}

#[tokio::test]
async fn scheduler_native_channel_requires_no_registry_factory_or_input_artifacts() {
    for kind in ["channel", "core/channel"] {
        let temp = tempfile::tempdir().unwrap();
        let mut spec = task("channel");
        spec.kind = kind.into();
        spec.spec =
            serde_json::json!({"operator":"of_items", "items":[{"synthetic":true}, 7, null]});
        spec.inputs.clear();
        let envelope = TypedWorkerEnvelope {
            spec_files: BTreeMap::new(),
            output_directories: BTreeMap::new(),
            original_vfs_principal: None,
            protocol: 1,
            worker: settings(),
            task: spec,
            input_files: BTreeMap::new(),
            bundles: vec![],
            plugin_identity: None,
        };
        let empty_registry =
            NodeRegistry::new(NodeCtx::new(SessionContext::new().runtime_env(), None));
        execute_typed_worker(&envelope, &empty_registry, BTreeMap::new(), temp.path())
            .await
            .unwrap();
        let output: serde_json::Value = serde_json::from_slice(
            &std::fs::read(temp.path().join("typed-outputs/port-0/value.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(output, envelope.task.spec["items"]);
    }
}

#[test]
fn plugin_identity_detects_changed_bytes_and_forbids_symlink_inputs() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("plugin.json"), b"first").unwrap();
    let first = typed_worker_tree_sha256(temp.path()).unwrap();
    std::fs::write(temp.path().join("plugin.json"), b"other").unwrap();
    assert_ne!(first, typed_worker_tree_sha256(temp.path()).unwrap());
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("plugin.json", temp.path().join("alias")).unwrap();
        assert!(typed_worker_tree_sha256(temp.path()).is_err());
    }
}

#[test]
fn worker_settings_reject_unpinned_or_ambient_deployments() {
    let mut config = settings();
    config.executable_sha256.clear();
    assert!(config.validate().is_err());
    let mut config = settings();
    config.plugins_root = Some("/cluster/plugins".into());
    assert!(config.validate().is_err());
    let mut config = settings();
    config.apptainer_catalog = Some("/cluster/catalog.json".into());
    assert!(config.validate().is_err());
}

#[test]
fn native_worker_environment_is_explicit_and_not_inlined() {
    let mut config = settings();
    config.native_environment = Some(TypedWorkerEnvironment {
        python: "/usr/bin/python3.11".into(),
        file: "/cluster/private/api-credentials.json".into(),
        network_policy_sha256: Some("b".repeat(64)),
    });
    let executor = TypedRemoteTaskExecutor::new(
        Arc::new(CaptureExecutor {
            calls: Arc::new(AtomicUsize::new(0)),
            capture: false,
        }),
        config.clone(),
    )
    .unwrap();
    let mut task = execution();
    executor.prepare(&mut task).unwrap();
    let script = task.submission.spec.spec["script"].as_str().unwrap();
    assert!(script.contains("sha256sum -- '/cluster/autonomics'"));
    assert!(script.contains("/cluster/private/api-credentials.json"));
    assert!(script.contains(&format!("--network-policy-sha256 '{}'", "b".repeat(64))));
    assert!(script.contains("os.execve"));
    assert!(!script.contains("export OPENGWAS"));
    config.native_environment.as_mut().unwrap().file = "/cluster/../private/credentials".into();
    assert!(config.validate().is_err());
    let mut old = serde_json::to_value(settings()).unwrap();
    old.as_object_mut().unwrap().remove("native_environment");
    assert!(
        serde_json::from_value::<TypedWorkerSettings>(old)
            .unwrap()
            .native_environment
            .is_none()
    );
}

#[tokio::test]
async fn worker_rejects_different_loaded_plugin_implementation() {
    let temp = tempfile::tempdir().unwrap();
    let envelope = TypedWorkerEnvelope {
        spec_files: BTreeMap::new(),
        output_directories: BTreeMap::new(),
        original_vfs_principal: None,
        protocol: 1,
        worker: settings(),
        task: task("channel"),
        input_files: BTreeMap::new(),
        bundles: vec![],
        plugin_identity: Some(crate::fingerprint::PluginIdentity {
            manifest_sha256: "a".repeat(64),
            script_sha256: None,
            image_reference: format!("registry/test@sha256:{}", "b".repeat(64)),
            panels: vec![],
        }),
    };
    let error = execute_typed_worker(&envelope, &registry(), BTreeMap::new(), temp.path())
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("plugin implementation identity differs")
    );
}

#[test]
fn file_publication_preserves_format_in_the_artifact_protocol() {
    let file = FileRef::new("/cluster/artifact-without-extension", Some("tsv".into()));
    assert_eq!(worker_output_name(&file, 2).unwrap(), "00000002.tsv");
    let invalid = FileRef::new("/cluster/file", Some("text/tab-separated-values".into()));
    assert!(worker_output_name(&invalid, 0).is_err());
}

struct AlreadyRemoteStore;

#[test]
fn structured_input_is_declared_in_worker_envelope() {
    let remote = TypedRemoteTaskExecutor::new(
        Arc::new(CaptureExecutor {
            calls: Arc::new(AtomicUsize::new(0)),
            capture: false,
        }),
        settings(),
    )
    .unwrap();
    let mut invocation = execution();
    invocation.submission.inputs[0].data =
        NodeValue::Structured(crate::dag::execution::ChannelTransportValue::Tuple(vec![
            crate::dag::execution::ChannelTransportValue::Json(serde_json::json!("sample")),
            crate::dag::execution::ChannelTransportValue::File(FileRef::new(
                "vfs:///research/input.tsv",
                Some("tsv".into()),
            )),
        ]));
    remote.prepare(&mut invocation).unwrap();
    let envelope: TypedWorkerEnvelope =
        serde_json::from_value(invocation.submission.spec.spec["typed_dispatch"].clone()).unwrap();
    assert_eq!(envelope.task.inputs[0].payload, "structured_channel");
    assert_eq!(
        envelope.input_files[&0][0].path,
        "vfs:///research/input.tsv"
    );
}

#[derive(Clone)]
struct PreparedTupleEcho(EchoNode);

#[async_trait::async_trait]
impl crate::DagNode for PreparedTupleEcho {
    fn ports(&self) -> &NodePorts {
        &self.0.ports
    }
    fn prepare_task_inputs(&self, inputs: &mut [NodeInput]) -> Result<(), DagError> {
        use crate::dag::execution::ChannelTransportValue as Value;
        inputs[0].data = NodeValue::Structured(Value::Tuple(vec![
            Value::Json(serde_json::json!("sample")),
            Value::File(FileRef::new(
                "vfs:///research/input.tsv",
                Some("tsv".into()),
            )),
        ]));
        Ok(())
    }
    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        inputs: &[NodeInput],
        reporter: &NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        self.0.execute(ctx, inputs, reporter).await
    }
    fn clone_box(&self) -> Box<dyn crate::DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "worker_echo"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[test]
fn remote_lowering_prepares_tuple_before_recording_file_artifacts() {
    let remote = TypedRemoteTaskExecutor::new(
        Arc::new(CaptureExecutor {
            calls: Arc::new(AtomicUsize::new(0)),
            capture: false,
        }),
        settings(),
    )
    .unwrap();
    let mut invocation = execution();
    invocation.node = Box::new(PreparedTupleEcho(EchoNode {
        omit_output: false,
        ports: NodePorts::new()
            .add_input_port_of_type(None, PortType::Channel)
            .add_output_port_of_type(None, PortType::Channel),
        path: None,
        container_dispatch: false,
        host_service: false,
    }));
    remote.prepare(&mut invocation).unwrap();
    let envelope: TypedWorkerEnvelope =
        serde_json::from_value(invocation.submission.spec.spec["typed_dispatch"].clone()).unwrap();
    assert_eq!(envelope.task.inputs[0].payload, "structured_channel");
    assert_eq!(
        envelope.input_files[&0][0].path,
        "vfs:///research/input.tsv"
    );
}

#[tokio::test]
async fn typed_worker_skips_only_declared_optional_outputs() {
    let remote = TypedRemoteTaskExecutor::new(
        Arc::new(CaptureExecutor {
            calls: Arc::new(AtomicUsize::new(0)),
            capture: false,
        }),
        settings(),
    )
    .unwrap();
    for optional in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("input.json");
        std::fs::write(&input, b"[1]").unwrap();
        let mut invocation = execution();
        invocation.submission.spec.spec["optional_output"] = serde_json::json!(optional);
        invocation.submission.spec.spec["omit_output"] = serde_json::json!(true);
        invocation.node = registry()
            .build_node(
                &invocation.submission.spec.kind,
                invocation.submission.spec.spec.clone(),
            )
            .unwrap();
        remote.prepare(&mut invocation).unwrap();
        assert_eq!(
            invocation.submission.spec.spec["outputs"][0]["optional"],
            optional
        );
        let envelope: TypedWorkerEnvelope =
            serde_json::from_value(invocation.submission.spec.spec["typed_dispatch"].clone())
                .unwrap();
        let result = execute_typed_worker(
            &envelope,
            &registry(),
            [("value".into(), vec![input])].into(),
            temp.path(),
        )
        .await;
        if optional {
            result.unwrap();
            assert!(!temp.path().join("typed-outputs/port-0").exists());
        } else {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("omitted a required output")
            );
        }
    }
}

#[tokio::test]
async fn structured_channel_round_trips_through_typed_worker_output() {
    use crate::dag::execution::ChannelTransportValue as Value;
    let temp = tempfile::tempdir().unwrap();
    let a = temp.path().join("a.txt");
    let b = temp.path().join("b.txt");
    std::fs::write(&a, b"first").unwrap();
    std::fs::write(&b, b"second").unwrap();
    let value = Value::Tuple(vec![
        Value::Json(serde_json::json!("sample")),
        Value::List(vec![
            Value::File(FileRef::local(&a, Some("txt".into())).unwrap()),
            Value::File(FileRef::local(&b, Some("txt".into())).unwrap()),
        ]),
    ]);
    let manifest = temp.path().join("input.json");
    std::fs::write(&manifest, serde_json::to_vec(&value).unwrap()).unwrap();
    let remote = TypedRemoteTaskExecutor::new(
        Arc::new(CaptureExecutor {
            calls: Arc::new(AtomicUsize::new(0)),
            capture: false,
        }),
        settings(),
    )
    .unwrap();
    let mut invocation = execution();
    invocation.submission.inputs[0].data = NodeValue::Structured(value);
    remote.prepare(&mut invocation).unwrap();
    let envelope: TypedWorkerEnvelope =
        serde_json::from_value(invocation.submission.spec.spec["typed_dispatch"].clone()).unwrap();
    let paths = BTreeMap::from([("value".into(), vec![manifest, a, b])]);
    execute_typed_worker(&envelope, &registry(), paths, temp.path())
        .await
        .unwrap();
    let root = temp.path().join("typed-outputs/port-0");
    let artifacts = ["00000000.json", "00000001.txt", "00000002.txt"]
        .into_iter()
        .map(|name| FileRef::local(root.join(name), None).unwrap())
        .collect::<Vec<_>>();
    let restored = crate::dag::execution::remote::RemoteTaskExecutor::load_output(
        &artifacts,
        "channel",
        invocation.engine_ctx.as_ref(),
    )
    .await
    .unwrap();
    let NodeValue::Structured(Value::Tuple(items)) = restored else {
        panic!("tuple expected")
    };
    assert!(matches!(&items[0], Value::Json(value) if value == "sample"));
    let Value::List(files) = &items[1] else {
        panic!("list expected")
    };
    for (file, expected) in files
        .iter()
        .zip([b"first".as_slice(), b"second".as_slice()])
    {
        let Value::File(file) = file else {
            panic!("file expected")
        };
        assert_eq!(std::fs::read(&file.path).unwrap(), expected);
    }
    assert!(
        crate::dag::execution::remote::RemoteTaskExecutor::load_output(
            &artifacts[..2],
            "channel",
            invocation.engine_ctx.as_ref(),
        )
        .await
        .is_err()
    );
}

#[async_trait::async_trait]
impl crate::TaskArtifactStore for AlreadyRemoteStore {
    async fn remote_input(
        &self,
        artifact: FileRef,
        _: Option<&vfs::OpendalFileStorage>,
    ) -> Result<Option<FileRef>, DagError> {
        assert_eq!(artifact.path, "vfs:///research/input.tsv");
        Ok(Some(FileRef::new(
            "/cluster/data/input.tsv",
            Some("tsv".into()),
        )))
    }
    async fn upload(&self, _: FileRef, _: &str, _: &str) -> Result<FileRef, DagError> {
        panic!("remote input must not be uploaded")
    }
    async fn download(&self, _: FileRef, _: &str, _: &str) -> Result<FileRef, DagError> {
        panic!("staging must not download")
    }
}

#[tokio::test]
async fn remote_file_staging_never_opens_cluster_paths_on_the_coordinator() {
    let temp = tempfile::tempdir().unwrap();
    let mut submission = TaskSubmission {
        spec: task("file"),
        resources: TaskResources::default(),
        inputs: vec![NodeInput::file(
            0,
            FileRef::new("vfs:///research/input.tsv", Some("tsv".into())),
        )],
    };
    let mut workspace =
        crate::dag::execution::workspace::LocalTaskWorkspace::create(temp.path(), &submission)
            .unwrap();
    let remote = workspace
        .stage_inputs_with_remote_observed(&mut submission, None, Some(&AlreadyRemoteStore), None)
        .await
        .unwrap();
    assert_eq!(remote[&(0, 0)].path, "/cluster/data/input.tsv");
    assert_eq!(
        submission.spec.inputs[0].staged_paths,
        vec!["/cluster/data/input.tsv"]
    );
    assert!(submission.into_remote_dispatch().is_ok());
    assert!(!Path::new("/cluster/data/input.tsv").exists());
}

#[test]
fn remote_task_resources_follow_compiled_node_budgets_before_submission() {
    let remote = TypedRemoteTaskExecutor::new(
        Arc::new(CaptureExecutor {
            calls: Arc::new(AtomicUsize::new(0)),
            capture: false,
        }),
        settings(),
    )
    .unwrap();
    let mut inferred = execution();
    remote.prepare(&mut inferred).unwrap();
    assert_eq!(inferred.submission.resources.cpus, Some(2));
    assert_eq!(
        inferred.submission.resources.memory_bytes,
        Some(128 * 1024 * 1024)
    );
    assert_eq!(inferred.submission.resources.max_duration_ms, Some(60_000));
    for insufficient in [
        TaskResources {
            cpus: Some(1),
            ..Default::default()
        },
        TaskResources {
            memory_bytes: Some(1),
            ..Default::default()
        },
    ] {
        let mut task = execution();
        task.submission.resources = insufficient;
        assert!(remote.prepare(&mut task).is_err());
        assert_eq!(task.submission.spec.kind, "core/worker_echo");
    }
    let mut capped = execution();
    capped.submission.resources = TaskResources {
        cpus: Some(4),
        memory_bytes: None,
        max_duration_ms: Some(1000),
    };
    remote.prepare(&mut capped).unwrap();
    assert_eq!(capped.submission.resources.cpus, Some(4));
    assert_eq!(capped.submission.resources.max_duration_ms, Some(1000));
}

#[tokio::test]
async fn native_gather_infers_concrete_contract_and_roundtrips_homogeneous_inputs() {
    let remote = TypedRemoteTaskExecutor::new(
        Arc::new(CaptureExecutor {
            calls: Arc::new(AtomicUsize::new(0)),
            capture: false,
        }),
        settings(),
    )
    .unwrap();
    for payload in ["dataframe", "file", "file_set"] {
        let temp = tempfile::tempdir().unwrap();
        let mut invocation = execution();
        invocation.node = Box::new(crate::dag::GatherNode::default());
        invocation.submission.spec.kind = "logical_gather".into();
        invocation.submission.spec.spec = serde_json::json!({});
        invocation.submission.spec.outputs[0].payload = "any".into();
        invocation.submission.inputs.clear();
        invocation.submission.spec.inputs.clear();
        let mut paths = BTreeMap::new();
        for port in 0..2u8 {
            let input_path = temp.path().join(format!("input-{port}.txt"));
            let value = if payload == "dataframe" {
                let frame = SessionContext::new()
                    .sql(&format!("select {} as id", port + 7))
                    .await
                    .unwrap();
                let mut writer = FileWriter::try_new(
                    std::fs::File::create(&input_path).unwrap(),
                    frame.schema().as_ref(),
                )
                .unwrap();
                for batch in frame.clone().collect().await.unwrap() {
                    writer.write(&batch).unwrap();
                }
                writer.finish().unwrap();
                NodeValue::DataFrame(frame)
            } else {
                std::fs::write(&input_path, format!("synthetic-{port}")).unwrap();
                let file = FileRef::local(&input_path, Some("txt".into())).unwrap();
                if payload == "file" {
                    NodeValue::File(file)
                } else {
                    NodeValue::FileSet(vec![file])
                }
            };
            let name = format!("value-{port}");
            paths.insert(name.clone(), vec![input_path]);
            let mut binding = task(payload).inputs.remove(0);
            binding.port = port;
            binding.name = name;
            // Scheduler-native Gather inputs are declared Any; prepare must
            // use actual runtime values to derive their transport contracts.
            binding.payload = "any".into();
            invocation.submission.spec.inputs.push(binding);
            invocation
                .submission
                .inputs
                .push(NodeInput { port, data: value });
        }
        remote.prepare(&mut invocation).unwrap();
        let envelope: TypedWorkerEnvelope =
            serde_json::from_value(invocation.submission.spec.spec["typed_dispatch"].clone())
                .unwrap();
        let expected = if payload == "dataframe" {
            "dataframe"
        } else {
            "file_set"
        };
        assert_eq!(envelope.task.outputs[0].payload, expected);
        assert_eq!(invocation.submission.spec.outputs[0].payload, expected);
        let empty_registry =
            NodeRegistry::new(NodeCtx::new(SessionContext::new().runtime_env(), None));
        execute_typed_worker(&envelope, &empty_registry, paths, temp.path())
            .await
            .unwrap();
        let output = temp.path().join("typed-outputs/port-0");
        if payload == "dataframe" {
            let reader = arrow::ipc::reader::FileReader::try_new(
                std::fs::File::open(output.join("value.arrow")).unwrap(),
                None,
            )
            .unwrap();
            assert_eq!(
                reader.map(|batch| batch.unwrap().num_rows()).sum::<usize>(),
                2
            );
        } else {
            let mut bytes = std::fs::read_dir(output)
                .unwrap()
                .map(|entry| std::fs::read(entry.unwrap().path()).unwrap())
                .collect::<Vec<_>>();
            bytes.sort();
            assert_eq!(
                bytes,
                vec![b"synthetic-0".to_vec(), b"synthetic-1".to_vec()]
            );
        }
    }
}

#[test]
fn gather_rejects_empty_mixed_and_non_gather_any_contracts() {
    assert!(gather_output_payload(std::iter::empty()).is_err());
    assert!(gather_output_payload(["file", "file_set"]).is_err());
    assert!(gather_output_payload(["dataframe", "file"]).is_err());
    assert!(gather_output_payload(["channel"]).is_err());
    let remote = TypedRemoteTaskExecutor::new(
        Arc::new(CaptureExecutor {
            calls: Arc::new(AtomicUsize::new(0)),
            capture: false,
        }),
        settings(),
    )
    .unwrap();
    let mut invocation = execution();
    invocation.submission.spec.outputs[0].payload = "any".into();
    assert!(remote.prepare(&mut invocation).is_err());
}

#[test]
fn typed_podman_requires_explicit_pins_and_is_exclusive_with_apptainer() {
    let podman = TypedPodmanSettings {
        executable: "/usr/bin/podman".into(),
        executable_sha256: "b".repeat(64),
        image_catalog: None,
        image_catalog_sha256: None,
        panel_cache_root: None,
        oci_runtime: None,
        oci_runtime_sha256: None,
        resource_control: TypedPodmanResourceControl::DelegatedCgroups,
        user_namespace: TypedPodmanUserNamespace::KeepId,
    };
    let mut config = settings();
    config.plugins_root = Some("/cluster/plugins".into());
    config.plugins_sha256 = Some("c".repeat(64));
    assert!(
        config.validate().is_err(),
        "plugins cannot fall back to ambient Podman"
    );
    config.podman = Some(podman.clone());
    config.validate().unwrap();
    config.apptainer_catalog = Some("/cluster/sif-catalog.json".into());
    config.apptainer_executable = Some("/usr/bin/apptainer".into());
    assert!(config.validate().is_err());
    for invalid in [
        TypedPodmanSettings {
            executable: "podman".into(),
            ..podman.clone()
        },
        TypedPodmanSettings {
            executable_sha256: String::new(),
            ..podman.clone()
        },
        TypedPodmanSettings {
            image_catalog: Some("/cluster/oci.json".into()),
            ..podman.clone()
        },
        TypedPodmanSettings {
            image_catalog_sha256: Some("a".repeat(64)),
            ..podman.clone()
        },
        TypedPodmanSettings {
            panel_cache_root: Some("relative/cache".into()),
            ..podman.clone()
        },
    ] {
        assert!(invalid.validate().is_err());
    }
    let pinned = TypedPodmanSettings {
        image_catalog: Some("/cluster/oci.json".into()),
        image_catalog_sha256: Some("a".repeat(64)),
        ..podman
    };
    pinned.validate().unwrap();
    let mut old = serde_json::to_value(settings()).unwrap();
    old.as_object_mut().unwrap().remove("podman");
    assert!(
        serde_json::from_value::<TypedWorkerSettings>(old)
            .unwrap()
            .podman
            .is_none()
    );
}

#[test]
fn typed_podman_legacy_policy_defaults_do_not_opt_into_slurm_or_root() {
    let settings: TypedPodmanSettings = serde_json::from_value(serde_json::json!({
        "executable":"/usr/bin/podman", "executable_sha256":"b".repeat(64),
        "image_catalog":null,"image_catalog_sha256":null,"panel_cache_root":null
    }))
    .unwrap();
    assert_eq!(
        settings.resource_control,
        TypedPodmanResourceControl::DelegatedCgroups
    );
    assert_eq!(settings.user_namespace, TypedPodmanUserNamespace::KeepId);
    assert_eq!(
        serde_json::to_value(settings.resource_control).unwrap(),
        serde_json::json!({"mode":"delegated_cgroups"})
    );
    settings.validate().unwrap();
}

#[test]
fn typed_podman_slurm_policy_is_explicit_closed_and_roundtrips() {
    let legacy = serde_json::json!({
        "executable":"/usr/bin/podman", "executable_sha256":"b".repeat(64),
        "image_catalog":null,"image_catalog_sha256":null,"panel_cache_root":null
    });
    for (name, expected) in [
        ("inherit", TypedSlurmSwapPolicy::Inherit),
        ("disabled", TypedSlurmSwapPolicy::Disabled),
    ] {
        let mut value = legacy.clone();
        value["resource_control"] =
            serde_json::json!({"mode":"slurm_allocation","swap_policy":name});
        value["user_namespace"] = serde_json::json!("single_uid_root");
        value["oci_runtime"] = serde_json::json!("/opt/technical/crun");
        value["oci_runtime_sha256"] = serde_json::json!(format!(
            "{:x}",
            Sha256::digest(b"technical unit OCI runtime")
        ));
        let settings: TypedPodmanSettings = serde_json::from_value(value).unwrap();
        settings.validate().unwrap();
        assert_eq!(
            settings.resource_control,
            TypedPodmanResourceControl::SlurmAllocation {
                swap_policy: expected
            }
        );
        assert_eq!(
            settings.user_namespace,
            TypedPodmanUserNamespace::SingleUidRoot
        );
        let decoded: TypedPodmanSettings =
            serde_json::from_value(serde_json::to_value(&settings).unwrap()).unwrap();
        assert_eq!(decoded.resource_control, settings.resource_control);
        assert_eq!(decoded.user_namespace, settings.user_namespace);
    }
    for policy in [
        serde_json::json!({"mode":"slurm_allocation"}),
        serde_json::json!({"mode":"slurm_allocation","swap_policy":"ignore"}),
        serde_json::json!({"mode":"slurm_allocation","swap_policy":"inherit","skip_limits":true}),
        serde_json::json!({"mode":"delegated_cgroups","swap_policy":"inherit"}),
        serde_json::json!({"mode":"unrestricted"}),
        serde_json::json!("slurm_allocation"),
        serde_json::Value::Null,
    ] {
        let mut value = legacy.clone();
        value["resource_control"] = policy.clone();
        assert!(
            serde_json::from_value::<TypedPodmanSettings>(value).is_err(),
            "accepted invalid resource-control policy: {policy}"
        );
    }
    let mut invalid = legacy;
    invalid["user_namespace"] = serde_json::json!("host_root");
    assert!(serde_json::from_value::<TypedPodmanSettings>(invalid).is_err());
}

#[test]
fn typed_podman_oci_runtime_is_paired_and_required_for_slurm_allocation() {
    let hash = format!("{:x}", Sha256::digest(b"technical unit OCI runtime"));
    let legacy = serde_json::json!({
        "executable":"/usr/bin/podman", "executable_sha256":"b".repeat(64)
    });
    for policy in [
        serde_json::json!({"mode":"delegated_cgroups"}),
        serde_json::json!({"mode":"slurm_allocation","swap_policy":"inherit"}),
    ] {
        let mut value = legacy.clone();
        value["resource_control"] = policy.clone();
        let unpinned: TypedPodmanSettings = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(
            unpinned.validate().is_ok(),
            policy["mode"] == "delegated_cgroups"
        );
        for (runtime, pin) in [
            (
                serde_json::json!("/opt/technical/crun"),
                serde_json::Value::Null,
            ),
            (serde_json::Value::Null, serde_json::json!(hash)),
            (serde_json::json!("relative/crun"), serde_json::json!(hash)),
            (
                serde_json::json!("/opt/technical/crun"),
                serde_json::json!("not-a-sha256"),
            ),
        ] {
            let mut invalid = value.clone();
            invalid["oci_runtime"] = runtime;
            invalid["oci_runtime_sha256"] = pin;
            assert!(
                serde_json::from_value::<TypedPodmanSettings>(invalid)
                    .unwrap()
                    .validate()
                    .is_err()
            );
        }
        value["oci_runtime"] = serde_json::json!("/opt/technical/crun");
        value["oci_runtime_sha256"] = serde_json::json!(hash);
        let pinned: TypedPodmanSettings = serde_json::from_value(value).unwrap();
        pinned.validate().unwrap();
        let decoded: TypedPodmanSettings =
            serde_json::from_value(serde_json::to_value(&pinned).unwrap()).unwrap();
        assert_eq!(decoded.oci_runtime, pinned.oci_runtime);
        assert_eq!(decoded.oci_runtime_sha256, pinned.oci_runtime_sha256);
    }
}

#[test]
fn typed_podman_operator_policy_survives_agent_spec_attempts_to_replace_it() {
    let podman: TypedPodmanSettings = serde_json::from_value(serde_json::json!({
        "executable":"/usr/bin/podman", "executable_sha256":"b".repeat(64),
        "resource_control":{"mode":"slurm_allocation","swap_policy":"disabled"},
        "user_namespace":"single_uid_root",
        "oci_runtime":"/opt/technical/crun",
        "oci_runtime_sha256":format!("{:x}", Sha256::digest(b"technical unit OCI runtime"))
    }))
    .unwrap();
    let mut worker = settings();
    use std::io::Read;
    worker.executable = std::env::current_exe().unwrap();
    let mut binary = std::fs::File::open(&worker.executable).unwrap();
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = binary.read(&mut buffer).unwrap();
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    worker.executable_sha256 = format!("{:x}", hasher.finalize());
    worker.podman = Some(podman);
    let adapter = TypedRemoteTaskExecutor::new(
        Arc::new(CaptureExecutor {
            calls: Arc::new(AtomicUsize::new(0)),
            capture: false,
        }),
        worker,
    )
    .unwrap();
    let mut invocation = execution();
    invocation.submission.spec.spec["podman"] = serde_json::json!({
        "resource_control":{"mode":"unrestricted"},"user_namespace":"host_root",
        "oci_runtime":"/untrusted/runtime","oci_runtime_sha256":"0".repeat(64)
    });
    adapter.prepare(&mut invocation).unwrap();
    let envelope = typed_worker_envelope(&TaskDispatch {
        spec: invocation.submission.spec,
        resources: invocation.submission.resources,
    })
    .unwrap();
    let selected = envelope.worker.podman.unwrap();
    assert_eq!(
        selected.resource_control,
        TypedPodmanResourceControl::SlurmAllocation {
            swap_policy: TypedSlurmSwapPolicy::Disabled
        }
    );
    assert_eq!(
        selected.user_namespace,
        TypedPodmanUserNamespace::SingleUidRoot
    );
    assert_eq!(
        selected.oci_runtime.as_deref(),
        Some(std::path::Path::new("/opt/technical/crun"))
    );
    assert_eq!(
        selected.oci_runtime_sha256.as_deref(),
        Some(format!(
            "{:x}",
            Sha256::digest(b"technical unit OCI runtime")
        ))
        .as_deref()
    );
}

#[test]
fn typed_envelope_transports_actual_storage_principal_and_ignores_spec_impersonation() {
    let fixture = tempfile::tempdir().unwrap();
    let principal = vfs::permission::VfsPrincipal::plugin_developer(12_345);
    let storage = vfs::OpendalFileStorage::new(fixture.path()).with_principal(principal.clone());
    let mut invocation = execution();
    let mut ctx = (*invocation.engine_ctx).clone();
    ctx.opendal = Some(Arc::new(storage));
    invocation.engine_ctx = Arc::new(ctx);
    invocation.submission.spec.spec["original_vfs_principal"] =
        serde_json::to_value(vfs::permission::VfsPrincipal::root()).unwrap();
    let adapter = TypedRemoteTaskExecutor::new(
        Arc::new(CaptureExecutor {
            calls: Arc::new(AtomicUsize::new(0)),
            capture: false,
        }),
        settings(),
    )
    .unwrap();
    adapter.prepare(&mut invocation).unwrap();
    let mut wire = invocation.submission.spec.spec["typed_dispatch"].clone();
    assert_eq!(
        wire["original_vfs_principal"],
        serde_json::to_value(&principal).unwrap()
    );
    let mut envelope: TypedWorkerEnvelope = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(envelope.vfs_principal().unwrap(), Some(principal));
    wire.as_object_mut()
        .unwrap()
        .remove("original_vfs_principal");
    let legacy: TypedWorkerEnvelope = serde_json::from_value(wire).unwrap();
    assert!(legacy.vfs_principal().unwrap().is_none());
    envelope.worker.worker_vfs_manifest = Some("/operator/worker-vfs.toml".into());
    envelope.worker.worker_vfs_sha256 = Some("b".repeat(64));
    envelope.original_vfs_principal = None;
    assert!(envelope.vfs_principal().is_err());
    let mut invalid = vfs::permission::VfsPrincipal::new(12_345, 100);
    invalid.superuser = true;
    envelope.original_vfs_principal = Some(invalid);
    assert!(envelope.vfs_principal().is_err());
}

#[tokio::test]
async fn worker_vfs_without_coordinator_identity_is_rejected_before_remote_submission() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut worker = settings();
    worker.worker_vfs_manifest = Some("/operator/worker-vfs.toml".into());
    worker.worker_vfs_sha256 = Some("b".repeat(64));
    let executor = TypedRemoteTaskExecutor::new(
        Arc::new(CaptureExecutor {
            calls: calls.clone(),
            capture: false,
        }),
        worker,
    )
    .unwrap();
    assert!(matches!(
        executor.run(execution()).await,
        JobResult::Failed { .. }
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}
