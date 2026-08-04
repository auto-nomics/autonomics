use super::*;
use crate::dag::history::{DagManifest, EdgeEntry, NodeEntry};

use std::sync::Arc;

use datafusion::prelude::SessionContext;
use datalake::Datalake;

/// Minimal registry for codegen tests (no Iceberg / opendal needed).
fn test_registry() -> crate::node_registry::registry::NodeRegistry {
    let ctx = SessionContext::new();
    let runtime_env = ctx.runtime_env();
    crate::node_registry::registry::NodeRegistry::new(
        runtime_env,
        None,
        Arc::new(Datalake::default()),
        None,
    )
}

// ── topo sort ───────────────────────────────────────────────────────────────

#[test]
fn topo_sort_linear_chain() {
    let manifest = DagManifest {
        nodes: vec![
            NodeEntry {
                id: "a".into(),
                kind: "echo".into(),
                spec: serde_json::json!({}),
            },
            NodeEntry {
                id: "b".into(),
                kind: "echo".into(),
                spec: serde_json::json!({}),
            },
            NodeEntry {
                id: "c".into(),
                kind: "echo".into(),
                spec: serde_json::json!({}),
            },
        ],
        edges: vec![
            EdgeEntry {
                from: "a".into(),
                from_port: 0,
                to: "b".into(),
                to_port: 0,
            },
            EdgeEntry {
                from: "b".into(),
                from_port: 0,
                to: "c".into(),
                to_port: 0,
            },
        ],
    };

    let registry = test_registry();
    let compiler = DagCompiler {
        registry: &registry,
    };
    let script = compiler.compile(&manifest, CodegenTarget::R).unwrap();

    // Nodes are unsupported (echo), so they'll appear as NOTE comments.
    // Verify topo order: a before b before c in the source string.
    let a_pos = script.source.find("'a'").unwrap();
    let b_pos = script.source.find("'b'").unwrap();
    let c_pos = script.source.find("'c'").unwrap();
    assert!(a_pos < b_pos, "a should come before b");
    assert!(b_pos < c_pos, "b should come before c");
}

#[test]
fn topo_sort_cycle_errors() {
    let manifest = DagManifest {
        nodes: vec![
            NodeEntry {
                id: "x".into(),
                kind: "echo".into(),
                spec: serde_json::json!({}),
            },
            NodeEntry {
                id: "y".into(),
                kind: "echo".into(),
                spec: serde_json::json!({}),
            },
        ],
        edges: vec![
            EdgeEntry {
                from: "x".into(),
                from_port: 0,
                to: "y".into(),
                to_port: 0,
            },
            EdgeEntry {
                from: "y".into(),
                from_port: 0,
                to: "x".into(),
                to_port: 0,
            },
        ],
    };

    let registry = test_registry();
    let compiler = DagCompiler {
        registry: &registry,
    };
    let err = compiler.compile(&manifest, CodegenTarget::R).unwrap_err();
    assert!(
        matches!(err, CodegenError::Topology(_)),
        "expected Topology error, got {err:?}"
    );
}

// ── unsupported nodes ───────────────────────────────────────────────────────

#[test]
fn compile_unsupported_emits_note() {
    // `echo` node has no codegen implementation — should produce a NOTE comment.
    let manifest = DagManifest {
        nodes: vec![NodeEntry {
            id: "echo1".into(),
            kind: "echo".into(),
            spec: serde_json::json!({}),
        }],
        edges: vec![],
    };

    let registry = test_registry();
    let compiler = DagCompiler {
        registry: &registry,
    };
    let script = compiler.compile(&manifest, CodegenTarget::R).unwrap();

    assert_eq!(script.skipped_nodes, vec!["echo".to_string()]);
    assert!(
        script.source.contains("# NOTE:"),
        "source should contain a NOTE comment"
    );
}

// ── variable flow: branch + merge ───────────────────────────────────────────

