//! P5 graph/list qualification with a local HTTP server, not live judgments.
use bib_base::literature_exploration::*;
use semantic_scholar::{S2Client, relation_types::RecommendationPool};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

fn request(kind: ExplorationKind) -> ExplorationRequest {
    ExplorationRequest {
        seed: "a".repeat(40),
        kind,
        page_size: 3,
        cursor: None,
        deadline_ms: 5_000,
    }
}
fn paper() -> Value {
    json!({"paperId":"b".repeat(40),"title":"Fixture study","year":2024,"externalIds":{"DOI":"10.1234/fixture"}})
}
async fn fixture(responses: Vec<(u16, Value)>) -> (S2Client, tokio::task::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let mut calls = Vec::new();
        for (status, value) in responses {
            let (mut socket, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept())
                .await
                .unwrap()
                .unwrap();
            let mut request = Vec::new();
            loop {
                let mut buf = [0u8; 2048];
                let n = socket.read(&mut buf).await.unwrap();
                assert!(n > 0);
                request.extend_from_slice(&buf[..n]);
                assert!(request.len() < 32768);
                if request.windows(4).any(|s| s == b"\r\n\r\n") {
                    break;
                }
            }
            calls.push(String::from_utf8(request).unwrap());
            let body = value.to_string();
            let retry = if status == 429 {
                "Retry-After: 7\r\n"
            } else {
                ""
            };
            socket.write_all(format!("HTTP/1.1 {status} Fixture\r\nConnection: close\r\nContent-Length: {}\r\n{retry}\r\n{body}",body.len()).as_bytes()).await.unwrap();
        }
        calls
    });
    (
        S2Client::new()
            .with_graph_base_url(&origin)
            .with_reco_base_url(&origin)
            .with_max_retries(0),
        task,
    )
}

#[tokio::test]
async fn references_preserve_unresolved_positions_original_next_and_edge_direction() {
    let (client,server)=fixture(vec![
        (200,json!({"offset":0,"next":3,"data":[{"citedPaper":paper(),"contexts":["Original context"],"intents":["background"],"isInfluential":true},{"citedPaper":{"paperId":null,"title":"Unresolved title"}},{"citedPaper":null}]})),
        (200,json!({"offset":3,"data":[{"citedPaper":paper()}]})),
    ]).await;
    let mut input = request(ExplorationKind::References);
    let page = explore(&client, input.clone()).await.unwrap();
    assert_eq!(page.items.len(), 3);
    assert_eq!(page.unresolved_records, 2);
    assert!(!page.upstream_snapshot);
    assert_eq!(
        page.items[1].unresolved.as_ref().unwrap()["title"],
        "Unresolved title"
    );
    assert_eq!(page.items[2].unresolved, Some(Value::Null));
    assert_eq!(
        page.items[0].contexts,
        Some(vec!["Original context".into()])
    );
    let edge = page.items[0].citation.as_ref().unwrap();
    assert_eq!(edge.citing_paper_id, "a".repeat(40));
    assert_eq!(edge.cited_paper_id, "b".repeat(40));
    assert_eq!(edge.retrieved_at, page.retrieved_at);
    assert_eq!(
        page.items[0].article.as_ref().unwrap().id,
        "doi:10.1234/fixture"
    );
    input.cursor = page.next_cursor;
    let next = explore(&client, input).await.unwrap();
    assert_eq!(next.items[0].position, 3);
    assert!(next.next_cursor.is_none());
    assert!(server.await.unwrap()[1].contains("offset=3"));
}

#[tokio::test]
async fn citations_reverse_edges_and_cursors_bind_seed_kind_and_page_size() {
    let (client, server) = fixture(vec![(
        200,
        json!({"offset":0,"next":1,"data":[{"citingPaper":paper()}]}),
    )])
    .await;
    let input = request(ExplorationKind::Citations);
    let page = explore(&client, input.clone()).await.unwrap();
    let edge = page.items[0].citation.as_ref().unwrap();
    assert_eq!(edge.citing_paper_id, "b".repeat(40));
    assert_eq!(edge.cited_paper_id, "a".repeat(40));
    for which in 0..3 {
        let mut changed = input.clone();
        changed.cursor = page.next_cursor.clone();
        match which {
            0 => changed.seed = "c".repeat(40),
            1 => changed.kind = ExplorationKind::References,
            _ => changed.page_size = 2,
        }
        assert!(matches!(
            explore(&client, changed).await,
            Err(ExplorationError::CursorExpired)
        ));
    }
    assert_eq!(server.await.unwrap().len(), 1);
}

#[tokio::test]
async fn similar_papers_keep_explicit_pool_without_creating_citation_edges() {
    for pool in [
        RecommendationPool::Recent,
        RecommendationPool::AllComputerScience,
    ] {
        let (client, server) = fixture(vec![(200, json!({"recommendedPapers":[paper()]}))]).await;
        let page = explore(&client, request(ExplorationKind::Similar { pool }))
            .await
            .unwrap();
        assert_eq!(page.items.len(), 1);
        assert!(page.items[0].citation.is_none());
        assert!(page.next_cursor.is_none());
        assert_eq!(page.kind, ExplorationKind::Similar { pool });
        assert!(server.await.unwrap()[0].contains(&format!("from={}", pool.as_str())));
    }
}

