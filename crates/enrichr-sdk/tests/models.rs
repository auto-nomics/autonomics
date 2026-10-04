//! Offline deserialization tests against response fixtures captured from
//! the live API (2026-10).

use std::collections::BTreeMap;

use enrichr_sdk::client::sanitize_nonfinite_json;
use enrichr_sdk::types::{
    AddedList, DatasetStatistics, EnrichmentResult, GeneMap, GeneSetTerm, SpeedrichrBackground,
    ViewedList,
};

#[test]
fn parses_add_list_acknowledgement() {
    let added: AddedList = serde_json::from_str(
        r#"{"shortId":"6881137a71c93ea0c6ff8691b2a5205d","userListId":138933542}"#,
    )
    .unwrap();
    assert_eq!(added.user_list_id, 138933542);
    assert_eq!(
        added.short_id.as_deref(),
        Some("6881137a71c93ea0c6ff8691b2a5205d")
    );
    assert!(added.has_share_id());

    let speedrichr: AddedList =
        serde_json::from_str(r#"{"userListId":1499005062, "shortId": "59590086"}"#).unwrap();
    assert_eq!(speedrichr.user_list_id, 1499005062);
}

#[test]
fn parses_viewed_list() {
    let viewed: ViewedList = serde_json::from_str(
        r#"{"genes": ["MYC","PTEN","BRCA1","EGFR","TP53"],"description": "probe"}"#,
    )
    .unwrap();
    assert_eq!(viewed.genes.len(), 5);
    assert_eq!(viewed.description.as_deref(), Some("probe"));
}

#[test]
fn parses_dataset_statistics() {
    let stats: DatasetStatistics = serde_json::from_str(
        r#"{"statistics": [
            {"geneCoverage": 13362, "genesPerTerm": 275, "libraryName": "Genome_Browser_PWMs",
             "link": "http://example.org", "numTerms": 615, "appyter": "x", "categoryId": 1}
        ]}"#,
    )
    .unwrap();
    assert_eq!(stats.statistics.len(), 1);
    assert_eq!(stats.statistics[0].library_name, "Genome_Browser_PWMs");
    assert_eq!(stats.statistics[0].num_terms, Some(615));
    assert_eq!(stats.statistics[0].genes_per_term, Some(275.0));
    assert_eq!(stats.library_names(), vec!["Genome_Browser_PWMs"]);
    assert_eq!(stats.filter("browser").len(), 1);
    assert_eq!(stats.filter("kegg").len(), 0);
}

#[test]
fn parses_enrichment_row_from_positional_array() {
    let row = serde_json::json!([
        1,
        "Breast cancer",
        2.0029875169873494e-11,
        99265.0,
        2445273.779636988,
        ["MYC", "PTEN", "BRCA1", "TP53", "EGFR"],
        1.742599139778994e-9,
        0,
        0
    ]);
    let term: GeneSetTerm = serde_json::from_value(row).unwrap();
    assert_eq!(term.rank, 1);
    assert_eq!(term.term, "Breast cancer");
    assert_eq!(term.overlap_count(), 5);
    assert!((term.p_value - 2.0029875169873494e-11).abs() < 1e-25);
    assert!((term.z_score - 99265.0).abs() < f64::EPSILON);
    assert_eq!(term.old_p_value, 0.0);
}

#[test]
fn accepts_string_joined_genes_and_extra_columns() {
    let row = serde_json::json!([
        3,
        "Wnt signaling pathway",
        0.222,
        6.0,
        8.03,
        "MYC;PTEN",
        0.9999,
        0,
        0,
        "extra"
    ]);
    let term: GeneSetTerm = serde_json::from_value(row).unwrap();
    assert_eq!(term.overlapping_genes, vec!["MYC", "PTEN"]);
    assert!((term.combined_score - 8.03).abs() < 1e-9);
}

#[test]
fn rejects_short_rows() {
    let row = serde_json::json!([1, "term", 0.5]);
    assert!(serde_json::from_value::<GeneSetTerm>(row).is_err());
}

#[test]
fn parses_speedrichr_infinity_payload() {
    // Captured from POST /speedrichr/api/backgroundenrich.
    let raw = r#"{"KEGG_2021_Human" : [[1,"MicroRNAs in cancer",0.08333308797119796, Infinity, Infinity, ["MYC","PTEN","BRCA1","TP53","EGFR"],0.9999997708591739, 0, 0 ], [2,"Breast cancer",0.22222198248930172, Infinity, Infinity, ["MYC","PTEN","BRCA1","TP53","EGFR"],0.9999997708591739, 0, 0 ], [4,"Central carbon metabolism in cancer",0.2619043694860644, 6.0, 8.038655062872337, ["MYC","PTEN","TP53","EGFR"],0.9999997708591739, 0, 0 ]]}"#;
    let map: BTreeMap<String, Vec<GeneSetTerm>> =
        serde_json::from_str(&sanitize_nonfinite_json(raw)).unwrap();
    let result = EnrichmentResult::from_singleton_map(map).unwrap();
    assert_eq!(result.library, "KEGG_2021_Human");
    assert_eq!(result.terms.len(), 3);
    assert_eq!(result.terms[0].z_score, 1e308);
    assert!((result.terms[2].z_score - 6.0).abs() < 1e-9);
    assert_eq!(result.significant(0.5).count(), 0);
}

#[test]
fn empty_enrichment_map_reports_unknown_library() {
    let map: BTreeMap<String, Vec<GeneSetTerm>> =
        serde_json::from_str(&sanitize_nonfinite_json("{}")).unwrap();
    let error = EnrichmentResult::from_singleton_map(map).unwrap_err();
    assert!(error.to_string().contains("backgroundType"));
}

#[test]
fn parses_speedrichr_background_token() {
    let background: SpeedrichrBackground =
        serde_json::from_str(r#"{"backgroundid": "29a65acb"}"#).unwrap();
    assert_eq!(background.background_id, "29a65acb");
}

#[test]
fn parses_genemap() {
    // Captured shape: the "gene" key holds library → terms; the sibling
    // "descriptions" catalog is not modeled.
    let raw = r#"{"gene": {"GeneSigDB":["17982488-Table1","19695104-AF1-1"],"ChEA_2013":["TP53-TP53" ]},"descriptions": [{"name": "GeneSigDB", "description": "x"}]}"#;
    #[derive(serde::Deserialize)]
    struct Envelope {
        gene: BTreeMap<String, Vec<String>>,
    }
    let envelope: Envelope = serde_json::from_str(raw).unwrap();
    let gene_map = GeneMap::from_envelope("TP53", envelope.gene);
    assert_eq!(gene_map.gene, "TP53");
    assert_eq!(gene_map.term_count(), 3);
    assert_eq!(gene_map.libraries["GeneSigDB"].len(), 2);
}
