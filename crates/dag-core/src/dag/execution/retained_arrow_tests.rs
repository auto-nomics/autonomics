use super::*;

use arrow::{
    array::{Int64Array, StringArray},
    datatypes::{DataType, Field, Schema},
    ipc::writer::FileWriter,
    record_batch::RecordBatch,
};
use datafusion::{
    execution::object_store::ObjectStoreUrl,
    prelude::{SessionContext, col, lit},
};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio_util::sync::CancellationToken;

use crate::dag::{
    ChannelNode, ChannelOperator,
    execution::{RemoteTaskExecutor, workspace::LocalTaskWorkspace},
    node_event::{JobResult, NodeReporter},
};
use crate::{
    FileFingerprint, NodeInput, NodeValue, TaskArtifactStore, TaskAttemptReceipt, TaskDispatch,
    TaskExecution, TaskExecutor, TaskInputBinding, TaskInputSource, TaskLease, TaskOutputBinding,
    TaskResources, TaskSpec, TaskSubmission, TaskTransport,
};

fn arrow_file(path: &Path, schema_only: bool) {
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("label", DataType::Utf8, false),
    ]));
    let file = std::fs::File::create(path).unwrap();
    let mut writer = FileWriter::try_new(file, schema.as_ref()).unwrap();
    if !schema_only {
        writer
            .write(
                &RecordBatch::try_new(
                    schema,
                    vec![
                        Arc::new(Int64Array::from(vec![7, 9])),
                        Arc::new(StringArray::from(vec!["seven", "nine"])),
                    ],
                )
                .unwrap(),
            )
            .unwrap();
    }
    writer.finish().unwrap();
}

fn aligned_ipc_file(alignment: usize, schema_only: bool) -> Vec<u8> {
    let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
    let options = arrow::ipc::writer::IpcWriteOptions::try_new(
        alignment,
        false,
        arrow::ipc::MetadataVersion::V5,
    )
    .unwrap();
    let mut bytes = vec![];
    {
        let mut writer =
            FileWriter::try_new_with_options(&mut bytes, schema.as_ref(), options).unwrap();
        if !schema_only {
            writer
                .write(
                    &RecordBatch::try_new(schema, vec![Arc::new(Int64Array::from(vec![7; 65536]))])
                        .unwrap(),
                )
                .unwrap();
        }
        writer.finish().unwrap();
    }
    bytes
}

async fn recorded_schema_reads(bytes: Vec<u8>) -> (Result<SchemaRef, DagError>, Vec<Range<u64>>) {
    let bytes = Arc::new(bytes);
    let reads = Arc::new(Mutex::new(vec![]));
    let result = read_ipc_file_schema(bytes.len() as u64, |range| {
        let bytes = bytes.clone();
        let reads = reads.clone();
        async move {
            reads.lock().unwrap().push(range.clone());
            Ok(bytes[range.start as usize..range.end as usize].to_vec())
        }
    })
    .await;
    let recorded = reads.lock().unwrap().clone();
    (result, recorded)
}

#[tokio::test]
async fn footer_schema_reads_are_bounded_and_independent_of_ipc_alignment_and_rows() {
    for alignment in [8, 16, 32, 64] {
        for schema_only in [false, true] {
            let bytes = aligned_ipc_file(alignment, schema_only);
            let size = bytes.len() as u64;
            // The schema message starts after alignment padding, not always
            // the eight bytes assumed by DataFusion's Stream inference.
            assert_eq!(&bytes[alignment..alignment + 4], &[0xff; 4]);
            let footer_size = arrow::ipc::reader::read_footer_length(
                bytes[bytes.len() - 10..].try_into().unwrap(),
            )
            .unwrap() as u64;
            let (schema, reads) = recorded_schema_reads(bytes).await;
            let schema = schema.unwrap();
            assert_eq!(schema.fields().len(), 1);
            assert_eq!(schema.field(0).name(), "id");
            assert_eq!(schema.field(0).data_type(), &DataType::Int64);
            assert_eq!(
                reads,
                vec![0..6, size - 10..size, size - 10 - footer_size..size - 10]
            );
            assert_eq!(
                reads.iter().map(|r| r.end - r.start).sum::<u64>(),
                16 + footer_size
            );
            if !schema_only {
                assert!(size > 512 * 1024);
                assert!(
                    16 + footer_size < 4096,
                    "metadata must not fetch row bodies"
                );
            }
        }
    }
}

