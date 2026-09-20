//! Live protocols.io integration checks. Set `PROTOCOLS_IO_ACCESS_TOKEN`.

use protocolio::{ContentFormat, ProtocolListQuery, ProtocolioClient};

#[tokio::test]
#[ignore = "requires a live protocols.io access token"]
async fn reads_protocol_metadata_steps_and_materials() {
    let client = ProtocolioClient::new().unwrap();
    let search = client
        .list_protocols(&ProtocolListQuery {
            filter: Some("public".into()),
            key: "PCR".into(),
            page_size: Some(1),
            ..Default::default()
        })
        .await
        .unwrap();
    let first = search
        .items
        .first()
        .unwrap_or_else(|| panic!("protocols.io returned no PCR result"));
    let identifier = first.uri.as_deref().unwrap();

    let protocol = client
        .protocol(identifier, false, ContentFormat::Markdown)
        .await
        .unwrap();
    assert!(protocol.summary.title.is_some());
    assert!(
        !client
            .steps(identifier, false, ContentFormat::Markdown)
            .await
            .unwrap()
            .is_empty()
    );
    client.materials(identifier).await.unwrap();
}
