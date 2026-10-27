//! Semantic Scholar discovery contracts over the actual SDK and local HTTP.
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use bib_base::query::{LiteratureSource, S2Source};
use bib_types::discovery::*;
use std::sync::Arc;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
async fn server(
    responses: Vec<(u16, String, Vec<(String, String)>)>,
) -> (String, tokio::task::JoinHandle<Vec<reqwest::Url>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let mut requests = Vec::new();
        for (status, body, headers) in responses {
            let (mut socket, _) =
                tokio::time::timeout(std::time::Duration::from_secs(10), listener.accept())
                    .await
                    .unwrap()
                    .unwrap();
            let mut request = Vec::new();
            let mut bytes = [0u8; 1024];
            while !request.windows(4).any(|v| v == b"\r\n\r\n") {
                let size = socket.read(&mut bytes).await.unwrap();
                assert!(size > 0);
                request.extend_from_slice(&bytes[..size]);
                assert!(request.len() < 65536);
            }
            let head = String::from_utf8(request).unwrap();
            let path = head
                .lines()
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap();
            requests.push(reqwest::Url::parse(&format!("http://mock{path}")).unwrap());
            let headers = headers
                .iter()
                .map(|(key, value)| format!("{key}: {value}\r\n"))
                .collect::<String>();
            let response = format!(
                "HTTP/1.1 {status} Mock\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        }
        requests
    });
    (url, task)
}
fn json(body: serde_json::Value) -> (u16, String, Vec<(String, String)>) {
    (200, body.to_string(), Vec::new())
}

fn source(url: String) -> S2Source {
    S2Source::new(Arc::new(
        semantic_scholar::S2Client::new().with_graph_base_url(url),
    ))
}
fn request(query: &str) -> DiscoveryRequest {
    DiscoveryRequest {
        query: query.into(),
        sources: vec!["semantic_scholar".into()],
        page_size: 1,
        ..Default::default()
    }
}
fn paper(id: u32) -> serde_json::Value {
    serde_json::json!({"paperId":format!("{id:040x}"),"title":format!("Gene paper {id}"),"year":2024,"externalIds":{"DOI":format!("10.1000/paper-{id}")}})
}
fn bulk(total: u64, ids: &[u32], token: Option<&str>) -> serde_json::Value {
    serde_json::json!({"total":total,"data":ids.iter().map(|id|paper(*id)).collect::<Vec<_>>(),"token":token})
}
fn param(url: &reqwest::Url, key: &str) -> Option<String> {
    url.query_pairs()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.into_owned())
}

#[tokio::test]
async fn bulk_batches_are_buffered_as_small_pages_without_extra_http_or_lost_records() {
    let (url, requests) = server(vec![
        json(bulk(5, &[1, 2, 3], Some("saved+/="))),
        json(bulk(5, &[4, 5], None)),
    ])
    .await;
    let source = source(url);
    let plan = source
        .plan_discovery(&request("gene AND NOT mice"))
        .unwrap();
    let mut cursor = None;
    for id in 1..=5 {
        let page = source
            .discovery_page(&plan, cursor.as_deref(), 1)
            .await
            .unwrap();
        assert_eq!(page.articles.len(), 1);
        assert_eq!(page.articles[0].title, format!("Gene paper {id}"));
        assert_eq!(page.total, Some(5));
        cursor = page.next_cursor;
        assert_eq!(cursor.is_some(), id < 5);
    }
    let requests = requests.await.unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].path(), "/paper/search/bulk");
    assert_eq!(param(&requests[1], "token").as_deref(), Some("saved+/="));
    assert!(param(&requests[0], "limit").is_none());
    assert_eq!(
        param(&requests[0], "query"),
        plan.parameters.get("query").cloned()
    );
}

