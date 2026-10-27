use bib_base::query::{
    ArxivSource, BiorxivSource, CrossrefSource, LiteratureSource, OpenAlexSource, PubmedSource,
};
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
fn request(source: &str) -> DiscoveryRequest {
    DiscoveryRequest {
        query: "title:asthma".into(),
        sources: vec![source.into()],
        page_size: 1,
        ..Default::default()
    }
}
fn param(url: &reqwest::Url, key: &str) -> Option<String> {
    url.query_pairs()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.into_owned())
}

fn crossref_source(url: String) -> CrossrefSource {
    CrossrefSource::new(Arc::new(
        crossref::CrossrefClient::builder().base_url(url).build(),
    ))
}
fn crossref_request() -> DiscoveryRequest {
    DiscoveryRequest {
        allow_approximate: true,
        ..request("crossref")
    }
}
fn crossref_list(total: u64, ids: &[&str], cursor: Option<&str>) -> serde_json::Value {
    serde_json::json!({"status":"ok", "message-type":"work-list", "message": {
        "total-results":total, "items":ids.iter().map(|id|serde_json::json!({"DOI":id,"title":["Asthma study"],"type":"journal-article"})).collect::<Vec<_>>(),
        "next-cursor":cursor
    }})
}

#[tokio::test]
async fn crossref_cursor_preserves_reserved_characters_and_repeated_provider_tokens() {
    let raw = "opaque+/=next";
    let (url, requests) = server(vec![
        json(crossref_list(3, &["10.1000/a"], Some(raw))),
        json(crossref_list(3, &["10.1000/b"], Some(raw))),
        json(crossref_list(3, &["10.1000/c"], None)),
    ])
    .await;
    let source = crossref_source(url);
    let plan = source.plan_discovery(&crossref_request()).unwrap();
    let first = source.discovery_page(&plan, None, 1).await.unwrap();
    let second = source
        .discovery_page(&plan, first.next_cursor.as_deref(), 1)
        .await
        .unwrap();
    assert_ne!(
        first.next_cursor, second.next_cursor,
        "progress belongs in the opaque wrapper even when provider cursor repeats"
    );
    let third = source
        .discovery_page(&plan, second.next_cursor.as_deref(), 1)
        .await
        .unwrap();
    assert!(third.next_cursor.is_none());
    assert_eq!(third.total, Some(3));
    let requests = requests.await.unwrap();
    assert_eq!(param(&requests[0], "cursor").as_deref(), Some("*"));
    assert_eq!(param(&requests[1], "cursor").as_deref(), Some(raw));
    assert_eq!(param(&requests[2], "cursor").as_deref(), Some(raw));
    assert_eq!(
        param(&requests[0], "query.bibliographic").as_deref(),
        Some("asthma")
    );
    assert_eq!(param(&requests[0], "rows").as_deref(), Some("1"));
    let changed = source
        .plan_discovery(&DiscoveryRequest {
            query: "title:gene".into(),
            ..crossref_request()
        })
        .unwrap();
    assert_eq!(
        source
            .discovery_page(&changed, first.next_cursor.as_deref(), 1)
            .await
            .unwrap_err()
            .kind,
        SourceFailureKind::CursorExpired
    );
}

#[tokio::test]
async fn crossref_singleton_normalizes_native_doi_and_validates_identity() {
    let (url, requests) = server(vec![json(serde_json::json!({"status":"ok","message-type":"work","message":{"DOI":"10.1000/Exact?literal","title":["Exact work"]}}))]).await;
    let source = crossref_source(url);
    let plan = source
        .plan_discovery(&DiscoveryRequest {
            sources: vec!["crossref".into()],
            native: Some(NativeSearch {
                source: "crossref".into(),
                parameters: [("doi".into(), "https://doi.org/10.1000/Exact?literal".into())].into(),
            }),
            ..Default::default()
        })
        .unwrap();
    let page = source.discovery_page(&plan, None, 25).await.unwrap();
    assert_eq!(page.articles.len(), 1);
    assert!(page.next_cursor.is_none());
    let requests = requests.await.unwrap();
    assert_eq!(requests[0].path(), "/works/10.1000%2Fexact%3Fliteral");
    assert!(requests[0].query().is_none());

    let (url, requests) = server(vec![json(serde_json::json!({"status":"ok","message-type":"work","message":{"DOI":"10.1000/wrong","title":["Other"]}}))]).await;
    assert_eq!(
        crossref_source(url)
            .discovery_page(&plan, None, 25)
            .await
            .unwrap_err()
            .kind,
        SourceFailureKind::InvalidResponse
    );
    requests.await.unwrap();
}

