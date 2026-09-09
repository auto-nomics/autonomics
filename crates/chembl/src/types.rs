use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

fn deserialize_optional_f64<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => number
            .as_f64()
            .map(Some)
            .ok_or_else(|| serde::de::Error::custom("number is not representable as f64")),
        Some(Value::String(text)) if text.is_empty() => Ok(None),
        Some(Value::String(text)) => text
            .parse::<f64>()
            .map(Some)
            .map_err(serde::de::Error::custom),
        Some(other) => Err(serde::de::Error::custom(format!(
            "expected number, string, or null, found {other}"
        ))),
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PageMeta {
    #[serde(default)]
    pub limit: u32,
    #[serde(default)]
    pub offset: u32,
    #[serde(default)]
    pub total_count: u64,
    #[serde(default)]
    pub next: Option<String>,
    #[serde(default)]
    pub previous: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Page<T> {
    pub records: Vec<T>,
    #[serde(default)]
    pub page_meta: PageMeta,
}

impl<T> Default for Page<T> {
    fn default() -> Self {
        Self {
            records: Vec::new(),
            page_meta: PageMeta::default(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CrossReference {
    #[serde(default)]
    pub xref_id: Option<String>,
    #[serde(default)]
    pub xref_name: Option<String>,
    #[serde(default)]
    pub xref_src: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MoleculeSynonym {
    #[serde(default)]
    pub synonyms: Option<String>,
    #[serde(default)]
    pub syn_type: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MoleculeProperties {
    #[serde(default, deserialize_with = "deserialize_optional_f64")]
    pub alogp: Option<f64>,
    #[serde(default, deserialize_with = "deserialize_optional_f64")]
    pub aromatic_rings: Option<f64>,
    #[serde(default)]
    pub full_molformula: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_f64")]
    pub full_mwt: Option<f64>,
    #[serde(default, deserialize_with = "deserialize_optional_f64")]
    pub hba: Option<f64>,
    #[serde(default, deserialize_with = "deserialize_optional_f64")]
    pub hbd: Option<f64>,
    #[serde(default, deserialize_with = "deserialize_optional_f64")]
    pub heavy_atoms: Option<f64>,
    #[serde(default, deserialize_with = "deserialize_optional_f64")]
    pub mw_freebase: Option<f64>,
    #[serde(default, deserialize_with = "deserialize_optional_f64")]
    pub num_ro5_violations: Option<f64>,
    #[serde(default, deserialize_with = "deserialize_optional_f64")]
    pub psa: Option<f64>,
    #[serde(default, deserialize_with = "deserialize_optional_f64")]
    pub qed_weighted: Option<f64>,
    #[serde(default)]
    pub ro3_pass: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_f64")]
    pub rtb: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MoleculeStructure {
    #[serde(default)]
    pub canonical_smiles: Option<String>,
    #[serde(default)]
    pub standard_inchi: Option<String>,
    #[serde(default)]
    pub standard_inchi_key: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Molecule {
    #[serde(default)]
    pub molecule_chembl_id: String,
    #[serde(default)]
    pub pref_name: Option<String>,
    #[serde(default)]
    pub molecule_type: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_f64")]
    pub max_phase: Option<f64>,
    #[serde(default, deserialize_with = "deserialize_optional_f64")]
    pub first_approval: Option<f64>,
    #[serde(default)]
    pub therapeutic_flag: bool,
    #[serde(default)]
    pub oral: Option<bool>,
    #[serde(default)]
    pub parenteral: Option<bool>,
    #[serde(default)]
    pub topical: Option<bool>,
    #[serde(default)]
    pub withdrawn_flag: bool,
    #[serde(default)]
    pub atc_classifications: Vec<String>,
    #[serde(default)]
    pub cross_references: Vec<CrossReference>,
    #[serde(default)]
    pub molecule_properties: Option<MoleculeProperties>,
    #[serde(default)]
    pub molecule_structures: Option<MoleculeStructure>,
    #[serde(default)]
    pub molecule_synonyms: Vec<MoleculeSynonym>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ComponentSynonym {
    #[serde(default)]
    pub component_synonym: Option<String>,
    #[serde(default)]
    pub syn_type: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TargetComponent {
    #[serde(default)]
    pub accession: Option<String>,
    #[serde(default)]
    pub component_description: Option<String>,
    #[serde(default)]
    pub component_type: Option<String>,
    #[serde(default)]
    pub relationship: Option<String>,
    #[serde(default)]
    pub target_component_synonyms: Vec<ComponentSynonym>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Target {
    #[serde(default)]
    pub target_chembl_id: String,
    #[serde(default)]
    pub pref_name: Option<String>,
    #[serde(default)]
    pub organism: Option<String>,
    #[serde(default)]
    pub target_type: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_f64")]
    pub tax_id: Option<f64>,
    #[serde(default)]
    pub target_components: Vec<TargetComponent>,
}

impl Target {
    pub fn gene_symbols(&self) -> Vec<&str> {
        self.target_components
            .iter()
            .flat_map(|component| component.target_component_synonyms.iter())
            .filter(|synonym| synonym.syn_type.as_deref() == Some("GENE_SYMBOL"))
            .filter_map(|synonym| synonym.component_synonym.as_deref())
            .collect()
    }

    pub fn uniprot_accessions(&self) -> Vec<&str> {
        self.target_components
            .iter()
            .filter_map(|component| component.accession.as_deref())
            .collect()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Activity {
    #[serde(default)]
    pub activity_id: u64,
    #[serde(default)]
    pub assay_chembl_id: String,
    #[serde(default)]
    pub assay_description: Option<String>,
    #[serde(default)]
    pub assay_type: Option<String>,
    #[serde(default)]
    pub document_chembl_id: Option<String>,
    #[serde(default)]
    pub molecule_chembl_id: String,
    #[serde(default)]
    pub molecule_pref_name: Option<String>,
    #[serde(default)]
    pub parent_molecule_chembl_id: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_f64")]
    pub pchembl_value: Option<f64>,
    #[serde(default)]
    pub relation: Option<String>,
    #[serde(default)]
    pub standard_flag: Option<i64>,
    #[serde(default)]
    pub standard_relation: Option<String>,
    #[serde(default)]
    pub standard_text_value: Option<String>,
    #[serde(default)]
    pub standard_type: Option<String>,
    #[serde(default)]
    pub standard_units: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_f64")]
    pub standard_value: Option<f64>,
    #[serde(default, deserialize_with = "deserialize_optional_f64")]
    pub standard_upper_value: Option<f64>,
    #[serde(default)]
    pub target_chembl_id: String,
    #[serde(default)]
    pub target_organism: Option<String>,
    #[serde(default)]
    pub target_pref_name: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_f64")]
    pub target_tax_id: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Assay {
    #[serde(default)]
    pub assay_chembl_id: String,
    #[serde(default)]
    pub assay_type: Option<String>,
    #[serde(default)]
    pub assay_type_description: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub document_chembl_id: Option<String>,
    #[serde(default)]
    pub target_chembl_id: Option<String>,
    #[serde(default)]
    pub organism: Option<String>,
    #[serde(default)]
    pub cell_chembl_id: Option<String>,
    #[serde(default)]
    pub tissue_chembl_id: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_f64")]
    pub confidence_score: Option<f64>,
    #[serde(default)]
    pub relationship_type: Option<String>,
    #[serde(default)]
    pub bao_label: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Document {
    #[serde(default)]
    pub document_chembl_id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub authors: Option<String>,
    #[serde(default)]
    pub journal: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_f64")]
    pub year: Option<f64>,
    #[serde(default)]
    pub volume: Option<String>,
    #[serde(default)]
    pub first_page: Option<String>,
    #[serde(default)]
    pub last_page: Option<String>,
    #[serde(default)]
    pub pubmed_id: Option<String>,
    #[serde(default)]
    pub doi: Option<String>,
    #[serde(default)]
    pub patent_id: Option<String>,
    #[serde(default)]
    pub doc_type: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Reference {
    #[serde(default)]
    pub ref_id: Option<String>,
    #[serde(default)]
    pub ref_type: Option<String>,
    #[serde(default)]
    pub ref_url: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Mechanism {
    #[serde(default)]
    pub mec_id: u64,
    #[serde(default)]
    pub molecule_chembl_id: String,
    #[serde(default)]
    pub target_chembl_id: String,
    #[serde(default)]
    pub mechanism_of_action: Option<String>,
    #[serde(default)]
    pub action_type: Option<String>,
    #[serde(default)]
    pub direct_interaction: Option<i64>,
    #[serde(default)]
    pub molecular_mechanism: Option<i64>,
    #[serde(default)]
    pub disease_efficacy: Option<i64>,
    #[serde(default, deserialize_with = "deserialize_optional_f64")]
    pub max_phase: Option<f64>,
    #[serde(default)]
    pub mechanism_refs: Vec<Reference>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DrugIndication {
    #[serde(default)]
    pub drugind_id: u64,
    #[serde(default)]
    pub molecule_chembl_id: String,
    #[serde(default)]
    pub efo_id: Option<String>,
    #[serde(default)]
    pub efo_term: Option<String>,
    #[serde(default)]
    pub mesh_id: Option<String>,
    #[serde(default)]
    pub mesh_heading: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_f64")]
    pub max_phase_for_ind: Option<f64>,
    #[serde(default)]
    pub indication_refs: Vec<Reference>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Status {
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub chembl_db_version: String,
    #[serde(default)]
    pub chembl_release_date: Option<String>,
    #[serde(default)]
    pub activities: u64,
    #[serde(default)]
    pub targets: u64,
    #[serde(default)]
    pub compound_records: u64,
    #[serde(default)]
    pub disinct_compounds: u64,
    #[serde(default)]
    pub publications: u64,
}