#[test]
fn var_flow_branch_merge() {
    // Diamond: source → branch_a / branch_b → merge
    // The merge node should reference both branch outputs.
    let manifest = DagManifest {
        nodes: vec![
            NodeEntry {
                id: "source".into(),
                kind: "source_file".into(),
                spec: serde_json::json!({"path": "/tmp/test.csv"}),
            },
            NodeEntry {
                id: "a".into(),
                kind: "echo".into(),
                spec: serde_json::json!({}),
            },
            NodeEntry {
                id: "b".into(),
                kind: "echo".into(),
                spec: serde_json::json!({}),
            },
        ],
        edges: vec![
            EdgeEntry {
                from: "source".into(),
                from_port: 0,
                to: "a".into(),
                to_port: 0,
            },
            EdgeEntry {
                from: "source".into(),
                from_port: 0,
                to: "b".into(),
                to_port: 0,
            },
        ],
    };

    let registry = test_registry();
    let compiler = DagCompiler {
        registry: &registry,
    };
    let script = compiler.compile(&manifest, CodegenTarget::R).unwrap();

    // Both `a` and `b` should reference `source` as their input.
    // Since echo is unsupported, it'll be skipped, but the topo order should
    // still be correct: source before a and b.
    let src_pos = script.source.find("source <- fread").unwrap();
    let a_pos = script.source.find("'a'").unwrap();
    let b_pos = script.source.find("'b'").unwrap();
    assert!(src_pos < a_pos, "source should come before a");
    assert!(src_pos < b_pos, "source should come before b");
}

// ── sanitize_var_name ───────────────────────────────────────────────────────

#[test]
fn sanitize_var_name_basics() {
    use super::context::sanitize_var_name;

    // Already valid
    assert_eq!(sanitize_var_name("my_node"), "my_node");
    assert_eq!(sanitize_var_name("node123"), "node123");

    // Leading digit → prefix
    assert_eq!(sanitize_var_name("1node"), "n_1node");

    // Special chars → underscore
    assert_eq!(sanitize_var_name("my-node"), "my_node");
    assert_eq!(sanitize_var_name("my.node"), "my_node");
    assert_eq!(sanitize_var_name("my node"), "my_node");

    // Empty
    assert_eq!(sanitize_var_name(""), "n_empty");
}

// ── empty DAG ───────────────────────────────────────────────────────────────

#[test]
fn compile_empty_dag() {
    let manifest = DagManifest {
        nodes: vec![],
        edges: vec![],
    };
    let registry = test_registry();
    let compiler = DagCompiler {
        registry: &registry,
    };
    let script = compiler.compile(&manifest, CodegenTarget::R).unwrap();

    assert!(
        script
            .source
            .contains("# Generated by autonomics DAG compiler")
    );
    assert!(script.skipped_nodes.is_empty());
    assert!(script.warnings.is_empty());
}

// ── golden tests for implemented node kinds ────────────────────────────────

#[test]
fn golden_source_file_csv_r() {
    let manifest = DagManifest {
        nodes: vec![NodeEntry {
            id: "src".into(),
            kind: "source_file".into(),
            spec: serde_json::json!({"path": "/tmp/gwas.csv"}),
        }],
        edges: vec![],
    };
    let registry = test_registry();
    let compiler = DagCompiler {
        registry: &registry,
    };
    let script = compiler.compile(&manifest, CodegenTarget::R).unwrap();

    assert!(
        script.source.contains("library(data.table)"),
        "should include data.table"
    );
    assert!(
        script.source.contains("src <- fread(\"/tmp/gwas.csv\")"),
        "should call fread"
    );
    assert!(script.skipped_nodes.is_empty());

    // Even a standalone source node writes its output as an edge CSV.
    assert!(
        script.source.contains("fwrite(src, \"_edge_src_0.csv\")"),
        "source should write edge CSV for its output port"
    );
}

#[test]
fn golden_source_file_parquet_r() {
    let manifest = DagManifest {
        nodes: vec![NodeEntry {
            id: "data".into(),
            kind: "source_file".into(),
            spec: serde_json::json!({"path": "/tmp/data.parquet", "format": "parquet"}),
        }],
        edges: vec![],
    };
    let registry = test_registry();
    let compiler = DagCompiler {
        registry: &registry,
    };
    let script = compiler.compile(&manifest, CodegenTarget::R).unwrap();

    assert!(
        script
            .source
            .contains("read_parquet(\"/tmp/data.parquet\")"),
        "should call read_parquet"
    );
}

