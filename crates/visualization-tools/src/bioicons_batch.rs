//! Bounded independent lexical searches; results never establish scientific suitability.
use serde_json::{Value, json};
pub(crate) fn validate(queries: &[String]) -> Result<(), &'static str> {
    if queries.is_empty() || queries.len() > 8 {
        return Err("search_batch requires queries with 1..8 distinct nonblank strings");
    }
    let mut seen = std::collections::HashSet::new();
    for query in queries {
        if query.trim().is_empty() || query.len() > 256 || !seen.insert(query.trim().to_lowercase())
        {
            return Err(
                "search_batch queries must be nonblank, at most 256 UTF-8 bytes each, and distinct ignoring surrounding whitespace/case",
            );
        }
    }
    Ok(())
}
#[cfg(test)]
pub(crate) async fn search(client: &::bioicons::BioIconsClient, queries: &[String]) -> Value {
    search_with_author(client, queries, None).await
}
pub(crate) async fn search_with_author(client: &::bioicons::BioIconsClient, queries: &[String], author: Option<&str>) -> Value {
    let mut items = Vec::new();
    let mut successes = 0;
    // Shared client caches the catalogue. Sequential searches avoid duplicate upstream work;
    // batching removes agent round trips, not the per-query matching or semantic review.
    for (index, query) in queries.iter().enumerate() {
        match client.search_with_author(query, 0, 4, author).await {
            Ok(page) => {
                successes += 1;
                items.push(json!({"query":query,"status":"success","result":crate::catalog_search::with_scope("BioIcons",query,json!(page))}));
            }
            Err(error) => {
                items.push(json!({"query":query,"status":"error","error":error.to_string()}));
                // Validated queries and fixed pagination cannot fail locally: search errors
                // originate in the shared catalogue. Do not retry the same failed load
                // for every remaining concept in this batch.
                for remaining in &queries[index + 1..] {
                    items.push(json!({"query":remaining,"status":"not_attempted","reason":"shared_catalogue_unavailable","proves_catalogue_absence":false}));
                }
                break;
            }
        }
    }
    let failures = items
        .iter()
        .filter(|item| item["status"] == "error")
        .count();
    let not_attempted = items
        .iter()
        .filter(|item| item["status"] == "not_attempted")
        .count();
    json!({"action":"search_batch","author_filter":author,"requested":queries.len(),"successful":successes,"failed":failures,"not_attempted":not_attempted,"all_queries_succeeded":successes==queries.len(),"limit_per_query":4,"items":items,"scientific_suitability_assessed":false,"proves_catalogue_absence":false,"next_step":"Inspect chosen assets with preview; refine individual queries with search for more results. Lexical hits and zero hits do not establish biological identity or catalogue-wide absence."})
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};
    #[tokio::test]
    async fn author_filter_is_retained_for_every_query_including_zero_hits() {
        let server=MockServer::start().await;
        Mock::given(path("/icons/icons.json")).respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"name":"protein","category":"Protein","license":"cc-0","author":"A"},
            {"name":"receptor","category":"Protein","license":"cc-by-4.0","author":"B"}
        ]))).expect(1).mount(&server).await;
        Mock::given(path("/icons/authors.json")).respond_with(ResponseTemplate::new(200).set_body_json(json!({}))).mount(&server).await;
        let client=::bioicons::BioIconsClient::with_endpoint(&format!("{}/",server.uri())).unwrap();
        let result=search_with_author(&client,&["protein".into(),"receptor".into()],Some("A")).await;
        assert_eq!(result["author_filter"],"A");assert_eq!(result["successful"],2);
        assert_eq!(result["items"][0]["result"]["total"],1);
        assert_eq!(result["items"][1]["result"]["total"],0);
        for item in result["items"].as_array().unwrap(){
            assert_eq!(item["result"]["search_scope"]["author_filter"],"A");
            assert_eq!(item["result"]["search_scope"]["proves_catalogue_absence"],false);
        }
    }
    #[tokio::test]
    async fn batch_keeps_each_query_scope_licence_ranking_and_zero_hits_without_downloading_art() {
        let server = MockServer::start().await;
        let mut rows=(0..6).map(|i|json!({"name":format!("protein {i}"),"category":"Cell","license":"cc-by-4.0","author":"Author"})).collect::<Vec<_>>();
        rows.push(
            json!({"name":"receptor","category":"Cell","license":"cc-by-4.0","author":"Author"}),
        );
        Mock::given(path("/icons/icons.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(rows))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(path("/icons/authors.json"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"Author":"https://example.org/author"})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let client =
            ::bioicons::BioIconsClient::with_endpoint(&format!("{}/", server.uri())).unwrap();
        let queries = vec!["protein".into(), "receptor".into(), "missing".into()];
        validate(&queries).unwrap();
        let result = search(&client, &queries).await;
        assert_eq!(result["successful"], 3);
        assert_eq!(result["failed"], 0);
        for (i, query) in queries.iter().enumerate() {
            let item = &result["items"][i];
            assert_eq!(item["query"], *query);
            assert_eq!(item["result"]["search_scope"]["query"], *query);
            assert_eq!(
                item["result"]["search_scope"]["scientific_suitability_assessed"],
                false
            );
            assert_eq!(
                item["result"]["search_scope"]["proves_catalogue_absence"],
                false
            );
        }
        let proteins = &result["items"][0]["result"];
        assert_eq!(proteins["total"], 6);
        assert_eq!(proteins["match_summary"]["name_tokens"], 6);
        assert_eq!(proteins["assets"].as_array().unwrap().len(), 4);
        for asset in proteins["assets"].as_array().unwrap() {
            assert_eq!(asset["author"], "Author");
            assert_eq!(asset["license"], "cc-by-4.0");
            assert!(asset["source_url"].is_string());
            assert_eq!(asset["catalog_sha256"].as_str().unwrap().len(), 64);
        }
        assert_eq!(
            result["items"][1]["result"]["match_summary"]["exact_name"],
            1
        );
        assert_eq!(result["items"][2]["result"]["total"], 0);
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }
    #[tokio::test]
    async fn shared_catalogue_failure_is_not_retried_or_reported_as_zero_hits() {
        let server = MockServer::start().await;
        Mock::given(path("/icons/icons.json"))
            .respond_with(ResponseTemplate::new(503))
            .expect(1)
            .mount(&server)
            .await;
        let client =
            ::bioicons::BioIconsClient::with_endpoint(&format!("{}/", server.uri())).unwrap();
        let result = search(&client, &["protein".into(), "receptor".into()]).await;
        assert_eq!(result["successful"], 0);
        assert_eq!(result["failed"], 1);
        assert_eq!(result["not_attempted"], 1);
        assert_eq!(result["all_queries_succeeded"], false);
        assert_eq!(result["items"][0]["status"], "error");
        assert_eq!(result["items"][1]["status"], "not_attempted");
        for item in result["items"].as_array().unwrap() {
            assert!(item.get("result").is_none());
        }
    }
    #[tokio::test]
    async fn invalid_batches_fail_before_network_or_storage() {
        let directory = tempfile::tempdir().unwrap();
        let store = directory.path().join("absent.sqlite");
        let tool = crate::bioicons::registrations(store.clone()).remove(0);
        for input in [
            json!({"action":"search_batch"}),
            json!({"action":"search_batch","queries":"protein"}),
            json!({"action":"search_batch","queries":[1]}),
            json!({"action":"parts","queries":["protein"]}),
            json!({"action":"search_batch","queries":[]}),
            json!({"action":"search_batch","queries":["a"," A "]}),
            json!({"action":"search_batch","queries":[" "]}),
            json!({"action":"search_batch","queries":vec!["x";9]}),
            json!({"action":"search_batch","queries":["x".repeat(257)]}),
            json!({"action":"search_batch","query":"x","queries":["x"]}),
            json!({"action":"search","queries":["x"]}),
        ] {
            assert!(tool.implementation.execute(input).await.is_err());
            assert!(!store.exists());
        }
    }
    #[tokio::test]
    #[ignore = "opt-in live author-filter catalogue metadata check; no image downloads or store writes"]
    async fn live_registered_author_search_keeps_all_query_scopes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("must-not-create.sqlite");
        let tool = crate::bioicons::registrations(path.clone()).remove(0);
        let first = tool.implementation.execute(json!({"action":"search","query":"protein"})).await.unwrap();
        let first = serde_json::to_value(first).unwrap();
        let author = first["content"]["assets"][0]["author"].as_str().expect("live catalogue protein author");
        let single = tool.implementation.execute(json!({"action":"search","query":"protein","author":author})).await.unwrap();
        let single = serde_json::to_value(single).unwrap();
        assert_eq!(single["content"]["search_scope"]["author_filter"],author);
        let batch = tool.implementation.execute(json!({"action":"search_batch","queries":["protein","receptor","codex-author-filter-nohit-7b2e"],"author":author})).await.unwrap();
        let batch = serde_json::to_value(batch).unwrap();
        let content = &batch["content"];assert_eq!(content["successful"],3);
        for item in content["items"].as_array().unwrap() {
            assert_eq!(item["result"]["search_scope"]["author_filter"],author);
            assert_eq!(item["result"]["search_scope"]["proves_catalogue_absence"],false);
            for asset in item["result"]["assets"].as_array().unwrap() {assert_eq!(asset["author"],author);}
        }
        assert_eq!(content["items"][2]["result"]["total"],0);
        assert!(!path.exists());
        println!("{}",json!({"passed":true,"author":author,"queries":content["items"].as_array().unwrap().iter().map(|i|json!({"query":i["query"],"total":i["result"]["total"],"scope":i["result"]["search_scope"]})).collect::<Vec<_>>(),"store_created":false,"image_downloaded":false}));
    }
    #[tokio::test]
    #[ignore = "opt-in live BioIcons catalogue smoke; downloads catalogue metadata only"]
    async fn live_registered_batch_search_keeps_scopes_without_creating_a_store() {
        let directory = tempfile::tempdir().unwrap();
        let store = directory.path().join("absent.sqlite");
        let tool = crate::bioicons::registrations(store.clone()).remove(0);
        let result = tool
            .implementation
            .execute(json!({"action":"search_batch","queries":["receptor","protein","nucleus"]}))
            .await
            .unwrap();
        let value = serde_json::to_value(result).unwrap();
        let content = &value["content"];
        assert_eq!(content["successful"], 3);
        assert_eq!(content["failed"], 0);
        assert!(!store.exists());
        let summary=content["items"].as_array().unwrap().iter().map(|item|json!({"query":item["query"],"total":item["result"]["total"],"returned":item["result"]["assets"].as_array().unwrap().len(),"scope":item["result"]["search_scope"]})).collect::<Vec<_>>();
        println!(
            "{}",
            json!({"live_registered_batch":summary,"store_created":false})
        );
    }
}
