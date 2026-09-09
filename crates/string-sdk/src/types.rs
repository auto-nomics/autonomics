use serde::{Deserialize, Serialize};

/// Current stable STRING version advertised by the API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Version {
    #[serde(rename = "string_version")]
    pub string_version: String,
    #[serde(rename = "stable_address")]
    pub stable_address: String,
}

/// One input identifier resolved by STRING.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StringId {
    #[serde(rename = "queryIndex")]
    pub query_index: u32,
    #[serde(rename = "queryItem", default)]
    pub query_item: Option<String>,
    #[serde(rename = "stringId")]
    pub string_id: String,
    #[serde(rename = "ncbiTaxonId")]
    pub ncbi_taxon_id: u64,
    #[serde(rename = "taxonName", default)]
    pub taxon_name: Option<String>,
    #[serde(rename = "preferredName", default)]
    pub preferred_name: Option<String>,
    #[serde(default)]
    pub annotation: Option<String>,
}

/// One STRING protein-protein interaction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Interaction {
    #[serde(rename = "stringId_A")]
    pub string_id_a: String,
    #[serde(rename = "stringId_B")]
    pub string_id_b: String,
    #[serde(rename = "preferredName_A", default)]
    pub preferred_name_a: Option<String>,
    #[serde(rename = "preferredName_B", default)]
    pub preferred_name_b: Option<String>,
    #[serde(rename = "ncbiTaxonId")]
    pub ncbi_taxon_id: TaxonId,
    pub score: f64,
    #[serde(default)]
    pub nscore: f64,
    #[serde(default)]
    pub fscore: f64,
    #[serde(default)]
    pub pscore: f64,
    #[serde(default)]
    pub ascore: f64,
    #[serde(default)]
    pub escore: f64,
    #[serde(default)]
    pub dscore: f64,
    #[serde(default)]
    pub tscore: f64,
}

/// JSON numbers in some STRING network endpoints are quoted. This type
/// accepts either representation while exposing the canonical numeric value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TaxonId {
    Number(u64),
    Text(String),
}

impl TaxonId {
    /// Numeric taxon ID when the endpoint returned a number or numeric string.
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Self::Number(value) => Some(*value),
            Self::Text(value) => value.parse().ok(),
        }
    }
}

impl From<TaxonId> for String {
    fn from(value: TaxonId) -> Self {
        match value {
            TaxonId::Number(value) => value.to_string(),
            TaxonId::Text(value) => value,
        }
    }
}

/// A protein homology score between two STRING identifiers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Homology {
    #[serde(rename = "ncbiTaxonId_A")]
    pub ncbi_taxon_id_a: TaxonId,
    #[serde(rename = "stringId_A")]
    pub string_id_a: String,
    #[serde(rename = "ncbiTaxonId_B")]
    pub ncbi_taxon_id_b: TaxonId,
    #[serde(rename = "stringId_B")]
    pub string_id_b: String,
    #[serde(deserialize_with = "lenient_f64")]
    pub bitscore: f64,
}

/// Decode JSON numbers even when STRING serializes them as strings.
fn lenient_f64<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match serde_json::Value::deserialize(deserializer)? {
        serde_json::Value::Number(value) => value
            .as_f64()
            .ok_or_else(|| serde::de::Error::invalid_type(serde::de::Unexpected::Map, &"an f64")),
        serde_json::Value::String(value) => value
            .trim()
            .parse()
            .map_err(|_| serde::de::Error::custom("expected a numeric string")),
        other => Err(serde::de::Error::custom(format!(
            "expected a number or numeric string, got {other}"
        ))),
    }
}

/// Functional enrichment for a term in the supplied protein set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Enrichment {
    pub category: String,
    pub term: String,
    #[serde(rename = "number_of_genes")]
    pub number_of_genes: u64,
    #[serde(rename = "number_of_genes_in_background")]
    pub number_of_genes_in_background: u64,
    #[serde(rename = "ncbiTaxonId")]
    pub ncbi_taxon_id: TaxonId,
    #[serde(rename = "inputGenes", default)]
    pub input_genes: Vec<String>,
    #[serde(rename = "preferredNames", default)]
    pub preferred_names: Vec<String>,
    #[serde(rename = "p_value")]
    pub p_value: f64,
    pub fdr: f64,
    #[serde(default)]
    pub description: Option<String>,
}

/// Functional annotation assigned to a protein or protein set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionalAnnotation {
    pub category: String,
    pub term: String,
    #[serde(rename = "number_of_genes")]
    pub number_of_genes: u64,
    #[serde(rename = "ratio_in_set")]
    pub ratio_in_set: f64,
    #[serde(rename = "ncbiTaxonId")]
    pub ncbi_taxon_id: TaxonId,
    #[serde(rename = "inputGenes", default)]
    pub input_genes: Vec<String>,
    #[serde(rename = "preferredNames", default)]
    pub preferred_names: Vec<String>,
    #[serde(default)]
    pub description: Option<String>,
}

/// Result of the network-level interaction enrichment test.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PpiEnrichment {
    #[serde(rename = "number_of_nodes")]
    pub number_of_nodes: u64,
    #[serde(rename = "number_of_edges")]
    pub number_of_edges: u64,
    #[serde(rename = "average_node_degree")]
    pub average_node_degree: f64,
    #[serde(rename = "local_clustering_coefficient")]
    pub local_clustering_coefficient: f64,
    #[serde(rename = "expected_number_of_edges")]
    pub expected_number_of_edges: f64,
    #[serde(rename = "p_value")]
    pub p_value: f64,
}

/// Stable STRING web link for a network query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkLink {
    pub url: String,
}

/// Anonymous job key used by the Values/Ranks enrichment endpoints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiKey {
    #[serde(rename = "api_key")]
    pub api_key: String,
    #[serde(default)]
    pub note: Option<String>,
}

/// Submission or status response for a Values/Ranks enrichment job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValuesRanksJob {
    #[serde(rename = "job_id", default)]
    pub job_id: String,
    #[serde(rename = "creation_time", default)]
    pub creation_time: Option<String>,
    #[serde(rename = "string_version", default)]
    pub string_version: Option<String>,
    pub status: String,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(rename = "page_url", default)]
    pub page_url: Option<String>,
    #[serde(rename = "download_url", default)]
    pub download_url: Option<String>,
    #[serde(rename = "graph_url", default)]
    pub graph_url: Option<String>,
}

impl ValuesRanksJob {
    /// True when the asynchronous job completed successfully.
    pub fn is_success(&self) -> bool {
        self.status.eq_ignore_ascii_case("success")
    }
}