#[tokio::test]
async fn crossref_only_not_found_is_a_genuine_empty_singleton() {
    for (status, kind) in [
        (404, None),
        (403, Some(SourceFailureKind::AuthRequired)),
        (500, Some(SourceFailureKind::Upstream)),
    ] {
        let (url, requests) = server(vec![(status, "upstream detail".into(), vec![])]).await;
        let source = crossref_source(url);
        let plan = source
            .plan_discovery(&DiscoveryRequest {
                query: "10.1000/missing".into(),
                sources: vec!["crossref".into()],
                ..Default::default()
            })
            .unwrap();
        let result = source.discovery_page(&plan, None, 25).await;
        if let Some(kind) = kind {
            assert_eq!(result.unwrap_err().kind, kind);
        } else {
            let page = result.unwrap();
            assert!(page.articles.is_empty());
            assert_eq!(page.total, Some(0));
        }
        requests.await.unwrap();
    }
}

#[tokio::test]
async fn crossref_rejects_error_envelopes_bad_counts_and_missing_continuation() {
    let bodies = vec![
        "<html>error</html>".into(), "{}".into(),
        serde_json::json!({"status":"failed","message-type":"work-list","message":{"total-results":0,"items":[]}}).to_string(),
        serde_json::json!({"status":"ok","message-type":"work-list","message":{"items":[]}}).to_string(),
        crossref_list(0, &["10.1000/a"], None).to_string(),
        crossref_list(4, &[], None).to_string(),
        crossref_list(4, &["10.1000/a"], None).to_string(),
        crossref_list(4, &["10.1000/a"], Some(&"x".repeat(32_768))).to_string(),
        crossref_list(1, &["not-a-doi"], None).to_string(),
    ];
    for body in bodies {
        let (url, requests) = server(vec![(200, body, vec![])]).await;
        let source = crossref_source(url);
        let plan = source.plan_discovery(&crossref_request()).unwrap();
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
}

#[tokio::test]
async fn crossref_retains_retry_after_and_sanitizes_public_errors() {
    let (url, requests) = server(vec![(
        429,
        "private upstream token".into(),
        vec![("Retry-After".into(), "17".into())],
    )])
    .await;
    let source = CrossrefSource::new(Arc::new(
        crossref::CrossrefClient::builder()
            .base_url(url)
            .bearer_token("configured-secret")
            .mailto("private@example.org")
            .build(),
    ));
    let plan = source.plan_discovery(&crossref_request()).unwrap();
    let failure = source.discovery_page(&plan, None, 1).await.unwrap_err();
    assert_eq!(failure.kind, SourceFailureKind::RateLimited);
    assert_eq!(failure.retry_after_seconds, Some(17));
    let public = format!("{plan:?} {failure:?}");
    for secret in [
        "configured-secret",
        "private@example.org",
        "private upstream token",
    ] {
        assert!(!public.contains(secret));
    }
    requests.await.unwrap();
}

#[tokio::test]
async fn crossref_continuation_errors_are_expired_cursors() {
    let (url, requests) = server(vec![
        json(crossref_list(2, &["10.1000/a"], Some("saved"))),
        (400, "private upstream response".into(), vec![]),
    ])
    .await;
    let source = crossref_source(url);
    let plan = source.plan_discovery(&crossref_request()).unwrap();
    let first = source.discovery_page(&plan, None, 1).await.unwrap();
    assert_eq!(
        source
            .discovery_page(&plan, first.next_cursor.as_deref(), 1)
            .await
            .unwrap_err()
            .kind,
        SourceFailureKind::CursorExpired
    );
    requests.await.unwrap();
}

#[tokio::test]
async fn crossref_exact_filters_survive_transport_and_reject_unrequested_dois() {
    for returned_id in ["10.1000/a", "10.1000/unrequested"] {
        let (url, requests) = server(vec![json(crossref_list(1, &[returned_id], None))]).await;
        let source = crossref_source(url);
        let input = DiscoveryRequest {
            query: "(doi:10.1000/a OR doi:10.1000/b) AND year:2020..2022".into(),
            sources: vec!["crossref".into()],
            filters: DiscoveryFilters {
                publication_types: vec!["article".into()],
                ..Default::default()
            },
            page_size: 2,
            ..Default::default()
        };
        let plan = source.plan_discovery(&input).unwrap();
        let result = source.discovery_page(&plan, None, 2).await;
        if returned_id.ends_with("/a") {
            let page = result.unwrap();
            assert_eq!(page.articles.len(), 1);
            assert!(page.next_cursor.is_none(), "a short page is terminal");
        } else {
            assert_eq!(result.unwrap_err().kind, SourceFailureKind::InvalidResponse);
        }
        let requests = requests.await.unwrap();
        assert_eq!(
            param(&requests[0], "filter").as_deref(),
            Some(
                "doi:10.1000/a,doi:10.1000/b,from-pub-date:2020-01-01,until-pub-date:2022-12-31,type:journal-article"
            )
        );
        assert_eq!(param(&requests[0], "sort").as_deref(), Some("published"));
    }
}

fn atom(offset: u64, total: u64, ids: &[&str]) -> (u16, String, Vec<(String, String)>) {
    let entries = ids.iter().map(|id|format!("<entry><id>https://arxiv.org/abs/{id}</id><title>Asthma paper {id}</title><summary>Source evidence</summary><published>2024-01-01T00:00:00Z</published></entry>")).collect::<String>();
    (
        200,
        format!(
            "<feed xmlns='http://www.w3.org/2005/Atom' xmlns:os='http://a9.com/-/spec/opensearch/1.1/'><os:totalResults>{total}</os:totalResults><os:startIndex>{offset}</os:startIndex><os:itemsPerPage>{}</os:itemsPerPage>{entries}</feed>",
            ids.len()
        ),
        Vec::new(),
    )
}
#[tokio::test]
async fn cancelling_arxiv_session_closes_the_real_response_body_connection() {
    cancellation_closes_response("arxiv", |url| {
        Arc::new(ArxivSource::new(Arc::new(
            arxiv::ArxivClient::new().with_base_url(url),
        )))
    })
    .await;
}
#[tokio::test]
async fn cancelling_crossref_session_closes_the_real_response_body_connection() {
    cancellation_closes_response("crossref", |url| Arc::new(crossref_source(url))).await;
}
#[tokio::test]
async fn cancelling_semantic_scholar_session_closes_the_real_response_body_connection() {
    cancellation_closes_response("semantic_scholar", |url| {
        Arc::new(bib_base::query::S2Source::new(Arc::new(
            semantic_scholar::S2Client::new().with_graph_base_url(url),
        )))
    })
    .await;
}
async fn cancellation_closes_response(
    source_name: &str,
    make_source: impl FnOnce(String) -> Arc<dyn LiteratureSource>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let source = make_source(format!("http://{}", listener.local_addr().unwrap()));
    let manager = bib_base::discovery_sessions::DiscoverySessions::default();
    let started = manager
        .create(
            Arc::new(bib_base::query::LiteratureGateway::new().with_source(source)),
            DiscoveryRequest {
                allow_approximate: true,
                ..request(source_name)
            },
        )
        .unwrap();
    let (mut stream, _) =
        tokio::time::timeout(std::time::Duration::from_secs(3), listener.accept())
            .await
            .unwrap()
            .unwrap();
    let mut head = Vec::new();
    let mut buffer = [0u8; 1024];
    while !head.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
        let count = stream.read(&mut buffer).await.unwrap();
        assert!(count > 0);
        head.extend_from_slice(&buffer[..count]);
    }
    stream
        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 200\r\nConnection: close\r\n\r\n<")
        .await
        .unwrap();
    manager.cancel(&started.id, None).unwrap();
    let count = tokio::time::timeout(std::time::Duration::from_secs(2), stream.read(&mut buffer))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        count, 0,
        "cancelling must close the pending SDK body stream"
    );
    let snapshot = manager.snapshot(&started.id).unwrap();
    assert_eq!(snapshot.merged_candidates, 0);
    assert_eq!(snapshot.phase, DiscoverySessionPhase::Cancelled);
}
#[tokio::test]
async fn arxiv_pages_use_real_offsets_and_query_bound_cursors() {
    let (url, requests) = server(vec![
        atom(0, 2, &["2401.00001v1"]),
        atom(1, 2, &["2401.00002v1"]),
    ])
    .await;
    let source = ArxivSource::new(Arc::new(arxiv::ArxivClient::new().with_base_url(url)));
    assert!(source.discovery_capabilities().searchable);
    assert!(
        !source
            .discovery_capabilities()
            .fields
            .contains(&bib_types::SearchField::Year)
    );
    let plan = source.plan_discovery(&request("arxiv")).unwrap();
    let first = source.discovery_page(&plan, None, 1).await.unwrap();
    assert_eq!(first.total, Some(2));
    assert_eq!(first.executed_parameters["start"], "0");
    let mut changed = request("arxiv");
    changed.query = "title:other".into();
    assert_eq!(
        source
            .discovery_page(
                &source.plan_discovery(&changed).unwrap(),
                first.next_cursor.as_deref(),
                1
            )
            .await
            .unwrap_err()
            .kind,
        SourceFailureKind::CursorExpired
    );
    let next = source
        .discovery_page(&plan, first.next_cursor.as_deref(), 1)
        .await
        .unwrap();
    assert!(next.next_cursor.is_none());
    assert_eq!(next.executed_parameters["start"], "1");
    assert_ne!(first.articles[0].id, next.articles[0].id);
    let requests = requests.await.unwrap();
    assert_eq!(
        param(&requests[0], "search_query").as_deref(),
        Some("ti:\"asthma\"")
    );
    assert_eq!(param(&requests[0], "max_results").as_deref(), Some("1"));
    assert_eq!(param(&requests[1], "start").as_deref(), Some("1"));
    assert_eq!(param(&requests[1], "sortBy").as_deref(), Some("relevance"));
}
#[tokio::test]
async fn arxiv_exact_versions_and_native_id_intersections_retain_provenance() {
    let (url, requests) = server(vec![
        atom(0, 1, &["2401.12345v2"]),
        atom(0, 1, &["2401.12345v2"]),
    ])
    .await;
    let source = ArxivSource::new(Arc::new(arxiv::ArxivClient::new().with_base_url(url)));
    let mut exact = request("arxiv");
    exact.query = "https://arxiv.org/abs/2401.12345v2".into();
    let first = source
        .discovery_page(&source.plan_discovery(&exact).unwrap(), None, 1)
        .await
        .unwrap();
    assert_eq!(
        first.articles[0].identifier(bib_types::IdKind::Arxiv),
        Some("2401.12345v2")
    );
    assert!(first.next_cursor.is_none());
    let mut native = request("arxiv");
    native.query.clear();
    native.native = Some(NativeSearch {
        source: "arxiv".into(),
        parameters: [
            ("id_list".into(), "2401.12345v2".into()),
            ("search_query".into(), "ti:asthma".into()),
        ]
        .into(),
    });
    let page = source
        .discovery_page(&source.plan_discovery(&native).unwrap(), None, 1)
        .await
        .unwrap();
    assert_eq!(page.executed_parameters["id_list"], "2401.12345v2");
    assert_eq!(page.executed_parameters["search_query"], "ti:asthma");
    let requests = requests.await.unwrap();
    assert_eq!(param(&requests[0], "search_query"), None);
    assert_eq!(
        param(&requests[0], "id_list").as_deref(),
        Some("2401.12345v2")
    );
    assert_eq!(
        param(&requests[1], "search_query").as_deref(),
        Some("ti:asthma")
    );
}
#[tokio::test]
async fn arxiv_changed_totals_invalidate_offsets_and_window_limits_are_explicit() {
    let (url, requests) = server(vec![
        atom(0, 2, &["2401.00001v1"]),
        atom(1, 3, &["2401.00002v1"]),
    ])
    .await;
    let source = ArxivSource::new(Arc::new(arxiv::ArxivClient::new().with_base_url(url)));
    let plan = source.plan_discovery(&request("arxiv")).unwrap();
    let first = source.discovery_page(&plan, None, 1).await.unwrap();
    assert_eq!(
        source
            .discovery_page(&plan, first.next_cursor.as_deref(), 1)
            .await
            .unwrap_err()
            .kind,
        SourceFailureKind::CursorExpired
    );
    requests.await.unwrap();
    let (url, requests) = server(vec![atom(29999, 30001, &["2401.30000v1"])]).await;
    let source = ArxivSource::new(Arc::new(arxiv::ArxivClient::new().with_base_url(url)));
    use base64::Engine;
    let cursor=base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(&serde_json::json!({"source":"arxiv","fingerprint":plan.fingerprint,"offset":29999,"total":30001})).unwrap());
    let page = source
        .discovery_page(&plan, Some(&cursor), 1)
        .await
        .unwrap();
    assert!(page.next_cursor.is_none());
    assert_eq!(page.total, Some(30001));
    assert!(
        page.notices
            .iter()
            .any(|notice| notice.code == "source_window_limit")
    );
    assert_eq!(
        param(&requests.await.unwrap()[0], "start").as_deref(),
        Some("29999")
    );
}
#[tokio::test]
async fn arxiv_explicit_version_lookups_reject_other_versions_or_papers() {
    for (requested, returned, accepted) in [
        ("2401.12345v2", "2401.12345v3", false),
        ("2401.12345", "2401.12345v3", true),
        ("2401.12345v2", "2401.99999v2", false),
    ] {
        let (url, requests) = server(vec![atom(0, 1, &[returned])]).await;
        let source = ArxivSource::new(Arc::new(arxiv::ArxivClient::new().with_base_url(url)));
        let mut request = request("arxiv");
        request.query = format!("arxiv:{requested}");
        let result = source
            .discovery_page(&source.plan_discovery(&request).unwrap(), None, 1)
            .await;
        if accepted {
            assert!(result.is_ok());
        } else {
            assert_eq!(result.unwrap_err().kind, SourceFailureKind::InvalidResponse);
        }
        requests.await.unwrap();
    }
}
#[tokio::test]
async fn arxiv_legacy_publication_years_are_rejected_without_network_access() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let source = ArxivSource::new(Arc::new(
        arxiv::ArxivClient::new()
            .with_base_url(format!("http://{}", listener.local_addr().unwrap())),
    ));
    let query = bib_types::StructuredSearch {
        keywords: Some(vec!["gene".into()]),
        year_range: Some(bib_types::YearRange {
            from: 2020,
            to: 2024,
        }),
        ..Default::default()
    };
    let error = tokio::time::timeout(std::time::Duration::from_secs(1), source.search(&query, 10))
        .await
        .unwrap()
        .unwrap_err();
    assert!(matches!(error, bib_base::Error::InvalidSearchQuery(_)));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(30), listener.accept())
            .await
            .is_err()
    );
}
#[tokio::test]
async fn arxiv_zero_errors_and_throttling_are_not_conflated() {
    for (response,expected) in [
        (atom(0,0,&[]),None),
        ((200,"<html>unavailable</html>".into(),Vec::new()),Some(SourceFailureKind::InvalidResponse)),
        ((200,"<feed xmlns='http://www.w3.org/2005/Atom'><entry><id>https://arxiv.org/api/errors#bad_query</id><title>Error</title><summary>private upstream detail</summary></entry></feed>".into(),Vec::new()),Some(SourceFailureKind::InvalidQuery)),
        ((429,"private throttle detail".into(),vec![("Retry-After".into(),"60".into())]),Some(SourceFailureKind::RateLimited)),
        (atom(1,2,&["2401.00001v1"]),Some(SourceFailureKind::InvalidResponse)),
    ] {
        let(url,requests)=server(vec![response]).await;
        let source=ArxivSource::new(Arc::new(arxiv::ArxivClient::new().with_base_url(url)));
        let result=source.discovery_page(&source.plan_discovery(&request("arxiv")).unwrap(),None,1).await;
        if let Some(expected)=expected {
            let error=result.unwrap_err(); assert_eq!(error.kind,expected); assert!(!error.message.contains("private"));
            if expected==SourceFailureKind::RateLimited { assert_eq!(error.retry_after_seconds,Some(60));assert!(error.retryable); }
        } else { let page=result.unwrap(); assert!(page.articles.is_empty());assert_eq!(page.total,Some(0)); }
        requests.await.unwrap();
    }
}

