//! Isolated HTTP contracts for Semantic Scholar, without upstream credentials.
use semantic_scholar::{PaperSearchFilter, S2Client, S2Error};
use std::time::{Duration, Instant, SystemTime};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
const WORK: &str = r#"{"paperId":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","title":"Gene study"}"#;
fn client(url: String) -> S2Client {
    S2Client::new().with_graph_base_url(url).with_max_retries(0)
}
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
async fn idle_time_does_not_create_burst_credits_for_cloned_clients() {
    let (url, listener) = endpoint().await;
    let server = tokio::spawn(async move {
        let mut times = Vec::new();
        for _ in 0..3 {
            let mut socket = accept(&listener).await;
            head(&mut socket).await;
            times.push(Instant::now());
            reply(&mut socket, 200, WORK, "").await;
        }
        times
    });
    let client = client(url);
    client.get_paper("a", None).await.unwrap();
    tokio::time::sleep(Duration::from_millis(2300)).await;
    let other = client.clone();
    let (a, b) = tokio::join!(client.get_paper("a", None), other.get_paper("b", None));
    a.unwrap();
    b.unwrap();
    let times = server.await.unwrap();
    assert!(times[2].duration_since(times[1]) >= Duration::from_millis(1000));
}

#[tokio::test]
async fn rate_limit_preserves_full_cooldown_across_clones() {
    let (url, listener) = endpoint().await;
    let server = tokio::spawn(async move {
        let mut socket = accept(&listener).await;
        head(&mut socket).await;
        let at = Instant::now();
        reply(&mut socket, 429, "", "Retry-After: 2\r\n").await;
        let mut socket = accept(&listener).await;
        let elapsed = at.elapsed();
        assert!(head(&mut socket).await.starts_with("GET /paper/id?"));
        reply(&mut socket, 200, WORK, "").await;
        elapsed
    });
    let client = client(url);
    assert!(matches!(
        client.search_paper("gene", 1, None).await,
        Err(S2Error::RateLimited {
            retry_after_seconds: Some(2)
        })
    ));
    client.clone().get_paper("id", None).await.unwrap();
    assert!(server.await.unwrap() >= Duration::from_millis(1950));
}

#[tokio::test]
async fn retry_after_http_date_and_overflow_are_not_truncated_or_ignored() {
    for retry in [
        httpdate::fmt_http_date(SystemTime::now() + Duration::from_secs(120)),
        u64::MAX.to_string(),
    ] {
        let (url, listener) = endpoint().await;
        let header = format!("Retry-After: {retry}\r\n");
        let server = tokio::spawn(async move {
            let mut socket = accept(&listener).await;
            head(&mut socket).await;
            reply(&mut socket, 429, "", &header).await;
        });
        let client = client(url);
        let error = client.search_paper("gene", 1, None).await.unwrap_err();
        if retry == u64::MAX.to_string() {
            assert!(matches!(
                error,
                S2Error::RateLimited {
                    retry_after_seconds: Some(u64::MAX)
                }
            ));
            assert!(matches!(
                tokio::time::timeout(
                    Duration::from_millis(100),
                    client.clone().get_paper("id", None)
                )
                .await
                .unwrap(),
                Err(S2Error::RateLimited {
                    retry_after_seconds: Some(u64::MAX)
                })
            ));
        } else {
            assert!(matches!(
                error,
                S2Error::RateLimited {
                    retry_after_seconds: Some(115..=120)
                }
            ));
        }
        server.await.unwrap();
    }
}

