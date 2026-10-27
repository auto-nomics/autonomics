//! Deterministic SDK contracts against isolated HTTP endpoints, never upstream.
use crossref::{CrossrefClient, CrossrefError, client::WorksQuery};
use std::time::{Duration, Instant, SystemTime};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

const EMPTY: &str =
    r#"{"status":"ok","message-type":"work-list","message":{"total-results":0,"items":[]}}"#;
const WORK: &str =
    r#"{"status":"ok","message-type":"work","message":{"DOI":"10.1000/a","title":["Title"]}}"#;

async fn head(socket: &mut TcpStream) -> String {
    let mut head = Vec::new();
    let mut bytes = [0u8; 2048];
    while !head.windows(4).any(|v| v == b"\r\n\r\n") {
        let size = socket.read(&mut bytes).await.unwrap();
        assert!(size > 0);
        head.extend_from_slice(&bytes[..size]);
        assert!(head.len() < 32_768);
    }
    String::from_utf8(head).unwrap()
}
async fn endpoint() -> (String, TcpListener) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    (
        format!("http://{}", listener.local_addr().unwrap()),
        listener,
    )
}
async fn accept(listener: &TcpListener) -> TcpStream {
    tokio::time::timeout(Duration::from_secs(5), listener.accept())
        .await
        .unwrap()
        .unwrap()
        .0
}
async fn reply(socket: &mut TcpStream, status: u16, body: &str, headers: &str) {
    socket.write_all(format!("HTTP/1.1 {status} Mock\r\nConnection: close\r\nContent-Length: {}\r\n{headers}\r\n{body}", body.len()).as_bytes()).await.unwrap();
}

#[tokio::test]
async fn clone_pacing_tracks_endpoint_headers_and_preserves_request_values() {
    let (url, listener) = endpoint().await;
    let server = tokio::spawn(async move {
        let mut times = Vec::new();
        let mut requests = Vec::new();
        for _ in 0..3 {
            let mut socket = accept(&listener).await;
            times.push(Instant::now());
            requests.push(head(&mut socket).await);
            reply(&mut socket, 200, EMPTY, "X-Rate-Limit-Limit: 2\r\nX-Rate-Limit-Interval: 0.2s\r\nX-Rate-Limit-Type: list\r\nX-Concurrency-Limit: 3\r\n").await;
        }
        (times, requests)
    });
    let builder = CrossrefClient::builder()
        .base_url(url)
        .mailto("private+contact@example.org")
        .bearer_token("private-token");
    let debug = format!("{builder:?}");
    assert!(!debug.contains("private"));
    let client = builder.build();
    assert!(!format!("{client:?}").contains("private"));
    let query = WorksQuery::new()
        .with_rows(1)
        .with_cursor("next+/=")
        .with_field_query("query.author&rows=999", "Smith & Jones+α");
    client.works(&query).await.unwrap();
    client.clone().works(&query).await.unwrap();
    client.works(&query).await.unwrap();
    let observed = client.observed_rate_limits();
    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0].requests, 2);
    assert_eq!(observed[0].interval_seconds, 0.2);
    assert_eq!(observed[0].concurrency_limit, Some(3));
    let (times, requests) = server.await.unwrap();
    for pair in times.windows(2) {
        assert!(pair[1].duration_since(pair[0]) >= Duration::from_millis(90));
    }
    for request in requests {
        let path = request
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap();
        let url = reqwest::Url::parse(&format!("http://mock{path}")).unwrap();
        let pairs: Vec<_> = url.query_pairs().collect();
        assert_eq!(pairs.iter().filter(|(key, _)| key == "rows").count(), 1);
        assert!(
            pairs
                .iter()
                .any(|(key, value)| key == "rows" && value == "1")
        );
        assert!(
            pairs
                .iter()
                .any(|(key, value)| key == "query.author&rows=999" && value == "Smith & Jones+α")
        );
        assert!(
            pairs
                .iter()
                .any(|(key, value)| key == "cursor" && value == "next+/=")
        );
        assert!(
            pairs
                .iter()
                .any(|(key, value)| key == "mailto" && value == "private+contact@example.org")
        );
        assert!(
            request
                .to_ascii_lowercase()
                .contains("crossref-plus-api-token: bearer private-token")
        );
        assert!(!url.query().unwrap().contains("private-token"));
    }
}

#[tokio::test]
async fn cloned_client_keeps_connection_slot_until_body_is_consumed() {
    let (url, listener) = endpoint().await;
    let client = CrossrefClient::builder().base_url(url).build();
    let first = client.clone();
    let first = tokio::spawn(async move { first.works(&WorksQuery::new()).await });
    let mut socket = accept(&listener).await;
    head(&mut socket).await;
    socket.write_all(format!("HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: {}\r\nX-Rate-Limit-Limit: 1000\r\nX-Rate-Limit-Interval: 1s\r\n\r\n", EMPTY.len()).as_bytes()).await.unwrap();
    let second = tokio::spawn(async move { client.works(&WorksQuery::new()).await });
    assert!(
        tokio::time::timeout(Duration::from_millis(100), listener.accept())
            .await
            .is_err(),
        "a cloned request must wait while the first body is pending"
    );
    socket.write_all(EMPTY.as_bytes()).await.unwrap();
    first.await.unwrap().unwrap();
    let mut socket = accept(&listener).await;
    head(&mut socket).await;
    reply(&mut socket, 200, EMPTY, "").await;
    second.await.unwrap().unwrap();
}