#[tokio::test]
async fn pubmed_uses_one_search_and_history_continuation_with_stable_query_details() {
    let (url,requests)=server(vec![
        json(serde_json::json!({"esearchresult":{"count":"2","retmax":"1","retstart":"0","idlist":["101"],"webenv":"stable-history","querykey":"1","querytranslation":"asthma[Title]"}})),
        (200,"PMID- 101\nTI  - First asthma paper\nAB  - Evidence one.\n".into(),Vec::new()),
        (200,"PMID- 102\nTI  - Second asthma paper\nAB  - Evidence two.\n".into(),Vec::new()),
    ]).await;
    let source = PubmedSource::new(Arc::new(
        eutils::EutilsClient::new("test", "test@example.invalid", None).with_base_url(url),
    ));
    let plan = source.plan_discovery(&request("pubmed")).unwrap();
    let first = source.discovery_page(&plan, None, 1).await.unwrap();
    assert_eq!(first.total, Some(2));
    assert_eq!(first.query_translation.as_deref(), Some("asthma[Title]"));
    assert_eq!(first.articles[0].title, "First asthma paper");
    let next = source
        .discovery_page(&plan, first.next_cursor.as_deref(), 1)
        .await
        .unwrap();
    assert_eq!(
        source
            .discovery_page(&plan, first.next_cursor.as_deref(), 2)
            .await
            .unwrap_err()
            .kind,
        SourceFailureKind::InvalidQuery
    );
    assert_eq!(next.articles[0].title, "Second asthma paper");
    assert!(next.next_cursor.is_none());
    assert_eq!(next.query_translation, first.query_translation);
    let requests = requests.await.unwrap();
    assert_eq!(requests.len(), 3);
    assert!(requests[0].path().ends_with("/esearch.fcgi"));
    assert_eq!(param(&requests[0], "usehistory").as_deref(), Some("y"));
    assert_eq!(param(&requests[1], "id").as_deref(), Some("101"));
    assert_eq!(param(&requests[2], "id"), None);
    assert_eq!(
        param(&requests[2], "WebEnv").as_deref(),
        Some("stable-history")
    );
    assert_eq!(param(&requests[2], "query_key").as_deref(), Some("1"));
    assert_eq!(param(&requests[2], "retstart").as_deref(), Some("1"));
    let mut other = request("pubmed");
    other.query = "title:other".into();
    let other = source.plan_discovery(&other).unwrap();
    assert_eq!(
        source
            .discovery_page(&other, first.next_cursor.as_deref(), 1)
            .await
            .unwrap_err()
            .kind,
        SourceFailureKind::CursorExpired
    );
}