#[tokio::test]
async fn footer_schema_rejects_malformed_and_unbounded_metadata_without_reading_rows() {
    let original = aligned_ipc_file(64, false);
    let mut no_magic = original.clone();
    no_magic[0] = 0;
    let (error, reads) = recorded_schema_reads(no_magic).await;
    assert!(
        error
            .unwrap_err()
            .to_string()
            .contains("missing file magic")
    );
    assert_eq!(reads, vec![0..6]);

    let mut oversized = original.clone();
    let trailer_start = oversized.len() - 10;
    oversized[trailer_start..trailer_start + 4]
        .copy_from_slice(&((MAX_IPC_FOOTER_BYTES + 1) as i32).to_le_bytes());
    let (error, reads) = recorded_schema_reads(oversized).await;
    assert!(error.unwrap_err().to_string().contains("metadata budget"));
    assert_eq!(reads.len(), 2, "reject before requesting a huge footer");

    let mut overlapping = original.clone();
    overlapping[trailer_start..trailer_start + 4]
        .copy_from_slice(&(trailer_start as i32).to_le_bytes());
    let (error, reads) = recorded_schema_reads(overlapping).await;
    assert!(error.unwrap_err().to_string().contains("overlaps"));
    assert_eq!(reads.len(), 2);

    let mut bad_footer = original.clone();
    let footer_size =
        arrow::ipc::reader::read_footer_length(original[trailer_start..].try_into().unwrap())
            .unwrap();
    bad_footer[trailer_start - footer_size..trailer_start].fill(0xff);
    let (error, reads) = recorded_schema_reads(bad_footer).await;
    assert!(error.unwrap_err().to_string().contains("malformed footer"));
    assert_eq!(reads.len(), 3);

    let (error, reads) = recorded_schema_reads(b"ARROW1".to_vec()).await;
    assert!(error.unwrap_err().to_string().contains("too short"));
    assert!(reads.is_empty());
}

#[tokio::test]
async fn footer_schema_rejects_short_range_responses() {
    let bytes = Arc::new(aligned_ipc_file(64, false));
    for truncate_trailer in [true, false] {
        let error = read_ipc_file_schema(bytes.len() as u64, |range| {
            let bytes = bytes.clone();
            async move {
                let mut result = bytes[range.start as usize..range.end as usize].to_vec();
                if (truncate_trailer && range.end - range.start == 10)
                    || (!truncate_trailer && range.start > 6 && range.end - range.start > 10)
                {
                    result.pop();
                }
                Ok(result)
            }
        })
        .await
        .unwrap_err();
        assert!(error.to_string().contains(if truncate_trailer {
            "truncated file trailer"
        } else {
            "truncated file footer"
        }));
    }
}

fn fixture(schema_only: bool) -> (tempfile::TempDir, NodeCtx, FileRef) {
    let temp = tempfile::tempdir().unwrap();
    arrow_file(&temp.path().join("retained"), schema_only);
    // The toy cluster copy lets resolver checks read the remote source even
    // when a test removes the coordinator-visible copy to prohibit collection.
    std::fs::copy(
        temp.path().join("retained"),
        temp.path().join("cluster-retained"),
    )
    .unwrap();
    let storage = Arc::new(vfs::OpendalFileStorage::new(temp.path()));
    let session = SessionContext::new();
    session.runtime_env().register_object_store(
        ObjectStoreUrl::parse("vfs://").unwrap().as_ref(),
        storage.clone(),
    );
    let ctx = NodeCtx::new(session.runtime_env(), Some(storage));
    let mut origin = FileRef::new("vfs:///retained", Some("arrow".into()));
    origin.fingerprint = Some(FileFingerprint::remote(
        std::fs::metadata(temp.path().join("retained"))
            .unwrap()
            .len(),
        Some(format!(
            "sha256:{:x}",
            Sha256::digest(std::fs::read(temp.path().join("retained")).unwrap())
        )),
    ));
    (temp, ctx, origin)
}

fn submission(frame: DataFrame) -> TaskSubmission {
    TaskSubmission {
        spec: TaskSpec {
            id: "retained-scan".into(),
            logical_node: None,
            kind: "core/sql".into(),
            spec: serde_json::json!({}),
            axis: None,
            item_key: None,
            item: None,
            inputs: vec![TaskInputBinding {
                name: "table".into(),
                port: 0,
                source: TaskInputSource::UpstreamPort {
                    from: "upstream".into(),
                    from_port: 0,
                },
                payload: "dataframe".into(),
                path: None,
                fingerprint: None,
                staged_paths: vec![],
            }],
            outputs: vec![],
        },
        inputs: vec![NodeInput::new_dataframe(0, frame)],
        resources: TaskResources::default(),
    }
}

