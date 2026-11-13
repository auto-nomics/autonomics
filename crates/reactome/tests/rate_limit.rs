use reactome::{ReactomeClient, ReactomeError};
use serde_json::Value;
use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
    time::{Duration, Instant},
};
fn server(
    replies: Vec<(u16, Option<String>, &'static str)>,
) -> (String, thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let worker = thread::spawn(move || {
        let mut requests = Vec::new();
        for (status, retry, body) in replies {
            let deadline = Instant::now() + Duration::from_secs(15);
            let (mut stream, _) = loop {
                match listener.accept() {
                    Ok(value) => break value,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "expected request never arrived");
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(e) => panic!("{e}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0; 1024];
            while !bytes.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = stream.read(&mut buffer).unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buffer[..n]);
            }
            requests.push(String::from_utf8(bytes).unwrap());
            let retry = retry
                .map(|value| format!("Retry-After: {value}\r\n"))
                .unwrap_or_default();
            write!(stream,"HTTP/1.1 {status} Response\r\n{retry}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
        }
        requests
    });
    (format!("http://{address}"), worker)
}
#[tokio::test]
async fn json_reads_retry_429_with_identical_query_and_accept_header() {
    let (base, worker) = server(vec![
        (429, Some("0".into()), "limited"),
        (429, Some("0".into()), "limited"),
        (200, None, r#"{"ok":true}"#),
    ]);
    let client = ReactomeClient::new().with_endpoints(&base, &base);
    let value: Value = client
        .get("/query", &[("q", "JAK1/JAK2".into())])
        .await
        .unwrap();
    assert_eq!(value["ok"], true);
    let requests = worker.join().unwrap();
    assert_eq!(requests.len(), 3);
    for request in requests {
        assert!(request.starts_with("GET /query?q=JAK1%2FJAK2 "));
        assert!(request.to_lowercase().contains("accept: application/json"));
    }
}
#[tokio::test]
async fn text_and_binary_reads_honor_retry_after_date_and_seconds() {
    let date = httpdate::fmt_http_date(std::time::SystemTime::now() - Duration::from_secs(60));
    let future = httpdate::fmt_http_date(std::time::SystemTime::now() + Duration::from_secs(2));
    let (base, worker) = server(vec![
        (429, Some(date), "limited"),
        (200, None, "release"),
        (429, Some("0".into()), "limited"),
        (200, None, "binary"),
        (429, Some(future), "limited"),
        (200, None, "after date"),
    ]);
    let client = ReactomeClient::new().with_endpoints(&base, &base);
    assert_eq!(client.get_text("/name").await.unwrap(), "release");
    assert_eq!(client.get_binary("/image").await.unwrap(), b"binary");
    assert_eq!(client.get_text("/date").await.unwrap(), "after date");
    assert_eq!(worker.join().unwrap().len(), 6);
}
#[tokio::test]
async fn repeated_limit_returns_final_error_after_three_attempts() {
    let (base, worker) = server(vec![
        (429, Some("0".into()), "one"),
        (429, Some("0".into()), "two"),
        (429, Some("0".into()), "still limited"),
    ]);
    let client = ReactomeClient::new().with_endpoints(&base, &base);
    assert!(
        matches!(client.get::<Value>("/record",&[]).await,Err(ReactomeError::Status{status:429,body}) if body=="still limited")
    );
    assert_eq!(worker.join().unwrap().len(), 3);
}
#[tokio::test]
async fn missing_header_uses_bounded_backoff() {
    let (base, worker) = server(vec![
        (429, None, "one"),
        (429, None, "two"),
        (200, None, "{}"),
    ]);
    let client = ReactomeClient::new().with_endpoints(&base, &base);
    let start = Instant::now();
    client.get::<Value>("/record", &[]).await.unwrap();
    assert!(start.elapsed() >= Duration::from_secs(3));
    assert_eq!(worker.join().unwrap().len(), 3);
}
#[tokio::test]
async fn non_rate_limit_and_excessive_or_invalid_wait_are_not_retried() {
    for (status, header) in [
        (404, None),
        (429, Some("3600".into())),
        (429, Some("invalid".into())),
        (
            429,
            Some(httpdate::fmt_http_date(
                std::time::SystemTime::now() + Duration::from_secs(3600),
            )),
        ),
    ] {
        let (base, worker) = server(vec![(status, header, "original error")]);
        let client = ReactomeClient::new().with_endpoints(&base, &base);
        assert!(
            matches!(tokio::time::timeout(Duration::from_secs(5),client.get::<Value>("/record",&[])).await.expect("non-retry response must not wait"),Err(ReactomeError::Status{status:actual,body}) if actual==status&&body=="original error")
        );
        assert_eq!(worker.join().unwrap().len(), 1);
    }
}
#[tokio::test]
async fn analysis_submission_is_never_retried() {
    let (base, worker) = server(vec![(429, Some("0".into()), "submission limited")]);
    let client = ReactomeClient::new().with_endpoints(&base, &base);
    assert!(matches!(
        client.analyse_identifiers(&["STAT1".into()], false).await,
        Err(ReactomeError::Status { status: 429, .. })
    ));
    let requests = worker.join().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].starts_with("POST /identifiers "));
}