fn work(id: &str, title: &str, next: Option<&str>) -> serde_json::Value {
    serde_json::json!({"meta":{"count":2,"next_cursor":next},"results":[{"id":format!("https://openalex.org/{id}"),"title":title}]})
}

#[tokio::test]
async fn openalex_pages_real_cursor_and_preserves_multiple_text_scopes() {
    let (url, requests) = server(vec![
        json(work("W1", "First", Some("provider-page-two"))),
        json(work("W2", "Second", None)),
    ])
    .await;
    let source = OpenAlexSource::new(Arc::new(
        openalex::OpenAlexClient::new(Some("fixture-key")).with_base_url(url),
    ));
    let mut r = request("openalex");
    r.query = "cancer AND title:asthma".into();
    let plan = source.plan_discovery(&r).unwrap();
    let first = source.discovery_page(&plan, None, 1).await.unwrap();
    let second = source
        .discovery_page(&plan, first.next_cursor.as_deref(), 1)
        .await
        .unwrap();
    assert_eq!(first.articles[0].title, "First");
    assert_eq!(second.articles[0].title, "Second");
    assert!(second.next_cursor.is_none());
    let requests = requests.await.unwrap();
    assert_eq!(param(&requests[0], "cursor").as_deref(), Some("*"));
    assert_eq!(
        param(&requests[1], "cursor").as_deref(),
        Some("provider-page-two")
    );
    assert_eq!(
        param(&requests[0], "search.exact").as_deref(),
        Some("(\"cancer\")")
    );
    assert_eq!(
        param(&requests[0], "search.title.exact").as_deref(),
        Some("(\"asthma\")")
    );
    assert_eq!(param(&requests[0], "per_page").as_deref(), Some("1"));
    assert!(
        !serde_json::to_string(&first)
            .unwrap()
            .contains("fixture-key")
    );
    assert!(
        !serde_json::to_string(&plan)
            .unwrap()
            .contains("fixture-key")
    );
}

