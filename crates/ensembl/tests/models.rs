use ensembl::convert::vep_table;
use ensembl::types::*;
use ensembl::{SequenceType, nucleotide_stats};

#[test]
fn decodes_expanded_lookup_entry() {
    let raw = serde_json::json!({
        "id": "ENSG00000139618",
        "object_type": "Gene",
        "biotype": "protein_coding",
        "display_name": "BRCA2",
        "species": "homo_sapiens",
        "start": 32315086,
        "end": 32400268,
        "strand": 1,
        "seq_region_name": "13",
        "assembly_name": "GRCh38",
        "Transcript": [{
            "id": "ENST00000380152",
            "Parent": "ENSG00000139618",
            "start": 32315508,
            "end": 32400268,
            "strand": 1,
            "seq_region_name": "13",
            "assembly_name": "GRCh38",
            "Translation": {
                "id": "ENSP00000369497",
                "Parent": "ENST00000380152",
                "length": 3418
            },
            "Exon": [{
                "id": "ENSE00003659301",
                "start": 32325076,
                "end": 32325184,
                "strand": 1,
                "seq_region_name": "13"
            }]
        }]
    });
    let entry: LookupEntry = serde_json::from_value(raw).unwrap();
    assert_eq!(entry.id, "ENSG00000139618");
    let transcript = entry.transcripts.first().unwrap();
    assert_eq!(transcript.parent.as_deref(), Some("ENSG00000139618"));
    assert_eq!(
        transcript.translation.as_ref().unwrap().id,
        "ENSP00000369497"
    );
    assert_eq!(transcript.exons.len(), 1);
}

#[test]
fn decodes_variation_with_ensembl_field_case() {
    let raw = serde_json::json!({
        "name": "rs80357906",
        "MAF": 0.001,
        "clinical_significance": ["pathogenic"],
        "mappings": [{
            "seq_region_name": "17",
            "start": 43057063,
            "end": 43057065,
            "strand": 1,
            "assembly_name": "GRCh38",
            "location": "17:43057063-43057065"
        }]
    });
    let variation: Variation = serde_json::from_value(raw).unwrap();
    assert_eq!(variation.maf, Some(0.001));
    assert_eq!(variation.mappings[0].assembly_name, "GRCh38");
}

#[test]
fn sequence_helpers_are_stable() {
    assert_eq!(SequenceType::Protein.as_str(), "protein");
    let stats = nucleotide_stats("ACGTN");
    assert_eq!(stats.length, 5);
    assert_eq!(stats.gc_fraction, 0.4);
}

#[test]
fn vep_table_expands_one_row_per_transcript() {
    let raw = serde_json::json!([{
        "id": "rs80357906",
        "input": "rs80357906",
        "most_severe_consequence": "frameshift_variant",
        "transcript_consequences": [
            {"transcript_id": "ENST00000352993", "gene_symbol": "BRCA1"},
            {"transcript_id": "ENST00000357670", "gene_symbol": "BRCA1"}
        ]
    }]);
    let results: Vec<VepResult> = serde_json::from_value(raw).unwrap();
    let table = vep_table(&results).unwrap();
    assert_eq!(table.len(), 2);
    assert!(
        table
            .columns
            .contains(&"transcript_consequence".to_string())
    );
    assert!(table.rows.iter().all(|row| row.iter().all(scalar_cell)));
}

fn scalar_cell(value: &serde_json::Value) -> bool {
    !value.is_array() && !value.is_object()
}
