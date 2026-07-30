//! Typed response structs for the Open Targets Platform GraphQL API.
//!
//! Only the high-value scalar fields are strongly typed. Heterogeneous or
//! rarely-used nested objects are kept as [`serde_json::Value`] so the crate
//! stays resilient to upstream schema additions. All structs use
//! `#[serde(default)]` where a field may legitimately be absent and
//! `rename_all = "camelCase"` to match the GraphQL field naming.

use serde_json::Value;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Common
// ---------------------------------------------------------------------------

/// A weighted component (datatype or datasource) contributing to an
/// overall association score.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScoredComponent {
    pub id: String,
    pub score: f64,
}

// ---------------------------------------------------------------------------
// Meta
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ApiVersion {
    #[serde(default)]
    pub x: String,
    #[serde(default)]
    pub y: String,
    #[serde(default)]
    pub z: String,
    #[serde(default)]
    pub suffix: Option<String>,
}

impl ApiVersion {
    /// Format as `x.y.z` (+ optional suffix).
    pub fn as_string(&self) -> String {
        let base = format!("{}.{}.{}", self.x, self.y, self.z);
        match &self.suffix {
            Some(s) if !s.is_empty() => format!("{base}-{s}"),
            _ => base,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DataVersion {
    #[serde(default)]
    pub year: String,
    #[serde(default)]
    pub month: String,
    #[serde(default)]
    pub iteration: Option<String>,
}

impl DataVersion {
    /// Format as `YYYY.MM` (+ optional iteration).
    pub fn as_string(&self) -> String {
        let base = format!("{}.{}", self.year, self.month);
        match &self.iteration {
            Some(i) if !i.is_empty() => format!("{base}.{i}"),
            _ => base,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub product: String,
    #[serde(default)]
    pub api_version: ApiVersion,
    #[serde(default)]
    pub data_version: DataVersion,
    #[serde(default)]
    pub data_prefix: String,
    #[serde(default)]
    pub downloads: Option<String>,
    #[serde(default)]
    pub enable_data_release_prefix: bool,
}

// ---------------------------------------------------------------------------
// Target (gene)
// ---------------------------------------------------------------------------

/// Genomic coordinates of a target.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenomicLocation {
    #[serde(default)]
    pub chromosome: String,
    #[serde(default)]
    pub start: i64,
    #[serde(default)]
    pub end: i64,
    #[serde(default)]
    pub strand: i32,
}

/// A gene / target record.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    /// Ensembl gene ID, e.g. `ENSG00000012048`.
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub approved_symbol: String,
    #[serde(default)]
    pub approved_name: String,
    #[serde(default)]
    pub biotype: String,
    #[serde(default)]
    pub genomic_location: GenomicLocation,
    /// Hierarchical target-class labels (Reactome / UniProt family etc.).
    #[serde(default)]
    pub target_class: Vec<Value>,
    #[serde(default)]
    pub db_xrefs: Vec<Value>,
    #[serde(default)]
    pub synonyms: Vec<Value>,
    #[serde(default)]
    pub protein_ids: Vec<Value>,
    #[serde(default)]
    pub transcript_ids: Vec<String>,
    #[serde(default)]
    pub function_descriptions: Vec<String>,
    #[serde(default)]
    pub subcellular_locations: Vec<Value>,
    #[serde(default)]
    pub is_essential: Option<bool>,
}

// ---------------------------------------------------------------------------
// Disease
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Disease {
    /// EFO / MONDO / HP / Orphanet ID, e.g. `MONDO_0004975`.
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub is_therapeutic_area: bool,
    #[serde(default)]
    pub therapeutic_areas: Vec<Value>,
    #[serde(default)]
    pub parents: Vec<Value>,
    #[serde(default)]
    pub children: Vec<Value>,
    #[serde(default)]
    pub ancestors: Vec<Value>,
    #[serde(default)]
    pub db_xrefs: Vec<Value>,
    #[serde(default)]
    pub synonyms: Vec<Value>,
}

// ---------------------------------------------------------------------------
// Drug
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Drug {
    /// ChEMBL ID, e.g. `CHEMBL112`.
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub drug_type: String,
    #[serde(default)]
    pub maximum_clinical_stage: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub trade_names: Vec<Value>,
    #[serde(default)]
    pub synonyms: Vec<Value>,
    #[serde(default)]
    pub cross_references: Vec<Value>,
    #[serde(default)]
    pub mechanisms_of_action: Option<Value>,
    #[serde(default)]
    pub adverse_events: Option<Value>,
    #[serde(default)]
    pub indications: Option<Value>,
}

// ---------------------------------------------------------------------------
// Study
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sample {
    #[serde(default)]
    pub sample_size: i64,
    #[serde(default)]
    pub ancestry: String,
}

/// A GWAS / study record.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Study {
    /// GCST / NEALE / FINNGEN etc. study identifier.
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub project_id: String,
    #[serde(default)]
    pub trait_from_source: String,
    #[serde(default)]
    pub study_type: Option<String>,
    #[serde(default)]
    pub pubmed_id: Option<String>,
    #[serde(default)]
    pub publication_title: Option<String>,
    #[serde(default)]
    pub publication_journal: Option<String>,
    #[serde(default)]
    pub publication_first_author: Option<String>,
    #[serde(default)]
    pub publication_date: Option<String>,
    #[serde(default)]
    pub condition: Option<String>,
    #[serde(default)]
    pub n_samples: Option<i64>,
    #[serde(default)]
    pub n_cases: Option<i64>,
    #[serde(default)]
    pub n_controls: Option<i64>,
    #[serde(default)]
    pub has_sumstats: Option<bool>,
    #[serde(default)]
    pub summarystats_location: Option<String>,
    #[serde(default)]
    pub initial_sample_size: Option<String>,
    #[serde(default)]
    pub discovery_samples: Vec<Sample>,
    #[serde(default)]
    pub replication_samples: Vec<Sample>,
    #[serde(default)]
    pub cohorts: Vec<String>,
    #[serde(default)]
    pub quality_controls: Vec<String>,
    #[serde(default)]
    pub analysis_flags: Vec<String>,
}

// ---------------------------------------------------------------------------
// Variant
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Variant {
    /// `chr_pos_ref_alt` build GRCh38 identifier.
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub chromosome: String,
    #[serde(default)]
    pub position: i64,
    #[serde(default)]
    pub reference_allele: String,
    #[serde(default)]
    pub alternate_allele: String,
    #[serde(default)]
    pub variant_description: String,
    #[serde(default)]
    pub hgvs_id: Option<String>,
    #[serde(default)]
    pub rs_ids: Vec<String>,
    #[serde(default)]
    pub allele_frequencies: Vec<Value>,
    #[serde(default)]
    pub variant_effect: Vec<Value>,
    #[serde(default)]
    pub transcript_consequences: Vec<Value>,
    #[serde(default)]
    pub db_xrefs: Vec<Value>,
    #[serde(default)]
    pub most_severe_consequence: Option<Value>,
}