#[tokio::test]
async fn auth_rate_limits_and_fetch_only_sources_are_distinct_from_zero_matches() {
    let (url, requests) = server(vec![json(work("W1", "First", Some("page-two")))]).await;
    let source = OpenAlexSource::new(Arc::new(
        openalex::OpenAlexClient::new(None).with_base_url(url),
    ));
    let plan = source.plan_discovery(&request("openalex")).unwrap();
    let first = source.discovery_page(&plan, None, 1).await.unwrap();
    assert!(
        first
            .notices
            .iter()
            .any(|notice| notice.code == "auth_required_for_more")
    );
    assert_eq!(
        source
            .discovery_page(&plan, first.next_cursor.as_deref(), 1)
            .await
            .unwrap_err()
            .kind,
        SourceFailureKind::AuthRequired
    );
    assert_eq!(requests.await.unwrap().len(), 1);
    let (url, requests) = server(vec![(
        429,
        "key echoed by upstream must not leak: fixture-secret".into(),
        vec![
            ("Retry-After".into(), "7".into()),
            ("X-RateLimit-Limit".into(), "10000".into()),
            ("X-RateLimit-Remaining".into(), "0".into()),
            ("X-RateLimit-Reset".into(), "1900000000".into()),
        ],
    )])
    .await;
    let source = OpenAlexSource::new(Arc::new(
        openalex::OpenAlexClient::new(Some("fixture-secret")).with_base_url(url),
    ));
    let error = source.discovery_page(&plan, None, 1).await.unwrap_err();
    assert_eq!(error.kind, SourceFailureKind::RateLimited);
    assert_eq!(error.retry_after_seconds, Some(7));
    assert!(error.retryable);
    assert!(!error.message.contains("fixture-secret"));
    let quota = source.discovery_capabilities().quota.unwrap();
    assert_eq!(quota.remaining, Some(0.0));
    assert_eq!(quota.limit, Some(10000.0));
    assert_eq!(quota.reset_unix_seconds, Some(1900000000));
    assert_eq!(quota.last_http_status, 429);
    requests.await.unwrap();
    let source = BiorxivSource::new();
    assert!(!source.discovery_capabilities().searchable);
    let batch = source
        .search(&bib_types::StructuredSearch::default(), 10)
        .await
        .unwrap();
    assert!(
        batch
            .error
            .as_ref()
            .is_some_and(|e| e.contains("unsupported"))
    );
}

