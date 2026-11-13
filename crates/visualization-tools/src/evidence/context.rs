//! Navigation context is not a full read of the ancestor pathway records.
use super::*;
fn clipped(value: &str, limit: usize) -> String {
    text(value).chars().take(limit).collect()
}
pub(super) async fn source_context(
    client: &reactome::ReactomeClient,
    id: &str,
    record: &Value,
) -> Value {
    let species = record["speciesName"]
        .as_str()
        .map(|s| vec![clipped(s, 200)])
        .unwrap_or_else(|| {
            record["species"]
                .as_array()
                .into_iter()
                .flatten()
                .take(20)
                .filter_map(|s| s["displayName"].as_str().map(|s| clipped(s, 200)))
                .collect()
        });
    let catalysts: Vec<_> = record["catalystActivity"]
        .as_array()
        .into_iter()
        .flatten()
        .take(20)
        .filter_map(|activity| {
            activity["displayName"]
                .as_str()
                .or_else(|| activity["physicalEntity"]["displayName"].as_str())
                .map(|s| clipped(s, 1000))
        })
        .collect();
    let mut context = json!({"species_names":species,"catalyst_activity_names":catalysts,"species_names_truncated":record["speciesName"].as_str().is_some_and(|s|text(s).chars().count()>200)||record["species"].as_array().is_some_and(|a|a.len()>20||a.iter().take(20).any(|v|v["displayName"].as_str().is_some_and(|s|text(s).chars().count()>200))),"catalyst_list_truncated":record["catalystActivity"].as_array().is_some_and(|a|a.len()>20||a.iter().take(20).any(|v|v["displayName"].as_str().or_else(||v["physicalEntity"]["displayName"].as_str()).is_some_and(|s|text(s).chars().count()>1000))),"instruction":"Compare catalyst receptor/scaffold identity and pathway context with the requested mechanism. The same substrate/product and enzyme name do not establish receptor-specific applicability. Ancestor paths are navigation metadata, not full reads of those pathway records or proof of claim correctness. Truncation, unavailable context or absent terms never proves biological absence."});
    let event = matches!(
        record["schemaClass"].as_str(),
        Some(
            "Pathway"
                | "TopLevelPathway"
                | "Reaction"
                | "ReactionLikeEvent"
                | "BlackBoxEvent"
                | "Polymerisation"
                | "Depolymerisation"
                | "FailedReaction"
        )
    );
    if !event {
        context["pathway_context"] = json!({"status":"not_requested_for_schema_class"});
        return context;
    }
    let retrieval_url = format!("https://reactome.org/ContentService/data/event/{id}/ancestors");
    context["pathway_context"] = match tokio::time::timeout(
        Duration::from_secs(8),
        client.event_ancestors(id),
    )
    .await
    {
        Ok(Ok(paths)) => {
            let expected = record["dbId"]
                .as_u64()
                .or_else(|| record["dbId"].as_str().and_then(|id| id.parse().ok()));
            if paths
                .iter()
                .any(|path| path.first().is_none_or(|node| Some(node.db_id) != expected))
            {
                json!({"status":"unavailable","retrieval_url":retrieval_url,"error":"Returned ancestor paths do not begin with the retrieved event identity; context was not accepted."})
            } else {
                project_paths(&paths, &retrieval_url)
            }
        }
        Ok(Err(_)) => {
            json!({"status":"unavailable","retrieval_url":retrieval_url,"error":"Pathway-context request failed; the record read remains available but its pathway context is unknown."})
        }
        Err(_) => {
            json!({"status":"unavailable","retrieval_url":retrieval_url,"error":"Pathway-context request timed out; do not assume a matching or absent pathway."})
        }
    };
    context
}
fn project_paths(paths: &[Vec<reactome::types::EventAncestor>], retrieval_url: &str) -> Value {
    let truncated = paths.len() > 8
        || paths.iter().take(8).any(|p| {
            p.len() > 8
                || p.iter()
                    .take(8)
                    .any(|n| text(&n.display_name).chars().count() > 200)
        });
    let projected:Vec<Vec<_>>=paths.iter().take(8).map(|path|path.iter().take(8).map(|node|json!({"id":node.stable_id.as_deref().filter(|id|valid_id(id)).map(str::to_owned).unwrap_or_else(||node.db_id.to_string()),"title":clipped(&node.display_name,200),"record_type":node.schema_class})).collect()).collect();
    json!({"status":"available","retrieval_url":retrieval_url,"coverage":"event_ancestor_membership_metadata_only","path_count":paths.len(),"returned_path_count":projected.len(),"max_paths":8,"max_nodes_per_path":8,"truncated":truncated,"paths":projected,"typed_paths_sha256":format!("{:x}",Sha256::digest(serde_json::to_vec(paths).expect("typed paths serialize"))),"hash_scope":"serde_json_serialization_of_typed_ancestor_paths_not_http_bytes_or_full_records"})
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn non_event_context_skips_network_and_reports_name_truncation() {
        let client = reactome::ReactomeClient::new()
            .with_endpoints("http://127.0.0.1:9", "http://127.0.0.1:9");
        let record = json!({"schemaClass":"Complex","speciesName":"X".repeat(300),"catalystActivity":[{"displayName":"Y".repeat(1100)}]});
        let result = source_context(&client, "R-HSA-1", &record).await;
        assert_eq!(
            result["pathway_context"]["status"],
            "not_requested_for_schema_class"
        );
        assert_eq!(result["species_names_truncated"], true);
        assert_eq!(result["catalyst_list_truncated"], true);
    }
    #[test]
    fn ancestor_projection_is_bounded_and_never_presents_paths_as_full_records() {
        let node = reactome::types::EventAncestor {
            db_id: 1,
            display_name: "X".repeat(300),
            stable_id: Some("R-HSA-1".into()),
            schema_class: Some("Pathway".into()),
            ancestors: None,
        };
        let result = project_paths(&vec![vec![node; 10]; 10], "retrieval");
        assert_eq!(result["path_count"], 10);
        assert_eq!(result["returned_path_count"], 8);
        assert_eq!(result["paths"][0].as_array().unwrap().len(), 8);
        assert_eq!(result["truncated"], true);
        assert_eq!(result["paths"][0][0]["title"].as_str().unwrap().len(), 200);
        assert_eq!(
            result["coverage"],
            "event_ancestor_membership_metadata_only"
        );
        assert!(result.get("record").is_none());
    }
}
