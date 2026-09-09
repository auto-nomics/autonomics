//! Typed models for Reactome API responses.
//!
//! Reactome adds endpoint-specific fields across releases. These models keep
//! the fields used by SDK methods and summaries stable and ignore unknown
//! response fields. Raw JSON remains available through
//! [`crate::ReactomeClient::get`] for unmodeled data.

use serde::{Deserialize, Serialize};

// ===========================================================================
// Database
// ===========================================================================

/// Database identity returned by `GET /data/database/name` and `version`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseInfo {
    pub name: String,
    pub version: String,
}

// ===========================================================================
// Species
// ===========================================================================

/// One Reactome species.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Species {
    pub db_id: u64,
    #[serde(rename = "displayName")]
    pub display_name: String,
    #[serde(default)]
    pub name: Vec<String>,
    #[serde(rename = "taxId")]
    pub tax_id: String,
    #[serde(default)]
    pub abbreviation: Option<String>,
    #[serde(rename = "schemaClass")]
    pub schema_class: Option<String>,
}

// ===========================================================================
// Pathway / Event
// ===========================================================================

/// A Reactome pathway or reaction (Event subtypes).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Pathway {
    #[serde(rename = "dbId")]
    pub db_id: u64,
    #[serde(rename = "displayName")]
    pub display_name: String,
    #[serde(rename = "stId")]
    pub stable_id: Option<String>,
    #[serde(rename = "stIdVersion")]
    pub stable_id_version: Option<String>,
    #[serde(default)]
    pub name: Vec<String>,
    #[serde(rename = "isInDisease", default)]
    pub is_in_disease: bool,
    #[serde(rename = "isInferred", default)]
    pub is_inferred: bool,
    #[serde(rename = "hasDiagram", default)]
    pub has_diagram: bool,
    #[serde(rename = "hasEHLD", default)]
    pub has_ehld: bool,
    #[serde(default)]
    pub species: Vec<serde_json::Value>,
    #[serde(rename = "speciesName")]
    pub species_name: Option<String>,
    #[serde(rename = "schemaClass")]
    pub schema_class: Option<String>,
    #[serde(rename = "releaseDate")]
    pub release_date: Option<String>,
}

/// Ancestor node in the event hierarchy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventAncestor {
    #[serde(rename = "dbId")]
    pub db_id: u64,
    #[serde(rename = "displayName")]
    pub display_name: String,
    #[serde(rename = "stId")]
    pub stable_id: Option<String>,
    #[serde(rename = "schemaClass")]
    pub schema_class: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ancestors: Option<Vec<EventAncestor>>,
}

// ===========================================================================
// PhysicalEntity / Participants
// ===========================================================================

/// A physical entity participant in a pathway or reaction.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Participant {
    #[serde(rename = "dbId")]
    pub db_id: u64,
    #[serde(rename = "displayName")]
    pub display_name: Option<String>,
    #[serde(rename = "stId")]
    pub stable_id: Option<String>,
    #[serde(rename = "schemaClass")]
    pub schema_class: Option<String>,
    #[serde(rename = "referenceEntity")]
    pub reference_entity: Option<ReferenceEntity>,
}

/// Cross-referenced external database entity.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReferenceEntity {
    #[serde(rename = "dbId")]
    pub db_id: u64,
    #[serde(rename = "displayName")]
    pub display_name: Option<String>,
    pub identifier: Option<String>,
    #[serde(rename = "databaseName")]
    pub database_name: Option<String>,
}

// ===========================================================================
// Search
// ===========================================================================

/// Search result entry from `/search/query`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchResult {
    #[serde(rename = "totalResults")]
    pub total_results: Option<u64>,
    #[serde(rename = "numberOfMatches", default)]
    pub number_of_matches: u64,
    #[serde(rename = "rowCount", default)]
    pub row_count: u64,
    #[serde(default)]
    pub results: Vec<SearchEntry>,
    #[serde(default)]
    pub facets: Vec<serde_json::Value>,
}

