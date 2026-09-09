//! Live integration tests for the public RCSB APIs.

use rcsb::search::SearchRequest;
use rcsb::{RcsbClient, StructureFormat};

#[tokio::test]
#[ignore = "live RCSB API test"]
async fn entry_and_polymer_metadata_fetch() {
    let client = RcsbClient::new();
    let entry = client.entry("4hhb").await.expect("entry metadata");

    assert_eq!(entry.rcsb_id, "4HHB");
    assert_eq!(entry.experimental_method(), Some("X-RAY DIFFRACTION"));
    assert_eq!(entry.resolution(), Some(1.74));
    assert_eq!(entry.rcsb_entry_info.deposited_atom_count, Some(4779));

    let polymer = client
        .polymer_entity("4HHB", 1)
        .await
        .expect("polymer metadata");
    assert_eq!(
        polymer.rcsb_polymer_entity.pdbx_description,
        "Hemoglobin subunit alpha"
    );
    assert_eq!(polymer.sequence_length(), 141);
    assert!(
        polymer
            .rcsb_polymer_entity_container_identifiers
            .uniprot_ids
            .contains(&"P69905".to_owned())
    );
}

#[tokio::test]
#[ignore = "live RCSB API test"]
async fn full_text_search_and_metadata_enrichment_fetch() {
    let client = RcsbClient::new();
    let response = client
        .search(&SearchRequest::full_text("deoxyhaemoglobin").rows(2))
        .await
        .expect("search");

    assert!(response.total_count > 0);
    assert_eq!(response.result_set.len(), 2);
    let entries = client
        .entries(
            response
                .result_set
                .iter()
                .map(|result| result.identifier.clone()),
        )
        .await
        .expect("entry enrichment");
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().all(|entry| !entry.rcsb_id.is_empty()));
}

#[tokio::test]
#[ignore = "live RCSB API test"]
async fn structure_text_and_bounded_preview_fetch() {
    let client = RcsbClient::new();
    let fasta = client
        .structure_text("4HHB", StructureFormat::Fasta)
        .await
        .expect("FASTA");
    assert!(fasta.starts_with('>'));
    assert!(fasta.contains("4HHB"));

    let preview = client
        .structure_preview("4HHB", StructureFormat::Pdb, 4096)
        .await
        .expect("PDB preview");
    assert_eq!(preview.entry_id, "4HHB");
    assert!(preview.downloaded_bytes <= 4096);
    assert!(preview.truncated);
    assert!(preview.text.contains("HEADER"));
}
