use crate::types::{
    Activity, Assay, Document, DrugIndication, Mechanism, Molecule, Page, PageMeta, Status, Target,
};

fn optional(value: &Option<String>) -> &str {
    value.as_deref().unwrap_or("-")
}

fn optional_number(value: Option<f64>) -> String {
    value
        .map(|number| number.to_string())
        .unwrap_or_else(|| "-".into())
}

fn escape_table(value: &str) -> String {
    value.replace('|', "\\|").replace('\n', " ")
}

fn pagination(page: &PageMeta, verb: &str) -> String {
    if page.next.is_some() {
        let next_offset = page.offset + page.limit;
        format!(
            "\n\n_Showing {} records; more available. Use offset {} for the next page._",
            page.limit, next_offset
        )
    } else {
        format!("\n\n_No further {verb} available._")
    }
}

pub fn format_status(status: &Status) -> String {
    format!(
        "## ChEMBL {}\n\n- **Service:** {}\n- **Release date:** {}\n- **Activities:** {}\n- **Targets:** {}\n- **Compound records:** {}\n- **Distinct compounds:** {}\n- **Publications:** {}\n",
        status.chembl_db_version,
        status.status,
        optional(&status.chembl_release_date),
        status.activities,
        status.targets,
        status.compound_records,
        status.disinct_compounds,
        status.publications,
    )
}

pub fn format_molecule_search(page: &Page<Molecule>) -> String {
    if page.records.is_empty() {
        return format!(
            "No molecules found (total count {}).",
            page.page_meta.total_count
        );
    }
    let mut out = format!(
        "**{} molecules** (total count {})\n\n| # | ChEMBL ID | Name | Type | Max phase | MW | SMILES |\n|---|---|---|---|---|---|---|\n",
        page.records.len(),
        page.page_meta.total_count
    );
    for (index, molecule) in page.records.iter().enumerate() {
        let mw = molecule
            .molecule_properties
            .as_ref()
            .and_then(|properties| properties.full_mwt)
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".into());
        let smiles = molecule
            .molecule_structures
            .as_ref()
            .and_then(|structure| structure.canonical_smiles.clone())
            .unwrap_or_else(|| "-".into());
        out.push_str(&format!(
            "| {} | `{}` | {} | {} | {} | {} | `{}` |\n",
            index + 1,
            molecule.molecule_chembl_id,
            escape_table(optional(&molecule.pref_name)),
            escape_table(optional(&molecule.molecule_type)),
            optional_number(molecule.max_phase),
            mw,
            escape_table(&smiles),
        ));
    }
    out.push_str(&pagination(&page.page_meta, "molecules"));
    out
}

pub fn format_target_search(page: &Page<Target>) -> String {
    if page.records.is_empty() {
        return format!(
            "No targets found (total count {}).",
            page.page_meta.total_count
        );
    }
    let mut out = format!(
        "**{} targets** (total count {})\n\n| # | ChEMBL ID | Name | Organism | Type | Gene symbols | UniProt |\n|---|---|---|---|---|---|---|\n",
        page.records.len(),
        page.page_meta.total_count
    );
    for (index, target) in page.records.iter().enumerate() {
        out.push_str(&format!(
            "| {} | `{}` | {} | {} | {} | {} | {} |\n",
            index + 1,
            target.target_chembl_id,
            escape_table(optional(&target.pref_name)),
            escape_table(optional(&target.organism)),
            escape_table(optional(&target.target_type)),
            escape_table(&target.gene_symbols().join(", ")),
            escape_table(&target.uniprot_accessions().join(", ")),
        ));
    }
    out.push_str(&pagination(&page.page_meta, "targets"));
    out
}

