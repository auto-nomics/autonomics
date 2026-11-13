use reactome::ReactomeClient;
use serde_json::json;
use std::{
    io::{Read, Write},
    net::TcpListener,
};
#[tokio::test]
async fn ancestor_endpoint_preserves_multiple_event_to_root_paths() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0; 2048];
        let size = stream.read(&mut request).unwrap();
        assert!(
            String::from_utf8_lossy(&request[..size])
                .starts_with("GET /data/event/R-HSA-1/ancestors ")
        );
        let node = |id, title| json!({"dbId":id,"stId":format!("R-HSA-{id}"),"displayName":title,"schemaClass":"Pathway"});
        let body = json!([
            [node(1, "Event"), node(2, "First parent")],
            [node(1, "Event"), node(3, "Other parent")]
        ])
        .to_string();
        write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
    });
    let paths = ReactomeClient::new()
        .with_endpoints(&endpoint, &endpoint)
        .event_ancestors("R-HSA-1")
        .await
        .unwrap();
    assert_eq!(paths.len(), 2);
    assert_eq!(paths[0][1].display_name, "First parent");
    assert_eq!(paths[1][1].stable_id.as_deref(), Some("R-HSA-3"));
    server.join().unwrap();
}
