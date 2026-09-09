//! Typed models for supported Ensembl REST endpoints.
//!
//! Ensembl adds endpoint-specific fields across releases. These models keep
//! the fields used by SDK methods and summaries stable and ignore unknown
//! response fields. Raw JSON remains available through
//! [`EnsemblClient::get`](crate::EnsemblClient::get) for unmodeled data.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One Ensembl species in `/info/species`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Species {
    /// Machine species name, e.g. `homo_sapiens`.
    pub name: String,
    /// Human-readable display name.
    pub display_name: Option<String>,
    /// Lowercase common name.
    pub common_name: Option<String>,
    /// Ensembl division.
    pub division: String,
    /// Ensembl release.
    pub release: u32,
    /// Stable assembly name.
    pub assembly: Option<String>,
    /// INSDC assembly accession.
    pub accession: Option<String>,
    /// NCBI taxonomy identifier as a string.
    pub taxon_id: String,
    /// Alternative species names accepted by the API.
    #[serde(default)]
    pub aliases: Vec<String>,
    /// Available database groups.
    #[serde(default)]
    pub groups: Vec<String>,
}

/// Response from `/info/species`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeciesResponse {
    #[serde(default)]
    pub species: Vec<Species>,
}

/// A coordinate region in assembly metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssemblyRegion {
    pub name: String,
    pub length: u64,
    pub coord_system: String,
}

/// Response from `/info/assembly/{species}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssemblyInfo {
    pub assembly_name: String,
    pub assembly_accession: Option<String>,
    pub assembly_date: Option<String>,
    pub default_coord_system_version: Option<String>,
    #[serde(default)]
    pub coord_system_versions: Vec<String>,
    pub genebuild_method: Option<String>,
    pub genebuild_start_date: Option<String>,
    pub genebuild_last_geneset_update: Option<String>,
    pub genebuild_initial_release_date: Option<String>,
    pub golden_path: Option<f64>,
    #[serde(default)]
    pub karyotype: Vec<String>,
    #[serde(default)]
    pub top_level_region: Vec<AssemblyRegion>,
}

/// Minimal coordinates shared by lookup and overlap features.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Coordinates {
    pub start: i64,
    pub end: i64,
    pub strand: i8,
    pub seq_region_name: String,
    pub assembly_name: Option<String>,
}

/// An Ensembl translation returned inside an expanded transcript.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Translation {
    pub id: String,
    #[serde(default)]
    pub start: Option<i64>,
    #[serde(default)]
    pub end: Option<i64>,
    #[serde(default)]
    pub length: Option<u64>,
    #[serde(default, rename = "Parent")]
    pub parent: Option<String>,
}

/// An Ensembl exon returned inside an expanded transcript.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Exon {
    pub id: String,
    pub start: i64,
    pub end: i64,
    pub strand: i8,
    pub seq_region_name: String,
}

/// An expanded Ensembl transcript.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transcript {
    pub id: String,
    #[serde(default, rename = "Parent")]
    pub parent: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub biotype: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub length: Option<u64>,
    #[serde(default)]
    pub is_canonical: Option<u8>,
    #[serde(default, rename = "Translation")]
    pub translation: Option<Translation>,
    #[serde(default, rename = "Exon")]
    pub exons: Vec<Exon>,
    #[serde(flatten)]
    pub coordinates: Coordinates,
}

/// An Ensembl gene, transcript, exon, or other lookup object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LookupEntry {
    pub id: String,
    #[serde(default)]
    pub species: Option<String>,
    #[serde(default)]
    pub object_type: Option<String>,
    #[serde(default)]
    pub biotype: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub canonical_transcript: Option<String>,
    #[serde(default)]
    pub version: Option<u32>,
    #[serde(default)]
    pub db_type: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub logic_name: Option<String>,
    #[serde(default, rename = "Transcript")]
    pub transcripts: Vec<Transcript>,
    #[serde(flatten)]
    pub coordinates: Coordinates,
}

/// A cross-reference returned by `/xrefs/id/{id}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Xref {
    pub dbname: String,
    pub db_display_name: Option<String>,
    pub display_id: Option<String>,
    pub primary_id: String,
    pub description: Option<String>,
    pub info_text: Option<String>,
    pub info_type: Option<String>,
    pub version: Option<String>,
    #[serde(default)]
    pub synonyms: Vec<String>,
}

/// A sequence response from `/sequence/id` or `/sequence/region`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sequence {
    pub query: String,
    pub id: Option<String>,
    pub seq: String,
    pub desc: Option<String>,
    pub molecule: Option<String>,
    pub version: Option<u32>,
}

/// A transcript consequence in a VEP response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptConsequence {
    pub transcript_id: String,
    #[serde(default)]
    pub gene_id: Option<String>,
    #[serde(default)]
    pub gene_symbol: Option<String>,
    #[serde(default)]
    pub consequence_terms: Vec<String>,
    #[serde(default)]
    pub impact: Option<String>,
    #[serde(default)]
    pub variant_allele: Option<String>,
    #[serde(default)]
    pub hgvs_t: Option<String>,
    #[serde(default)]
    pub hgvs_p: Option<String>,
    #[serde(default)]
    pub hgvsc: Option<String>,
    #[serde(default)]
    pub hgvsp: Option<String>,
    #[serde(default)]
    pub protein_start: Option<u64>,
    #[serde(default)]
    pub protein_end: Option<u64>,
    #[serde(default)]
    pub amino_acids: Option<String>,
    #[serde(default)]
    pub codons: Option<String>,
    #[serde(default)]
    pub canonical: Option<Value>,
}

/// A VEP annotation for one input identifier or region.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VepResult {
    pub id: String,
    #[serde(default)]
    pub input: Option<String>,
    #[serde(default)]
    pub most_severe_consequence: Option<String>,
    #[serde(default)]
    pub allele_string: Option<String>,
    #[serde(default)]
    pub assembly_name: Option<String>,
    #[serde(default)]
    pub seq_region_name: Option<String>,
    #[serde(default)]
    pub start: Option<i64>,
    #[serde(default)]
    pub end: Option<i64>,
    #[serde(default)]
    pub strand: Option<i8>,
    #[serde(default)]
    pub transcript_consequences: Vec<TranscriptConsequence>,
    #[serde(default)]
    pub colocated_variants: Vec<Value>,
}

/// A variation coordinate mapping.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VariationMapping {
    pub seq_region_name: String,
    pub start: i64,
    pub end: i64,
    pub strand: i8,
    pub assembly_name: String,
    pub location: String,
    pub allele_string: Option<String>,
    pub coord_system: Option<String>,
    pub ancestral_allele: Option<String>,
}

/// Response from `/variation/{species}/{id}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Variation {
    pub name: String,
    #[serde(default, rename = "MAF")]
    pub maf: Option<f64>,
    #[serde(default)]
    pub minor_allele: Option<String>,
    #[serde(default)]
    pub minor_allele_freq: Option<f64>,
    #[serde(default)]
    pub ambiguity: Option<String>,
    #[serde(default)]
    pub var_class: Option<String>,
    #[serde(default)]
    pub most_severe_consequence: Option<String>,
    #[serde(default)]
    pub clinical_significance: Vec<String>,
    #[serde(default)]
    pub evidence: Vec<String>,
    #[serde(default)]
    pub synonyms: Vec<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub mappings: Vec<VariationMapping>,
}
