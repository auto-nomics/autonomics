//! Read-only curated source discovery; returned records never establish a figure claim.
use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{sync::Arc, time::Duration};
fn error(e: impl std::fmt::Display) -> ToolError {
    ToolError::ExecutionFailed {
        source: Box::new(std::io::Error::other(e.to_string())),
    }
}
fn valid_id(id: &str) -> bool {
    if id.is_empty() || id.len() > 40 {
        return false;
    }
    if id.bytes().all(|b| b.is_ascii_digit()) {
        return id.parse::<u64>().is_ok_and(|n| n > 0);
    }
    let parts: Vec<_> = id.split('-').collect();
    parts.len() == 3
        && parts[0] == "R"
        && parts[1].len() == 3
        && parts[1].bytes().all(|b| b.is_ascii_uppercase())
        && parts[2].split('.').count() <= 2
        && parts[2]
            .split('.')
            .all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
}
fn text(input: &str) -> String {
    let mut output = String::new();
    let mut rest = input;
    while let Some(start) = rest.find('<') {
        output.push_str(&rest[..start]);
        let Some(end) = rest[start..].find('>').map(|end| start + end) else {
            output.push_str(&rest[start..]);
            return output;
        };
        let tag = rest[start + 1..end]
            .trim()
            .trim_start_matches('/')
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        if ![
            "span", "i", "b", "sup", "sub", "p", "br", "em", "strong", "a", "div",
        ]
        .contains(&tag.as_str())
        {
            output.push('<');
            rest = &rest[start + 1..];
            continue;
        }
        if ["p", "br", "div"].contains(&tag.as_str()) {
            output.push('\n');
        }
        rest = &rest[end + 1..];
    }
    output.push_str(rest);
    output
}
#[tool(
    name = "figure_evidence",
    description = "Read-only Reactome source discovery for scientific figure planning. Actions search and record. If organism/stimulus scope is missing, perform only lightweight provisional discovery and ask up to three essential clarifications before detailed retrieval; do not enumerate every competing branch. Zero hits, 404s and retrieval failures never demonstrate biological absence or species exclusivity. Search uses query and optional explicit species (never assume organism) and returns candidate record IDs, titles and source URLs; a search hit is not scientific evidence. record requires an ID returned by search and retrieves the pathway/reaction/entity record, curated summation, species, inference/disease flags, inputs/outputs, catalysts, regulators, events and literature references where present. Follow returned event/entity IDs to resolve abstraction and check each intended relationship; a broad pathway summary does not validate every edge, directness, species or experiment. Projection has explicit omission/truncation flags and is not the full source record. URLs identify sources, not proof of claim correctness; verification is not_verified. Copy actual relevant record source_url into node/edge evidence only after reading it, preserve scope/uncertainty in assumptions/questions, and ask for missing scientific intent. Retrieval never saves a plan, changes a canvas or adopts a proposal. Network failures are explicit; never invent a source or silently substitute an unrelated record."
)]
pub struct Input {
    #[desc = "search | record"]
    pub action: String,
    #[desc = "Full-text phrase for search, 1..300 characters"]
    pub query: Option<String>,
    #[desc = "Optional explicit species filter for search, e.g. Homo sapiens"]
    pub species: Option<String>,
    #[desc = "Stable Reactome ID or positive dbId obtained from search or a related record; required for record"]
    pub id: Option<String>,
    #[desc = "Search output limit 1..20, defaults to 10; server results may themselves be a limited window"]
    pub limit: Option<usize>,
}
struct EvidenceTool {
    client: Arc<reactome::ReactomeClient>,
}
fn note(notes: &mut Vec<String>, path: &str) {
    if notes.len() < 100 {
        notes.push(path.into());
    }
}
fn project(
    value: &Value,
    depth: u8,
    path: &str,
    notes: &mut Vec<String>,
    omitted: &mut bool,
) -> Value {
    match value {
        Value::String(s) => {
            let clean = text(s);
            if clean.chars().count() > 6000 {
                note(notes, path);
            }
            json!(clean.chars().take(6000).collect::<String>())
        }
        Value::Array(values) => {
            if depth == 0 {
                if !values.is_empty() {
                    note(notes, path);
                }
                return json!([]);
            }
            if values.len() > 40 {
                note(notes, path);
            }
            Value::Array(
                values
                    .iter()
                    .take(40)
                    .enumerate()
                    .map(|(i, v)| project(v, depth - 1, &format!("{path}[{i}]"), notes, omitted))
                    .collect(),
            )
        }
        Value::Object(map) => {
            let mut result = serde_json::Map::new();
            for (k, v) in map {
                let identity = [
                    "dbId",
                    "stId",
                    "stIdVersion",
                    "displayName",
                    "schemaClass",
                    "name",
                ];
                let allowed = (identity.contains(&k.as_str())
                    && (depth > 0 || !v.is_object() && !v.is_array()))
                    || depth > 0
                        && [
                            "text",
                            "species",
                            "speciesName",
                            "taxId",
                            "stoichiometry",
                            "isInferred",
                            "isInDisease",
                            "hasDiagram",
                            "releaseDate",
                            "lastUpdatedDate",
                            "reviewStatus",
                            "summation",
                            "literatureReference",
                            "hasEvent",
                            "input",
                            "output",
                            "catalystActivity",
                            "physicalEntity",
                            "activeUnit",
                            "regulatedBy",
                            "regulator",
                            "regulationType",
                            "compartment",
                            "referenceEntity",
                            "identifier",
                            "databaseName",
                            "title",
                            "journal",
                            "year",
                            "pages",
                            "pubMedIdentifier",
                            "doi",
                            "url",
                        ]
                        .contains(&k.as_str());
                if allowed {
                    result.insert(
                        k.clone(),
                        project(
                            v,
                            depth.saturating_sub(1),
                            &format!("{path}.{k}"),
                            notes,
                            omitted,
                        ),
                    );
                } else {
                    *omitted = true;
                    if depth == 0 {
                        note(notes, path);
                    }
                }
            }
            Value::Object(result)
        }
        other => other.clone(),
    }
}
#[async_trait]
impl ToolFunction for EvidenceTool {
    type Input = Input;
    async fn run(&self, input: Input) -> Result<ToolResult, ToolError> {
        let common = json!({"provider":"Reactome","verification":"not_verified"});
        let mut output = common;
        match input.action.as_str() {
            "search" => {
                if input.id.is_some() {
                    return Err(error("id is only supported for record"));
                }
                let query = input
                    .query
                    .as_deref()
                    .map(str::trim)
                    .filter(|q| !q.is_empty() && q.chars().count() <= 300)
                    .ok_or_else(|| error("query requires 1..300 characters"))?;
                let limit = input.limit.unwrap_or(10);
                if !(1..=20).contains(&limit) {
                    return Err(error("limit requires 1..20"));
                }
                if input
                    .species
                    .as_deref()
                    .is_some_and(|s| s.trim().is_empty() || s.chars().count() > 100)
                {
                    return Err(error(
                        "species must be an explicit nonblank filter of at most 100 characters",
                    ));
                }
                let result = tokio::time::timeout(
                    Duration::from_secs(30),
                    self.client.search(
                        query,
                        input.species.as_deref(),
                        Some(&["Pathway", "Reaction"]),
                    ),
                )
                .await
                .map_err(|_| error("Reactome search timed out"))?
                .map_err(error)?;
                if result
                    .results
                    .iter()
                    .any(|hit| hit.db_id == 0 && !hit.stable_id.as_deref().is_some_and(valid_id))
                {
                    return Err(error(
                        "Reactome search returned a hit without a usable record ID",
                    ));
                }
                let count = result.results.len();
                let hits:Vec<_>=result.results.iter().take(limit).map(|hit|{let id=hit.stable_id.as_deref().filter(|id|valid_id(id)).map(str::to_owned).unwrap_or_else(||hit.db_id.to_string());json!({"id":id,"title":hit.display_name.as_deref().map(text),"record_type":hit.schema_class.as_ref().or(hit.entry_type.as_ref()),"species":hit.species,"source_url":format!("https://reactome.org/content/detail/{id}")})}).collect();
                output["query"] = json!(query);
                output["species_filter"] = json!(input.species);
                output["matches"] = json!(result.total_results.unwrap_or(result.number_of_matches));
                output["server_window_count"] = json!(count);
                output["returned_count"] = json!(hits.len());
                output["complete"] = json!(false);
                output["results"] = json!(hits);
            }
            "record" => {
                if input.query.is_some() || input.species.is_some() || input.limit.is_some() {
                    return Err(error(
                        "query/species/limit are only supported for search; record returns the species stored in that record",
                    ));
                }
                let id=input.id.as_deref().filter(|id|valid_id(id)).ok_or_else(||error("id requires a stable Reactome ID or positive numeric dbId from a retrieved record"))?;
                let record = tokio::time::timeout(Duration::from_secs(30), self.client.query(id))
                    .await
                    .map_err(|_| error("Reactome record timed out"))?
                    .map_err(error)?;
                let db_id = record["dbId"]
                    .as_u64()
                    .or_else(|| record["dbId"].as_str().and_then(|v| v.parse().ok()));
                if !record.is_object() || db_id.is_none_or(|id| id == 0) {
                    return Err(error("Reactome returned an unsupported record shape"));
                }
                let canonical = record
                    .get("stId")
                    .and_then(Value::as_str)
                    .filter(|v| valid_id(v))
                    .unwrap_or(id);
                let hash = format!(
                    "{:x}",
                    Sha256::digest(serde_json::to_vec(&record).map_err(error)?)
                );
                let mut notes = Vec::new();
                let mut omitted = false;
                let projected = project(&record, 4, "record", &mut notes, &mut omitted);
                output["requested_id"] = json!(id);
                output["source_url"] = if canonical.starts_with("R-") {
                    json!(format!("https://reactome.org/content/detail/{canonical}"))
                } else {
                    json!(format!(
                        "https://reactome.org/ContentService/data/query/{canonical}"
                    ))
                };
                output["retrieval_url"] = json!(format!(
                    "https://reactome.org/ContentService/data/query/{id}"
                ));
                output["record_json_sha256"] = json!(hash);
                output["hash_scope"] =
                    json!("serde_json_serialization_of_retrieved_record_not_http_bytes");
                output["record"] = projected;
                output["projection"] = json!({"complete":false,"omitted_fields":omitted,"truncated_paths":notes,"max_depth":4,"max_list_items":40,"max_text_chars":6000,"truncation_list_complete":notes.len()<100,"instruction":"Follow relevant event/entity IDs for detail; source retrieval does not verify a figure claim"});
            }
            _ => {
                return Err(error(
                    "unknown figure_evidence action; use search or record",
                ));
            }
        }
        output["retrieved_at"] = json!(chrono::Utc::now().to_rfc3339());
        Ok(ToolResult::success_json(output))
    }
}
pub fn registrations() -> Vec<ToolRegistration> {
    vec![ToolRegistration::from(EvidenceTool {
        client: Arc::new(reactome::ReactomeClient::new()),
    })]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::atomic::{AtomicUsize, Ordering},
    };
    #[tokio::test]
    async fn registered_source_discovery_preserves_record_context_and_never_writes_a_figure() {
        let record = json!({"dbId":20,"stId":"R-HSA-20","displayName":"Declared reaction","schemaClass":"ReactionLikeEvent","isInferred":true,"species":[{"dbId":1,"displayName":"Homo sapiens"}],"summation":[{"text":"STAT1 < 5 units remains an explicit comparison; <i>curated</i> text."}],"input":[{"dbId":2,"stId":"R-HSA-2","displayName":"Substrate"}],"output":[{"dbId":3,"displayName":"Product"}],"catalystActivity":[{"dbId":4,"physicalEntity":{"dbId":5,"displayName":"Kinase"}}],"literatureReference":[{"pubMedIdentifier":12345,"title":"Supplied citation"}],"unmodeled_metadata":"not silently claimed complete"});
        let search = json!({"numberOfMatches":50,"rowCount":10,"results":[{"typeName":"Reaction","entriesCount":50,"entries":[{"dbId":"20","stId":"R-HSA-20","name":"<span class='highlighting'>Declared</span> reaction","type":"Reaction","species":["Homo sapiens"]}]}]});
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let count = Arc::new(AtomicUsize::new(0));
        let received = count.clone();
        let server_record = record.clone();
        let server = std::thread::spawn(move || {
            for mut stream in listener.incoming().take(2).map(Result::unwrap) {
                let mut request = Vec::new();
                loop {
                    let mut buf = [0; 1024];
                    let n = stream.read(&mut buf).unwrap();
                    request.extend_from_slice(&buf[..n]);
                    if n == 0 || request.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                received.fetch_add(1, Ordering::SeqCst);
                let request = String::from_utf8_lossy(&request);
                let body = if request.starts_with("GET /search/query?") {
                    search.to_string()
                } else {
                    assert!(request.starts_with("GET /data/query/R-HSA-20 "));
                    server_record.to_string()
                };
                write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
            }
        });
        let tool = ToolRegistration::from(EvidenceTool {
            client: Arc::new(reactome::ReactomeClient::new().with_endpoints(&endpoint, &endpoint)),
        });
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("never-created.sqlite");
        assert!(
            crate::evidence_registrations()
                .iter()
                .any(|t| t.definition.name == "figure_evidence")
        );
        let search:Value=serde_json::to_value(tool.implementation.execute(json!({"action":"search","query":"Declared reaction","species":"Homo sapiens","limit":1})).await.unwrap()).unwrap();
        assert_eq!(search["content"]["results"][0]["id"], "R-HSA-20");
        assert_eq!(
            search["content"]["results"][0]["title"],
            "Declared reaction"
        );
        assert_eq!(search["content"]["matches"], 50);
        assert_eq!(search["content"]["complete"], false);
        let result: Value = serde_json::to_value(
            tool.implementation
                .execute(json!({"action":"record","id":"R-HSA-20"}))
                .await
                .unwrap(),
        )
        .unwrap();
        let data = &result["content"];
        assert_eq!(data["verification"], "not_verified");
        assert_eq!(
            data["source_url"],
            "https://reactome.org/content/detail/R-HSA-20"
        );
        assert_eq!(data["record"]["isInferred"], true);
        assert_eq!(
            data["record"]["summation"][0]["text"],
            "STAT1 < 5 units remains an explicit comparison; curated text."
        );
        assert_eq!(data["record"]["input"][0]["stId"], "R-HSA-2");
        assert_eq!(data["record"]["output"][0]["displayName"], "Product");
        assert_eq!(
            data["record"]["literatureReference"][0]["pubMedIdentifier"],
            12345
        );
        assert_eq!(
            data["record_json_sha256"],
            format!("{:x}", Sha256::digest(serde_json::to_vec(&record).unwrap()))
        );
        assert_eq!(data["projection"]["complete"], false);
        assert_eq!(data["projection"]["omitted_fields"], true);
        for input in [
            json!({"action":"record","id":"../private"}),
            json!({"action":"record","id":"https://example.org"}),
            json!({"action":"record","id":"R-HSA-20","species":"Homo sapiens"}),
            json!({"action":"search","query":"x","limit":0}),
            json!({"action":"search","query":"x","id":"20"}),
        ] {
            assert!(tool.implementation.execute(input).await.is_err());
        }
        server.join().unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 2);
        assert!(!path.exists());
    }
    #[test]
    fn projection_is_bounded_and_plain_text_does_not_destroy_mathematical_comparisons() {
        assert_eq!(
            text("x < 5 and y > 2; <span class='highlighting'>STAT1</span>"),
            "x < 5 and y > 2; STAT1"
        );
        assert_eq!(text("x < 5"), "x < 5");
        let record = json!({"summation":[{"text":"λ".repeat(7000)}],"input":(0..45).map(|i|json!({"dbId":i,"displayName":format!("N{i}")})).collect::<Vec<_>>(),"name":[[[[["deep"]]]]]});
        let mut notes = Vec::new();
        let mut omitted = false;
        let result = project(&record, 4, "record", &mut notes, &mut omitted);
        assert_eq!(result["input"].as_array().unwrap().len(), 40);
        assert_eq!(
            result["summation"][0]["text"]
                .as_str()
                .unwrap()
                .chars()
                .count(),
            6000
        );
        assert!(notes.iter().any(|p| p == "record.input"));
        assert!(notes.iter().any(|p| p == "record.summation[0].text"));
        assert!(!result.to_string().contains("deep"));
    }
}
