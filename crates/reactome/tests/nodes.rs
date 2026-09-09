//! DAG node factory registration and Arrow batch construction tests.

use reactome::nodes::analysis::{ReactomeAnalysisNodeFactory, build_analysis_batch};
use reactome::nodes::mapping::{ReactomeMappingNodeFactory, build_mapping_batch};
use reactome::nodes::participants::{ReactomeParticipantsNodeFactory, build_participants_batch};
use reactome::nodes::pathways::{ReactomePathwaysNodeFactory, build_pathways_batch};
use reactome::types::*;

fn build(factory: &dyn dag_core::registry::NodeFactory) -> Box<dyn dag_core::dag::DagNode> {
    let ctx = dag_core::registry::NodeCtx::new(
        std::sync::Arc::new(datafusion::execution::runtime_env::RuntimeEnv::default()),
        None,
    );
    let kind = factory.kind();
    let schema = factory.spec_schema();
    let spec = match kind {
        "source_reactome_pathways" => serde_json::json!({"species": "Homo sapiens"}),
        "source_reactome_mapping" => serde_json::json!({"resource": "UniProt", "identifier": "P04637"}),
        "source_reactome_analysis" => serde_json::json!({"identifiers": ["TP53"]}),
        "source_reactome_participants" => serde_json::json!({"id": "R-HSA-1640170"}),
        _ => panic!("unknown kind: {kind}"),
    };
    // Confirm the spec schema can be serialized (used by get_node_spec tools).
    let _ = serde_json::to_value(&schema).unwrap();
    factory
        .build(spec, ctx)
        .unwrap_or_else(|e| panic!("factory build failed for {kind}: {e}"))
}

#[test]
fn pathway_node_builds_and_emits_correct_kind() {
    let node = build(&ReactomePathwaysNodeFactory {});
    assert_eq!(node.kind(), "source_reactome_pathways");
    assert_eq!(node.ports().output_ports().len(), 1);
    assert_eq!(node.ports().input_ports().len(), 0);
}

#[test]
fn mapping_node_builds_and_emits_correct_kind() {
    let node = build(&ReactomeMappingNodeFactory {});
    assert_eq!(node.kind(), "source_reactome_mapping");
    assert_eq!(node.ports().output_ports().len(), 1);
}

#[test]
fn analysis_node_builds_and_emits_correct_kind() {
    let node = build(&ReactomeAnalysisNodeFactory {});
    assert_eq!(node.kind(), "source_reactome_analysis");
    assert_eq!(node.ports().output_ports().len(), 1);
}

#[test]
fn participants_node_builds_and_emits_correct_kind() {
    let node = build(&ReactomeParticipantsNodeFactory {});
    assert_eq!(node.kind(), "source_reactome_participants");
    assert_eq!(node.ports().output_ports().len(), 1);
}

#[test]
fn pathways_batch_has_expected_schema() {
    let pathways = vec![Pathway {
        db_id: 1640170,
        display_name: "Cell Cycle".to_string(),
        stable_id: Some("R-HSA-1640170".to_string()),
        schema_class: Some("TopLevelPathway".to_string()),
        has_diagram: true,
        ..Default::default()
    }];
    let batch = build_pathways_batch(&pathways).unwrap();
    assert_eq!(batch.num_rows(), 1);
    assert_eq!(batch.num_columns(), 9);
    let names: Vec<&str> = (0..batch.num_columns())
        .map(|i| batch.schema_ref().field(i).name().as_str())
        .collect();
    assert_eq!(
        names,
        vec![
            "db_id", "stable_id", "display_name", "schema_class", "species_name",
            "is_in_disease", "is_inferred", "has_diagram", "has_ehld",
        ]
    );
}

#[test]
fn mapping_batch_flattens_optional_fields() {
    let mapped = vec![MappedPathway {
        db_id: 6796648,
        display_name: "TP53 Regulates Transcription".to_string(),
        stable_id: Some("R-HSA-6796648".to_string()),
        schema_class: Some("Pathway".to_string()),
        ..Default::default()
    }];
    let batch = build_mapping_batch(&mapped).unwrap();
    assert_eq!(batch.num_rows(), 1);
    assert_eq!(batch.num_columns(), 4);
}

#[test]
fn analysis_batch_carries_p_value_and_fdr() {
    let result = AnalysisResult {
        pathways: vec![EnrichedPathway {
            stable_id: "R-HSA-6796648".to_string(),
            name: "TP53 Regulates Transcription".to_string(),
            entities: PathwayEntities {
                found: 4,
                total: 86,
                p_value: Some(5.4e-8),
                fdr: Some(1.4e-5),
                ..Default::default()
            },
            ..Default::default()
        }],
        ..Default::default()
    };
    let batch = build_analysis_batch(&result).unwrap();
    assert_eq!(batch.num_rows(), 1);
    assert_eq!(batch.num_columns(), 6);
    let names: Vec<&str> = (0..batch.num_columns())
        .map(|i| batch.schema_ref().field(i).name().as_str())
        .collect();
    assert!(names.contains(&"p_value"));
    assert!(names.contains(&"fdr"));
}

#[test]
fn participants_batch_extracts_reference_fields() {
    let participants = vec![Participant {
        db_id: 109712,
        display_name: Some("TP53".to_string()),
        stable_id: Some("R-HSA-109712".to_string()),
        schema_class: Some("EntityWithAccessionedSequence".to_string()),
        reference_entity: Some(ReferenceEntity {
            identifier: Some("P04637".to_string()),
            database_name: Some("UniProt".to_string()),
            ..Default::default()
        }),
    }];
    let batch = build_participants_batch(&participants).unwrap();
    assert_eq!(batch.num_rows(), 1);
    assert_eq!(batch.num_columns(), 6);
    let names: Vec<&str> = (0..batch.num_columns())
        .map(|i| batch.schema_ref().field(i).name().as_str())
        .collect();
    assert!(names.contains(&"reference_identifier"));
    assert!(names.contains(&"reference_database"));
}