#[tokio::test]
async fn bulk_filters_and_tokens_are_encoded_without_inventing_a_limit() {
    let (url, listener) = endpoint().await;
    let server = tokio::spawn(async move {
        let mut socket = accept(&listener).await;
        let request = head(&mut socket).await;
        reply(&mut socket,200,r#"{"total":1,"token":"next+/=","data":[{"paperId":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","title":"Gene","abstract":"Actual upstream abstract evidence"}]}"#,"").await;
        request
    });
    let client = client(url).with_api_key("private-key");
    assert!(!format!("{client:?}").contains("private-key"));
    let response = client
        .search_paper_bulk_filtered(
            "(gene | asthma) -mice",
            Some("saved+/="),
            None,
            Some("citationCount:desc"),
            &PaperSearchFilter {
                year: Some("2020-2024".into()),
                venue: Some("Nature,Science".into()),
                publication_types: Some("JournalArticle,Review".into()),
                open_access_pdf: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(response.token.as_deref(), Some("next+/="));
    assert_eq!(
        response.data[0].abstract_text.as_deref(),
        Some("Actual upstream abstract evidence")
    );
    let request = server.await.unwrap();
    let url = reqwest::Url::parse(&format!(
        "http://mock{}",
        request
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
    ))
    .unwrap();
    assert_eq!(url.path(), "/paper/search/bulk");
    let params: std::collections::BTreeMap<_, _> = url.query_pairs().collect();
    assert_eq!(params["token"], "saved+/=");
    assert_eq!(params["query"], "(gene | asthma) -mice");
    assert_eq!(params["year"], "2020-2024");
    assert_eq!(params["publicationTypes"], "JournalArticle,Review");
    assert_eq!(params["venue"], "Nature,Science");
    assert!(params.contains_key("openAccessPdf"));
    assert!(!params.contains_key("limit"));
    assert!(!url.as_str().contains("private-key"));
    assert!(request.contains("x-api-key: private-key"));
}

#[tokio::test]
async fn successful_error_objects_and_incomplete_lists_do_not_become_zero_results() {
    for (path, body) in [
        ("relevance", "{}"),
        ("relevance", r#"{"total":0,"data":[]}"#),
        (
            "relevance",
            r#"{"error":"bad query","total":0,"offset":0,"data":[]}"#,
        ),
        ("bulk", r#"{"data":[]}"#),
        ("bulk", r#"{"total":0,"data":[],"token":12}"#),
        ("lookup", "{}"),
        ("relevance", "<html>error</html>"),
    ] {
        let (url, listener) = endpoint().await;
        let server = tokio::spawn(async move {
            let mut socket = accept(&listener).await;
            head(&mut socket).await;
            reply(&mut socket, 200, body, "").await;
        });
        let client = client(url);
        let failure = match path {
            "bulk" => client
                .search_paper_bulk("gene", None, None, None)
                .await
                .unwrap_err(),
            "lookup" => client.get_paper("id", None).await.unwrap_err(),
            _ => client.search_paper("gene", 1, None).await.unwrap_err(),
        };
        assert!(matches!(
            failure,
            S2Error::InvalidResponse(_) | S2Error::Parse(_)
        ));
        server.await.unwrap();
    }
}

#[tokio::test]
async fn custom_client_timeout_is_preserved_for_incomplete_response_bodies() {
    let (url, listener) = endpoint().await;
    let server = tokio::spawn(async move {
        let mut socket = accept(&listener).await;
        head(&mut socket).await;
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n{")
            .await
            .unwrap();
        let mut bytes = [0u8; 1];
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), socket.read(&mut bytes))
                .await
                .unwrap()
                .unwrap(),
            0
        );
    });
    let client = S2Client::with_client(
        reqwest::Client::builder()
            .timeout(Duration::from_millis(200))
            .build()
            .unwrap(),
    )
    .with_graph_base_url(url)
    .with_max_retries(0);
    let failure =
        tokio::time::timeout(Duration::from_secs(2), client.search_paper("gene", 1, None))
            .await
            .unwrap()
            .unwrap_err();
    assert!(matches!(failure,S2Error::Http(error) if error.is_timeout()));
    server.await.unwrap();
}

#[tokio::test]
async fn content_length_and_chunked_bodies_are_bounded_without_losing_error_status() {
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
                    socket.write_all(format!("HTTP/1.1 {status} Mock\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n",limit+1).as_bytes()).await.unwrap();
                    let _ = socket.write_all(&vec![b'x'; limit + 1]).await;
                    let _ = socket.write_all(b"\r\n0\r\n\r\n").await;
                } else {
                    socket.write_all(format!("HTTP/1.1 {status} Mock\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",limit+1).as_bytes()).await.unwrap();
                }
            });
            let error = client(url).search_paper("gene", 1, None).await.unwrap_err();
            if status == 200 {
                assert!(matches!(error, S2Error::ResponseTooLarge));
            } else {
                assert!(matches!(error, S2Error::Status { status: 503, .. }));
            }
            server.await.unwrap();
        }
    }
}
