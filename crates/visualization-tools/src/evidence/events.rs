//! Pathway-scoped navigation, distinct from reading reaction evidence.
use super::*;

pub(super) async fn list(
    client: &reactome::ReactomeClient,
    input: &Input,
) -> Result<Value, ToolError> {
    if input.query.is_some() || input.species.is_some() {
        return Err(error(
            "events requires a discovered pathway id; query/species are only supported for search",
        ));
    }
    let id = input
        .id
        .as_deref()
        .filter(|id| valid_id(id))
        .ok_or_else(|| {
            error("events requires a valid pathway id from discovery or a related record")
        })?;
    let limit = input.limit.unwrap_or(100);
    if !(1..=200).contains(&limit) {
        return Err(error("events limit requires 1..200"));
    }
    let events = tokio::time::timeout(
        Duration::from_secs(30),
        client.contained_event_references(id),
    )
    .await
    .map_err(|_| error("Reactome contained events timed out"))?
    .map_err(error)?;
    if events.iter().any(|event| match event {
        reactome::types::EventReference::Id(id) => *id == 0,
        reactome::types::EventReference::Event(event) => {
            event.db_id == 0 && !event.stable_id.as_deref().is_some_and(valid_id)
        }
    }) {
        return Err(error(
            "Reactome returned an event without a usable identity",
        ));
    }
    let results: Vec<_> = events.iter().take(limit).map(|event| {
        let reactome::types::EventReference::Event(event) = event else {
            let reactome::types::EventReference::Id(id) = event else { unreachable!() };
            return json!({"id":id.to_string(),"metadata_status":"identity_reference_only"});
        };
        let id = event.stable_id.as_deref().filter(|id| valid_id(id))
            .map(str::to_owned).unwrap_or_else(|| event.db_id.to_string());
        json!({"id":id,"title":text(&event.display_name).chars().take(500).collect::<String>(),"record_type":event.schema_class,"species":event.species_name,"is_inferred":event.is_inferred,"is_in_disease":event.is_in_disease})
    }).collect();
    Ok(
        json!({"provider":"Reactome","verification":"not_verified","requested_id":id,
        "retrieval_url":format!("https://reactome.org/ContentService/data/pathway/{id}/containedEvents"),
        "coverage":"pathway_contained_event_navigation_only","server_returned_count":events.len(),
        "returned_count":results.len(),"truncated":events.len()>limit,"max_events":limit,
        "titles_truncated":events.iter().take(limit).any(|event|matches!(event,reactome::types::EventReference::Event(event) if text(&event.display_name).chars().count()>500)),
        "typed_events_sha256":format!("{:x}",Sha256::digest(serde_json::to_vec(&events).map_err(error)?)),
        "hash_scope":"serde_json_serialization_of_typed_event_list_not_http_bytes_or_full_records",
        "instruction":"Navigation metadata only: none of these event records has been read by this action. Read relevant IDs with record and compare inputs, outputs, catalyst and receptor/scaffold identity before citing. Prefer a narrow discovered receptor branch over a top-level pathway. An empty or truncated list never proves database-wide absence. No plan, canvas or proposal is changed.",
        "results":results}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };
    #[tokio::test]
    async fn pathway_navigation_is_bounded_and_does_not_present_events_as_read_records() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 2048];
            let n = stream.read(&mut request).unwrap();
            assert!(
                String::from_utf8_lossy(&request[..n])
                    .starts_with("GET /data/pathway/R-HSA-10/containedEvents ")
            );
            let body=json!([21,{"dbId":20,"stId":"R-HSA-20","displayName":"<b>Receptor reaction</b>","schemaClass":"Reaction","speciesName":"Homo sapiens"},{"dbId":21,"displayName":"Other reaction"}]).to_string();
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
        });
        let tool = ToolRegistration::from(EvidenceTool {
            client: Arc::new(reactome::ReactomeClient::new().with_endpoints(&endpoint, &endpoint)),
        });
        for input in [
            json!({"action":"events","id":"../x"}),
            json!({"action":"events","id":"R-HSA-10","limit":201}),
            json!({"action":"events","id":"R-HSA-10","species":"Homo sapiens"}),
        ] {
            assert!(tool.implementation.execute(input).await.is_err());
        }
        let result = serde_json::to_value(
            tool.implementation
                .execute(json!({"action":"events","id":"R-HSA-10","limit":2}))
                .await
                .unwrap(),
        )
        .unwrap();
        let data = &result["content"];
        assert_eq!(data["truncated"], true);
        assert_eq!(data["results"][0]["id"], "21");
        assert_eq!(
            data["results"][0]["metadata_status"],
            "identity_reference_only"
        );
        assert!(data["results"][0].get("title").is_none());
        assert_eq!(data["server_returned_count"], 3);
        assert_eq!(data["returned_count"], 2);
        assert_eq!(data["results"][1]["id"], "R-HSA-20");
        assert_eq!(data["results"][1]["title"], "Receptor reaction");
        assert_eq!(data["coverage"], "pathway_contained_event_navigation_only");
        assert!(data.get("record").is_none());
        assert!(data.get("source_url").is_none());
        assert_eq!(data["verification"], "not_verified");
        server.join().unwrap();
    }
}
