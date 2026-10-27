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
    /// When true, positive clauses must match one evidence document.
    /// When false, clauses may match across a parent and different children.
    /// NOT always excludes the whole article.
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
    /// Query-language input. Supply either query or expression.
    pub query: String,
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
            query: String::new(),
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

/// A document-level local search page. A cursor is valid only for the same
/// request and unchanged ordered result snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchPageV2 {
    pub hits: Vec<crate::SearchHit>,
    pub total: usize,
    pub next_cursor: Option<String>,
    pub index_generation: String,
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

/// Local projection progress. Extraction and indexing have separate lifecycles.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchIndexStatus {
    pub engine: String,
    pub source_revision: u64,
    pub indexed_revision: Option<u64>,
    pub pending_articles: usize,
    pub indexed_documents: usize,
    pub durable: bool,
    pub last_error: Option<String>,
    pub building: bool,
    pub processed_documents: usize,
    pub target_revision: Option<u64>,
}