#[tokio::test]
async fn pubmed_rate_limit_http_date_and_real_zero_results_remain_distinct() {
    let (url, requests) = server(vec![(
        429,
        "upstream limit".into(),
        vec![("Retry-After".into(), "Fri, 01 Jan 2100 00:00:00 GMT".into())],
    )])
    .await;
    let source = PubmedSource::new(Arc::new(
        eutils::EutilsClient::new("test", "test@example.invalid", None).with_base_url(url),
    ));
    let plan = source.plan_discovery(&request("pubmed")).unwrap();
    let error = source.discovery_page(&plan, None, 1).await.unwrap_err();
    assert_eq!(error.kind, SourceFailureKind::RateLimited);
    assert!(error.retry_after_seconds.is_some_and(|n| n > 1_000_000));
    requests.await.unwrap();
    let (url, requests) = server(vec![json(
        serde_json::json!({"esearchresult":{"count":"0","retmax":"0","retstart":"0","idlist":[]}}),
    )])
    .await;
    let source = PubmedSource::new(Arc::new(
        eutils::EutilsClient::new("test", "test@example.invalid", None).with_base_url(url),
    ));
    let page = source.discovery_page(&plan, None, 1).await.unwrap();
    assert_eq!(page.total, Some(0));
    assert!(page.articles.is_empty());
    assert!(page.next_cursor.is_none());
    requests.await.unwrap();
}
