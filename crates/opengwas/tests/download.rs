use agentik_core::tools::ToolFunction;
use opengwas::tools::download::{DownloadFilesInput, DownloadFilesTool};
use std::sync::Arc;

use opengwas::OpengwasClient;
use vfs::OpendalFileStorage;

/// Integration test — hits the live OpenGWAS API and downloads to memory storage.
/// Requires `OPENGWAS_TOKEN` env var.
#[tokio::test]
#[ignore]
async fn test_download_ieu_a_2() {
    let client = Arc::new(OpengwasClient::new(None).expect("opengwas client"));
    let storage = Arc::new(OpendalFileStorage::new("/mnt/disk3/test"));
    let tool = DownloadFilesTool::new(client, storage.clone());

    let input = DownloadFilesInput {
        id: vec!["ieu-a-2".to_string()],
    };

    let input_value = serde_json::to_value(input).unwrap();
    let result = tool.execute(input_value).await.unwrap();

    dbg!(&result);

    let file_list = storage.resolve("/").list("/").await.unwrap();
    dbg!(&file_list);
    assert!(
        !file_list.is_empty(),
        "expected at least one file in storage"
    );
}
