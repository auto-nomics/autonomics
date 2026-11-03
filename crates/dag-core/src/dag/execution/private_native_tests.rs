use super::*;
use crate::dag::node_event::NodeReporter;
use crate::registry::error::Result as RegistryResult;
use crate::{
    DagNode, NodeCtx, NodeFactory, NodeInput, NodePorts, TaskOutputBinding, TaskSpec,
    TaskSubmission,
};
use datafusion::prelude::SessionContext;
use std::os::unix::fs::PermissionsExt;
use tokio_util::sync::CancellationToken;
const RUN: &str = "b0000000-0000-0000-0000-000000000001";
const ENGINE_RUN: &str = "c0000000-0000-0000-0000-000000000002";
fn settings() -> TypedWorkerSettings {
    TypedWorkerSettings {
        executable: "/software/research_worker".into(),
        executable_sha256: "b".repeat(64),
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
fn binding() -> PrivateNativeBinding {
    PrivateNativeBinding {
        scope: SCOPE.into(),
        contract_sha256: "a".repeat(64),
        module_id: "SYN".into(),
        run_id: RUN.into(),
        engine_run_id: ENGINE_RUN.into(),
        task_id: "native-task".into(),
        kind: "core/native_test_publisher".into(),
        spec_sha256: spec_hash(
            &serde_json::json!({"output_directory":"/frozen/output","value":"toy"}),
        )
        .unwrap(),
        remote_user: "private_test".into(),
        engine_sha256: "b".repeat(64),
        compute_scratch_root: std::env::temp_dir(),
        projection_identity_blake3: None,
        projected_directories: BTreeMap::new(),
        sink: None,
        metadata_port: 1,
        budget: NativeComputeBudget {
            resources: TaskResources {
                cpus: Some(1),
                memory_bytes: Some(128 * 1024 * 1024),
                max_duration_ms: Some(60000),
            },
            memory_reserve_bytes: 0,
            disk_reserve_fraction: 0.1,
            peak_disk_bytes: 1024,
        },
    }
}
#[derive(Clone)]
struct Publisher {
    ports: NodePorts,
    spec: serde_json::Value,
}
#[async_trait::async_trait]
impl DagNode for Publisher {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }
    fn kind(&self) -> &'static str {
        "native_test_publisher"
    }
    fn runtime_output_directories(&self) -> Vec<PathBuf> {
        vec![self.spec["output_directory"].as_str().unwrap().into()]
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        _: &[NodeInput],
        _: &NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let logical = Path::new(self.spec["output_directory"].as_str().unwrap());
        let root = ctx
            .runtime_output_projection
            .as_ref()
            .ok_or_else(|| refused("projection missing"))?
            .resolve(logical)?;
        private_dir(&root)?;
        private_dir(&root.join("nested"))?;
        let file = root.join("nested/value.tsv");
        fs::write(&file, self.spec["value"].as_str().unwrap()).map_err(refused)?;
        let format = if self.spec.get("format").is_some() {
            self.spec["format"].as_str().map(str::to_owned)
        } else {
            Some("tsv".into())
        };
        let mut out = PortOutputs::new();
        out.insert_file(0, FileRef::local(file, format)?);
        Ok(out)
    }
}
struct Factory;
impl NodeFactory for Factory {
    fn kind(&self) -> &'static str {
        "native_test_publisher"
    }
    fn desc(&self) -> &'static str {
        "SYN only"
    }
    fn doc(&self) -> &'static str {
        "SYN only"
    }
    fn spec_schema(&self) -> schemars::Schema {
        schemars::schema_for!(serde_json::Value)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_output_port_of_type(None, crate::PortType::File)
    }
    fn build(&self, spec: serde_json::Value, _: NodeCtx) -> RegistryResult<Box<dyn DagNode>> {
        Ok(Box::new(Publisher {
            ports: self.ports(),
            spec,
        }))
    }
}
fn registry() -> NodeRegistry {
    let mut r = NodeRegistry::new(NodeCtx::new(SessionContext::new().runtime_env(), None));
    r.register(Box::new(Factory));
    r
}
fn projection(root: &Path) -> RuntimeOutputProjection {
    let staging = root.join("staging");
    private_dir(&staging).unwrap();
    RuntimeOutputProjection::new(
        ENGINE_RUN.into(),
        staging.clone(),
        [(
            "/frozen/output".into(),
            staging.join(ENGINE_RUN).join("publisher"),
        )]
        .into(),
    )
    .unwrap()
    .initialize_run_directory()
    .unwrap()
}

