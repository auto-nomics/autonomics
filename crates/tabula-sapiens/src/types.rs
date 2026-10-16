use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Version {
    pub version: u32,
    pub url: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct License {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ArticleFile {
    pub name: String,
    pub size: Option<u64>,
    pub download_url: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Article {
    pub id: u64,
    pub title: String,
    pub doi: String,
    pub version: u32,
    #[serde(default)]
    pub published_date: Option<String>,
    #[serde(default)]
    pub modified_date: Option<String>,
    #[serde(default)]
    pub license: Option<License>,
    #[serde(default)]
    pub url_public_html: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub files: Vec<ArticleFile>,
}

pub fn classify_file(name: &str) -> (&'static str, Option<String>) {
    if name.eq_ignore_ascii_case("Tabula_Sapiens_metadata.csv") {
        return ("metadata", Some("cell metadata".into()));
    }
    if name.eq_ignore_ascii_case("TabulaSapiens.h5ad.zip") {
        return ("full_atlas_h5ad", None);
    }
    let compendia = ["endothelial", "stromal", "immune", "epithelial"];
    if let Some(category) = compendia
        .iter()
        .find(|category| name.to_ascii_lowercase().contains(**category))
    {
        return ("compartment_h5ad", Some((*category).to_owned()));
    }
    if let Some(stem) = name
        .strip_prefix("TS_")
        .and_then(|value| value.strip_suffix(".h5ad.zip"))
    {
        let organ = stem.replace('_', " ");
        return ("organ_h5ad", Some(organ));
    }
    ("other", None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_release_files() {
        assert_eq!(
            classify_file("TS_Bone_Marrow.h5ad.zip"),
            ("organ_h5ad", Some("Bone Marrow".into()))
        );
        assert_eq!(
            classify_file("TS_immune.h5ad.zip"),
            ("compartment_h5ad", Some("immune".into()))
        );
        assert_eq!(
            classify_file("TabulaSapiens.h5ad.zip"),
            ("full_atlas_h5ad", None)
        );
        assert_eq!(
            classify_file("Tabula_Sapiens_metadata.csv"),
            ("metadata", Some("cell metadata".into()))
        );
    }
}
