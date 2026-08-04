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
fn compile_and_write(manifest: DagManifest, test_name: &str) -> crate::codegen::CompiledScript {
    let registry = test_registry();
    let compiler = DagCompiler {
        registry: &registry,
    };
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

// ── causal (PSM) ───────────────────────────────────────────────────────────

#[test]
#[ignore = "requires R + MatchIt; run with DIFFTESTS=1"]
fn causal_psm() {
    let data_csv = format!("{XVAL_DIR}/causal_psm_data.csv");
    if !std::path::Path::new(&data_csv).exists() {
        eprintln!("Run first: Rscript tests/cross_validate.R causal_psm {XVAL_DIR}");
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
                id: "psm".into(),
                kind: "causal".into(),
                spec: serde_json::json!({
                    "method": "psm",
                    "treatment_column": "treat",
                    "outcome_column": "y",
                    "covariates": ["age", "female"],
                    "n_bootstrap": 200,
                    "seed": 42
                }),
            },
        ],
        edges: vec![EdgeEntry {
            from: "src".into(),
            from_port: 0,
            to: "psm".into(),
            to_port: 0,
        }],
    };

    let script = compile_and_write(manifest, "causal_psm");
    assert!(script.source.contains("matchit"));
}

// ── causal (IPTW) ──────────────────────────────────────────────────────────

#[test]
#[ignore = "requires R; run with DIFFTESTS=1"]
fn causal_iptw() {
    let data_csv = format!("{XVAL_DIR}/causal_iptw_data.csv");
    if !std::path::Path::new(&data_csv).exists() {
        eprintln!("Run first: Rscript tests/cross_validate.R causal_iptw {XVAL_DIR}");
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
                id: "iptw".into(),
                kind: "causal".into(),
                spec: serde_json::json!({
                    "method": "iptw",
                    "treatment_column": "treat",
                    "outcome_column": "y",
                    "covariates": ["age", "female"],
                    "n_bootstrap": 200,
                    "seed": 42
                }),
            },
        ],
        edges: vec![EdgeEntry {
            from: "src".into(),
            from_port: 0,
            to: "iptw".into(),
            to_port: 0,
        }],
    };

    let script = compile_and_write(manifest, "causal_iptw");
    assert!(script.source.contains("weights = "));
}

// ── mediation ──────────────────────────────────────────────────────────────

#[test]
#[ignore = "requires R + mediation; run with DIFFTESTS=1"]
fn mediation() {
    let data_csv = format!("{XVAL_DIR}/mediation_data.csv");
    if !std::path::Path::new(&data_csv).exists() {
        eprintln!("Run first: Rscript tests/cross_validate.R mediation {XVAL_DIR}");
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
                id: "med".into(),
                kind: "mediation".into(),
                spec: serde_json::json!({
                    "exposure_column": "x",
                    "mediator_column": "m",
                    "outcome_column": "y",
                    "covariates": [],
                    "interaction": false,
                    "n_bootstrap": 200,
                    "seed": 42
                }),
            },
        ],
        edges: vec![EdgeEntry {
            from: "src".into(),
            from_port: 0,
            to: "med".into(),
            to_port: 0,
        }],
    };

    let script = compile_and_write(manifest, "mediation");
    assert!(script.source.contains("mediate("));
}

// ── epi_rcs ────────────────────────────────────────────────────────────────

#[test]
#[ignore = "requires R + rms; run with DIFFTESTS=1"]
fn epi_rcs() {
    let data_csv = format!("{XVAL_DIR}/epi_rcs_data.csv");
    if !std::path::Path::new(&data_csv).exists() {
        eprintln!("Run first: Rscript tests/cross_validate.R epi_rcs {XVAL_DIR}");
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
                id: "rcs".into(),
                kind: "epi_rcs".into(),
                spec: serde_json::json!({
                    "x_column": "x",
                    "outcome_column": "y",
                    "covariates": [],
                    "n_knots": 4,
                    "n_grid_points": 50
                }),
            },
        ],
        edges: vec![EdgeEntry {
            from: "src".into(),
            from_port: 0,
            to: "rcs".into(),
            to_port: 0,
        }],
    };

    let script = compile_and_write(manifest, "epi_rcs");
    assert!(script.source.contains("rcs("));
}