#[tokio::test]
async fn unresolved_entries_never_turn_into_citation_edges_or_importable_articles() {
    let (client, server) = fixture(vec![(
        200,
        json!({"offset":0,"data":[{"citedPaper":{"title":"Missing identity"}},{}]}),
    )])
    .await;
    let page = explore(&client, request(ExplorationKind::References))
        .await
        .unwrap();
    assert_eq!(page.unresolved_records, 2);
    assert!(
        page.items
            .iter()
            .all(|item| item.article.is_none() && item.citation.is_none())
    );
    server.await.unwrap();
}

#[tokio::test]
async fn failures_preserve_retry_semantics_and_do_not_expose_upstream_body() {
    use bib_types::discovery::SourceFailureKind;
    for (status, kind, retryable) in [
        (429, SourceFailureKind::RateLimited, true),
        (401, SourceFailureKind::AuthRequired, false),
        (404, SourceFailureKind::InvalidQuery, false),
        (503, SourceFailureKind::Upstream, true),
    ] {
        let (client, server) =
            fixture(vec![(status, json!({"private":"must not be exposed"}))]).await;
        let error = explore(&client, request(ExplorationKind::Citations))
            .await
            .unwrap_err();
        assert!(!format!("{error:?}").contains("must not be exposed"));
        let ExplorationError::Upstream(failure) = error else {
            panic!()
        };
        assert_eq!(failure.kind, kind);
        assert_eq!(failure.retryable, retryable);
        if status == 429 {
            assert_eq!(failure.retry_after_seconds, Some(7));
        }
        server.await.unwrap();
    }
}

#[tokio::test]
async fn invalid_metadata_and_duplicate_recommendations_fail_the_page() {
    let mut invalid = paper();
    invalid["year"] = json!(10000);
    let (client, server) = fixture(vec![(
        200,
        json!({"offset":0,"data":[{"citingPaper":invalid}]}),
    )])
    .await;
    assert!(matches!(
        explore(&client, request(ExplorationKind::Citations)).await,
        Err(ExplorationError::Upstream(_))
    ));
    server.await.unwrap();
    let (client, server) =
        fixture(vec![(200, json!({"recommendedPapers":[paper(),paper()]}))]).await;
    assert!(matches!(
        explore(
            &client,
            request(ExplorationKind::Similar {
                pool: RecommendationPool::Recent
            })
        )
        .await,
        Err(ExplorationError::Upstream(_))
    ));
    server.await.unwrap();
}

#[tokio::test]
async fn invalid_requests_fail_before_network_io() {
    let client = S2Client::new().with_graph_base_url("http://127.0.0.1:1");
    for which in 0..4 {
        let mut input = request(ExplorationKind::References);
        match which {
            0 => input.seed = "title:Fixture".into(),
            1 => input.page_size = 0,
            2 => input.page_size = 101,
            _ => input.cursor = Some("x".repeat(2049)),
        }
        assert!(matches!(
            explore(&client, input).await,
            Err(ExplorationError::Invalid(_) | ExplorationError::CursorExpired)
        ));
    }
}

#[tokio::test]
async fn operation_deadline_covers_shared_pacing_without_starting_another_request() {
    let (client, server) = fixture(vec![(200, json!({"offset":0,"data":[]}))]).await;
    explore(&client, request(ExplorationKind::References))
        .await
        .unwrap();
    assert_eq!(server.await.unwrap().len(), 1);
    let mut input = request(ExplorationKind::Citations);
    input.deadline_ms = 100;
    let result = explore(&client, input).await;
    let Err(ExplorationError::Upstream(failure)) = result else {
        panic!()
    };
    assert_eq!(
        failure.kind,
        bib_types::discovery::SourceFailureKind::Timeout
    );
    assert!(failure.retryable);
}

#[tokio::test]
async fn exact_doi_seed_lookup_is_verified_before_graph_traversal() {
    let (client, server) =
        fixture(vec![(200, paper()), (200, json!({"offset":0,"data":[]}))]).await;
    let mut input = request(ExplorationKind::References);
    input.seed = "https://doi.org/10.1234/fixture".into();
    let page = explore(&client, input).await.unwrap();
    assert_eq!(page.seed, "b".repeat(40));
    let calls = server.await.unwrap();
    assert!(calls[0].contains("DOI%3A10.1234%2Ffixture"));
    assert!(calls[1].contains(&format!("/paper/{}/references?", "b".repeat(40))));
    let (client, server) = fixture(vec![(200, paper())]).await;
    let mut input = request(ExplorationKind::Citations);
    input.seed = "DOI:10.1234/different".into();
    assert!(matches!(
        explore(&client, input).await,
        Err(ExplorationError::Upstream(_))
    ));
    assert_eq!(server.await.unwrap().len(), 1);
}