#[test]
fn golden_sink_file_r() {
    let manifest = DagManifest {
        nodes: vec![
            NodeEntry {
                id: "src".into(),
                kind: "source_file".into(),
                spec: serde_json::json!({"path": "/tmp/in.csv"}),
            },
            NodeEntry {
                id: "out".into(),
                kind: "sink_file".into(),
                spec: serde_json::json!({"path": "/tmp/out.csv", "format": "csv"}),
            },
        ],
        edges: vec![EdgeEntry {
            from: "src".into(),
            from_port: 0,
            to: "out".into(),
            to_port: 0,
        }],
    };
    let registry = test_registry();
    let compiler = DagCompiler {
        registry: &registry,
    };
    let script = compiler.compile(&manifest, CodegenTarget::R).unwrap();

    // The sink should reference the source's output variable.
    assert!(
        script.source.contains("fwrite(src"),
        "should call fwrite with src variable"
    );
    assert!(
        script.source.contains("\"/tmp/out.csv\""),
        "should write to out.csv"
    );

    // Edge CSV: source writes edge, sink reads it.
    assert!(
        script.source.contains("fwrite(src, \"_edge_src_0.csv\")"),
        "source should write edge CSV"
    );
    assert!(
        script.source.contains("src <- fread(\"_edge_src_0.csv\")"),
        "sink should read edge CSV"
    );
}

#[test]
fn golden_sql_node_r() {
    let manifest = DagManifest {
        nodes: vec![
            NodeEntry {
                id: "src".into(),
                kind: "source_file".into(),
                spec: serde_json::json!({"path": "/tmp/data.csv"}),
            },
            NodeEntry {
                id: "filter".into(),
                kind: "sql".into(),
                spec: serde_json::json!({"sql_query": "SELECT * FROM port_0 WHERE pval < 5e-8"}),
            },
        ],
        edges: vec![EdgeEntry {
            from: "src".into(),
            from_port: 0,
            to: "filter".into(),
            to_port: 0,
        }],
    };
    let registry = test_registry();
    let compiler = DagCompiler {
        registry: &registry,
    };
    let script = compiler.compile(&manifest, CodegenTarget::R).unwrap();

    assert!(
        script.source.contains("port_0 <- src"),
        "should alias input as port_0"
    );
    assert!(script.source.contains("sqldf("), "should use sqldf");
    assert!(
        script.packages.contains(&"sqldf".to_string()),
        "should list sqldf package"
    );

    // Edge CSV I/O: source writes, sql reads.
    assert!(
        script.source.contains("fwrite(src, \"_edge_src_0.csv\")"),
        "source should write edge CSV"
    );
    assert!(
        script.source.contains("src <- fread(\"_edge_src_0.csv\")"),
        "sql node should read edge CSV"
    );
}

#[test]
fn golden_ldsc_hsq_r() {
    let manifest = DagManifest {
        nodes: vec![
            NodeEntry {
                id: "sumstats".into(),
                kind: "source_file".into(),
                spec: serde_json::json!({"path": "/tmp/gwas.csv"}),
            },
            NodeEntry {
                id: "h2".into(),
                kind: "ldsc".into(),
                spec: serde_json::json!({"n_blocks": 200}),
            },
        ],
        edges: vec![EdgeEntry {
            from: "sumstats".into(),
            from_port: 0,
            to: "h2".into(),
            to_port: 0,
        }],
    };
    let registry = test_registry();
    let compiler = DagCompiler {
        registry: &registry,
    };
    let script = compiler.compile(&manifest, CodegenTarget::R).unwrap();

    // Should prepare LDSC input from upstream columns.
    assert!(
        script.source.contains("rsid = sumstats$rsid"),
        "should reference upstream rsid"
    );
    assert!(script.source.contains("ldsc.py"), "should call ldsc.py");
    assert!(
        script.source.contains("--n-blocks"),
        "should pass n_blocks parameter"
    );

    // Edge CSV I/O.
    assert!(
        script.source.contains("fwrite(sumstats, \"_edge_sumstats_0.csv\")"),
        "sumstats should write edge CSV"
    );
    assert!(
        script.source.contains("sumstats <- fread(\"_edge_sumstats_0.csv\")"),
        "ldsc should read edge CSV"
    );
}

