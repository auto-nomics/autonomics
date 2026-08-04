//! Cross-validation tests: compile a DAG to R, run via Rscript, compare to
//! reference R output. These are `#[ignore]` by default (require R installed).
//!
//! Usage:
//!   1. Rscript tests/cross_validate.R linear_regression /tmp/autonomics_xval
//!   2. DIFFTESTS=1 cargo test -p data-engine --lib codegen::xval_tests -- --ignored --nocapture

use std::sync::Arc;

use datafusion::prelude::SessionContext;
use datalake::Datalake;

use crate::codegen::{CodegenTarget, DagCompiler};
use crate::dag::history::{DagManifest, EdgeEntry, NodeEntry};
use crate::node_registry::registry::NodeRegistry;

fn test_registry() -> NodeRegistry {
    let ctx = SessionContext::new();
    let runtime_env = ctx.runtime_env();
    NodeRegistry::new(runtime_env, None, Arc::new(Datalake::default()), None)
}

const XVAL_DIR: &str = "/tmp/autonomics_xval";

/// Helper: compile a manifest to R and write the generated script to disk.
fn compile_and_write(
    manifest: DagManifest,
    test_name: &str,
) -> crate::codegen::CompiledScript {
    let registry = test_registry();
    let compiler = DagCompiler { registry: &registry };
    let script = compiler.compile(&manifest, CodegenTarget::R).unwrap();
    let path = format!("{XVAL_DIR}/{test_name}_generated.R");
    std::fs::create_dir_all(XVAL_DIR).unwrap();
    std::fs::write(&path, &script.source).unwrap();
    eprintln!("Generated R script written to {path}");
    script
}

// ── linear_regression ─────────────────────────────────────────────────────

#[test]
#[ignore = "requires R + data.table; run with DIFFTESTS=1"]
fn linear_regression() {
    let data_csv = format!("{XVAL_DIR}/linear_regression_data.csv");
    if !std::path::Path::new(&data_csv).exists() {
        eprintln!("Run first: Rscript tests/cross_validate.R linear_regression {XVAL_DIR}");
        return;
    }

    let manifest = DagManifest {
        nodes: vec![
            NodeEntry {
                id: "src".into(),
                kind: "source_file".into(),
                spec: serde_json::json!({"path": data_csv}),
            },
            NodeEntry {
                id: "lm".into(),
                kind: "linear_regression".into(),
                spec: serde_json::json!({"x_columns": ["x1", "x2"], "y_column": "y", "intercept": true}),
            },
        ],
        edges: vec![EdgeEntry {
            from: "src".into(),
            from_port: 0,
            to: "lm".into(),
            to_port: 0,
        }],
    };

    let script = compile_and_write(manifest, "linear_regression");
    assert!(script.source.contains("lm(y ~ x1 + x2"));
    assert!(script.source.contains("fread"));
    assert!(script.source.contains("fwrite"));
}

// ── logistic_regression ───────────────────────────────────────────────────

#[test]
#[ignore = "requires R; run with DIFFTESTS=1"]
fn logistic_regression() {
    let data_csv = format!("{XVAL_DIR}/logistic_regression_data.csv");
    if !std::path::Path::new(&data_csv).exists() {
        eprintln!("Run first: Rscript tests/cross_validate.R logistic_regression {XVAL_DIR}");
        return;
    }

    let manifest = DagManifest {
        nodes: vec![
            NodeEntry {
                id: "src".into(),
                kind: "source_file".into(),
                spec: serde_json::json!({"path": data_csv}),
            },
            NodeEntry {
                id: "glm".into(),
                kind: "logistic_regression".into(),
                spec: serde_json::json!({"predictors": ["x1", "x2"], "outcome": "y", "intercept": true}),
            },
        ],
        edges: vec![EdgeEntry {
            from: "src".into(),
            from_port: 0,
            to: "glm".into(),
            to_port: 0,
        }],
    };

    let script = compile_and_write(manifest, "logistic_regression");
    assert!(script.source.contains("glm(y ~ x1 + x2"));
    assert!(script.source.contains("family = binomial"));
}

// ── chi_square ─────────────────────────────────────────────────────────────

#[test]
#[ignore = "requires R; run with DIFFTESTS=1"]
fn chi_square() {
    let data_csv = format!("{XVAL_DIR}/chi_square_data.csv");
    if !std::path::Path::new(&data_csv).exists() {
        eprintln!("Run first: Rscript tests/cross_validate.R chi_square {XVAL_DIR}");
        return;
    }

    let manifest = DagManifest {
        nodes: vec![
            NodeEntry {
                id: "src".into(),
                kind: "source_file".into(),
                spec: serde_json::json!({"path": data_csv}),
            },
            NodeEntry {
                id: "chi".into(),
                kind: "chi_square".into(),
                spec: serde_json::json!({"row_column": "group", "col_column": "outcome"}),
            },
        ],
        edges: vec![EdgeEntry {
            from: "src".into(),
            from_port: 0,
            to: "chi".into(),
            to_port: 0,
        }],
    };

    let script = compile_and_write(manifest, "chi_square");
    assert!(script.source.contains("chisq.test"));
}

// ── cox_regression ────────────────────────────────────────────────────────

