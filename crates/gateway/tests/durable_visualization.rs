//! Long live evaluations retain isolated project records after host shutdown.
use gateway::testing::{renamed_mock_model, start_persistent_gateway_with_model};
use serde_json::json;
#[tokio::test]
async fn durable_evaluation_retains_project_records_and_refuses_existing_root() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("evaluation");
    let server =
        start_persistent_gateway_with_model(renamed_mock_model("unused", 8192), root.clone())
            .await
            .unwrap();
    let response = reqwest::Client::new()
        .post(format!(
            "http://{}/api/v1/visualization/projects",
            server.addr
        ))
        .bearer_auth(&server.token)
        .json(&json!({"id":"durable-project","title":"Reviewed study"}))
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    let document = json!({"elements":[],"files":{},"source":"durability fixture"});
    let figure: serde_json::Value = reqwest::Client::new()
        .post(format!(
            "http://{}/api/v1/visualization/projects/durable-project/figures",
            server.addr
        ))
        .bearer_auth(&server.token)
        .json(&json!({"title":"Figure","format":"excalidraw/v2","document":document}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let figure_id = figure["id"].as_str().unwrap();
    server.stop().await;
    let store = visualization::Store::open(root.join("state/visualizations.sqlite")).unwrap();
    assert_eq!(
        store
            .revision("durable-project", figure_id, 1)
            .unwrap()
            .document,
        document
    );
    let projects = store.projects().unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].id, "durable-project");
    let error =
        start_persistent_gateway_with_model(renamed_mock_model("unused", 8192), root.clone())
            .await
            .err()
            .expect("existing state must not be reused");
    assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(store.projects().unwrap()[0].title, "Reviewed study");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
}
