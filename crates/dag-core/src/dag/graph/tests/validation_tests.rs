//! Validation: port payload/format compatibility, schema compatibility,
//! required-input completeness, and path-aliasing rejection.

use std::assert_matches;

use super::common::*;
use crate::dag::NodePorts;
use crate::dag::error::DagError;
use crate::dag::graph::DAG;
use crate::dag::graph::validation::schema_compatible;
use crate::dag::runtime::{RuntimeStatus, SchedulerConfig};
use crate::value::PortType;

#[test]
fn port_payload_type_mismatch_rejected() {
    let mut dag = DAG::default();
    dag.add_node(
        "file".into(),
        Box::new(PortedNode(
            NodePorts::new().add_output_port_of_type(None, PortType::File),
        )),
    )
    .unwrap();
    dag.add_node(
        "df".into(),
        Box::new(PortedNode(NodePorts::new().add_input_port(None))),
    )
    .unwrap();

    let err = dag.add_edge("file", "df", 0, 0).unwrap_err();
    assert_matches!(err, DagError::PortTypeMismatch { .. });
}

#[test]
fn add_edge_rejects_incompatible_port_formats() {
    let mut dag = DAG::default();
    dag.add_node(
        "munge".into(),
        Box::new(PortedNode(
            NodePorts::new().add_output_port_of_type_with_label_and_format(
                None,
                PortType::File,
                "log",
                "ldsc_log",
            ),
        )),
    )
    .unwrap();
    dag.add_node(
        "h2".into(),
        Box::new(PortedNode(
            NodePorts::new().add_input_port_of_type_with_label_and_format(
                None,
                PortType::File,
                "sumstats",
                "sumstats_gz",
            ),
        )),
    )
    .unwrap();

    let err = dag.add_edge("munge", "h2", 0, 0).unwrap_err();
    assert_matches!(
        err,
        DagError::PortFormatMismatch {
            expected, actual, ..
        } if expected == "sumstats_gz" && actual == "ldsc_log"
    );
}

#[test]
fn add_edge_allows_declared_alternate_input_formats() {
    let mut dag = DAG::default();
    dag.add_node(
        "reference".into(),
        Box::new(PortedNode(
            NodePorts::new().add_output_port_of_type_with_label_and_format(
                None,
                PortType::File,
                "sumstats",
                "sumstats_tsv",
            ),
        )),
    )
    .unwrap();
    dag.add_node(
        "h2".into(),
        Box::new(PortedNode(
            NodePorts::new().add_input_port_of_type_with_accepted_formats(
                None,
                PortType::File,
                "sumstats",
                "sumstats_gz",
                ["sumstats_gz", "sumstats_tsv"],
            ),
        )),
    )
    .unwrap();

    dag.add_edge("reference", "h2", 0, 0).unwrap();
}

#[tokio::test]
async fn missing_upstream_output_fails_loudly_at_dispatch() {
    // PortedNode declares one DataFrame output port but executes to an
    // empty PortOutputs — the edge validates yet delivers nothing.
    let mut dag = DAG::default();
    dag.add_node(
        "src".into(),
        Box::new(PortedNode(NodePorts::new().add_output_port(None))),
    )
    .unwrap();
    dag.add_node(
        "sink".into(),
        Box::new(PortedNode(
            NodePorts::new().add_input_port(None).add_output_port(None),
        )),
    )
    .unwrap();
    dag.add_edge("src", "sink", 0, 0).unwrap();

    let err = dag
        .run(&SchedulerConfig::default(), &test_ctx(), None)
        .await
        .unwrap_err();
    assert_matches!(
        err,
        DagError::MissingUpstreamOutput {
            from_node,
            from_port: 0,
            to_node,
            to_port: 0,
        } if from_node == "src" && to_node == "sink"
    );
}

#[tokio::test]
async fn runtime_output_type_mismatch_fails_node() {
    let mut dag = DAG::default();
    dag.add_node("bad".into(), Box::new(MisdeclaredOutputNode::default()))
        .unwrap();

    let report = dag
        .run(&SchedulerConfig::default(), &test_ctx(), None)
        .await
        .unwrap();

    assert!(!report.ok);
    assert_eq!(dag.status("bad"), Some(RuntimeStatus::Failed));
    assert_matches!(
        report.errors.get("bad"),
        Some(DagError::PortTypeMismatch { .. })
    );
}

#[test]
fn validation_rejects_path_reference_aliased_to_an_in_dag_sink() {
    let mut dag = DAG::default();
    dag.add_node(
        "writer".into(),
        Box::new(DeclaredSinkNode::new("/data/out.parquet")),
    )
    .unwrap();
    dag.add_node(
        "reader".into(),
        Box::new(PathReaderNode::new("file:///data/out.parquet")),
    )
    .unwrap();

    let error = dag.validate().unwrap_err().to_string();
    assert!(error.contains("`reader` references file"), "{error}");
    assert!(error.contains("`writer`"), "{error}");
    assert!(error.contains("output port"), "{error}");
}

#[test]
fn validation_allows_path_references_to_external_files() {
    let mut dag = DAG::default();
    dag.add_node(
        "writer".into(),
        Box::new(DeclaredSinkNode::new("/data/out.parquet")),
    )
    .unwrap();
    // Different path — an external input, exactly the intended use.
    dag.add_node(
        "reader".into(),
        Box::new(PathReaderNode::new("/data/external.parquet")),
    )
    .unwrap();
    dag.validate().unwrap();
}

#[test]
fn schema_compatible_passes() {
    // Output schema is a superset of input schema → OK.
    let out = make_schema(&[
        ("a", arrow_schema::DataType::Int32),
        ("b", arrow_schema::DataType::Utf8),
    ]);
    let inp = make_schema(&[("a", arrow_schema::DataType::Int32)]);
    assert!(schema_compatible(&out, &inp).is_ok());
}

#[test]
fn schema_mismatch_rejected() {
    // Input requires a column the output lacks, and a type differs.
    let out = make_schema(&[("a", arrow_schema::DataType::Int32)]);
    let inp = make_schema(&[
        ("a", arrow_schema::DataType::Int64),
        ("b", arrow_schema::DataType::Utf8),
    ]);
    let err = schema_compatible(&out, &inp).unwrap_err();
    assert!(err.contains("a"), "{err}");
}

#[test]
fn validate_schema_mismatch_between_ports() {
    let mut dag = DAG::default();
    let out_schema = make_schema(&[("a", arrow_schema::DataType::Int32)]);
    let in_schema = make_schema(&[("a", arrow_schema::DataType::Int64)]);
    dag.add_node(
        "src".into(),
        Box::new(PortedNode(
            NodePorts::new().add_output_port(Some(out_schema)),
        )),
    )
    .unwrap();
    dag.add_node(
        "dst".into(),
        Box::new(PortedNode(NodePorts::new().add_input_port(Some(in_schema)))),
    )
    .unwrap();
    // Schema mismatch is caught at `add_edge` time (via validate_edge_schema).
    let err = dag.add_edge("src", "dst", 0, 0).unwrap_err();
    assert!(
        matches!(err, DagError::SchemaMismatch { ref from_node, ref to_port, .. } if from_node == "src" && *to_port == 0),
        "expected SchemaMismatch, got {err:?}"
    );
}
