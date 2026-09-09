use serde::{Deserialize, Serialize};

/// High-value fields from an RCSB entry.
///
/// RCSB adds fields regularly. Unknown fields are ignored intentionally;
/// archival callers can download the complete mmCIF instead.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Entry {
    #[serde(default)]
    pub rcsb_id: String,
    #[serde(default, rename = "struct")]
    pub structure: StructureMetadata,
    #[serde(default)]
    pub rcsb_entry_info: EntryInfo,
    #[serde(default)]
    pub exptl: Vec<Experiment>,
    #[serde(default)]
    pub citation: Vec<Citation>,
    #[serde(default)]
    pub audit_author: Vec<Author>,
    #[serde(default)]
    pub symmetry: Symmetry,
    #[serde(default)]
    pub rcsb_accession_info: AccessionInfo,
    #[serde(default)]
    pub rcsb_entry_container_identifiers: EntryContainerIdentifiers,
}

impl Entry {
    pub fn title(&self) -> &str {
        if self.structure.title.is_empty() {
            "(untitled structure)"
        } else {
            &self.structure.title
        }
    }

    pub fn experimental_method(&self) -> Option<&str> {
        self.exptl
            .first()
            .map(|experiment| experiment.method.as_str())
    }

    pub fn resolution(&self) -> Option<f64> {
        self.rcsb_entry_info
            .resolution_combined
            .as_ref()?
            .iter()
            .copied()
            .filter(|resolution| *resolution > 0.0)
            .reduce(f64::min)
    }

    pub fn primary_citation(&self) -> Option<&Citation> {
        self.citation
            .iter()
            .find(|citation| citation.rcsb_is_primary.eq_ignore_ascii_case("Y"))
            .or_else(|| self.citation.first())
    }

