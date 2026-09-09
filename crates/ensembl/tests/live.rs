//! Live endpoint checks. Run explicitly with:
//! `cargo test -p ensembl --test live -- --ignored`

use ensembl::{EnsemblClient, SequenceType};

#[tokio::test]
#[ignore = "live Ensembl API test"]
async fn core_endpoints_decode() {
    let client = EnsemblClient::new();

    let species = client.species().await.unwrap();
    assert!(
        species
            .species
            .iter()
            .any(|item| item.name == "homo_sapiens")
    );

    let assembly = client.assembly("human").await.unwrap();
    assert_eq!(assembly.assembly_name, "GRCh38.p14");

    let lookup = client.lookup_id("ENSG00000139618", false).await.unwrap();
    assert_eq!(lookup.display_name.as_deref(), Some("BRCA2"));

    let expanded = client.lookup_id("ENSG00000139618", true).await.unwrap();
    assert!(
        expanded
            .transcripts
            .iter()
            .any(|transcript| transcript.id == "ENST00000380152")
    );

    let batch = client
        .lookup_ids(
            &["ENSG00000139618".to_string(), "ENST00000380152".to_string()],
            false,
        )
        .await
        .unwrap();
    assert_eq!(batch.len(), 2);
    assert_eq!(
        batch["ENSG00000139618"]["display_name"].as_str(),
        Some("BRCA2")
    );

    let xrefs = client.xrefs("ENSG00000139618", true, None).await.unwrap();
    assert!(xrefs.iter().any(|xref| xref.dbname == "HGNC"));

    let overlap = client
        .overlap_region(
            "human",
            "13:32355000-32357000",
            &["gene".to_string(), "transcript".to_string()],
        )
        .await
        .unwrap();
    assert!(!overlap.is_empty());

    let sequence = client
        .sequence_id("ENST00000380152", SequenceType::Protein, None, None)
        .await
        .unwrap();
    assert!(sequence.seq.starts_with('M'));

    let vep = client.vep_id("human", "rs80357906").await.unwrap();
    assert_eq!(
        vep[0].most_severe_consequence.as_deref(),
        Some("frameshift_variant")
    );
    assert!(!vep[0].transcript_consequences.is_empty());

    let variation = client.variation("human", "rs80357906").await.unwrap();
    assert!(
        variation
            .clinical_significance
            .iter()
            .any(|value| value == "pathogenic")
    );
}

#[tokio::test]
#[ignore = "live Ensembl API test"]
async fn vep_region_decodes() {
    let results = EnsemblClient::new()
        .vep_region("human", "17:43057063-43057065:1", "G")
        .await
        .unwrap();
    assert_eq!(
        results[0].most_severe_consequence.as_deref(),
        Some("frameshift_variant")
    );
}
