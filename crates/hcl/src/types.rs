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
    #[serde(default)]
    pub supplied_md5: Option<String>,
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
    pub files: Vec<ArticleFile>,
}

pub fn classify_file(name: &str) -> &'static str {
    let name = name.to_ascii_lowercase();
    if name.starts_with("hcl_fig1") {
        if name.ends_with(".h5ad") {
            "hcl_h5ad"
        } else {
            "hcl_cell_metadata"
        }
    } else if name.starts_with("mca1.1") {
        if name.ends_with(".h5ad") {
            "mca_h5ad"
        } else {
            "mca_cell_metadata"
        }
    } else if name.contains("dge_raw_data") {
        "dge_raw_archive"
    } else if name.contains("dge_rmbatch_data") {
        "dge_rmbatch_archive"
    } else if name.contains("cluster_markers") {
        "markers"
    } else if name.contains("annotation_cluster_info") {
        "cluster_annotation"
    } else if name.contains("annotation_rmbatch_data") {
        "dge_rmbatch_annotation"
    } else {
        "other"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_hcl_release_files() {
        assert_eq!(classify_file("HCL_Fig1_adata.h5ad"), "hcl_h5ad");
        assert_eq!(
            classify_file("HCL_Fig1_cell_Info.xlsx"),
            "hcl_cell_metadata"
        );
        assert_eq!(classify_file("MCA1.1_adata.h5ad"), "mca_h5ad");
        assert_eq!(classify_file("cluster_markers_HCL&MCA1.1.xlsx"), "markers");
        assert_eq!(
            classify_file("annotation_rmbatch_data_revised417.zip"),
            "dge_rmbatch_annotation"
        );
    }
}