pub fn format_molecule(molecule: &Molecule) -> String {
    let mut out = format!(
        "## {} ({})\n\n",
        optional(&molecule.pref_name),
        molecule.molecule_chembl_id
    );
    out.push_str(&format!(
        "- **Type:** {}\n- **Max phase:** {}\n- **First approval:** {}\n",
        optional(&molecule.molecule_type),
        optional_number(molecule.max_phase),
        optional_number(molecule.first_approval)
    ));
    out.push_str(&format!(
        "- **Route flags:** oral={}, parenteral={}, topical={}\n- **Withdrawn:** {}\n",
        molecule
            .oral
            .map(|value| value.to_string())
            .unwrap_or("-".into()),
        molecule
            .parenteral
            .map(|value| value.to_string())
            .unwrap_or("-".into()),
        molecule
            .topical
            .map(|value| value.to_string())
            .unwrap_or("-".into()),
        molecule.withdrawn_flag
    ));
    if !molecule.atc_classifications.is_empty() {
        out.push_str(&format!(
            "- **ATC:** {}\n",
            molecule.atc_classifications.join(", ")
        ));
    }
    if let Some(properties) = &molecule.molecule_properties {
        out.push_str("\n### Properties\n\n");
        out.push_str(&format!(
            "- **Formula:** {}\n- **Molecular weight:** {}\n- **cLogP:** {}\n- **HBD/HBA:** {}/{}\n- **PSA:** {}\n- **QED:** {}\n- **Ro5 violations:** {}\n",
            optional(&properties.full_molformula),
            optional_number(properties.full_mwt),
            optional_number(properties.alogp),
            optional_number(properties.hbd),
            optional_number(properties.hba),
            optional_number(properties.psa),
            optional_number(properties.qed_weighted),
            optional_number(properties.num_ro5_violations),
        ));
    }
    if let Some(structure) = &molecule.molecule_structures {
        out.push_str("\n### Structure\n\n");
        if let Some(smiles) = &structure.canonical_smiles {
            out.push_str(&format!("- **SMILES:** `{smiles}`\n"));
        }
        if let Some(inchi_key) = &structure.standard_inchi_key {
            out.push_str(&format!("- **InChIKey:** `{inchi_key}`\n"));
        }
    }
    if !molecule.molecule_synonyms.is_empty() {
        let names: Vec<_> = molecule
            .molecule_synonyms
            .iter()
            .filter_map(|synonym| synonym.synonyms.as_deref())
            .take(12)
            .collect();
        out.push_str(&format!("\n**Synonyms:** {}\n", names.join(", ")));
    }
    out
}

pub fn format_target(target: &Target) -> String {
    let mut out = format!(
        "## {} ({})\n\n- **Organism:** {}\n- **Type:** {}\n- **Tax ID:** {}\n\n### Components\n",
        optional(&target.pref_name),
        target.target_chembl_id,
        optional(&target.organism),
        optional(&target.target_type),
        optional_number(target.tax_id)
    );
    if target.target_components.is_empty() {
        out.push_str("\nNo protein components reported.\n");
        return out;
    }
    out.push_str("\n| Accession | Description | Type | Gene symbols |\n|---|---|---|---|\n");
    for component in &target.target_components {
        let genes: Vec<_> = component
            .target_component_synonyms
            .iter()
            .filter(|synonym| synonym.syn_type.as_deref() == Some("GENE_SYMBOL"))
            .filter_map(|synonym| synonym.component_synonym.as_deref())
            .collect();
        out.push_str(&format!(
            "| `{}` | {} | {} | {} |\n",
            optional(&component.accession),
            escape_table(optional(&component.component_description)),
            escape_table(optional(&component.component_type)),
            escape_table(&genes.join(", ")),
        ));
    }
    out
}

pub fn format_activities(page: &Page<Activity>) -> String {
    if page.records.is_empty() {
        return format!(
            "No activities found (total count {}).",
            page.page_meta.total_count
        );
    }
    let mut out = format!(
        "**{} activities** (total count {})\n\n| Activity | Molecule | Target | Assay | Standard type | Value | Units | pChEMBL |\n|---|---|---|---|---|---|---|---|\n",
        page.records.len(),
        page.page_meta.total_count
    );
    for activity in &page.records {
        out.push_str(&format!(
            "| `{}` | `{}` | `{}` | `{}` | {} | {}{} | {} | {} |\n",
            activity.activity_id,
            activity.molecule_chembl_id,
            activity.target_chembl_id,
            activity.assay_chembl_id,
            escape_table(optional(&activity.standard_type)),
            optional(&activity.standard_relation),
            optional_number(activity.standard_value),
            optional(&activity.standard_units),
            optional_number(activity.pchembl_value),
        ));
    }
    out.push_str(&pagination(&page.page_meta, "activities"));
    out
}