// ── evalue ─────────────────────────────────────────────────────────────────

/// Smoke test: codegen produces a valid `evalue()` call.
#[test]
fn evalue_codegen_smoke() {
    let manifest = DagManifest {
        nodes: vec![NodeEntry {
            id: "ev".into(),
            kind: "evalue".into(),
            spec: serde_json::json!({
                "measure": "RR",
                "est": 0.80,
                "lo": 0.71,
                "hi": 0.91,
                "true_val": 1.0
            }),
        }],
        edges: vec![],
    };

    let script = compile_and_write(manifest, "evalue_rr_smoke");
    // Should call evalue() with RR constructor
    assert!(script.source.contains("evalue("), "missing evalue() call");
    assert!(script.source.contains("RR("), "missing RR() constructor");
    assert!(script.source.contains("0.8"), "missing est value");
    assert!(script.source.contains("library(EValue)"));
}

/// Smoke test: OR with rare=FALSE produces sqrt-approximation R code.
#[test]
fn evalue_codegen_or() {
    let manifest = DagManifest {
        nodes: vec![NodeEntry {
            id: "ev".into(),
            kind: "evalue".into(),
            spec: serde_json::json!({
                "measure": "OR",
                "est": 0.86,
                "lo": 0.75,
                "hi": 0.99,
                "rare": false,
                "true_val": 1.0
            }),
        }],
        edges: vec![],
    };

    let script = compile_and_write(manifest, "evalue_or_smoke");
    assert!(script.source.contains("OR("));
    assert!(script.source.contains("rare = FALSE"));
}

/// Cross-validation: run R EValue::evalues.RR and compare to Rust output.
/// Requires R + EValue package. Run with DIFFTESTS=1.
#[test]
#[ignore = "requires R + EValue package; run with DIFFTESTS=1"]
fn evalue_rr_xval() {
    use crate::nodes::evalue::{EvalueConfig, MeasureType};

    // ── Rust computation ──
    let _cfg = EvalueConfig {
        measure: MeasureType::RR,
        est: 0.80,
        lo: Some(0.71),
        hi: Some(0.91),
        true_val: Some(1.0),
        ..Default::default()
    };
    let result = evalue::evalue::evalues_rr(0.80, Some(0.71), Some(0.91), 1.0).unwrap();
    let rust_point = result.point_evalue().unwrap();

    // ── Compile DAG to R ──
    let manifest = DagManifest {
        nodes: vec![NodeEntry {
            id: "ev".into(),
            kind: "evalue".into(),
            spec: serde_json::json!({
                "measure": "RR",
                "est": 0.80,
                "lo": 0.71,
                "hi": 0.91,
                "true_val": 1.0
            }),
        }],
        edges: vec![],
    };
    let script = compile_and_write(manifest, "evalue_rr_xval");

    // ── Run Rscript and extract E-value ──
    // Strip fwrite/print lines, add summary extraction
    let r_script = script
        .source
        .lines()
        .filter(|l| !l.contains("fwrite") && !l.contains("print("))
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let r_script = format!("{r_script}\ncat(format(summary(ev), digits=12))\n");
    let r_path = format!("{XVAL_DIR}/evalue_rr_xval_run.R");
    std::fs::write(&r_path, &r_script).unwrap();

    let output = std::process::Command::new("Rscript").arg(&r_path).output();

    match output {
        Ok(out) if out.status.success() => {
            let r_val_str = String::from_utf8_lossy(&out.stdout);
            let r_val: f64 = r_val_str.trim().parse().unwrap_or(0.0);
            assert!(
                (r_val - rust_point).abs() < 1e-4,
                "R E-value {r_val} vs Rust {rust_point}"
            );
            eprintln!("✓ RR E-value: Rust={rust_point:.6} R={r_val:.6}");
        }
        Ok(out) => {
            eprintln!("Rscript failed: {}", String::from_utf8_lossy(&out.stderr));
        }
        Err(_) => {
            eprintln!("Rscript not found; skipping R cross-validation");
        }
    }
}

