use serde::{Deserialize, Serialize};

/// A search result identifier and relevance score.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub identifier: String,
    pub score: f64,
}

/// One page of RCSB Search API results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResponse {
    pub query_id: String,
    pub result_type: String,
    pub total_count: u64,
    pub result_set: Vec<SearchResult>,
}

/// Request options accepted by the RCSB Search API.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchRequestOptions {
    pub paginate: Pagination,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub results_content_type: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Pagination {
    pub start: u32,
    pub rows: u32,
}

/// A validated RCSB Search API query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchRequest {
    pub query: SearchQuery,
    pub return_type: SearchReturnType,
    pub request_options: SearchRequestOptions,
}

impl SearchRequest {
    /// Build a full-text query, the most common interactive search mode.
    pub fn full_text(text: impl Into<String>) -> Self {
        Self {
            query: SearchQuery::full_text(text),
            return_type: SearchReturnType::Entry,
            request_options: SearchRequestOptions {
                paginate: Pagination { start: 0, rows: 25 },
                results_content_type: vec!["experimental".into()],
            },
        }
    }

    /// Build a query from a raw terminal node's `service` and `parameters`.
    ///
    /// This is the escape hatch for attribute, sequence, structure, and
    /// chemical searches. `parameters` must match the schema for the chosen
    /// service.
    pub fn terminal(service: impl Into<String>, parameters: serde_json::Value) -> Self {
        Self {
            query: SearchQuery::terminal(service, parameters),
            return_type: SearchReturnType::Entry,
            request_options: SearchRequestOptions {
                paginate: Pagination { start: 0, rows: 25 },
                results_content_type: vec!["experimental".into()],
            },
        }
    }

    pub fn rows(mut self, rows: u32) -> Self {
        self.request_options.paginate.rows = rows;
        self
    }

    pub fn start(mut self, start: u32) -> Self {
        self.request_options.paginate.start = start;
        self
    }

    pub fn return_type(mut self, return_type: SearchReturnType) -> Self {
        self.return_type = return_type;
        self
    }
}

/// Query node in the RCSB Search API tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SearchQuery {
    Terminal {
        service: SearchService,
        parameters: serde_json::Value,
    },
    Group {
        logical_operator: LogicalOperator,
        nodes: Vec<SearchQuery>,
    },
}

impl SearchQuery {
    pub fn full_text(value: impl Into<String>) -> Self {
        Self::terminal("full_text", serde_json::json!({ "value": value.into() }))
    }

    pub fn terminal(service: impl Into<String>, parameters: serde_json::Value) -> Self {
        Self::Terminal {
            service: SearchService(service.into()),
            parameters,
        }
    }

    pub fn and(nodes: impl IntoIterator<Item = SearchQuery>) -> Self {
        Self::Group {
            logical_operator: LogicalOperator::And,
            nodes: nodes.into_iter().collect(),
        }
    }

    pub fn or(nodes: impl IntoIterator<Item = SearchQuery>) -> Self {
        Self::Group {
            logical_operator: LogicalOperator::Or,
            nodes: nodes.into_iter().collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SearchService(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogicalOperator {
    And,
    Or,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchReturnType {
    Entry,
    PolymerEntity,
    PolymerEntityInstance,
    Assembly,
    ChemComp,
}

impl SearchReturnType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Entry => "entry",
            Self::PolymerEntity => "polymer_entity",
            Self::PolymerEntityInstance => "polymer_entity_instance",
            Self::Assembly => "assembly",
            Self::ChemComp => "chem_comp",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_text_request_serializes_to_search_api_shape() {
        let request = SearchRequest::full_text("deoxyhaemoglobin").rows(2);
        let value = serde_json::to_value(request).unwrap();

        assert_eq!(value["query"]["type"], "terminal");
        assert_eq!(value["query"]["service"], "full_text");
        assert_eq!(value["query"]["parameters"]["value"], "deoxyhaemoglobin");
        assert_eq!(value["return_type"], "entry");
        assert_eq!(value["request_options"]["paginate"]["rows"], 2);
    }

    #[test]
    fn search_response_deserializes_identifiers_and_scores() {
        let response: SearchResponse = serde_json::from_str(
            r#"{
              "query_id": "q",
              "result_type": "entry",
              "total_count": 2,
              "result_set": [
                {"identifier": "2HHB", "score": 1.0},
                {"identifier": "3HHB", "score": 0.9}
              ]
            }"#,
        )
        .unwrap();

        assert_eq!(response.result_set[1].identifier, "3HHB");
        assert_eq!(response.result_set[1].score, 0.9);
    }
}