#[test]
fn formal_store_run_is_not_the_recorded_projection_run() {
    let t = tempfile::tempdir().unwrap();
    let (e, mut b) = prepared(t.path());
    let p = e.engine_ctx.runtime_output_projection.as_deref().unwrap();
    assert_ne!(b.run_id, b.engine_run_id);
    assert_eq!(p.run_id(), b.engine_run_id);
    b.verify_projection(Some(p)).unwrap();
    // A compute-only binding cannot authorize a Store run. Its formal UUID
    // is opaque here; the trusted recorder binds it and producer restore
    // rejects substitution in the returned publication manifest.
    b.run_id = "d0000000-0000-0000-0000-000000000003".into();
    b.verify_projection(Some(p)).unwrap();
    b.engine_run_id = b.run_id.clone();
    assert!(b.verify_projection(Some(p)).is_err());
}

#[test]
fn private_native_v3_requires_both_identities_and_refuses_v2_fallback() {
    let mut b = binding();
    b.validate().unwrap();
    b.scope = "PRIVATE_NATIVE_COMPUTE_V2".into();
    assert!(b.validate().is_err());
    b.scope = SCOPE.into();
    b.engine_run_id = "not-a-recorded-uuid".into();
    assert!(b.validate().is_err());
    let mut legacy = serde_json::to_value(binding()).unwrap();
    legacy.as_object_mut().unwrap().remove("engine_run_id");
    assert!(serde_json::from_value::<PrivateNativeBinding>(legacy).is_err());
}
fn prepared(root: &Path) -> (TaskExecution, PrivateNativeBinding) {
    let r = registry();
    let p = projection(root);
    let spec = TaskSpec {
        id: "native-task".into(),
        kind: "core/native_test_publisher".into(),
        spec: serde_json::json!({"output_directory":"/frozen/output","value":"toy"}),
        logical_node: None,
        axis: None,
        item_key: None,
        item: None,
        inputs: vec![],
        outputs: vec![TaskOutputBinding {
            name: "table".into(),
            port: 0,
            payload: "file".into(),
            artifact_paths: vec![],
        }],
    };
    let mut b = binding();
    b.projected_directories = p.destinations().clone();
    b.projection_identity_blake3 = Some(p.identity_blake3().unwrap());
    let mut ctx = r.ctx().clone();
    ctx.runtime_output_projection = Some(Arc::new(p));
    let e = TaskExecution {
        node: r.build_node_strict(&spec.kind, spec.spec.clone()).unwrap(),
        submission: TaskSubmission {
            spec,
            inputs: vec![],
            resources: b.budget.resources.clone(),
        },
        engine_ctx: Arc::new(ctx),
        reporter: NodeReporter::noop(),
        cancellation: CancellationToken::new(),
    };
    (e, b)
}
fn envelope(e: &TaskExecution) -> TypedWorkerEnvelope {
    serde_json::from_value(e.submission.spec.spec["private_typed_dispatch"].clone()).unwrap()
}
fn downloaded(workspace: &Path, b: &PrivateNativeBinding) -> (PortOutputs, NodeRunDetails) {
    let f = FileRef::local(
        workspace.join("typed-outputs/port-0/00000000.tsv"),
        Some("tsv".into()),
    )
    .unwrap();
    let m = FileRef::local(
        workspace.join(format!(
            "typed-outputs/port-{}/publication.json",
            b.metadata_port
        )),
        Some("json".into()),
    )
    .unwrap();
    let mut outputs = PortOutputs::new();
    outputs.insert_file(0, f.clone());
    outputs.insert_file(b.metadata_port, m.clone());
    let details = NodeRunDetails {
        output_artifacts: vec![f.clone(), m.clone()],
        output_artifacts_by_port: [(0, vec![f]), (b.metadata_port, vec![m])].into(),
        ..Default::default()
    };
    (outputs, details)
}
#[test]
fn binding_is_closed_and_budget_is_complete() {
    let b = binding();
    b.validate().unwrap();
    let mut wire = serde_json::to_value(&b).unwrap();
    wire["allow_formal"] = true.into();
    assert!(serde_json::from_value::<PrivateNativeBinding>(wire).is_err());
    for mode in 0..8 {
        let mut bad = b.clone();
        match mode {
            0 => bad.scope = "FORMAL_ACCEPTED".into(),
            1 => bad.run_id = "other".into(),
            2 => bad.budget.resources.cpus = None,
            3 => bad.budget.disk_reserve_fraction = f64::NAN,
            4 => bad.engine_sha256 = "".into(),
            5 => bad.remote_user = "user; command".into(),
            6 => bad
                .projected_directories
                .insert("/frozen".into(), "/actual".into())
                .map(|_| ())
                .unwrap_or(()),
            _ => bad.budget.peak_disk_bytes = 0,
        };
        assert!(bad.validate().is_err(), "mode {mode}");
    }
}
#[test]
fn lowering_preserves_scientific_spec_and_separates_ordinary_entry() {
    let t = tempfile::tempdir().unwrap();
    let (mut e, b) = prepared(t.path());
    let original = e.submission.spec.spec.clone();
    lower_private_native_task(&mut e, &settings(), &b).unwrap();
    let env = envelope(&e);
    assert_eq!(env.task.spec, original);
    assert!(env.output_directories.is_empty());
    assert_eq!(e.submission.resources, b.budget.resources);
    assert_eq!(e.submission.spec.outputs.len(), 2);
    assert_eq!(e.submission.spec.outputs[1].port, 1);
    assert!(
        e.submission.spec.spec["script"]
            .as_str()
            .unwrap()
            .contains("delegated-native-compute")
    );
    let dispatch = TaskDispatch {
        spec: e.submission.spec,
        resources: e.submission.resources,
    };
    assert!(
        super::super::typed_worker::typed_worker_envelope(&dispatch)
            .unwrap_err()
            .to_string()
            .contains("compute-only")
    );
}

