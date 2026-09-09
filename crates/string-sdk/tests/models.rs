use string_sdk::request::{NetworkQuery, OutputFormat, StringIdQuery};
use string_sdk::types::{Enrichment, Homology, Interaction, TaxonId, Version};

#[test]
fn parses_conventional_json_models() {
    let versions: Vec<Version> = serde_json::from_str(
        r#"[{"string_version":"12.0","stable_address":"https://version-12-0.string-db.org"}]"#,
    )
    .unwrap();
    assert_eq!(versions[0].string_version, "12.0");

    let interaction: Interaction = serde_json::from_value(serde_json::json!({
        "stringId_A": "9606.ENSP00000269305",
        "stringId_B": "9606.ENSP00000266970",
        "preferredName_A": "TP53",
        "preferredName_B": "CDK2",
        "ncbiTaxonId": "9606",
        "score": 0.999,
        "escore": 0.5
    }))
    .unwrap();
    assert_eq!(interaction.ncbi_taxon_id.as_u64(), Some(9606));
    assert_eq!(interaction.escore, 0.5);

    let homology: Vec<Homology> = serde_json::from_str(
        r#"[{
            "ncbiTaxonId_A": 9606,
            "stringId_A": "9606.ENSP00000269305",
            "ncbiTaxonId_B": 10090,
            "stringId_B": "10090.ENSMUSP00000031126",
            "bitscore": "815.8"
        }]"#,
    )
    .unwrap();
    assert_eq!(homology[0].bitscore, 815.8);
}

#[test]
fn builds_typed_queries() {
    let query = StringIdQuery::new([" TP53 ", "CDK2"])
        .species("9606")
        .echo_query(true);
    assert_eq!(
        query.identifiers,
        vec![" TP53 ".to_owned(), "CDK2".to_owned()]
    );

    let network = NetworkQuery::from_term("hsa04110")
        .required_score(400)
        .network_type(string_sdk::NetworkType::Physical);
    assert_eq!(network.required_score, Some(400));
}

#[test]
fn output_formats_match_string_paths() {
    assert_eq!(OutputFormat::Json.as_str(), "json");
    assert_eq!(OutputFormat::TsvNoHeader.as_str(), "tsv-no-header");
}