#[tokio::test]
async fn retry_after_cooldown_is_shared_across_endpoint_types() {
    let (url, listener) = endpoint().await;
    let server = tokio::spawn(async move {
        let mut socket = accept(&listener).await;
        head(&mut socket).await;
        let limited = Instant::now();
        reply(&mut socket, 429, "", "Retry-After: 1\r\n").await;
        let mut socket = accept(&listener).await;
        let elapsed = limited.elapsed();
        head(&mut socket).await;
        reply(&mut socket, 200, WORK, "X-Rate-Limit-Limit: 10\r\nX-Rate-Limit-Interval: 1s\r\nX-Rate-Limit-Type: singleton\r\n").await;
        elapsed
    });
    let client = CrossrefClient::builder().base_url(url).build();
    assert!(matches!(
        client.works(&WorksQuery::new()).await,
        Err(CrossrefError::RateLimited {
            retry_after_seconds: Some(1)
        })
    ));
    client.clone().works_by_doi("10.1000/a").await.unwrap();
    assert!(server.await.unwrap() >= Duration::from_millis(950));
    assert_eq!(client.observed_rate_limits()[0].request_type, "singleton");
}

#[tokio::test]
async fn retry_after_accepts_http_date_and_overflow_cannot_disable_cooldown() {
    for retry in [
        httpdate::fmt_http_date(SystemTime::now() + Duration::from_secs(5)),
        u64::MAX.to_string(),
    ] {
        let (url, listener) = endpoint().await;
        let header = format!("Retry-After: {retry}\r\n");
        let server = tokio::spawn(async move {
            let mut socket = accept(&listener).await;
            head(&mut socket).await;
            reply(&mut socket, 429, "", &header).await;
        });
        let client = CrossrefClient::builder().base_url(url).build();
        let failure = client.works(&WorksQuery::new()).await.unwrap_err();
        if retry == u64::MAX.to_string() {
            assert!(matches!(
                failure,
                CrossrefError::RateLimited {
                    retry_after_seconds: Some(u64::MAX)
                }
            ));
            assert!(matches!(
                tokio::time::timeout(
                    Duration::from_millis(100),
                    client.clone().works_by_doi("10.1000/a")
                )
                .await
                .unwrap(),
                Err(CrossrefError::RateLimited {
                    retry_after_seconds: Some(u64::MAX)
                })
            ));
        } else {
            assert!(matches!(
                failure,
                CrossrefError::RateLimited {
                    retry_after_seconds: Some(1..=5)
                }
            ));
        }
        server.await.unwrap();
    }
}

#[tokio::test]
async fn malformed_success_never_deserializes_to_a_zero_result_list() {
    for body in [
        "{}",
        "<html>bad gateway</html>",
        r#"{"status":"failed","message":{"total-results":0,"items":[]}}"#,
        r#"{"status":"ok","message-type":"work","message":{"total-results":0,"items":[]}}"#,
        r#"{"status":"ok","message-type":"work-list","message":{"items":[]}}"#,
    ] {
        let (url, listener) = endpoint().await;
        let body = body.to_owned();
        let server = tokio::spawn(async move {
            let mut socket = accept(&listener).await;
            head(&mut socket).await;
            reply(&mut socket, 200, &body, "").await;
        });
        let failure = CrossrefClient::builder()
            .base_url(url)
            .build()
            .works(&WorksQuery::new())
            .await
            .unwrap_err();
        assert!(matches!(
            failure,
            CrossrefError::InvalidResponse(_) | CrossrefError::Parse(_)
        ));
        server.await.unwrap();
    }
}

#[tokio::test]
async fn bodies_are_bounded_with_and_without_content_length() {
    for chunked in [false, true] {
        for status in [200, 503] {
            let (url, listener) = endpoint().await;
            let limit = if status == 200 {
                16 * 1024 * 1024
            } else {
                64 * 1024
            };
            let server = tokio::spawn(async move {
                let mut socket = accept(&listener).await;
                head(&mut socket).await;
                if chunked {
                    socket.write_all(format!("HTTP/1.1 {status} Mock\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n", limit + 1).as_bytes()).await.unwrap();
                    // The client may close as soon as the size bound is reached.
                    let _ = socket.write_all(&vec![b'x'; limit + 1]).await;
                    let _ = socket.write_all(b"\r\n0\r\n\r\n").await;
                } else {
                    socket.write_all(format!("HTTP/1.1 {status} Mock\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", limit + 1).as_bytes()).await.unwrap();
                }
            });
            let failure = CrossrefClient::builder()
                .base_url(url)
                .build()
                .works(&WorksQuery::new())
                .await
                .unwrap_err();
            if status == 200 {
                assert!(matches!(failure, CrossrefError::ResponseTooLarge));
            } else {
                assert!(matches!(failure, CrossrefError::Status { status: 503, .. }));
            }
            server.await.unwrap();
        }
    }
}