#[test]
fn publication_port_keeps_the_real_process_output_contract() {
    let mut worker = settings();
    worker.executable = std::env::current_exe().unwrap();
    worker.executable_sha256 = hash_file(&worker.executable).unwrap().1;
    for count in [0usize, 1, 2, 255] {
        let t = tempfile::tempdir().unwrap();
        let (mut e, mut b) = prepared(t.path());
        let template = e.submission.spec.outputs[0].clone();
        e.submission.spec.outputs = (0..count)
            .map(|port| TaskOutputBinding {
                name: format!("output_{port}"),
                port: port as u8,
                ..template.clone()
            })
            .collect();
        let scientific = e.submission.spec.outputs.clone();
        b.engine_sha256 = worker.executable_sha256.clone();
        b.metadata_port = PrivateNativeBinding::metadata_port_for_task(&e.submission.spec).unwrap();
        assert_eq!(usize::from(b.metadata_port), count);
        lower_private_native_task(&mut e, &worker, &b).unwrap();
        let process: super::super::ProcessTaskSpec =
            serde_json::from_value(e.submission.spec.spec.clone()).unwrap();
        process.validate().unwrap();
        assert_eq!(process.outputs.len(), count + 1);
        assert_eq!(
            serde_json::to_value(&e.submission.spec.outputs[..count]).unwrap(),
            serde_json::to_value(&scientific).unwrap()
        );
        let dispatch = TaskDispatch {
            spec: e.submission.spec,
            resources: e.submission.resources,
        };
        let (envelope, recovered) = private_native_worker_envelope(&dispatch).unwrap();
        assert_eq!(
            serde_json::to_value(&envelope.task.outputs).unwrap(),
            serde_json::to_value(&scientific).unwrap()
        );
        assert_eq!(recovered.metadata_port, b.metadata_port);
        let mut substituted = dispatch;
        let wrong_hash = if worker.executable_sha256 == "0".repeat(64) {
            "1".repeat(64)
        } else {
            "0".repeat(64)
        };
        substituted.spec.spec["private_typed_dispatch"]["worker"]["executable_sha256"] =
            serde_json::json!(wrong_hash);
        assert!(
            private_native_worker_envelope(&substituted)
                .unwrap_err()
                .to_string()
                .contains("running typed worker executable SHA256 mismatch")
        );
    }
}