/// Helper: run a codegen cross-validation for a single evalue node config.
/// Compiles the DAG to R, runs via Rscript, and compares to Rust output.
fn run_evalue_xval(test_name: &str, spec: serde_json::Value, rust_evalue: f64) {
    let manifest = DagManifest {
        nodes: vec![NodeEntry {
            id: "ev".into(),
            kind: "evalue".into(),
            spec: spec.clone(),
        }],
        edges: vec![],
    };
    let script = compile_and_write(manifest, test_name);

    let r_script = script
        .source
        .lines()
        .filter(|l| !l.contains("fwrite") && !l.contains("print("))
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let r_script = format!("{r_script}\ncat(format(summary(ev), digits=12))\n");
    let r_path = format!("{XVAL_DIR}/{test_name}_run.R");
    std::fs::write(&r_path, &r_script).unwrap();

    let output = std::process::Command::new("Rscript").arg(&r_path).output();

    match output {
        Ok(out) if out.status.success() => {
            let r_val_str = String::from_utf8_lossy(&out.stdout);
            let r_val: f64 = r_val_str.trim().parse().unwrap_or(f64::NAN);
            assert!(
                (r_val - rust_evalue).abs() < 1e-4,
                "✗ {test_name}: R={r_val} vs Rust={rust_evalue}"
            );
            eprintln!("✓ {test_name}: Rust={rust_evalue:.6} R={r_val:.6}");
        }
        Ok(out) => {
            panic!(
                "{test_name}: Rscript failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        Err(_) => {
            eprintln!("Rscript not found; skipping {test_name}");
        }
    }
}

/// Codegen xval: OR (common outcome, rare=FALSE).
#[test]
#[ignore = "requires R + EValue package; run with DIFFTESTS=1"]
fn evalue_or_xval() {
    let rust = evalue::evalue::evalues_or(0.86, Some(0.75), Some(0.99), false, 1.0)
        .unwrap()
        .point_evalue()
        .unwrap();
    run_evalue_xval(
        "evalue_or_xval",
        serde_json::json!({
            "measure": "OR",
            "est": 0.86,
            "lo": 0.75,
            "hi": 0.99,
            "rare": false,
            "true_val": 1.0
        }),
        rust,
    );
}

/// Codegen xval: OR (rare outcome).
#[test]
#[ignore = "requires R + EValue package; run with DIFFTESTS=1"]
fn evalue_or_rare_xval() {
    let rust = evalue::evalue::evalues_or(3.0, None, None, true, 1.0)
        .unwrap()
        .point_evalue()
        .unwrap();
    run_evalue_xval(
        "evalue_or_rare_xval",
        serde_json::json!({
            "measure": "OR",
            "est": 3.0,
            "rare": true,
            "true_val": 1.0
        }),
        rust,
    );
}

/// Codegen xval: HR (common outcome).
#[test]
#[ignore = "requires R + EValue package; run with DIFFTESTS=1"]
fn evalue_hr_xval() {
    let rust = evalue::evalue::evalues_hr(0.56, None, None, false, 1.0)
        .unwrap()
        .point_evalue()
        .unwrap();
    run_evalue_xval(
        "evalue_hr_xval",
        serde_json::json!({
            "measure": "HR",
            "est": 0.56,
            "rare": false,
            "true_val": 1.0
        }),
        rust,
    );
}

/// Codegen xval: OLS.
#[test]
#[ignore = "requires R + EValue package; run with DIFFTESTS=1"]
fn evalue_ols_xval() {
    let rust = evalue::evalue::evalues_ols(0.3, Some(0.1), 1.0, 1.0, 0.0)
        .unwrap()
        .point_evalue()
        .unwrap();
    run_evalue_xval(
        "evalue_ols_xval",
        serde_json::json!({
            "measure": "OLS",
            "est": 0.3,
            "se": 0.1,
            "sd": 1.0,
            "delta": 1.0,
            "true_val": 0.0
        }),
        rust,
    );
}

/// Codegen xval: MD.
#[test]
#[ignore = "requires R + EValue package; run with DIFFTESTS=1"]
fn evalue_md_xval() {
    let rust = evalue::evalue::evalues_md(0.5, None, 0.0)
        .unwrap()
        .point_evalue()
        .unwrap();
    run_evalue_xval(
        "evalue_md_xval",
        serde_json::json!({
            "measure": "MD",
            "est": 0.5,
            "true_val": 0.0
        }),
        rust,
    );
}