#[derive(Clone)]
struct RetainedStore {
    origin: FileRef,
    manifest: FileRef,
    cluster_file: PathBuf,
    remote_resolves: Arc<AtomicUsize>,
    artifact_downloads: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl TaskArtifactStore for RetainedStore {
    async fn remote_input(
        &self,
        artifact: FileRef,
        _: Option<&vfs::OpendalFileStorage>,
    ) -> Result<Option<FileRef>, DagError> {
        assert_eq!(artifact.path, self.origin.path);
        assert_eq!(artifact.fingerprint, self.origin.fingerprint);
        self.remote_resolves.fetch_add(1, Ordering::SeqCst);
        let bytes = std::fs::read(&self.cluster_file)
            .map_err(|error| DagError::Schedule(error.to_string()))?;
        let recorded = artifact
            .fingerprint
            .as_ref()
            .ok_or_else(|| DagError::Schedule("missing expected identity".into()))?;
        if recorded.size != bytes.len() as u64
            || recorded.content_hash.as_deref()
                != Some(format!("sha256:{:x}", Sha256::digest(&bytes)).as_str())
        {
            return Err(DagError::Schedule(
                "retained toy cluster source SHA256 mismatch".into(),
            ));
        }
        let mut remote = artifact;
        remote.path = "/cluster/retained".into();
        Ok(Some(remote))
    }
    async fn retained_reference(&self, artifact: FileRef) -> Result<Option<FileRef>, DagError> {
        assert_eq!(artifact.path, "/cluster/retained");
        Ok(Some(self.origin.clone()))
    }
    async fn upload(&self, _: FileRef, _: &str, _: &str) -> Result<FileRef, DagError> {
        panic!("retained Arrow must not be uploaded");
    }
    async fn download(&self, _: FileRef, _: &str, name: &str) -> Result<FileRef, DagError> {
        if name != "task.json" {
            self.artifact_downloads.fetch_add(1, Ordering::SeqCst);
            panic!("retained Arrow must not be downloaded");
        }
        Ok(self.manifest.clone())
    }
}

fn store(temp: &Path, origin: FileRef) -> RetainedStore {
    let manifest = temp.join("remote-task.json");
    std::fs::write(&manifest, b"{}").unwrap();
    RetainedStore {
        origin,
        manifest: FileRef::local(manifest, Some("json".into())).unwrap(),
        cluster_file: temp.join("cluster-retained"),
        remote_resolves: Arc::new(AtomicUsize::new(0)),
        artifact_downloads: Arc::new(AtomicUsize::new(0)),
    }
}

#[tokio::test]
async fn zero_batch_ipc_keeps_its_schema_for_local_and_retained_outputs() {
    let (temp, ctx, origin) = fixture(true);
    let local = FileRef::local(temp.path().join("retained"), Some("arrow".into())).unwrap();
    for artifact in [&local, &origin] {
        let value =
            RemoteTaskExecutor::load_output(std::slice::from_ref(artifact), "dataframe", &ctx)
                .await
                .unwrap();
        let NodeValue::DataFrame(frame) = value else {
            panic!("expected dataframe")
        };
        assert_eq!(frame.schema().fields().len(), 2);
        assert_eq!(frame.schema().field(0).name(), "id");
        if artifact.path.starts_with("vfs://") {
            assert_eq!(
                serde_json::to_value(pure_retained_arrow_origin(&frame)).unwrap(),
                serde_json::to_value(&origin).unwrap()
            );
            assert!(
                frame
                    .count()
                    .await
                    .unwrap_err()
                    .to_string()
                    .contains("require remote placement")
            );
        } else {
            assert_eq!(frame.count().await.unwrap(), 0);
        }
    }
}

#[tokio::test]
async fn retained_arrow_stage_forwards_origin_without_reading_rows_or_writing_a_copy() {
    let (temp, ctx, origin) = fixture(false);
    let frame = load_retained_arrow(&origin, &ctx).await.unwrap();
    assert_eq!(
        serde_json::to_value(pure_retained_arrow_origin(&frame)).unwrap(),
        serde_json::to_value(&origin).unwrap()
    );
    // Any attempt to collect this lazy table from now on fails. Successful
    // staging proves that the coordinator did not read its rows.
    std::fs::remove_file(temp.path().join("retained")).unwrap();
    let mut task = submission(frame);
    let store = store(temp.path(), origin);
    let mut workspace = LocalTaskWorkspace::create(&temp.path().join("attempts"), &task).unwrap();
    let resolved = workspace
        .stage_inputs_with_remote_observed(&mut task, ctx.opendal.as_deref(), Some(&store), None)
        .await
        .unwrap();
    assert_eq!(resolved[&(0, 0)].path, "/cluster/retained");
    assert_eq!(task.spec.inputs[0].payload, "dataframe");
    assert_eq!(task.spec.inputs[0].staged_paths, vec!["/cluster/retained"]);
    assert_eq!(store.remote_resolves.load(Ordering::SeqCst), 1);
    for attempt in std::fs::read_dir(temp.path().join("attempts")).unwrap() {
        assert_eq!(
            std::fs::read_dir(attempt.unwrap().path().join("inputs"))
                .unwrap()
                .count(),
            0
        );
    }
}

#[tokio::test]
async fn user_tables_and_transformed_retained_tables_cannot_claim_an_unchanged_origin() {
    let (temp, ctx, origin) = fixture(false);
    let frame = load_retained_arrow(&origin, &ctx).await.unwrap();
    for variant in 0..3 {
        let (state, plan) = frame.clone().into_parts();
        let LogicalPlan::TableScan(mut scan) = plan else {
            panic!("retained source should be a scan")
        };
        match variant {
            0 => scan.projection = Some(vec![0]),
            1 => scan.filters = vec![col("id").gt(lit(7i64))],
            _ => scan.fetch = Some(1),
        }
        assert!(
            pure_retained_arrow_origin(&DataFrame::new(state, LogicalPlan::TableScan(scan)))
                .is_none()
        );
    }
    assert!(pure_retained_arrow_origin(&frame.clone().select_columns(&["id"]).unwrap()).is_none());
    assert!(
        pure_retained_arrow_origin(&frame.clone().filter(col("id").gt(lit(7i64))).unwrap())
            .is_none()
    );
    assert!(pure_retained_arrow_origin(&frame.clone().limit(0, Some(1)).unwrap()).is_none());
    let user = ctx
        .session()
        .read_arrow(
            &origin.path,
            ArrowReadOptions {
                // User-owned tables still receive no retained provenance.
                // Use the known schema to avoid DataFusion's 8-byte-alignment
                // assumption when inferring a 64-byte-aligned file stream.
                schema: Some(frame.schema().as_arrow()),
                file_extension: "",
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(pure_retained_arrow_origin(&user).is_none());
    let store = store(temp.path(), origin);
    let mut task = submission(frame.select_columns(&["id"]).unwrap());
    let mut workspace = LocalTaskWorkspace::create(&temp.path().join("attempts"), &task).unwrap();
    let rejected = workspace
        .stage_inputs_with_remote_observed(&mut task, ctx.opendal.as_deref(), Some(&store), None)
        .await
        .unwrap_err();
    assert!(rejected.to_string().contains("require remote placement"));
    assert_eq!(store.remote_resolves.load(Ordering::SeqCst), 0);
    // An unrelated user-created table has no retained provenance and keeps
    // the existing explicit local-origin staging behavior.
    let mut task = submission(user.select_columns(&["id"]).unwrap());
    let mut workspace =
        LocalTaskWorkspace::create(&temp.path().join("local-attempts"), &task).unwrap();
    assert!(
        workspace
            .stage_inputs_with_remote_observed(
                &mut task,
                ctx.opendal.as_deref(),
                Some(&store),
                None
            )
            .await
            .unwrap()
            .is_empty()
    );
    let file = std::fs::File::open(&task.spec.inputs[0].staged_paths[0]).unwrap();
    let reader = arrow::ipc::reader::FileReader::try_new(file, None).unwrap();
    assert_eq!(reader.schema().fields().len(), 1);
    assert_eq!(
        reader.map(|batch| batch.unwrap().num_rows()).sum::<usize>(),
        2
    );
}

#[tokio::test]
async fn changed_retained_bytes_cannot_be_scanned_locally_or_forwarded_under_the_old_hash() {
    let (temp, ctx, origin) = fixture(false);
    let frame = load_retained_arrow(&origin, &ctx).await.unwrap();
    arrow_file(&temp.path().join("retained"), true);
    arrow_file(&temp.path().join("cluster-retained"), true);
    let error = frame.clone().collect().await.unwrap_err();
    assert!(error.to_string().contains("require remote placement"));
    let LogicalPlan::TableScan(scan) = frame.logical_plan() else {
        panic!("expected retained scan")
    };
    let provider = source_as_provider(&scan.source).unwrap();
    let session = ctx.session();
    let state = session.state();
    assert!(
        provider
            .scan(&state, None, &[], None)
            .await
            .unwrap_err()
            .to_string()
            .contains("require remote placement")
    );
    assert!(
        provider
            .scan_with_args(&state, ScanArgs::default())
            .await
            .unwrap_err()
            .to_string()
            .contains("require remote placement")
    );
    let store = store(temp.path(), origin);
    let mut task = submission(frame);
    let mut workspace = LocalTaskWorkspace::create(&temp.path().join("attempts"), &task).unwrap();
    let error = workspace
        .stage_inputs_with_remote_observed(&mut task, ctx.opendal.as_deref(), Some(&store), None)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("SHA256 mismatch"));
    assert_eq!(store.remote_resolves.load(Ordering::SeqCst), 1);
}

struct OutputTransport {
    manifest: FileRef,
}
#[async_trait::async_trait]
impl TaskTransport for OutputTransport {
    async fn submit(
        &self,
        dispatch: TaskDispatch,
        artifacts: Vec<FileRef>,
    ) -> Result<TaskLease, DagError> {
        assert!(artifacts.is_empty());
        Ok(TaskLease {
            lease_id: "fixture".into(),
            task_id: dispatch.spec.id,
        })
    }
    async fn wait(&self, lease: &TaskLease) -> Result<TaskAttemptReceipt, DagError> {
        let output = FileRef::new("/cluster/retained", Some("arrow".into()));
        Ok(TaskAttemptReceipt {
            stdout_log: None,
            stderr_log: None,
            log_capture_error: None,
            task_id: lease.task_id.clone(),
            executor: "fixture".into(),
            status: "success".into(),
            elapsed_ms: 0,
            exit_code: 0,
            workspace: "/cluster/fixture".into(),
            task_manifest: self.manifest.clone(),
            output_artifacts: vec![output.clone()],
            output_artifacts_by_port: BTreeMap::from([(0, vec![output])]),
        })
    }
    async fn cancel(&self, _: &TaskLease) -> Result<(), DagError> {
        Ok(())
    }
}

#[tokio::test]
async fn remote_dataframe_completion_retains_the_artifact_instead_of_downloading_it() {
    let (temp, ctx, origin) = fixture(false);
    let store = Arc::new(store(temp.path(), origin.clone()));
    let executor = RemoteTaskExecutor::new(
        Arc::new(OutputTransport {
            manifest: store.manifest.clone(),
        }),
        store.clone(),
        temp.path().join("coordinator"),
    );
    let spec = TaskSpec {
        id: "retained-output".into(),
        logical_node: None,
        kind: "process".into(),
        spec: serde_json::json!({}),
        axis: None,
        item_key: None,
        item: None,
        inputs: vec![],
        outputs: vec![TaskOutputBinding {
            name: "table".into(),
            port: 0,
            payload: "dataframe".into(),
            artifact_paths: vec![],
        }],
    };
    let execution = TaskExecution {
        submission: TaskSubmission {
            spec,
            inputs: vec![],
            resources: TaskResources::default(),
        },
        node: Box::new(ChannelNode::new(ChannelOperator::OfItems { items: vec![] })),
        engine_ctx: Arc::new(ctx),
        reporter: NodeReporter::noop(),
        cancellation: CancellationToken::new(),
    };
    let JobResult::Success { outputs, .. } = executor.run(execution).await else {
        panic!("remote fixture should succeed")
    };
    let NodeValue::DataFrame(frame) = &outputs[&0] else {
        panic!("expected dataframe")
    };
    assert_eq!(
        serde_json::to_value(pure_retained_arrow_origin(frame)).unwrap(),
        serde_json::to_value(&origin).unwrap()
    );
    assert_eq!(store.artifact_downloads.load(Ordering::SeqCst), 0);
}
