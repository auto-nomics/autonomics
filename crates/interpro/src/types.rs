use std::collections::BTreeMap;

use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct EntryResponse {
    #[serde(default)]
    pub metadata: EntryMetadata,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub struct EntryMetadata {
    pub accession: String,
    pub entry_id: Option<String>,
    pub entry_type: Option<String>,
    pub source_database: Option<String>,
    pub name: Option<ResourceName>,
    pub description: Vec<DescriptionItem>,
    pub go_terms: Vec<GoTerm>,
    pub member_databases: BTreeMap<String, BTreeMap<String, String>>,
    pub integrated: Option<IntegratedEntry>,
    pub counters: Option<EntryCounters>,
    pub representative_structure: Option<StructureReference>,
    pub is_llm: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ResourceName {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub short: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct DescriptionItem {
    #[serde(default)]
    pub text: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct GoTerm {
    #[serde(default)]
    pub identifier: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub category: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct IntegratedEntry {
    pub accession: String,
    pub entry_id: Option<String>,
    pub name: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct EntryCounters {
    pub proteins: Option<u64>,
    pub proteomes: Option<u64>,
    pub structures: Option<u64>,
    pub taxa: Option<u64>,
    pub pathways: Option<u64>,
    pub structural_models: Option<StructuralModelCounts>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StructuralModelCounts {
    pub alphafold: Option<u64>,
    pub modbase: Option<u64>,
    pub swissmodel: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct StructureReference {
    #[serde(default)]
    pub accession: String,
    #[serde(default)]
    pub name: String,
}