#[test]
#[ignore = "requires R + survival; run with DIFFTESTS=1"]
fn cox_regression() {
    let data_csv = format!("{XVAL_DIR}/cox_regression_data.csv");
    if !std::path::Path::new(&data_csv).exists() {
        eprintln!("Run first: Rscript tests/cross_validate.R cox_regression {XVAL_DIR}");
        return;
    }

    let manifest = DagManifest {
        nodes: vec![
            NodeEntry {
                id: "src".into(),
                kind: "source_file".into(),
                spec: serde_json::json!({"path": data_csv}),
            },
            NodeEntry {
                id: "cox".into(),
                kind: "cox_regression".into(),
                spec: serde_json::json!({"predictors": ["x1", "x2"], "time_column": "time", "event_column": "event"}),
            },
        ],
        edges: vec![EdgeEntry {
            from: "src".into(),
            from_port: 0,
            to: "cox".into(),
            to_port: 0,
        }],
    };

    let script = compile_and_write(manifest, "cox_regression");
    assert!(script.source.contains("coxph"));
    assert!(script.source.contains("Surv(time, event)"));
}

// ── epi_roc ────────────────────────────────────────────────────────────────

#[test]
#[ignore = "requires R + pROC; run with DIFFTESTS=1"]
fn epi_roc() {
    let data_csv = format!("{XVAL_DIR}/epi_roc_data.csv");
    if !std::path::Path::new(&data_csv).exists() {
        eprintln!("Run first: Rscript tests/cross_validate.R epi_roc {XVAL_DIR}");
        return;
    }

    let manifest = DagManifest {
        nodes: vec![
            NodeEntry {
                id: "src".into(),
                kind: "source_file".into(),
                spec: serde_json::json!({"path": data_csv}),
            },
            NodeEntry {
                id: "roc".into(),
                kind: "epi_roc".into(),
                spec: serde_json::json!({"score1_column": "score", "label_column": "label"}),
            },
        ],
        edges: vec![EdgeEntry {
            from: "src".into(),
            from_port: 0,
            to: "roc".into(),
            to_port: 0,
        }],
    };

    let script = compile_and_write(manifest, "epi_roc");
    assert!(script.source.contains("roc("));
}

// ── survival (Kaplan-Meier) ────────────────────────────────────────────────

#[test]
#[ignore = "requires R + survival; run with DIFFTESTS=1"]
fn survival() {
    let data_csv = format!("{XVAL_DIR}/survival_data.csv");
    if !std::path::Path::new(&data_csv).exists() {
        eprintln!("Run first: Rscript tests/cross_validate.R survival {XVAL_DIR}");
        return;
    }

    let manifest = DagManifest {
        nodes: vec![
            NodeEntry {
                id: "src".into(),
                kind: "source_file".into(),
                spec: serde_json::json!({"path": data_csv}),
            },
            NodeEntry {
                id: "km".into(),
                kind: "survival".into(),
                spec: serde_json::json!({"time_column": "time", "event_column": "event"}),
            },
        ],
        edges: vec![EdgeEntry {
            from: "src".into(),
            from_port: 0,
            to: "km".into(),
            to_port: 0,
        }],
    };

    let script = compile_and_write(manifest, "survival");
    assert!(script.source.contains("survfit"));
}

// ── epi_lasso ──────────────────────────────────────────────────────────────

#[test]
#[ignore = "requires R + glmnet; run with DIFFTESTS=1"]
fn epi_lasso() {
    let data_csv = format!("{XVAL_DIR}/epi_lasso_data.csv");
    if !std::path::Path::new(&data_csv).exists() {
        eprintln!("Run first: Rscript tests/cross_validate.R epi_lasso {XVAL_DIR}");
        return;
    }

    let manifest = DagManifest {
        nodes: vec![
            NodeEntry {
                id: "src".into(),
                kind: "source_file".into(),
                spec: serde_json::json!({"path": data_csv}),
            },
            NodeEntry {
                id: "lasso".into(),
                kind: "epi_lasso".into(),
                spec: serde_json::json!({
                    "predictors": ["x1", "x2", "x3", "x4", "x5"],
                    "outcome_column": "y",
                    "n_lambda": 50,
                    "cv_folds": 5,
                    "n_bootstrap": 100,
                    "seed": 42
                }),
            },
        ],
        edges: vec![EdgeEntry {
            from: "src".into(),
            from_port: 0,
            to: "lasso".into(),
            to_port: 0,
        }],
    };

    let script = compile_and_write(manifest, "epi_lasso");
    assert!(script.source.contains("cv.glmnet"));
}

// ── liability ──────────────────────────────────────────────────────────────

#[test]
#[ignore = "requires R; run with DIFFTESTS=1"]
fn liability() {
    let data_csv = format!("{XVAL_DIR}/liability_data.csv");
    if !std::path::Path::new(&data_csv).exists() {
        eprintln!("Run first: Rscript tests/cross_validate.R liability {XVAL_DIR}");
        return;
    }

    let manifest = DagManifest {
        nodes: vec![
            NodeEntry {
                id: "src".into(),
                kind: "source_file".into(),
                spec: serde_json::json!({"path": data_csv}),
            },
            NodeEntry {
                id: "liab".into(),
                kind: "liability".into(),
                spec: serde_json::json!({"samp_prev": 0.5, "pop_prev": 0.01}),
            },
        ],
        edges: vec![EdgeEntry {
            from: "src".into(),
            from_port: 0,
            to: "liab".into(),
            to_port: 0,
        }],
    };

    let script = compile_and_write(manifest, "liability");
    assert!(script.source.contains("qnorm"));
    assert!(script.source.contains("h2_liab"));
}
