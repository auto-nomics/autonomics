use reactome::types::SearchResult;
use serde_json::json;
#[test]
fn grouped_content_service_search_and_legacy_flat_hits_are_both_preserved() {
    let grouped = json!({"numberOfMatches":25,"rowCount":10,"results":[{"typeName":"Pathway","entriesCount":20,"entries":[{"dbId":"877300","stId":"R-HSA-877300","name":"<span>Interferon</span> gamma signaling","type":"Pathway","species":["Homo sapiens"]}]},{"typeName":"Reaction","entriesCount":5,"entries":[{"dbId":"873922","stId":"R-HSA-873922","name":"Phosphorylation of STAT1","type":"Reaction","species":["Homo sapiens"]}]}]});
    let result: SearchResult = serde_json::from_value(grouped).unwrap();
    assert_eq!(result.number_of_matches, 25);
    assert_eq!(result.results.len(), 2);
    assert_eq!(result.results[0].db_id, 877300);
    assert_eq!(result.results[1].stable_id.as_deref(), Some("R-HSA-873922"));
    assert!(
        result.results[0]
            .display_name
            .as_ref()
            .unwrap()
            .contains("Interferon")
    );
    let flat: SearchResult = serde_json::from_value(
        json!({"results":[{"dbId":1,"displayName":"Legacy","stId":"R-HSA-1"}]}),
    )
    .unwrap();
    assert_eq!(flat.results[0].display_name.as_deref(), Some("Legacy"));
    for invalid in [
        json!({"results":[{"entries":{}}]}),
        json!({"results":[{"entries":[{"dbId":"not-a-number"}]}]}),
    ] {
        assert!(serde_json::from_value::<SearchResult>(invalid).is_err());
    }
}