    pub fn author_string(&self) -> String {
        self.audit_author
            .iter()
            .map(|author| author.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub fn to_record(&self) -> EntryRecord {
        let citation = self.primary_citation();
        EntryRecord {
            entry_id: self.rcsb_id.clone(),
            title: self.title().to_owned(),
            experimental_method: self.experimental_method().unwrap_or("unknown").to_owned(),
            resolution: self.resolution(),
            deposited_atom_count: self.rcsb_entry_info.deposited_atom_count,
            deposited_model_count: self.rcsb_entry_info.deposited_model_count,
            assembly_count: self.rcsb_entry_info.assembly_count,
            polymer_entity_count: self.rcsb_entry_info.polymer_entity_count,
            protein_entity_count: self.rcsb_entry_info.polymer_entity_count_protein,
            molecular_weight: self.rcsb_entry_info.molecular_weight,
            polymer_composition: self.rcsb_entry_info.polymer_composition.clone(),
            space_group: self.symmetry.space_group_name_h_m.clone(),
            deposit_date: self.rcsb_accession_info.deposit_date.clone(),
            release_date: self.rcsb_accession_info.initial_release_date.clone(),
            doi: citation.and_then(|c| c.doi.clone()),
            pubmed_id: citation.and_then(|c| c.pubmed_id),
            authors: self.author_string(),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct StructureMetadata {
    #[serde(default)]
    pub title: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct EntryInfo {
    #[serde(default)]
    pub assembly_count: Option<u64>,
    #[serde(default)]
    pub deposited_atom_count: Option<u64>,
    #[serde(default)]
    pub deposited_model_count: Option<u64>,
    #[serde(default)]
    pub deposited_polymer_monomer_count: Option<u64>,
    #[serde(default)]
    pub experimental_method: Option<String>,
    #[serde(default)]
    pub molecular_weight: Option<f64>,
    #[serde(default)]
    pub polymer_composition: Option<String>,
    #[serde(default)]
    pub polymer_entity_count: Option<u64>,
    #[serde(default)]
    pub polymer_entity_count_protein: Option<u64>,
    #[serde(default)]
    pub resolution_combined: Option<Vec<f64>>,
    #[serde(default)]
    pub structure_determination_methodology: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Experiment {
    #[serde(default)]
    pub method: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Author {
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Citation {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub year: Option<i64>,
    #[serde(default)]
    pub rcsb_journal_abbrev: Option<String>,
    #[serde(default)]
    pub journal_volume: Option<String>,
    #[serde(default)]
    pub page_first: Option<String>,
    #[serde(default)]
    pub page_last: Option<String>,
    #[serde(default)]
    pub doi: Option<String>,
    #[serde(default, rename = "pdbx_database_id_PubMed")]
    pub pubmed_id: Option<u64>,
    #[serde(default)]
    pub rcsb_is_primary: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Symmetry {
    #[serde(default)]
    pub space_group_name_h_m: Option<String>,
    #[serde(default)]
    pub int_tables_number: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AccessionInfo {
    #[serde(default)]
    pub deposit_date: Option<String>,
    #[serde(default)]
    pub initial_release_date: Option<String>,
    #[serde(default)]
    pub revision_date: Option<String>,
    #[serde(default)]
    pub status_code: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct EntryContainerIdentifiers {
    #[serde(default)]
    pub entry_id: String,
    #[serde(default)]
    pub polymer_entity_ids: Vec<String>,
    #[serde(default)]
    pub assembly_ids: Vec<String>,
    #[serde(default)]
    pub pubmed_id: Option<u64>,
}

/// Flat row emitted by DAG entry nodes and useful in previews.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntryRecord {
    pub entry_id: String,
    pub title: String,
    pub experimental_method: String,
    pub resolution: Option<f64>,
    pub deposited_atom_count: Option<u64>,
    pub deposited_model_count: Option<u64>,
    pub assembly_count: Option<u64>,
    pub polymer_entity_count: Option<u64>,
    pub protein_entity_count: Option<u64>,
    pub molecular_weight: Option<f64>,
    pub polymer_composition: Option<String>,
    pub space_group: Option<String>,
    pub deposit_date: Option<String>,
    pub release_date: Option<String>,
    pub doi: Option<String>,
    pub pubmed_id: Option<u64>,
    pub authors: String,
}

/// High-value fields from a polymer entity within a PDB entry.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PolymerEntity {
    #[serde(default)]
    pub rcsb_id: String,
    #[serde(default)]
    pub rcsb_polymer_entity: PolymerEntityInfo,
    #[serde(default)]
    pub rcsb_polymer_entity_container_identifiers: PolymerContainerIdentifiers,
    #[serde(default)]
    pub entity_poly: EntityPoly,
    #[serde(default)]
    pub rcsb_entity_source_organism: Vec<SourceOrganism>,
    #[serde(default)]
    pub rcsb_polymer_entity_annotation: Vec<Annotation>,
}

impl PolymerEntity {
    pub fn description(&self) -> &str {
        if self.rcsb_polymer_entity.pdbx_description.is_empty() {
            "(unnamed polymer)"
        } else {
            &self.rcsb_polymer_entity.pdbx_description
        }
    }

    pub fn sequence(&self) -> &str {
        if self.entity_poly.pdbx_seq_one_letter_code_can.is_empty() {
            &self.entity_poly.pdbx_seq_one_letter_code
        } else {
            &self.entity_poly.pdbx_seq_one_letter_code_can
        }
    }

    pub fn sequence_length(&self) -> u64 {
        self.entity_poly
            .rcsb_sample_sequence_length
            .unwrap_or_else(|| self.sequence().len() as u64)
    }

    pub fn first_source_organism(&self) -> Option<&SourceOrganism> {
        self.rcsb_entity_source_organism.first()
    }

    pub fn to_record(&self) -> PolymerEntityRecord {
        let source = self.first_source_organism();
        PolymerEntityRecord {
            entry_id: self
                .rcsb_polymer_entity_container_identifiers
                .entry_id
                .clone(),
            entity_id: self
                .rcsb_polymer_entity_container_identifiers
                .entity_id
                .clone(),
            description: self.description().to_owned(),
            sequence: self.sequence().to_owned(),
            sequence_length: self.sequence_length(),
            polymer_type: self.entity_poly.rcsb_entity_polymer_type.clone(),
            copies: self.rcsb_polymer_entity.pdbx_number_of_molecules,
            formula_weight: self.rcsb_polymer_entity.formula_weight,
            strand_ids: self.entity_poly.pdbx_strand_id.clone(),
            uniprot_ids: self
                .rcsb_polymer_entity_container_identifiers
                .uniprot_ids
                .join(","),
            gene_names: source
                .map(|source| {
                    source
                        .rcsb_gene_name
                        .iter()
                        .map(|gene| gene.value.as_str())
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .unwrap_or_default(),
            organism_scientific: source.and_then(|source| source.scientific_name.clone()),
            taxonomy_id: source.and_then(|source| source.ncbi_taxonomy_id),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PolymerEntityInfo {
    #[serde(default)]
    pub formula_weight: Option<f64>,
    #[serde(default)]
    pub pdbx_description: String,
    #[serde(default)]
    pub pdbx_number_of_molecules: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PolymerContainerIdentifiers {
    #[serde(default)]
    pub entry_id: String,
    #[serde(default)]
    pub entity_id: String,
    #[serde(default)]
    pub asym_ids: Vec<String>,
    #[serde(default)]
    pub auth_asym_ids: Vec<String>,
    #[serde(default)]
    pub uniprot_ids: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct EntityPoly {
    #[serde(default)]
    pub pdbx_seq_one_letter_code: String,
    #[serde(default)]
    pub pdbx_seq_one_letter_code_can: String,
    #[serde(default)]
    pub pdbx_strand_id: Option<String>,
    #[serde(default)]
    pub rcsb_entity_polymer_type: Option<String>,
    #[serde(default)]
    pub rcsb_sample_sequence_length: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SourceOrganism {
    #[serde(default)]
    pub scientific_name: Option<String>,
    #[serde(default)]
    pub common_name: Option<String>,
    #[serde(default)]
    pub ncbi_taxonomy_id: Option<u64>,
    #[serde(default)]
    pub rcsb_gene_name: Vec<GeneName>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct GeneName {
    #[serde(default)]
    pub value: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Annotation {
    #[serde(default)]
    pub annotation_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub r#type: String,
    #[serde(default)]
    pub provenance_source: String,
}

/// Flat row emitted by polymer-entity DAG nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolymerEntityRecord {
    pub entry_id: String,
    pub entity_id: String,
    pub description: String,
    pub sequence: String,
    pub sequence_length: u64,
    pub polymer_type: Option<String>,
    pub copies: Option<u64>,
    pub formula_weight: Option<f64>,
    pub strand_ids: Option<String>,
    pub uniprot_ids: String,
    pub gene_names: String,
    pub organism_scientific: Option<String>,
    pub taxonomy_id: Option<u64>,
}

/// Assembly metadata for computational structure workflows.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Assembly {
    #[serde(default)]
    pub rcsb_id: String,
    #[serde(default)]
    pub pdbx_struct_assembly: StructAssembly,
    #[serde(default)]
    pub rcsb_assembly_info: Option<AssemblyInfo>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct StructAssembly {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub details: Option<String>,
    #[serde(default)]
    pub oligomeric_count: Option<u64>,
    #[serde(default)]
    pub oligomeric_details: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AssemblyInfo {
    #[serde(default)]
    pub polymer_entity_instance_count: Option<u64>,
    #[serde(default)]
    pub modeled_atom_count: Option<u64>,
    #[serde(default)]
    pub modeled_polymer_monomer_count: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_extracts_metadata_and_record() {
        let entry: Entry = serde_json::from_str(
            r#"{
              "rcsb_id": "4HHB",
              "struct": {"title": "Human hemoglobin"},
              "rcsb_entry_info": {
                "assembly_count": 1,
                "deposited_atom_count": 4779,
                "polymer_entity_count": 2,
                "polymer_entity_count_protein": 2,
                "molecular_weight": 64.74,
                "resolution_combined": [1.74, 1.90]
              },
              "exptl": [{"method": "X-RAY DIFFRACTION"}],
              "audit_author": [{"name": "Fermi, G."}],
              "symmetry": {"space_group_name_H_M": "P 1 21 1"},
              "citation": [{
                "title": "Crystal structure",
                "year": 1984,
                "rcsb_is_primary": "Y",
                "pdbx_database_id_DOI": "10.1/example",
                "pdbx_database_id_PubMed": 6726807
              }]
            }"#,
        )
        .unwrap();

        assert_eq!(entry.experimental_method(), Some("X-RAY DIFFRACTION"));
        assert_eq!(entry.resolution(), Some(1.74));
        assert_eq!(entry.author_string(), "Fermi, G.");
        let record = entry.to_record();
        assert_eq!(record.entry_id, "4HHB");
        assert_eq!(record.pubmed_id, Some(6726807));
    }

    #[test]
    fn polymer_entity_extracts_sequence_and_mapping() {
        let entity: PolymerEntity = serde_json::from_str(
            r#"{
              "rcsb_polymer_entity": {
                "formula_weight": 15.15,
                "pdbx_description": "Hemoglobin subunit alpha",
                "pdbx_number_of_molecules": 2
              },
              "rcsb_polymer_entity_container_identifiers": {
                "entry_id": "4HHB",
                "entity_id": "1",
                "uniprot_ids": ["P69905"]
              },
              "entity_poly": {
                "pdbx_seq_one_letter_code_can": "VLSPADKTNVK",
                "pdbx_strand_id": "A,C",
                "rcsb_entity_polymer_type": "Protein",
                "rcsb_sample_sequence_length": 11
              },
              "rcsb_entity_source_organism": [{
                "scientific_name": "Homo sapiens",
                "ncbi_taxonomy_id": 9606,
                "rcsb_gene_name": [{"value": "HBA1"}]
              }]
            }"#,
        )
        .unwrap();

        let record = entity.to_record();
        assert_eq!(record.description, "Hemoglobin subunit alpha");
        assert_eq!(record.sequence, "VLSPADKTNVK");
        assert_eq!(record.uniprot_ids, "P69905");
        assert_eq!(record.taxonomy_id, Some(9606));
    }
}