#[test]
fn publication_port_rejects_sparse_reordered_duplicate_and_exhausted_outputs() {
    let t = tempfile::tempdir().unwrap();
    let (e, _) = prepared(t.path());
    for ports in [
        vec![1],
        vec![0, 2],
        vec![1, 0],
        vec![0, 0],
        (0..=255).collect(),
    ] {
        let mut task = e.submission.spec.clone();
        let template = task.outputs[0].clone();
        task.outputs = ports
            .into_iter()
            .map(|port| TaskOutputBinding {
                name: format!("output_{port}"),
                port,
                ..template.clone()
            })
            .collect();
        assert!(PrivateNativeBinding::metadata_port_for_task(&task).is_err());
    }
}

#[test]
fn unused_high_publication_port_is_rejected_before_transport() {
    let t = tempfile::tempdir().unwrap();
    let (mut e, mut b) = prepared(t.path());
    b.metadata_port = 255;
    assert!(lower_private_native_task(&mut e, &settings(), &b).is_err());
}

#[test]
fn actual_producer_projection_task_resource_and_port_changes_are_rejected() {
    let t = tempfile::tempdir().unwrap();
    let (e, b) = prepared(t.path());
    for mode in 0..7 {
        let mut altered = e.clone();
        let mut changed = b.clone();
        match mode {
            0 => changed.metadata_port = 0,
            1 => changed.task_id = "replacement".into(),
            2 => changed.spec_sha256 = "0".repeat(64),
            3 => changed.engine_run_id = "b0000000-0000-0000-0000-000000000002".into(),
            4 => changed.projected_directories.clear(),
            5 => altered.submission.resources.cpus = Some(2),
            _ => altered.engine_ctx = Arc::new(registry().ctx().clone()),
        };
        assert!(
            lower_private_native_task(&mut altered, &settings(), &changed).is_err(),
            "mode {mode}"
        );
    }
}
#[test]
fn ambient_plugins_or_credentials_cannot_enter_native_worker() {
    let t = tempfile::tempdir().unwrap();
    let (e, b) = prepared(t.path());
    for mode in 0..3 {
        let mut e = e.clone();
        let mut s = settings();
        match mode {
            0 => s.plugins_root = Some("/ambient/plugins".into()),
            1 => s.worker_vfs_manifest = Some("/ambient/vfs.json".into()),
            _ => s.executable_sha256 = "c".repeat(64),
        };
        assert!(lower_private_native_task(&mut e, &s, &b).is_err());
    }
}
#[tokio::test]
async fn native_publication_roundtrip_restores_original_names_and_keeps_manifest_evidence() {
    let t = tempfile::tempdir().unwrap();
    let (mut e, b) = prepared(t.path());
    lower_private_native_task(&mut e, &settings(), &b).unwrap();
    let workspace = t.path().join("remote");
    private_dir(&workspace).unwrap();
    let env = envelope(&e);
    execute_private_native_worker(&env, &b, &registry(), BTreeMap::new(), &workspace)
        .await
        .unwrap();
    assert_eq!(env.task.spec["output_directory"], "/frozen/output");
    let manifest: PublicationManifest = serde_json::from_slice(
        &fs::read(workspace.join("typed-outputs/port-1/publication.json")).unwrap(),
    )
    .unwrap();
    assert!(manifest.scratch_removed);
    assert_eq!(
        manifest.scratch_directory.parent(),
        Some(b.compute_scratch_root.as_path())
    );
    assert!(!manifest.scratch_directory.exists());
    let (outputs, mut details) = downloaded(&workspace, &b);
    let out = restore_private_native_outputs(
        &b,
        e.engine_ctx.runtime_output_projection.as_deref(),
        outputs,
        &mut details,
    )
    .unwrap();
    let f = out.get(&0).unwrap().as_file().unwrap();
    let expected = b.projected_directories[Path::new("/frozen/output")].join("nested/value.tsv");
    assert_eq!(Path::new(&f.path), expected);
    assert_eq!(fs::read(&expected).unwrap(), b"toy");
    assert_eq!(out.len(), 1);
    assert!(!f.fingerprint.as_ref().unwrap().immutable_remote);
    assert!(!details.output_artifacts_by_port.contains_key(&1));
    assert_eq!(details.output_artifacts.len(), 2);
}
#[tokio::test]
async fn tampered_download_is_rejected_before_publishing_any_file() {
    let t = tempfile::tempdir().unwrap();
    let (mut e, b) = prepared(t.path());
    lower_private_native_task(&mut e, &settings(), &b).unwrap();
    let w = t.path().join("remote");
    private_dir(&w).unwrap();
    execute_private_native_worker(&envelope(&e), &b, &registry(), BTreeMap::new(), &w)
        .await
        .unwrap();
    let (outputs, mut details) = downloaded(&w, &b);
    fs::write(w.join("typed-outputs/port-0/00000000.tsv"), b"different").unwrap();
    assert!(
        restore_private_native_outputs(
            &b,
            e.engine_ctx.runtime_output_projection.as_deref(),
            outputs,
            &mut details
        )
        .is_err()
    );
    assert!(!b.projected_directories[Path::new("/frozen/output")].exists());
}
#[tokio::test]
async fn preexisting_result_is_preserved_and_not_treated_as_a_retry() {
    let t = tempfile::tempdir().unwrap();
    let (mut e, b) = prepared(t.path());
    lower_private_native_task(&mut e, &settings(), &b).unwrap();
    let w = t.path().join("remote");
    private_dir(&w).unwrap();
    execute_private_native_worker(&envelope(&e), &b, &registry(), BTreeMap::new(), &w)
        .await
        .unwrap();
    let target = b.projected_directories[Path::new("/frozen/output")].join("nested/value.tsv");
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(&target, b"existing").unwrap();
    let (outputs, mut details) = downloaded(&w, &b);
    assert!(
        restore_private_native_outputs(
            &b,
            e.engine_ctx.runtime_output_projection.as_deref(),
            outputs,
            &mut details
        )
        .is_err()
    );
    assert_eq!(fs::read(&target).unwrap(), b"existing");
}
#[tokio::test]
async fn changed_logical_publisher_and_parent_traversal_manifest_are_refused() {
    for mode in 0..5 {
        let t = tempfile::tempdir().unwrap();
        let (mut e, b) = prepared(t.path());
        lower_private_native_task(&mut e, &settings(), &b).unwrap();
        let w = t.path().join("remote");
        private_dir(&w).unwrap();
        execute_private_native_worker(&envelope(&e), &b, &registry(), BTreeMap::new(), &w)
            .await
            .unwrap();
        let m = w.join("typed-outputs/port-1/publication.json");
        let mut value: PublicationManifest =
            serde_json::from_slice(&fs::read(&m).unwrap()).unwrap();
        match mode {
            0 => value.files[0].relative_path = "../escape.tsv".into(),
            1 => value.files[0].logical_directory = Some("/other".into()),
            2 => value.binding.contract_sha256 = "e".repeat(64),
            3 => value.binding.run_id = "d0000000-0000-0000-0000-000000000003".into(),
            _ => value.binding.engine_run_id = "d0000000-0000-0000-0000-000000000003".into(),
        };
        fs::write(m, serde_json::to_vec(&value).unwrap()).unwrap();
        let (outputs, mut details) = downloaded(&w, &b);
        assert!(
            restore_private_native_outputs(
                &b,
                e.engine_ctx.runtime_output_projection.as_deref(),
                outputs,
                &mut details
            )
            .is_err()
        );
        assert!(!b.projected_directories[Path::new("/frozen/output")].exists());
    }
}
#[test]
fn aliased_or_replaced_producer_directory_stays_blocked() {
    let t = tempfile::tempdir().unwrap();
    let (e, b) = prepared(t.path());
    let p = e.engine_ctx.runtime_output_projection.as_ref().unwrap();
    fs::set_permissions(
        p.staging_root().join(ENGINE_RUN),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    assert!(b.verify_projection(Some(p)).is_err());
}

#[tokio::test]
async fn transport_suffix_normalization_does_not_change_the_producer_format_contract() {
    for format in [serde_json::Value::Null, serde_json::json!("TSV")] {
        let t = tempfile::tempdir().unwrap();
        let (mut e, mut b) = prepared(t.path());
        e.submission.spec.spec["format"] = format.clone();
        e.node = registry()
            .build_node_strict(&e.submission.spec.kind, e.submission.spec.spec.clone())
            .unwrap();
        b.spec_sha256 = spec_hash(&e.submission.spec.spec).unwrap();
        lower_private_native_task(&mut e, &settings(), &b).unwrap();
        let w = t.path().join("remote");
        private_dir(&w).unwrap();
        execute_private_native_worker(&envelope(&e), &b, &registry(), BTreeMap::new(), &w)
            .await
            .unwrap();
        let (outputs, mut details) = downloaded(&w, &b);
        let result = restore_private_native_outputs(
            &b,
            e.engine_ctx.runtime_output_projection.as_deref(),
            outputs,
            &mut details,
        )
        .unwrap();
        assert_eq!(
            result.get(&0).unwrap().as_file().unwrap().format,
            format.as_str().map(str::to_owned)
        );
    }
}

#[test]
fn explicit_scratch_binding_rejects_legacy_and_relative_placement() {
    let value = binding();
    value.validate().unwrap();
    let mut old = value.clone();
    old.scope = "PRIVATE_NATIVE_COMPUTE_V1".into();
    assert!(old.validate().is_err());
    let mut relative = value;
    relative.compute_scratch_root = "relative/scratch".into();
    assert!(relative.validate().is_err());
}

#[test]
fn scratch_capability_probe_keeps_existing_unknown_target_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join(".atomic-target");
    private_dir(&target).unwrap();
    fs::write(target.join("marker"), b"existing bytes").unwrap();
    assert!(prove_atomic_directory_publication(temp.path()).is_err());
    assert_eq!(fs::read(target.join("marker")).unwrap(), b"existing bytes");
}

#[tokio::test]
async fn scratch_cleanup_evidence_is_bound_before_producer_publication() {
    for mode in 0..2 {
        let t = tempfile::tempdir().unwrap();
        let (mut e, b) = prepared(t.path());
        lower_private_native_task(&mut e, &settings(), &b).unwrap();
        let w = t.path().join("remote");
        private_dir(&w).unwrap();
        execute_private_native_worker(&envelope(&e), &b, &registry(), BTreeMap::new(), &w)
            .await
            .unwrap();
        let path = w.join("typed-outputs/port-1/publication.json");
        let mut manifest: PublicationManifest =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        if mode == 0 {
            manifest.scratch_removed = false;
        } else {
            manifest.scratch_directory = "/unapproved/scratch".into();
        }
        fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let (outputs, mut details) = downloaded(&w, &b);
        assert!(
            restore_private_native_outputs(
                &b,
                e.engine_ctx.runtime_output_projection.as_deref(),
                outputs,
                &mut details
            )
            .is_err()
        );
        assert!(!b.projected_directories[Path::new("/frozen/output")].exists());
    }
}
