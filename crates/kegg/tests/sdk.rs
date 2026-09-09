use std::time::Instant;
use std::{collections::VecDeque, sync::Arc};

use kegg::{KeggClient, convert::entry_table, kegg_registrations, parser};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[test]
fn parses_list_find_conv_link_and_ddi_rows() {
    let entries =
        parser::entry_summaries("hsa:10458\tBAIAP2 adapter protein\n\nko:K05627\tBAIAP2\n");
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].id, "hsa:10458");
    assert_eq!(entries[1].description, "BAIAP2");

    let pairs = parser::pairs("hsa:10458\tpath:hsa04517\n");
    assert_eq!(pairs[0].target, "path:hsa04517");

    let ddi = parser::drug_interactions("dr:D00564\tdr:D00100\tP\tincreased toxicity\n");
    assert_eq!(ddi[0].category, "P");
    assert_eq!(ddi[0].description, "increased toxicity");
}

#[test]
fn parses_info_database_counts_and_linked_databases() {
    let info = parser::info(
        "kegg\tKEGG integrated database\n\
         \tpathway\t587\t2026/09/03\n\
         \tcompound\t19,632\t2026/09/07\n\
         \n\
         linked db\n\
         \tpubmed\n",
    );
    assert_eq!(info.title, "kegg\tKEGG integrated database");
    assert_eq!(info.databases.len(), 2);
    assert_eq!(info.databases[0].database, "pathway");
    assert_eq!(info.databases[0].entry_count, Some(587));
    assert_eq!(info.databases[1].entry_count, Some(19_632));
    assert_eq!(info.linked_databases, ["pubmed"]);
}

#[test]
fn parses_flat_file_entry_header() {
    let raw = "ENTRY       10458             CDS       T01001\nNAME        BAIAP2\n";
    let entry = parser::flat_entry(raw);
    assert_eq!(entry.id, "10458");
    assert_eq!(entry.entry_type.as_deref(), Some("CDS"));
    assert_eq!(entry.organism.as_deref(), Some("T01001"));
}

#[test]
fn tables_have_stable_dag_friendly_columns() {
    let entries = vec![kegg::EntrySummary {
        id: "hsa:10458".into(),
        description: "BAIAP2".into(),
    }];
    let table = entry_table(&entries);
    assert_eq!(table.columns, ["id", "description"]);
    assert_eq!(table.len(), 1);
}

#[test]
fn registrations_have_unique_schemas() {
    let registrations = kegg_registrations(Arc::new(KeggClient::new()));
    assert_eq!(registrations.len(), 6);
    let mut names: Vec<_> = registrations
        .iter()
        .map(|registration| registration.definition.name.as_str())
        .collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), registrations.len());
    assert!(
        registrations
            .iter()
            .all(|registration| { !registration.definition.input_schema.properties.is_empty() })
    );
}

async fn spawn_stub(responses: Vec<(&'static str, String)>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let responses: VecDeque<_> = responses.into_iter().collect();
    tokio::spawn(async move {
        for (content_type, body) in responses {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            loop {
                let read = socket.read(&mut buffer).await.unwrap();
                request.extend_from_slice(&buffer[..read]);
                if read == 0 || request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
            socket.write_all(body.as_bytes()).await.unwrap();
            socket.shutdown().await.unwrap();
        }
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn client_parses_text_over_local_rest_stub() {
    let endpoint = spawn_stub(vec![(
        "text/plain",
        "hsa:10458\tpath:hsa04517\n".to_string(),
    )])
    .await;
    let client = KeggClient::with_rate_limit(50)
        .unwrap()
        .with_endpoint(endpoint);
    let links = client.link("pathway", "hsa:10458").await.unwrap();
    assert_eq!(links[0].source, "hsa:10458");
    assert_eq!(links[0].target, "path:hsa04517");
}

#[tokio::test]
async fn client_enforces_configured_rate_limit() {
    let responses = vec![("text/plain", "id\tdescription\n".to_string()); 4];
    let endpoint = spawn_stub(responses).await;
    let client = KeggClient::with_rate_limit(50)
        .unwrap()
        .with_endpoint(endpoint);
    let start = Instant::now();
    let mut tasks = tokio::task::JoinSet::new();
    for index in 0..4 {
        let client = client.clone();
        tasks.spawn(async move {
            client
                .operation(&format!("list/database-{index}"))
                .await
                .unwrap();
        });
    }
    while let Some(task) = tasks.join_next().await {
        task.unwrap();
    }
    assert!(start.elapsed() >= std::time::Duration::from_millis(55));
}
