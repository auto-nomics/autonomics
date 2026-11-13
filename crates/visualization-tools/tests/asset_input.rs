use serde_json::json;

#[tokio::test]
async fn registered_asset_tools_explain_invalid_review_links_before_accessing_storage() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("must-not-create.sqlite");
    let tools = [
        visualization_tools::bioart_registrations(path.clone()).remove(0),
        visualization_tools::bioicons_registrations(path.clone()).remove(0),
    ];
    for tool in tools {
        let error = tool.implementation.execute(json!({"action":"replace","markup_ids":{}})).await.unwrap_err().to_string();
        assert!(error.contains("input.markup_ids"), "{error}");
        assert!(error.contains("[] is valid"), "{error}");
        assert!(!path.exists());
        // The valid array gets past decoding and reaches ordinary scope validation.
        let error = tool.implementation.execute(json!({"action":"replace","markup_ids":[]})).await.unwrap_err().to_string();
        assert!(!error.contains("expected a sequence"), "{error}");
        assert!(!error.contains("[] is valid"), "{error}");
        assert!(!path.exists());
    }
}

#[tokio::test]
async fn bioicons_author_filter_rejects_invalid_or_nonsearch_use_before_storage() {
    let directory=tempfile::tempdir().unwrap();let path=directory.path().join("must-not-create.sqlite");
    let tool=visualization_tools::bioicons_registrations(path.clone()).remove(0);
    for input in [json!({"action":"preview","author":"A"}),json!({"action":"search","query":"protein","author":" "}),json!({"action":"search_batch","queries":["protein"],"author":"x".repeat(301)})] {
        let error=tool.implementation.execute(input).await.unwrap_err().to_string();
        assert!(error.contains("author"),"{error}");assert!(!path.exists());
    }
}
