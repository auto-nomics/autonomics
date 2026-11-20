//! Live API tests. Run with:
//!
//! ```text
//! ALPHAGENOME_API_KEY=... cargo test -p alphagenome --test live -- --ignored --nocapture
//! ```

use alphagenome::{AlphaGenomeClient, Organism, VariantQuery};

#[tokio::test]
#[ignore = "live API test; requires ALPHAGENOME_API_KEY"]
async fn atlas_scorer_metadata_lists_avi() {
    let mut client = AlphaGenomeClient::from_env().await.unwrap();
    let metadata = client
        .atlas_scorer_metadata(Organism::HomoSapiens)
        .await
        .unwrap();
    assert!(
        metadata.iter().any(|m| m.name == "AVI_SCORE"),
        "{metadata:?}"
    );
}

#[tokio::test]
#[ignore = "live API test; requires ALPHAGENOME_API_KEY"]
async fn atlas_avi_for_known_variant() {
    let mut client = AlphaGenomeClient::from_env().await.unwrap();
    let variant: VariantQuery = "chr22:36201698:A>C".parse().unwrap();
    let filter = AlphaGenomeClient::scorer_filter(&["AVI_SCORE"]).unwrap();
    let scores = client
        .atlas_variant_scores(&variant, Some(&filter))
        .await
        .unwrap();
    assert_eq!(scores.variant.chromosome, "chr22");
    assert_eq!(scores.variant.position, 36201698);
    let avi = scores
        .scores
        .iter()
        .find(|s| s.scorer == "AVI_SCORE")
        .expect("AVI_SCORE in response");
    assert_eq!(avi.values.len(), 1);
    println!("AVI raw: {:?}", avi.values);
    println!("AVI calibrated (CDF): {:?}", avi.calibrated_values);
}
