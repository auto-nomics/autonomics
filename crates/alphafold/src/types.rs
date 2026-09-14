use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Prediction {
    pub entry_id: Option<String>,
    pub model_entity_id: Option<String>,
    pub uniprot_accession: Option<String>,
    pub uniprot_id: Option<String>,
    pub uniprot_description: Option<String>,
    pub gene: Option<String>,
    pub organism_scientific_name: Option<String>,
    pub tax_id: Option<u64>,
    pub sequence: Option<String>,
    pub sequence_start: Option<u64>,
    pub sequence_end: Option<u64>,
    pub latest_version: Option<u32>,
    pub all_versions: Vec<u32>,
    pub global_metric_value: Option<f64>,
    pub tool_used: Option<String>,
    pub model_created_date: Option<String>,
    pub is_reviewed: Option<bool>,
    pub is_reference_proteome: Option<bool>,
    pub cif_url: Option<String>,
    pub pdb_url: Option<String>,
    pub bcif_url: Option<String>,
    pub plddt_doc_url: Option<String>,
    pub pae_doc_url: Option<String>,
    pub msa_url: Option<String>,
}