#[test]
fn golden_two_sample_mr_r() {
    let manifest = DagManifest {
        nodes: vec![
            NodeEntry {
                id: "merged".into(),
                kind: "source_file".into(),
                spec: serde_json::json!({"path": "/tmp/merged.csv"}),
            },
            NodeEntry {
                id: "mr".into(),
                kind: "two_sample_mr".into(),
                spec: serde_json::json!({
                    "id_exposure": "ieu-a-2",
                    "id_outcome": "ieu-a-7",
                    "method_list": ["mr_ivw"]
                }),
            },
        ],
        edges: vec![EdgeEntry {
            from: "merged".into(),
            from_port: 0,
            to: "mr".into(),
            to_port: 0,
        }],
    };
    let registry = test_registry();
    let compiler = DagCompiler {
        registry: &registry,
    };
    let script = compiler.compile(&manifest, CodegenTarget::R).unwrap();

    assert!(
        script.packages.contains(&"TwoSampleMR".to_string()),
        "should list TwoSampleMR"
    );
    assert!(
        script.source.contains("library(TwoSampleMR)"),
        "should load TwoSampleMR"
    );
    assert!(
        script.source.contains("clump_data("),
        "should perform LD clumping"
    );
    assert!(
        script.source.contains("harmonise_data("),
        "should harmonise"
    );
    assert!(script.source.contains("mr("), "should call mr()");
    assert!(
        script.source.contains("id.exposure = \"ieu-a-2\""),
        "should pass exposure id"
    );
    assert!(
        script.source.contains("\"mr_ivw\""),
        "should request IVW method"
    );
}

#[test]
fn golden_mixed_supported_unsupported() {
    // A DAG where some nodes have codegen and some don't.
    let manifest = DagManifest {
        nodes: vec![
            NodeEntry {
                id: "src".into(),
                kind: "source_file".into(),
                spec: serde_json::json!({"path": "/tmp/data.csv"}),
            },
            NodeEntry {
                id: "echo".into(),
                kind: "echo".into(),
                spec: serde_json::json!({}),
            },
        ],
        edges: vec![EdgeEntry {
            from: "src".into(),
            from_port: 0,
            to: "echo".into(),
            to_port: 0,
        }],
    };
    let registry = test_registry();
    let compiler = DagCompiler {
        registry: &registry,
    };
    let script = compiler.compile(&manifest, CodegenTarget::R).unwrap();

    // source_file should be compiled, echo should be skipped.
    assert!(
        script.source.contains("fread("),
        "source_file should have R code"
    );
    assert_eq!(
        script.skipped_nodes,
        vec!["echo".to_string()],
        "echo should be skipped"
    );
}

// =====================================================================
// Survey-package nodes
// =====================================================================

#[test]
fn golden_svymean_r() {
    let manifest = DagManifest {
        nodes: vec![
            NodeEntry {
                id: "data".into(),
                kind: "source_file".into(),
                spec: serde_json::json!({"path": "/tmp/nhanes.csv"}),
            },
            NodeEntry {
                id: "mean".into(),
                kind: "svymean".into(),
                spec: serde_json::json!({
                    "design": {"ids": ["psu"], "strata": ["stratum"], "weights": "wt", "nest": true},
                    "variables": ["bp", "bmi"],
                    "na_rm": true
                }),
            },
        ],
        edges: vec![EdgeEntry {
            from: "data".into(),
            from_port: 0,
            to: "mean".into(),
            to_port: 0,
        }],
    };
    let registry = test_registry();
    let compiler = DagCompiler {
        registry: &registry,
    };
    let script = compiler.compile(&manifest, CodegenTarget::R).unwrap();

    assert!(
        script.source.contains("library(survey)"),
        "should include library(survey), got:\n{}",
        script.source
    );
    assert!(
        script.source.contains("svydesign(ids = ~psu"),
        "should build svydesign with ids"
    );
    assert!(
        script.source.contains("strata = ~stratum"),
        "should include strata"
    );
    assert!(
        script.source.contains("weights = ~wt"),
        "should include weights"
    );
    assert!(
        script.source.contains("nest = TRUE"),
        "should include nest = TRUE"
    );
    assert!(
        script.source.contains("svymean(~bp + bmi,"),
        "should call svymean with the right formula"
    );
    assert!(
        script.source.contains("na.rm = TRUE"),
        "should pass na.rm = TRUE"
    );
    assert!(script.skipped_nodes.is_empty());
}

