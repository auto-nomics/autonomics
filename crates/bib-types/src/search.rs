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
    pub library_id: Option<i64>,
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
    /// Default fields for unfielded text; explicit fields retain their meaning.
    pub default_fields: Vec<SearchField>,
    pub scope: SearchScope,
    pub sort: SearchSort,
    pub filters: SearchFilters,
    /// Return disjunctive facets for the query and scope, before pagination.
    pub include_facets: bool,
    /// Batch-hydrate only the current page for GUI presentation.
    pub include_articles: bool,
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
            default_fields: Vec::new(),
            scope: SearchScope::default(),
            sort: SearchSort::default(),
            filters: SearchFilters::default(),
            include_facets: false,
            include_articles: false,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub articles: Vec<crate::Article>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub associations: Vec<crate::CollectionArticle>,
    pub total: usize,
    pub next_cursor: Option<String>,
    pub index_generation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facets: Option<SearchFacets>,
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

/// Exact metadata predicates: OR within each list, AND between fields.
/// Empty lists impose no restriction; year bounds exclude missing years.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct SearchFilters {
    pub tag_ids: Vec<i64>,
    pub creator_ids: Vec<i64>,
    pub publication_types: Vec<String>,
    pub year_from: Option<u16>,
    pub year_to: Option<u16>,
    /// A local/file attachment or extracted text, excluding URL-only links.
    pub has_fulltext: Option<bool>,
    pub has_annotations: Option<bool>,
    /// Match any attachment in this state. An article may be both indexed
    /// and failed when it has several attachments. Missing means no state.
    pub fulltext_status: Option<SearchFulltextStatus>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SearchFulltextStatus {
    Indexed,
    Pending,
    Failed,
    Missing,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchFacets {
    pub tags: Vec<SearchFacetValue>,
    pub creators: Vec<SearchFacetValue>,
    pub publication_types: Vec<SearchFacetValue>,
    pub years: Vec<SearchFacetValue>,
    pub fulltext_status: Vec<SearchFacetValue>,
}

/// Counts are distinct parent articles; each facet ignores its own selection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchFacetValue {
    pub value: String,
    pub label: String,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedSearch {
    pub id: String,
    pub name: String,
    pub schema_version: u32,
    pub version: u64,
    pub request: SearchRequestV2,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveSearchInput {
    pub name: String,
    pub request: SearchRequestV2,
    /// Required when updating; prevents overwriting another window's edits.
    pub expected_version: Option<u64>,
}

/// Half-open Unicode code point range in the returned snippet (not UTF-8 bytes
/// or JavaScript UTF-16 code units).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchHighlight {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchEvidenceRequest {
    pub search: SearchRequestV2,
    /// Reject stale evidence after edits rather than silently relocating it.
    pub index_generation: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchEvidence {
    pub article_id: String,
    pub item_key: String,
    pub attachment_key: Option<String>,
    pub attachment_hash: Option<String>,
    pub annotation_key: Option<String>,
    pub annotation_id: Option<String>,
    pub matched_field: String,
    pub snippet: String,
    pub highlight_ranges: Vec<SearchHighlight>,
    pub matched_query: String,
    pub page: Option<u32>,
    pub content_revision: String,
    /// metadata, annotation, or text_search; page is only supplied from a
    /// stored annotation anchor, never inferred from extracted text.
    pub location_status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchEvidencePage {
    pub evidence: Vec<SearchEvidence>,
    /// Distinct evidence objects, not occurrence count or article count.
    pub total: usize,
    pub next_cursor: Option<String>,
    pub index_generation: String,
}
