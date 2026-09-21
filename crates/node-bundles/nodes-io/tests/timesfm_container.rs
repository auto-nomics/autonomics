//! End-to-end test for the offline TimesFM container node.

use std::sync::Arc;

use container_runtime::{PanelCache, PodmanConfig, PodmanRuntime};
use dag_core::dag::runtime::SchedulerConfig;
use dag_core::registry::{NodeCtx, NodeRegistry};
use nodes_io::file_reference::FileReferenceNode;
use nodes_io::timesfm_container::{
    TIMESFM_FORECAST_CONTAINER_KIND, TimesfmForecastContainerNodeFactory,
};
use vfs::{
    BackendConfig, BackendDefinition, MountDefinition, MountedObjectStore, OpendalFileStorage,
    VfsManifest,
};

fn workspace_ctx(root: &std::path::Path) -> NodeCtx {
    let manifest = VfsManifest {
        backend: vec![BackendDefinition {
            id: "timesfm-test".into(),
            config: BackendConfig::local("/"),
        }],
        mount: vec![MountDefinition {
            path: "/".into(),
            backend: "timesfm-test".into(),
            source: root.to_string_lossy().into_owned(),
            read_only: false,
        }],
    };
    let mounted = Arc::new(MountedObjectStore::from_manifest(&manifest).unwrap());
    let storage = Arc::new(OpendalFileStorage::with_mounts(root, mounted.clone()));
    let session = datafusion::prelude::SessionContext::new();
    session.runtime_env().register_object_store(
        datafusion::execution::object_store::ObjectStoreUrl::parse("vfs://")
            .unwrap()
            .as_ref(),
        mounted,
    );
    NodeCtx::new(session.runtime_env(), Some(storage))
}

async fn read_text(ctx: &NodeCtx, path: &str) -> String {
    let storage = ctx.opendal.as_ref().expect("test VFS storage");
    let virtual_path = path.strip_prefix("vfs://").expect("VFS URI");
    let bytes = storage
        .resolve(virtual_path)
        .read(&storage.resolve_path(virtual_path))
        .await
        .unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
#[ignore = "requires Podman and the pinned TimesFM GHCR image"]
async fn real_official_timesfm_forecast_runs_in_podman() {
    unsafe {
        std::env::set_var(
            nodes_io::image_registry::IMAGE_PREFIX_ENV,
            std::env::var("AUTONOMICS_TIMESFM_IMAGE_PREFIX")
                .unwrap_or_else(|_| nodes_io::image_registry::DEFAULT_IMAGE_PREFIX.into()),
        );
    }

    let scratch = tempfile::tempdir().unwrap();
    let ctx = workspace_ctx(scratch.path());
    let workspace_root = scratch.path().join("podman-workspace");
    let panel_root = scratch.path().join("podman-panels");
    std::fs::create_dir_all(&workspace_root).unwrap();
    std::fs::create_dir_all(&panel_root).unwrap();
    let runtime: Arc<dyn container_runtime::PodmanConnection> =
        Arc::new(PodmanRuntime::new(PodmanConfig {
            program: std::env::var("AUTONOMICS_PODMAN_PROGRAM").unwrap_or_else(|_| "podman".into()),
            workspace_root,
            panel_cache_root: panel_root,
        }));
    let mut registry = NodeRegistry::new(ctx.clone());
    registry.register(Box::new(TimesfmForecastContainerNodeFactory::new(
        runtime,
        Arc::new(PanelCache::new(scratch.path().join("panels"))),
    )));
    let timesfm = registry
        .build_node(
            TIMESFM_FORECAST_CONTAINER_KIND,
            serde_json::json!({"horizon": 12, "max_context": 128}),
        )
        .unwrap();

    let input = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../containers/timesfm/fixtures/forecast_request.json");
    let mut dag = dag_core::dag::DAG::default();
    dag.add_node(
        "forecast_request".into(),
        Box::new(FileReferenceNode::new(
            input.to_string_lossy().into_owned(),
            Some("timesfm_forecast_request".into()),
        )),
    )
    .unwrap();
    dag.add_node("timesfm".into(), timesfm).unwrap();
    dag.add_edge("forecast_request", "timesfm", 0, 0).unwrap();
    let report = dag
        .run(&SchedulerConfig::default(), &ctx, None)
        .await
        .unwrap();
    assert_eq!(
        report.statuses.get("timesfm"),
        Some(&dag_core::dag::RuntimeStatus::Success),
        "TimesFM node failed: {report:#?}"
    );

    let outputs = dag.output("timesfm").unwrap();
    let result = outputs.get(&0).unwrap().as_file().unwrap().clone();
    let log = outputs.get(&1).unwrap().as_file().unwrap().clone();
    assert!(result.path.ends_with("/forecast_result.json"));
    assert!(log.path.ends_with("/forecast.log"));

    let result_text = read_text(&ctx, &result.path).await;
    let result_json: serde_json::Value = serde_json::from_str(&result_text).unwrap();
    assert_eq!(result_json["model"], "google/timesfm-2.5-200m-pytorch");
    assert_eq!(
        result_json["checkpoint_revision"],
        "1d952420fba87f3c6dee4f240de0f1a0fbc790e3"
    );
    assert_eq!(result_json["horizon"], 12);
    let forecast = result_json["forecast"].as_array().unwrap();
    assert_eq!(forecast.len(), 1);
    assert_eq!(forecast[0].as_array().unwrap().len(), 12);
    assert!(
        forecast[0]
            .as_array()
            .unwrap()
            .iter()
            .all(|value| value.as_f64().unwrap().is_finite())
    );
    let log_text = read_text(&ctx, &log.path).await;
    assert!(log_text.contains("TimesFM 2.5 forecast completed"));
}