/// A single search hit.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchEntry {
    #[serde(rename = "dbId")]
    pub db_id: u64,
    #[serde(rename = "displayName")]
    pub display_name: Option<String>,
    #[serde(rename = "stId")]
    pub stable_id: Option<String>,
    #[serde(rename = "schemaClass")]
    pub schema_class: Option<String>,
    #[serde(rename = "type")]
    pub entry_type: Option<String>,
    #[serde(default)]
    pub species: Vec<serde_json::Value>,
    #[serde(default)]
    pub compartment: Option<String>,
}

// ===========================================================================
// Mapping
// ===========================================================================

/// Result of mapping an external identifier to Reactome pathways.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MappedPathway {
    #[serde(rename = "dbId")]
    pub db_id: u64,
    #[serde(rename = "displayName")]
    pub display_name: String,
    #[serde(rename = "stId")]
    pub stable_id: Option<String>,
    #[serde(rename = "evidenceType")]
    pub evidence_type: Option<serde_json::Value>,
    #[serde(default)]
    pub species: Vec<serde_json::Value>,
    #[serde(rename = "schemaClass")]
    pub schema_class: Option<String>,
}

// ===========================================================================
// Analysis
// ===========================================================================

/// Analysis summary returned after submitting identifiers.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AnalysisSummary {
    pub token: String,
    #[serde(rename = "projection", default)]
    pub projection: bool,
    #[serde(rename = "interactors", default)]
    pub interactors: bool,
    #[serde(rename = "type")]
    pub analysis_type: Option<String>,
    #[serde(rename = "sampleName", default)]
    pub sample_name: Option<String>,
    #[serde(rename = "text", default)]
    pub text: bool,
    #[serde(rename = "includeDisease", default)]
    pub include_disease: bool,
}

/// Resource usage summary in an analysis result.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResourceSummary {
    pub resource: String,
    pub pathways: u64,
    pub filtered: u64,
}

/// Species summary in an analysis result.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SpeciesSummary {
    #[serde(rename = "dbId")]
    pub db_id: u64,
    #[serde(rename = "taxId")]
    pub tax_id: String,
    pub name: String,
    pub pathways: u64,
    pub filtered: u64,
}

/// A single enriched pathway from the analysis result.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EnrichedPathway {
    #[serde(rename = "stId")]
    pub stable_id: String,
    pub name: String,
    #[serde(rename = "dbId")]
    pub db_id: Option<u64>,
    pub entities: PathwayEntities,
    #[serde(default)]
    pub species: Option<serde_json::Value>,
    #[serde(rename = "inDisease", default)]
    pub in_disease: bool,
}

/// Entity statistics for an enriched pathway.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PathwayEntities {
    pub total: u64,
    pub found: u64,
    #[serde(rename = "foundInteractors", default)]
    pub found_interactors: u64,
    #[serde(rename = "totalInteractors", default)]
    pub total_interactors: u64,
    #[serde(rename = "ratio", default)]
    pub ratio: f64,
    #[serde(rename = "pValue")]
    pub p_value: Option<f64>,
    pub fdr: Option<f64>,
    #[serde(rename = "timesAncestors")]
    pub times_ancestors: Option<u64>,
}

/// Full analysis result body returned by `GET /token/{token}`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AnalysisResult {
    pub summary: AnalysisSummary,
    #[serde(rename = "pathwaysFound", default)]
    pub pathways_found: u64,
    #[serde(rename = "identifiersNotFound", default)]
    pub identifiers_not_found: u64,
    #[serde(default)]
    pub pathways: Vec<EnrichedPathway>,
    #[serde(rename = "resourceSummary", default)]
    pub resource_summary: Vec<ResourceSummary>,
    #[serde(rename = "speciesSummary", default)]
    pub species_summary: Vec<SpeciesSummary>,
    #[serde(default)]
    pub warnings: Vec<String>,
}