#[tokio::test]
async fn bulk_metadata_filters_and_source_sort_are_real_http_parameters() {
    let (url, requests) = server(vec![json(bulk(1, &[1], None))]).await;
    let source = source(url);
    let mut input = request("gene AND year:2020..2024 AND journal:Nature");
    input.filters.publication_types = vec!["review".into()];
    input.sort = DiscoverySort::CitationCount;
    let plan = source.plan_discovery(&input).unwrap();
    source.discovery_page(&plan, None, 1).await.unwrap();
    let requests = requests.await.unwrap();
    for (key, value) in [
        ("year", "2020-2024"),
        ("venue", "Nature"),
        ("publicationTypes", "Review"),
        ("sort", "citationCount:desc"),
    ] {
        assert_eq!(param(&requests[0], key).as_deref(), Some(value));
    }
}

#[tokio::test]
async fn relevance_uses_real_offsets_and_rejects_changed_totals() {
    for changed in [false, true] {
        let (url, requests) = server(vec![
            json(serde_json::json!({"total":2,"offset":0,"next":1,"data":[paper(1)]})),
            json(serde_json::json!({"total":if changed{3}else{2},"offset":1,"data":[paper(2)]})),
        ])
        .await;
        let source = source(url);
        let plan = source
            .plan_discovery(&DiscoveryRequest {
                allow_approximate: true,
                ..request("gene")
            })
            .unwrap();
        let first = source.discovery_page(&plan, None, 1).await.unwrap();
        let second = source
            .discovery_page(&plan, first.next_cursor.as_deref(), 1)
            .await;
        if changed {
            assert_eq!(second.unwrap_err().kind, SourceFailureKind::CursorExpired);
        } else {
            let page = second.unwrap();
            assert_eq!(page.articles[0].title, "Gene paper 2");
            assert!(page.next_cursor.is_none());
        }
        let requests = requests.await.unwrap();
        assert_eq!(requests[0].path(), "/paper/search");
        assert_eq!(param(&requests[1], "offset").as_deref(), Some("1"));
        assert_eq!(param(&requests[1], "limit").as_deref(), Some("1"));
        let other = source
            .plan_discovery(&DiscoveryRequest {
                allow_approximate: true,
                ..request("asthma")
            })
            .unwrap();
        assert_eq!(
            source
                .discovery_page(&other, first.next_cursor.as_deref(), 1)
                .await
                .unwrap_err()
                .kind,
            SourceFailureKind::CursorExpired
        );
    }
}

#[tokio::test]
async fn relevance_window_clips_requested_page_and_never_claims_full_retrieval() {
    let (url,requests)=server(vec![json(serde_json::json!({"total":1200,"offset":950,"next":1000,"data":(950..1000).map(paper).collect::<Vec<_>>()}))]).await;
    let source = source(url);
    let plan = source
        .plan_discovery(&DiscoveryRequest {
            allow_approximate: true,
            page_size: 100,
            ..request("gene")
        })
        .unwrap();
    let cursor=URL_SAFE_NO_PAD.encode(serde_json::to_vec(&serde_json::json!({"source":"semantic_scholar","fingerprint":plan.fingerprint,"position":{"kind":"relevance","offset":950,"total":1200}})).unwrap());
    let page = source
        .discovery_page(&plan, Some(&cursor), 100)
        .await
        .unwrap();
    assert_eq!(page.articles.len(), 50);
    assert!(page.next_cursor.is_none());
    assert_eq!(page.total, Some(1200));
    assert!(
        page.notices
            .iter()
            .any(|notice| notice.code == "result_window")
    );
    let requests = requests.await.unwrap();
    assert_eq!(param(&requests[0], "limit").as_deref(), Some("50"));
}

