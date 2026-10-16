use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct QueryPage<T> {
    pub rows: Vec<T>,
    pub total_rows: u64,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct BrainArea {
    pub id: u64,
    pub graph_id: u64,
    #[serde(default)]
    pub parent_structure_id: Option<u64>,
    pub acronym: String,
    pub name: String,
    #[serde(default)]
    pub structure_id_path: Option<String>,
    #[serde(default)]
    pub depth: Option<u32>,
    #[serde(default)]
    pub color_hex_triplet: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Gene {
    pub id: u64,
    pub acronym: String,
    pub name: String,
    #[serde(default)]
    pub organism_id: Option<u64>,
    #[serde(default)]
    pub entrez_id: Option<u64>,
    #[serde(default)]
    pub homologene_id: Option<u64>,
    #[serde(default)]
    pub alias_tags: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct TaxonomyReference {
    pub filename: String,
    pub species: Option<String>,
    pub assay: Option<String>,
    pub region: Option<String>,
    pub release_date: Option<String>,
    pub download_url: String,
}

pub fn classify_taxonomy_filename(filename: &str) -> TaxonomyReference {
    let stem = filename.strip_suffix(".h5ad").unwrap_or(filename);
    let lower = stem.to_ascii_lowercase().replace('_', " ");
    let species = ["human", "mouse", "macaque", "marmoset"]
        .iter()
        .find(|species| lower.contains(**species))
        .map(|species| species.to_title_case());
    let assay = if lower.contains("10x") {
        Some("10x".to_owned())
    } else if lower.contains("smart") {
        Some("SMART-seq".to_owned())
    } else {
        None
    };
    let region = if lower.contains("basalganglia") {
        Some("Basal Ganglia".to_owned())
    } else if lower.contains("cortexhippocampus") {
        Some("Cortex Hippocampus".to_owned())
    } else if lower.contains("neocortex") {
        Some("Neocortex".to_owned())
    } else if lower.contains("visp alm") {
        Some("VISp ALM".to_owned())
    } else if lower.contains("mtg") {
        Some("MTG".to_owned())
    } else if lower.contains("m1") {
        Some("M1".to_owned())
    } else if lower.contains("lgn") {
        Some("LGN".to_owned())
    } else {
        None
    };
    let release_date = stem.rsplit('_').find_map(|part| {
        (part.len() == 8 && part.chars().all(|character| character.is_ascii_digit()))
            .then(|| format!("{}-{}-{}", &part[4..8], &part[0..2], &part[2..4]))
    });

    TaxonomyReference {
        filename: filename.to_owned(),
        species,
        assay,
        region,
        release_date,
        download_url: String::new(),
    }
}

trait TitleCase {
    fn to_title_case(&self) -> String;
}

impl TitleCase for str {
    fn to_title_case(&self) -> String {
        self.split(' ')
            .map(|word| {
                let mut characters = word.chars();
                characters
                    .next()
                    .map(|first| {
                        first.to_uppercase().collect::<String>()
                            + &characters.as_str().to_ascii_lowercase()
                    })
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_taxonomy_filenames() {
        let reference = classify_taxonomy_filename("Mouse_VISp_ALM_SMART_seq_04042025.h5ad");
        assert_eq!(reference.species.as_deref(), Some("Mouse"));
        assert_eq!(reference.assay.as_deref(), Some("SMART-seq"));
        assert_eq!(reference.region.as_deref(), Some("VISp ALM"));
        assert_eq!(reference.release_date.as_deref(), Some("2025-04-04"));

        let reference = classify_taxonomy_filename("Human_HMBA_basalganglia_AIT_pre-print.h5ad");
        assert_eq!(reference.species.as_deref(), Some("Human"));
        assert_eq!(reference.region.as_deref(), Some("Basal Ganglia"));
        assert!(reference.release_date.is_none());
    }
}