pub fn format_assays(page: &Page<Assay>) -> String {
    if page.records.is_empty() {
        return format!(
            "No assays found (total count {}).",
            page.page_meta.total_count
        );
    }
    let mut out = format!(
        "**{} assays** (total count {})\n\n| ChEMBL ID | Description | Type | Target | Organism | Confidence |\n|---|---|---|---|---|---|\n",
        page.records.len(),
        page.page_meta.total_count
    );
    for assay in &page.records {
        out.push_str(&format!(
            "| `{}` | {} | {} | `{}` | {} | {} |\n",
            assay.assay_chembl_id,
            escape_table(optional(&assay.description)),
            escape_table(optional(&assay.assay_type_description)),
            optional(&assay.target_chembl_id),
            escape_table(optional(&assay.organism)),
            optional_number(assay.confidence_score),
        ));
    }
    out.push_str(&pagination(&page.page_meta, "assays"));
    out
}

pub fn format_documents(page: &Page<Document>) -> String {
    if page.records.is_empty() {
        return format!(
            "No documents found (total count {}).",
            page.page_meta.total_count
        );
    }
    let mut out = format!(
        "**{} documents** (total count {})\n\n| ChEMBL ID | Title | Journal | Year | PubMed ID | DOI |\n|---|---|---|---|---|---|\n",
        page.records.len(),
        page.page_meta.total_count
    );
    for document in &page.records {
        out.push_str(&format!(
            "| `{}` | {} | {} | {} | `{}` | `{}` |\n",
            document.document_chembl_id,
            escape_table(optional(&document.title)),
            escape_table(optional(&document.journal)),
            optional_number(document.year),
            optional(&document.pubmed_id),
            optional(&document.doi),
        ));
    }
    out.push_str(&pagination(&page.page_meta, "documents"));
    out
}

pub fn format_mechanisms(page: &Page<Mechanism>) -> String {
    if page.records.is_empty() {
        return format!(
            "No mechanisms found (total count {}).",
            page.page_meta.total_count
        );
    }
    let mut out = format!(
        "**{} mechanisms** (total count {})\n\n| Molecule | Target | Mechanism | Action | Max phase | Direct |\n|---|---|---|---|---|---|\n",
        page.records.len(),
        page.page_meta.total_count
    );
    for mechanism in &page.records {
        out.push_str(&format!(
            "| `{}` | `{}` | {} | {} | {} | {} |\n",
            mechanism.molecule_chembl_id,
            mechanism.target_chembl_id,
            escape_table(optional(&mechanism.mechanism_of_action)),
            escape_table(optional(&mechanism.action_type)),
            optional_number(mechanism.max_phase),
            mechanism
                .direct_interaction
                .map(|value| value != 0)
                .unwrap_or(false),
        ));
    }
    out.push_str(&pagination(&page.page_meta, "mechanisms"));
    out
}

pub fn format_drug_indications(page: &Page<DrugIndication>) -> String {
    if page.records.is_empty() {
        return format!(
            "No drug indications found (total count {}).",
            page.page_meta.total_count
        );
    }
    let mut out = format!(
        "**{} drug indications** (total count {})\n\n| EFO ID | Indication | MeSH heading | Max phase |\n|---|---|---|---|\n",
        page.records.len(),
        page.page_meta.total_count
    );
    for indication in &page.records {
        out.push_str(&format!(
            "| `{}` | {} | {} | {} |\n",
            optional(&indication.efo_id),
            escape_table(optional(&indication.efo_term)),
            escape_table(optional(&indication.mesh_heading)),
            optional_number(indication.max_phase_for_ind),
        ));
    }
    out.push_str(&pagination(&page.page_meta, "drug indications"));
    out
}
