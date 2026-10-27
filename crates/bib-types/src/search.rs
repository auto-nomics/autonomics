//! Versioned, backend-neutral query contract for bibliography search.
//!
//! The AST represents user intent. Backend adapters can translate only the
//! operations they support and must report unsupported clauses explicitly.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Bibliographic or textual field addressed by a search expression.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SearchField {
    Any,
    Identifier,
    Title,
    Creator,
    Abstract,
    Publication,
    Year,
    Keyword,
    Tag,
    Note,
    AttachmentContent,
}

/// Boolean and fielded search expression.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SearchExpr {
    Term {
        value: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        field: Option<SearchField>,
    },
    Phrase {
        value: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        field: Option<SearchField>,
    },
    And {
        clauses: Vec<SearchExpr>,
    },
    Or {
        clauses: Vec<SearchExpr>,
    },
    Not {
        clause: Box<SearchExpr>,
    },
    Range {
        field: SearchField,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gte: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lte: Option<String>,
    },
}

/// The local bibliography region searched by a request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(default)]
pub struct SearchScope {
    pub collection_id: Option<String>,
    pub include_descendants: bool,
    pub unfiled: bool,
    /// When true, each child attachment or note must satisfy its own clause.
    /// When false, matching clauses may be satisfied across a parent and child.
    pub require_same_child: bool,
}

/// Requested ordering for results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum SearchSort {
    #[default]
    Relevance,
    Newest,
    Oldest,
    Title,
    CitationCount,
}

/// Unified query request. Cursors are opaque and scoped to the query and index
/// generation that produced them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct SearchRequestV2 {
    pub expression: Option<SearchExpr>,
    pub scope: SearchScope,
    pub sort: SearchSort,
    pub page_size: u16,
    pub cursor: Option<String>,
    /// Empty or absent means use the service's configured defaults.
    pub sources: Vec<String>,
    /// `lexical` preserves exact query semantics; `hybrid` may include semantic recall.
    pub mode: SearchMode,
}

impl Default for SearchRequestV2 {
    fn default() -> Self {
        Self {
            expression: None,
            scope: SearchScope::default(),
            sort: SearchSort::default(),
            page_size: 50,
            cursor: None,
            sources: Vec::new(),
            mode: SearchMode::Lexical,
        }
    }
}

/// Recall strategy. Hybrid search must preserve exact identifier and Boolean
/// constraints when those are present in the expression.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum SearchMode {
    #[default]
    Lexical,
    Hybrid,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_expression_round_trips() {
        let request = SearchRequestV2 {
            expression: Some(SearchExpr::And {
                clauses: vec![
                    SearchExpr::Phrase {
                        value: "gene editing".into(),
                        field: Some(SearchField::Title),
                    },
                    SearchExpr::Not {
                        clause: Box::new(SearchExpr::Term {
                            value: "retracted".into(),
                            field: None,
                        }),
                    },
                ],
            }),
            scope: SearchScope {
                collection_id: Some("review".into()),
                include_descendants: true,
                ..Default::default()
            },
            ..Default::default()
        };

        let encoded = serde_json::to_string(&request).unwrap();
        let decoded: SearchRequestV2 = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, request);
        assert_eq!(SearchRequestV2::default().page_size, 50);
    }
}