#[tokio::test]
async fn invalid_success_and_repeated_bulk_tokens_are_not_empty_results() {
    for body in [
        "{}".into(),
        serde_json::json!({"error":"upstream failure"}).to_string(),
        bulk(2, &[], Some("next")).to_string(),
        bulk(0, &[1], None).to_string(),
        bulk(2, &[1, 1], None).to_string(),
        serde_json::json!({"total":1,"data":[{"paperId":"bad-id","title":"bad"}]}).to_string(),
    ] {
        let (url, requests) = server(vec![(200, body, vec![])]).await;
        let source = source(url);
        let plan = source.plan_discovery(&request("gene")).unwrap();
        assert_eq!(
            source
                .discovery_page(&plan, None, 1)
                .await
                .unwrap_err()
                .kind,
            SourceFailureKind::InvalidResponse
        );
        requests.await.unwrap();
    }
    let (url, requests) = server(vec![
        json(bulk(4, &[1], Some("same"))),
        json(bulk(4, &[2], Some("same"))),
    ])
    .await;
    let source = source(url);
    let plan = source.plan_discovery(&request("gene")).unwrap();
    let first = source.discovery_page(&plan, None, 1).await.unwrap();
    assert_eq!(
        source
            .discovery_page(&plan, first.next_cursor.as_deref(), 1)
            .await
            .unwrap_err()
            .kind,
        SourceFailureKind::InvalidResponse
    );
    requests.await.unwrap();
}

#[tokio::test]
async fn singleton_lookup_verifies_identity_and_only_404_is_empty() {
    for (status, body, kind) in [
        (200, paper(1), None),
        (200, paper(2), Some(SourceFailureKind::InvalidResponse)),
        (404, serde_json::json!({"error":"not found"}), None),
        (
            500,
            serde_json::json!({"error":"private failure"}),
            Some(SourceFailureKind::Upstream),
        ),
    ] {
        let (url, requests) = server(vec![(status, body.to_string(), vec![])]).await;
        let source = source(url);
        let plan = source.plan_discovery(&request("10.1000/paper-1")).unwrap();
        let response = source.discovery_page(&plan, None, 1).await;
        if let Some(kind) = kind {
            assert_eq!(response.unwrap_err().kind, kind);
        } else {
            let page = response.unwrap();
            assert_eq!(page.articles.len(), usize::from(status == 200));
            assert_eq!(page.total, Some(u64::from(status == 200)));
        }
        let requests = requests.await.unwrap();
        assert_eq!(requests[0].path(), "/paper/DOI%3A10.1000%2Fpaper-1");
    }
}

#[tokio::test]
async fn discovery_owns_retries_and_public_failures_do_not_expose_credentials() {
    let (url, requests) = server(vec![(
        429,
        "private body".into(),
        vec![("Retry-After".into(), "120".into())],
    )])
    .await;
    let source = S2Source::new(Arc::new(
        semantic_scholar::S2Client::new()
            .with_graph_base_url(url)
            .with_api_key("private-configured-key"),
    ));
    let plan = source.plan_discovery(&request("gene")).unwrap();
    let failure = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        source.discovery_page(&plan, None, 1),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert_eq!(failure.kind, SourceFailureKind::RateLimited);
    assert_eq!(failure.retry_after_seconds, Some(120));
    assert!(!format!("{plan:?} {failure:?}").contains("private"));
    requests.await.unwrap();
}

#[tokio::test]
async fn legacy_structured_boolean_queries_with_metadata_also_use_bulk_endpoint() {
    let (url, requests) = server(vec![json(bulk(1, &[1], None))]).await;
    let source = source(url);
    let result = source
        .search(
            &bib_types::StructuredSearch {
                keywords: Some(vec!["gene".into(), "mice".into()]),
                keywords_op: Some(bib_types::BoolOp::Not),
                year_range: Some(bib_types::YearRange {
                    from: 2020,
                    to: 2024,
                }),
                ..Default::default()
            },
            1,
        )
        .await
        .unwrap();
    assert_eq!(result.articles.len(), 1);
    let requests = requests.await.unwrap();
    assert_eq!(requests[0].path(), "/paper/search/bulk");
    assert_eq!(param(&requests[0], "query").as_deref(), Some("gene -mice"));
    assert_eq!(param(&requests[0], "year").as_deref(), Some("2020-2024"));
}
