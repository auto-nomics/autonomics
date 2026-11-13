use super::*;
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{
        Condvar, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
struct Server {
    endpoint: String,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}
impl Server {
    fn new(handler: impl Fn(&str) -> (u16, String) + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let done = stop.clone();
        let handler = Arc::new(handler);
        let handle = std::thread::spawn(move || {
            let mut workers = Vec::new();
            while !done.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let handler = handler.clone();
                        workers.push(std::thread::spawn(move || {
                        stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                        let mut bytes=Vec::new();
                        loop {let mut buffer=[0;1024];let Ok(n)=stream.read(&mut buffer) else{return};if n==0{return}bytes.extend_from_slice(&buffer[..n]);if bytes.windows(4).any(|p|p==b"\r\n\r\n"){break}}
                        let request=String::from_utf8_lossy(&bytes);let path=request.lines().next().and_then(|line|line.split_whitespace().nth(1)).unwrap_or("");
                        let (status,body)=handler(path);
                        let _=write!(stream,"HTTP/1.1 {status} Response\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
                    }));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2))
                    }
                    Err(_) => break,
                }
            }
            for worker in workers {
                let _ = worker.join();
            }
        });
        Self {
            endpoint,
            stop,
            handle: Some(handle),
        }
    }
    fn tool(&self) -> ToolRegistration {
        ToolRegistration::from(EvidenceTool {
            client: Arc::new(
                reactome::ReactomeClient::new().with_endpoints(&self.endpoint, &self.endpoint),
            ),
        })
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[tokio::test]
async fn batch_is_concurrent_ordered_partial_and_identical_to_individual_provenance() {
    let gate = Arc::new((Mutex::new(0usize), Condvar::new()));
    let concurrent = Arc::new(AtomicBool::new(false));
    let signal = gate.clone();
    let observed = concurrent.clone();
    let record = json!({"dbId":1,"stId":"R-HSA-1","schemaClass":"ReactionLikeEvent","displayName":"Declared reaction","speciesName":"Homo sapiens","catalystActivity":[{"physicalEntity":{"displayName":"Declared catalyst"}}]});
    let raw = record.clone();
    let server = Server::new(move |path| {
        if path.starts_with("/data/query/") {
            let (lock, condition) = &*signal;
            let mut count = lock.lock().unwrap();
            *count += 1;
            condition.notify_all();
            let (count, _) = condition
                .wait_timeout_while(count, Duration::from_secs(2), |count| *count < 3)
                .unwrap();
            if *count < 3 {
                return (503, json!({"error":"queries were serialized"}).to_string());
            }
            observed.store(true, Ordering::SeqCst);
            drop(count);
            if path == "/data/query/R-HSA-2" {
                return (404, json!({"error":"missing fixture"}).to_string());
            }
            if path == "/data/query/R-HSA-1" {
                std::thread::sleep(Duration::from_millis(30));
                return (200, raw.to_string());
            }
            return (200,json!({"dbId":3,"stId":"R-HSA-3","schemaClass":"SimpleEntity","displayName":"Third"}).to_string());
        }
        (200,json!([[{"dbId":1,"stId":"R-HSA-1","schemaClass":"ReactionLikeEvent"},{"dbId":99,"stId":"R-HSA-99","schemaClass":"Pathway","displayName":"Declared pathway"}]]).to_string())
    });
    let tool = server.tool();
    let result = serde_json::to_value(
        tool.implementation
            .execute(json!({"action":"record_batch","ids":["R-HSA-1","R-HSA-2","R-HSA-3"]}))
            .await
            .unwrap(),
    )
    .unwrap();
    let batch = &result["content"];
    assert!(concurrent.load(Ordering::SeqCst));
    assert_eq!(batch["success_count"], 2);
    assert_eq!(batch["failure_count"], 1);
    assert_eq!(batch["all_requested_records_retrieved"], false);
    let entries = batch["results"].as_array().unwrap();
    assert_eq!(
        entries
            .iter()
            .map(|r| r["requested_id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["R-HSA-1", "R-HSA-2", "R-HSA-3"]
    );
    assert_eq!(entries[1]["status"], "error");
    assert!(entries[1].get("record").is_none());
    assert!(entries[1].get("source_url").is_none());
    assert_eq!(entries[0]["status"], "ok");
    assert_eq!(entries[0]["verification"], "not_verified");
    assert_eq!(
        entries[0]["record_json_sha256"],
        format!("{:x}", Sha256::digest(serde_json::to_vec(&record).unwrap()))
    );
    let single = serde_json::to_value(
        tool.implementation
            .execute(json!({"action":"record","id":"R-HSA-1"}))
            .await
            .unwrap(),
    )
    .unwrap();
    for key in [
        "provider",
        "verification",
        "record",
        "projection",
        "source_url",
        "retrieval_url",
        "record_json_sha256",
        "hash_scope",
        "source_context",
    ] {
        assert_eq!(entries[0][key], single["content"][key], "{key}");
    }
}

#[tokio::test]
async fn invalid_batch_is_rejected_before_any_network_request() {
    let count = Arc::new(AtomicUsize::new(0));
    let received = count.clone();
    let server = Server::new(move |_| {
        received.fetch_add(1, Ordering::SeqCst);
        (404, "{}".into())
    });
    let tool = server.tool();
    for input in [
        json!({"action":"record_batch","ids":[]}),
        json!({"action":"record_batch","ids":["R-HSA-1","R-HSA-1"]}),
        json!({"action":"record_batch","ids":["R-HSA-1","../secret"]}),
        json!({"action":"record_batch","ids":["1","2","3","4","5"]}),
        json!({"action":"record_batch","ids":["1"],"species":"Homo sapiens"}),
        json!({"action":"record_batch","ids":["1"],"id":"1"}),
        json!({"action":"record","id":"1","ids":["1"]}),
        json!({"action":"search","query":"test","ids":["1"]}),
    ] {
        assert!(tool.implementation.execute(input).await.is_err());
    }
    assert_eq!(count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn oversized_batch_record_is_explicitly_deferred_without_losing_individual_access() {
    let text = "x".repeat(7000);
    let record = json!({"dbId":1,"stId":"R-HSA-1","schemaClass":"SimpleEntity","summation":vec![json!({"text":text});40],"input":vec![json!({"displayName":text});40],"output":vec![json!({"displayName":text});40]});
    let server = Server::new(move |_| (200, record.to_string()));
    let tool = server.tool();
    let batch = serde_json::to_value(
        tool.implementation
            .execute(json!({"action":"record_batch","ids":["R-HSA-1"]}))
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(batch["content"]["success_count"], 0);
    assert_eq!(batch["content"]["failure_count"], 1);
    assert_eq!(batch["content"]["results"][0]["retry_individually"], true);
    assert!(batch["content"]["results"][0].get("record").is_none());
    let single = serde_json::to_value(
        tool.implementation
            .execute(json!({"action":"record","id":"R-HSA-1"}))
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(single["content"]["record"]["stId"], "R-HSA-1");
    assert_eq!(single["content"]["projection"]["complete"], false);
    assert!(serde_json::to_vec(&single["content"]).unwrap().len() > 512 * 1024);
}
