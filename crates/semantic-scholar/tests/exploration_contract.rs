//! Local protocol fixtures, not evidence of live provider coverage or quality.
use semantic_scholar::{S2Client, S2Error, relation_types::*};
use serde_json::json;
use std::time::{Duration, Instant};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

async fn server(
    responses: Vec<(u16, serde_json::Value)>,
) -> (S2Client, tokio::task::JoinHandle<Vec<(String, Instant)>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let mut requests = Vec::new();
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
            requests.push((String::from_utf8(request).unwrap(), Instant::now()));
            let body = value.to_string();
            let retry = if status == 429 {
                "Retry-After: 1\r\n"
            } else {
                ""
            };
            socket.write_all(format!("HTTP/1.1 {status} Fixture\r\nConnection: close\r\nContent-Length: {}\r\n{retry}\r\n{body}",body.len()).as_bytes()).await.unwrap();
        }
        requests
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
async fn unresolved_papers_keep_raw_window_and_share_pacing_across_endpoints() {
    let (client, task) = server(vec![
        (200,json!({"offset":5,"next":8,"data":[{"citedPaper":{"paperId":"a","title":"Resolved"}},{"citedPaper":{"paperId":null,"title":"Unresolved title"}},{"citedPaper":null}]})),
        (200,json!({"recommendedPapers":[]})),
    ]).await;
    let page = client
        .get_relations("DOI:10.1/a", PaperRelation::References, 3, 5, None)
        .await
        .unwrap();
    assert_eq!(page.next, Some(8));
    assert_eq!(page.data.len(), 3);
    assert!(
        page.data[0]
            .paper(PaperRelation::References)
            .unwrap()
            .resolved()
            .is_some()
    );
    let Some(RelationPaper::Unresolved(raw)) = page.data[1].paper(PaperRelation::References) else {
        panic!()
    };
    assert_eq!(raw["title"], "Unresolved title");
    assert!(page.data[2].paper(PaperRelation::References).is_none());
    client
        .clone()
        .recommendations_from("a", RecommendationPool::AllComputerScience, 2, None)
        .await
        .unwrap();
    let calls = task.await.unwrap();
    assert!(
        calls[0]
            .0
            .starts_with("GET /paper/DOI%3A10.1%2Fa/references?")
    );
    assert!(calls[0].0.contains("offset=5"));
    assert!(calls[1].0.contains("from=all-cs"));
    assert!(calls[1].1.duration_since(calls[0].1) >= Duration::from_millis(1000));
}

#[tokio::test]
async fn inconsistent_graph_windows_and_malformed_resolved_metadata_fail() {
    let bodies = vec![
        json!({"offset":1,"data":[]}),
        json!({"offset":0,"next":0,"data":[]}),
        json!({"offset":0,"next":3,"data":[{"citedPaper":null}]}),
        json!({"offset":0,"data":[{"citingPaper":{"paperId":"a"}}]}),
        json!({"offset":0,"data":[{"citedPaper":{"paperId":7}}]}),
        json!({"offset":0,"data":[{"citedPaper":{"paperId":"a","year":"bad"}}]}),
        json!({"offset":0}),
    ];
    for body in bodies {
        let (client, task) = server(vec![(200, body)]).await;
        assert!(
            client
                .get_relations("a", PaperRelation::References, 1, 0, None)
                .await
                .is_err()
        );
        task.await.unwrap();
    }
}

#[tokio::test]
async fn recommendations_require_envelope_and_cannot_silently_exceed_limit() {
    for body in [
        json!({}),
        json!({"recommendedPapers":null}),
        json!({"recommendedPapers":[{"paperId":"a"},{"paperId":"b"}]}),
    ] {
        let (client, task) = server(vec![(200, body)]).await;
        assert!(
            client
                .recommendations_from("a", RecommendationPool::Recent, 1, None)
                .await
                .is_err()
        );
        task.await.unwrap();
    }
    let client = S2Client::new().with_graph_base_url("http://127.0.0.1:1");
    assert!(matches!(
        client
            .get_relations("a", PaperRelation::References, 0, 0, None)
            .await,
        Err(S2Error::Param(_))
    ));
    assert!(matches!(
        client
            .recommendations_from("a", RecommendationPool::Recent, 501, None)
            .await,
        Err(S2Error::Param(_))
    ));
}

#[tokio::test]
async fn graph_rate_limit_cooldown_applies_to_recommendation_clone() {
    let (client, task) = server(vec![
        (429, json!({})),
        (200, json!({"recommendedPapers":[]})),
    ])
    .await;
    assert!(matches!(
        client
            .get_relations("a", PaperRelation::Citations, 1, 0, None)
            .await,
        Err(S2Error::RateLimited {
            retry_after_seconds: Some(1)
        })
    ));
    client
        .clone()
        .recommendations_from("a", RecommendationPool::Recent, 1, None)
        .await
        .unwrap();
    let calls = task.await.unwrap();
    assert!(calls[1].1.duration_since(calls[0].1) >= Duration::from_millis(1000));
}