#[test]
fn golden_svyglm_r() {
    let manifest = DagManifest {
        nodes: vec![
            NodeEntry {
                id: "data".into(),
                kind: "source_file".into(),
                spec: serde_json::json!({"path": "/tmp/api.csv"}),
            },
            NodeEntry {
                id: "model".into(),
                kind: "svyglm".into(),
                spec: serde_json::json!({
                    "design": {"ids": ["dnum"], "weights": "pw"},
                    "response": "api00",
                    "predictors": ["meals", "ell"],
                    "family": "gaussian",
                    "std_errors": "Bell-McCaffrey"
                }),
            },
        ],
        edges: vec![EdgeEntry {
            from: "data".into(),
            from_port: 0,
            to: "model".into(),
            to_port: 0,
        }],
    };
    let registry = test_registry();
    let compiler = DagCompiler {
        registry: &registry,
    };
    let script = compiler.compile(&manifest, CodegenTarget::R).unwrap();

    assert!(
        script.source.contains("svyglm(api00 ~ meals + ell,"),
        "should build svyglm with the right formula, got:\n{}",
        script.source
    );
    assert!(
        script.source.contains("family = gaussian()"),
        "should pass family = gaussian()"
    );
    assert!(
        script.source.contains("std.errors = \"Bell-McCaffrey\""),
        "should pass Bell-McCaffrey SE option"
    );
    assert!(
        script.source.contains("print(summary("),
        "should print summary"
    );
}

#[test]
fn golden_calibrate_chain_r() {
    // A calibration chain: source → calibrate → svymean
    let manifest = DagManifest {
        nodes: vec![
            NodeEntry {
                id: "data".into(),
                kind: "source_file".into(),
                spec: serde_json::json!({"path": "/tmp/survey.csv"}),
            },
            NodeEntry {
                id: "cal".into(),
                kind: "calibrate".into(),
                spec: serde_json::json!({
                    "design": {"ids": ["psu"], "weights": "wt"},
                    "variables": ["age_group", "sex"],
                    "population_totals": [100.0, 200.0, 150.0, 250.0]
                }),
            },
            NodeEntry {
                id: "mean".into(),
                kind: "svymean".into(),
                spec: serde_json::json!({
                    "design": {"ids": ["psu"], "weights": "calibrated_weight"},
                    "variables": ["income"]
                }),
            },
        ],
        edges: vec![
            EdgeEntry {
                from: "data".into(),
                from_port: 0,
                to: "cal".into(),
                to_port: 0,
            },
            EdgeEntry {
                from: "cal".into(),
                from_port: 0,
                to: "mean".into(),
                to_port: 0,
            },
        ],
    };
    let registry = test_registry();
    let compiler = DagCompiler {
        registry: &registry,
    };
    let script = compiler.compile(&manifest, CodegenTarget::R).unwrap();

    assert!(
        script.source.contains("calibrate("),
        "should call calibrate(), got:\n{}",
        script.source
    );
    assert!(
        script
            .source
            .contains("weights("),
        "should extract calibrated weights"
    );
    assert!(
        script.source.contains("calibrated_weight"),
        "should name the weight column"
    );
    // The downstream svymean should use the calibrated weight column.
    assert!(
        script.source.contains("weights = ~calibrated_weight"),
        "downstream svymean should reference calibrated_weight"
    );
}
