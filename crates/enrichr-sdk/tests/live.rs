//! Live API smoke tests. Run explicitly:
//! `cargo test -p enrichr-sdk --test live -- --ignored`

use std::sync::LazyLock;

use enrichr_sdk::EnrichrClient;

/// One shared, rate-limited client so parallel tests hit the public API as a
/// single politely paced stream instead of a burst.
static CLIENT: LazyLock<EnrichrClient> = LazyLock::new(EnrichrClient::new);

fn client() -> &'static EnrichrClient {
    &CLIENT
}

#[tokio::test]
#[ignore = "requires network access and uses the public Enrichr service"]
async fn lists_libraries_from_dataset_statistics() {
    let stats = client().dataset_statistics().await.unwrap();
    assert!(stats.statistics.len() > 100, "expected a large catalog");
    let names = stats.library_names();
    assert!(names.contains(&"KEGG_2021_Human".to_owned()));
    assert!(!stats.filter("go_biological").is_empty());
}

#[tokio::test]
#[ignore = "requires network access and uses the public Enrichr service"]
async fn submits_lists_and_enriches() {
    let client = client();
    let added = client
        .add_list(
            ["TP53", "BRCA1", "EGFR", "MYC", "PTEN"],
            "enrichr-sdk smoke",
        )
        .await
        .unwrap();
    assert!(added.user_list_id > 0);
    assert!(added.has_share_id());
    assert!(
        client
            .share_url(added.short_id.as_deref().unwrap())
            .contains("/enrich?dataset=")
    );

    let viewed = client.view(added.user_list_id).await.unwrap();
    assert_eq!(viewed.genes.len(), 5);
    assert!(viewed.genes.contains(&"TP53".to_owned()));

    let result = client
        .enrich(added.user_list_id, "KEGG_2021_Human")
        .await
        .unwrap();
    assert_eq!(result.library, "KEGG_2021_Human");
    assert!(!result.terms.is_empty());
    assert!(result.terms.iter().all(|term| term.p_value >= 0.0));
    assert!(result.terms.iter().all(|term| term.rank >= 1));
    assert!(
        result
            .terms
            .iter()
            .all(|term| !term.overlapping_genes.is_empty())
    );

    let exported = client
        .export(added.user_list_id, "KEGG_2021_Human", "smoke")
        .await
        .unwrap();
    assert!(exported.starts_with("Term\tOverlap\tP-value"));
    assert!(exported.contains("Breast cancer"));

    let error = client
        .enrich(added.user_list_id, "Not_A_Library")
        .await
        .unwrap_err();
    assert!(error.to_string().contains("backgroundType"), "got: {error}");
}

#[tokio::test]
#[ignore = "requires network access and uses the public Enrichr service"]
async fn downloads_gene_set_library_gmt() {
    let gmt = client().gene_set_library("ChEA_2013").await.unwrap();
    assert!(gmt.lines().count() > 50);
    let first = gmt.lines().next().unwrap();
    assert!(
        first.split('\t').count() > 3,
        "GMT rows carry term, description, genes"
    );
}

#[tokio::test]
#[ignore = "requires network access and uses the public Enrichr service"]
async fn maps_one_gene_across_libraries() {
    let map = client().genemap("TP53").await.unwrap();
    assert!(map.term_count() > 100);
    assert!(map.libraries.contains_key("ChEA_2013"));
}

#[tokio::test]
#[ignore = "requires network access and uses the public Speedrichr service"]
async fn runs_background_corrected_enrichment() {
    let client = client();
    let list = client
        .speedrichr_add_list(["TP53", "BRCA1", "EGFR", "MYC", "PTEN"], "speedrichr smoke")
        .await
        .unwrap();
    assert!(list.user_list_id > 0);

    let background = client
        .speedrichr_add_background([
            "TP53", "BRCA1", "EGFR", "MYC", "PTEN", "AKT1", "KRAS", "CDK2", "RB1", "MDM2",
        ])
        .await
        .unwrap();
    assert!(!background.background_id.is_empty());

    let result = client
        .speedrichr_background_enrich(
            list.user_list_id,
            &background.background_id,
            "KEGG_2021_Human",
        )
        .await
        .unwrap();
    assert_eq!(result.library, "KEGG_2021_Human");
    assert!(!result.terms.is_empty());
    // Bare Infinity odds ratios must have been sanitized, not rejected.
    assert!(result.terms.iter().all(|term| term.z_score.is_finite()));
}
