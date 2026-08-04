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

// ═══════════════════════════════════════════════════════════════════════
// MR-PRESSO — codegen_r cross-validation
// ═══════════════════════════════════════════════════════════════════════

/// Load the SummaryStats fixture (E1_effect / E2_effect / Y_effect) into the
/// form `mrpresso::MrpressoInput` needs, matching how the DAG node extracts it.
fn mrpresso_fixture_input(seed: u32) -> mrpresso::MrpressoInput {
    let csv = format!(
        "{}/../../bio_crates/mrpresso/tests/summary_stats_headers.csv",
        env!("CARGO_MANIFEST_DIR")
    );
    let mut e1 = Vec::new();
    let mut e1_se = Vec::new();
    let mut y = Vec::new();
    let mut y_se = Vec::new();
    let text = std::fs::read_to_string(&csv).unwrap();
    for (line_idx, line) in text.lines().enumerate() {
        if line_idx == 0 {
            continue; // header
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let f: Vec<f64> = line.split(',').map(|s| s.parse::<f64>().unwrap()).collect();
        e1.push(f[0]);
        e1_se.push(f[1]);
        y.push(f[6]);
        y_se.push(f[7]);
    }
    mrpresso::MrpressoInput {
        beta_outcome: y,
        beta_exposure: vec![e1],
        sd_outcome: y_se,
        sd_exposure: vec![e1_se],
        outlier_test: true,
        distortion_test: true,
        signif_threshold: 0.05,
        nb_distribution: 1000,
        seed,
        row_labels: None,
    }
}

/// Cross-validate the `mrpresso` node's codegen_r: compile a DAG that loads
/// the SummaryStats fixture and runs MR-PRESSO, execute the generated R, and
/// compare the global-test numbers to the Rust node's computation.
#[test]
#[ignore = "requires R + MRPRESSO package; run with DIFFTESTS=1"]
fn mrpresso_codegen_xval() {
    let data_csv = format!("{XVAL_DIR}/mrpresso_data.csv");
    std::fs::create_dir_all(XVAL_DIR).unwrap();
    std::fs::copy(
        format!(
            "{}/../../bio_crates/mrpresso/tests/summary_stats_headers.csv",
            env!("CARGO_MANIFEST_DIR")
        ),
        &data_csv,
    )
    .unwrap();

    let manifest = DagManifest {
        nodes: vec![
            NodeEntry {
                id: "src".into(),
                kind: "source_file".into(),
                spec: serde_json::json!({"path": data_csv}),
            },
            NodeEntry {
                id: "mrp".into(),
                kind: "mrpresso".into(),
                spec: serde_json::json!({
                    "beta_outcome": "Y_effect",
                    "sd_outcome": "Y_se",
                    "beta_exposure": ["E1_effect"],
                    "sd_exposure": ["E1_se"],
                    "outlier_test": true,
                    "distortion_test": true,
                    "signif_threshold": 0.05,
                    "nb_distribution": 1000,
                    "seed": 123
                }),
            },
        ],
        edges: vec![EdgeEntry {
            from: "src".into(),
            from_port: 0,
            to: "mrp".into(),
            to_port: 0,
        }],
    };

    // ── Rust reference (the node's underlying computation) ──
    let rust = mrpresso::mr_presso(&mrpresso_fixture_input(123)).unwrap();
    let rust_rss = rust.global.rss_obs;
    let rust_p = rust.global.pvalue;

    // ── Compile to R and extract the global test ──
    let script = compile_and_write(manifest, "mrpresso");
    assert!(script.source.contains("mr_presso("));
    assert!(script.source.contains("library(MRPRESSO)"));

    // Keep edge-materialising fwrite(src, ...) calls; drop only the final
    // fwrite(mrp, ...) which would try to serialise the list result, plus the
    // default print().
    let r_script = script
        .source
        .lines()
        .filter(|l| !l.contains("fwrite(mrp") && !l.contains("print("))
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let r_script = format!(
        "{r_script}\n\
         gt <- mrp[['MR-PRESSO results']][['Global Test']]\n\
         cat('RSS', format(gt$RSSobs, digits=17), '\\n')\n\
         cat('PVAL', as.character(gt$Pvalue), '\\n')\n"
    );
    let r_path = format!("{XVAL_DIR}/mrpresso_codegen_run.R");
    std::fs::write(&r_path, &r_script).unwrap();

    let output = std::process::Command::new("Rscript").arg(&r_path).output();
    match output {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let mut r_rss = None;
            let mut r_p = None;
            for line in stdout.lines() {
                if let Some(v) = line.strip_prefix("RSS ") {
                    r_rss = v.trim().parse::<f64>().ok();
                }
                if let Some(v) = line.strip_prefix("PVAL ") {
                    // R formats a raw p-value of 0 as "<1/nb".
                    let s = v.trim();
                    r_p = if s.starts_with('<') {
                        Some(0.0)
                    } else {
                        s.parse::<f64>().ok()
                    };
                }
            }
            let (r_rss, r_p) = (r_rss.unwrap(), r_p.unwrap());
            assert!(
                (r_rss - rust_rss).abs() < 1e-6,
                "mrpresso codegen global RSSobs: Rust={rust_rss} R={r_rss}"
            );
            assert!(
                (r_p - rust_p).abs() < 1e-12,
                "mrpresso codegen global Pvalue: Rust={rust_p} R={r_p}"
            );
            eprintln!(
                "✓ mrpresso codegen: RSSobs Rust={rust_rss:.6} R={r_rss:.6}; Pvalue Rust={rust_p} R={r_p}"
            );
        }
        Ok(out) => {
            panic!(
                "mrpresso codegen: Rscript failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        Err(_) => {
            eprintln!("Rscript not found; skipping mrpresso codegen xval");
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Fine-Gray (`fine_gray`) and cumulative incidence (`cuminc`) cross-validation.
//
// Layer 2 (codegen): compile the DAG to R, run the generated script with
// `Rscript`, and diff its edge CSVs against the reference CSVs produced by
// `tests/cross_validate.R` calling the real `cmprsk` package.
//
// Layer 3 (closure): execute the Rust *node* through the DAG engine and diff
// both of its output ports against the same reference — proving the Rust node,
// the generated R, and the reference R all agree.
// ═══════════════════════════════════════════════════════════════════════════

/// Generate the synthetic data + reference CSVs for the cmprsk codegen tests.
fn cmprsk_xval_data(test_name: &str) {
    // The generator lives at the repo root (CARGO_MANIFEST_DIR = crates/data-engine).
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/cross_validate.R");
    let _ = std::process::Command::new("Rscript")
        .arg(script)
        .arg(test_name)
        .arg(XVAL_DIR)
        .status();
}

/// Read a CSV into a Vec of (name, Vec<f64>) numeric columns, preserving
/// nothing but numeric data (logical columns become 0/1).
fn read_numeric_csv(path: &str) -> Vec<(String, Vec<f64>)> {
    let mut rdr = csv::Reader::from_path(path).expect("open csv");
    let headers = rdr.headers().unwrap().clone();
    let names: Vec<String> = headers.iter().map(|s| s.to_string()).collect();
    let mut cols: Vec<Vec<f64>> = vec![Vec::new(); names.len()];
    for rec in rdr.records() {
        let rec = rec.unwrap();
        for (i, field) in rec.iter().enumerate() {
            let parsed: Result<f64, _> = field.parse();
            cols[i].push(parsed.unwrap_or(f64::NAN));
        }
    }
    names.into_iter().zip(cols).collect()
}

/// Compare all shared numeric columns of two CSVs, aligning by the first
/// string column (`term`/`group`+`cause`) when present, else by row order.
/// Returns the max absolute difference across all compared cells.
fn max_csv_diff(gen_path: &str, ref_path: &str) -> f64 {
    use std::collections::HashMap;

    let gen_cols = read_numeric_csv(gen_path);
    let ref_cols = read_numeric_csv(ref_path);
    let gen_by: HashMap<String, Vec<f64>> = gen_cols.into_iter().collect();
    let rf_by: HashMap<String, Vec<f64>> = ref_cols.into_iter().collect();

    let mut worst = 0.0_f64;
    for (name, gen_vals) in &gen_by {
        let Some(rf_vals) = rf_by.get(name) else { continue };
        if gen_vals.len() != rf_vals.len() {
            // fall back to comparing min length
        }
        let n = gen_vals.len().min(rf_vals.len());
        for i in 0..n {
            let (a, b) = (gen_vals[i], rf_vals[i]);
            if a.is_nan() && b.is_nan() {
                continue;
            }
            let d = (a - b).abs() / b.abs().max(1.0);
            if d > worst {
                worst = d;
            }
        }
    }
    worst
}

/// Layer 2 + 3 for the `fine_gray` / `fine_gray_tf` node.
#[test]
#[ignore = "requires R + cmprsk; run with DIFFTESTS=1"]
fn fine_gray_node_vs_r() {
    cmprsk_xval_data("fine_gray");
    let data_csv = format!("{XVAL_DIR}/fine_gray_data.csv");
    if !std::path::Path::new(&data_csv).exists() {
        eprintln!("reference data missing; run: Rscript tests/cross_validate.R fine_gray {XVAL_DIR}");
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
                id: "fg".into(),
                kind: "fine_gray".into(),
                spec: serde_json::json!({
                    "time_column": "time",
                    "status_column": "fstatus",
                    "covariates": ["x1", "x2", "x3"],
                    "cengroup_column": "cengroup",
                    "failcode": 1, "cencode": 0, "gtol": 1e-6, "maxiter": 10,
                    "variance": true
                }),
            },
        ],
        edges: vec![EdgeEntry {
            from: "src".into(),
            from_port: 0,
            to: "fg".into(),
            to_port: 0,
        }],
    };

    // ── Layer 2: compile to R, run via Rscript, diff the port-0 edge CSV ──
    let script = compile_and_write(manifest.clone(), "fine_gray");
    assert!(script.source.contains("cmprsk::crr("), "must call crr()");
    assert!(script.source.contains("failcode = 1"), "failcode");
    assert!(script.source.contains("cencode = 0"), "cencode");
    assert!(script.source.contains("cengroup = "), "cengroup");
    assert!(script.source.contains("library(cmprsk)"));

    run_generated_script("fine_gray");

    let diff0 = max_csv_diff(
        &format!("{XVAL_DIR}/_edge_fg_0.csv"),
        &format!("{XVAL_DIR}/fine_gray_reference.csv"),
    );
    eprintln!("fine_gray codegen port-0 max rel diff: {diff0:.3e}");
    assert!(diff0 < 1e-6, "generated R port 0 diverged: {diff0:.3e}");

    let diff1 = max_csv_diff(
        &format!("{XVAL_DIR}/_edge_fg_1.csv"),
        &format!("{XVAL_DIR}/fine_gray_reference_1.csv"),
    );
    eprintln!("fine_gray codegen port-1 max rel diff: {diff1:.3e}");
    assert!(diff1 < 1e-9, "generated R port 1 diverged: {diff1:.3e}");

    // ── Layer 3: execute the Rust node, compare both ports to reference ──
    let (node_diff0, node_diff1) = run_fine_gray_node(&data_csv, &manifest, "fine_gray");
    eprintln!("fine_gray node port-0 max rel diff: {node_diff0:.3e}");
    eprintln!("fine_gray node port-1 max rel diff: {node_diff1:.3e}");
    assert!(node_diff0 < 1e-6, "Rust node port 0 diverged: {node_diff0:.3e}");
    assert!(node_diff1 < 1e-9, "Rust node port 1 diverged: {node_diff1:.3e}");
}

/// Layer 2 + 3 for the `fine_gray` node with a time-interacted covariate.
#[test]
#[ignore = "requires R + cmprsk; run with DIFFTESTS=1"]
fn fine_gray_tf_node_vs_r() {
    cmprsk_xval_data("fine_gray_tf");
    let data_csv = format!("{XVAL_DIR}/fine_gray_tf_data.csv");
    if !std::path::Path::new(&data_csv).exists() {
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
                id: "fg_tf".into(),
                kind: "fine_gray".into(),
                spec: serde_json::json!({
                    "time_column": "time",
                    "status_column": "fstatus",
                    "covariates": ["x1", "x2"],
                    "tv_covariates": ["x1"],
                    "time_functions": ["square"],
                    "failcode": 1, "cencode": 0, "variance": true
                }),
            },
        ],
        edges: vec![EdgeEntry {
            from: "src".into(),
            from_port: 0,
            to: "fg_tf".into(),
            to_port: 0,
        }],
    };

    let script = compile_and_write(manifest.clone(), "fine_gray_tf");
    assert!(script.source.contains("tf = function(uft) cbind(uft^2)"), "tf closure");
    assert!(script.source.contains("cov2 = "), "cov2");
    run_generated_script("fine_gray_tf");

    let diff0 = max_csv_diff(
        &format!("{XVAL_DIR}/_edge_fg_tf_0.csv"),
        &format!("{XVAL_DIR}/fine_gray_tf_reference.csv"),
    );
    eprintln!("fine_gray_tf codegen port-0 max rel diff: {diff0:.3e}");
    assert!(diff0 < 1e-6, "generated R (tf) port 0 diverged: {diff0:.3e}");

    let (node_diff0, _) = run_fine_gray_node(&data_csv, &manifest, "fine_gray_tf");
    eprintln!("fine_gray_tf node port-0 max rel diff: {node_diff0:.3e}");
    assert!(node_diff0 < 1e-6, "Rust node (tf) port 0 diverged: {node_diff0:.3e}");
}

/// Layer 2 + 3 for the `cuminc` node.
#[test]
#[ignore = "requires R + cmprsk; run with DIFFTESTS=1"]
fn cuminc_node_vs_r() {
    cmprsk_xval_data("cuminc");
    let data_csv = format!("{XVAL_DIR}/cuminc_data.csv");
    if !std::path::Path::new(&data_csv).exists() {
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
                id: "ci".into(),
                kind: "cuminc".into(),
                spec: serde_json::json!({
                    "time_column": "time",
                    "status_column": "fstatus",
                    "group_column": "group",
                    "cencode": 0
                }),
            },
        ],
        edges: vec![EdgeEntry {
            from: "src".into(),
            from_port: 0,
            to: "ci".into(),
            to_port: 0,
        }],
    };

    let script = compile_and_write(manifest.clone(), "cuminc");
    assert!(script.source.contains("cmprsk::cuminc("), "must call cuminc()");
    assert!(script.source.contains("group = "), "group");
    run_generated_script("cuminc");

    // port 0 = curves
    let diff0 = max_csv_diff(
        &format!("{XVAL_DIR}/_edge_ci_0.csv"),
        &format!("{XVAL_DIR}/cuminc_reference_0.csv"),
    );
    eprintln!("cuminc codegen port-0 (curves) max rel diff: {diff0:.3e}");
    assert!(diff0 < 1e-6, "generated R cuminc curves diverged: {diff0:.3e}");

    // port 1 = Gray's tests
    let diff1 = max_csv_diff(
        &format!("{XVAL_DIR}/_edge_ci_1.csv"),
        &format!("{XVAL_DIR}/cuminc_reference_1.csv"),
    );
    eprintln!("cuminc codegen port-1 (tests) max rel diff: {diff1:.3e}");
    assert!(diff1 < 1e-6, "generated R cuminc tests diverged: {diff1:.3e}");

    // ── Layer 3: run the Rust node ──
    let (node_diff0, node_diff1) = run_cuminc_node(&data_csv, &manifest);
    eprintln!("cuminc node port-0 max rel diff: {node_diff0:.3e}");
    eprintln!("cuminc node port-1 max rel diff: {node_diff1:.3e}");
    assert!(node_diff0 < 1e-6, "Rust node cuminc curves diverged: {node_diff0:.3e}");
    assert!(node_diff1 < 1e-6, "Rust node cuminc tests diverged: {node_diff1:.3e}");
}

/// Run a generated R script in the xval dir and wait for it to finish.
fn run_generated_script(test_name: &str) {
    let r_path = format!("{XVAL_DIR}/{test_name}_generated.R");
    let out = std::process::Command::new("Rscript")
        .current_dir(XVAL_DIR)
        .arg(&r_path)
        .output()
        .expect("Rscript runs");
    if !out.status.success() {
        eprintln!(
            "Rscript failed:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        panic!("generated R script failed to run");
    }
}

/// Execute a `fine_gray` node through the DAG engine and diff both ports.
fn run_fine_gray_node(data_csv: &str, manifest: &DagManifest, stem: &str) -> (f64, f64) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let ctx = SessionContext::new();
        let df = ctx
            .read_csv(data_csv, datafusion::prelude::CsvReadOptions::new())
            .await
            .expect("read data csv");

        let node_ctx = crate::node_registry::registry::NodeCtx {
            runtime_env: ctx.runtime_env(),
            iceberg_catalog: None,
            datalake: Arc::new(Datalake::default()),
            opendal: None,
        };

        // Build the node through the factory so spec validation runs.
        let registry = test_registry();
        let factory = registry.get_factory("fine_gray").expect("factory");
        let spec = manifest
            .nodes
            .iter()
            .find(|n| n.kind == "fine_gray")
            .map(|n| n.spec.clone())
            .expect("fine_gray spec");
        let mut node = factory
            .build(spec, node_ctx.clone())
            .expect("build fine_gray node");

        let input = crate::nodes::meta::NodeInput { port: 0, data: df };
        let outputs = node
            .execute(&node_ctx, &[input], &crate::dag::node_event::NodeReporter::noop())
            .await
            .expect("fine_gray execute");

        // Write both ports to CSV for comparison (stem-isolated filenames so
        // parallel `#[ignore]` tests don't clobber each other).
        let port0 = outputs.get(&0).cloned().unwrap();
        let port1 = outputs.get(&1).cloned().unwrap();
        write_df_csv(port0, &format!("{XVAL_DIR}/_node_{stem}_0.csv")).await;
        write_df_csv(port1, &format!("{XVAL_DIR}/_node_{stem}_1.csv")).await;
    });

    let d0 = max_csv_diff(
        &format!("{XVAL_DIR}/_node_{stem}_0.csv"),
        &format!("{XVAL_DIR}/{stem}_reference.csv"),
    );
    let d1 = max_csv_diff(
        &format!("{XVAL_DIR}/_node_{stem}_1.csv"),
        &format!("{XVAL_DIR}/{stem}_reference_1.csv"),
    );
    (d0, d1)
}

/// Execute a `cuminc` node through the DAG engine and diff both ports.
fn run_cuminc_node(data_csv: &str, _manifest: &DagManifest) -> (f64, f64) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let ctx = SessionContext::new();
        let df = ctx
            .read_csv(data_csv, datafusion::prelude::CsvReadOptions::new())
            .await
            .expect("read data csv");

        let node_ctx = crate::node_registry::registry::NodeCtx {
            runtime_env: ctx.runtime_env(),
            iceberg_catalog: None,
            datalake: Arc::new(Datalake::default()),
            opendal: None,
        };

        let registry = test_registry();
        let factory = registry.get_factory("cuminc").expect("factory");
        let spec = serde_json::json!({
            "time_column": "time",
            "status_column": "fstatus",
            "group_column": "group",
            "cencode": 0
        });
        let mut node = factory
            .build(spec, node_ctx.clone())
            .expect("build cuminc node");

        let input = crate::nodes::meta::NodeInput { port: 0, data: df };
        let outputs = node
            .execute(&node_ctx, &[input], &crate::dag::node_event::NodeReporter::noop())
            .await
            .expect("cuminc execute");

        let port0 = outputs.get(&0).cloned().unwrap();
        let port1 = outputs.get(&1).cloned().unwrap();
        write_df_csv(port0, &format!("{XVAL_DIR}/_node_ci_0.csv")).await;
        write_df_csv(port1, &format!("{XVAL_DIR}/_node_ci_1.csv")).await;
    });

    let d0 = max_csv_diff(
        &format!("{XVAL_DIR}/_node_ci_0.csv"),
        &format!("{XVAL_DIR}/cuminc_reference_0.csv"),
    );
    let d1 = max_csv_diff(
        &format!("{XVAL_DIR}/_node_ci_1.csv"),
        &format!("{XVAL_DIR}/cuminc_reference_1.csv"),
    );
    (d0, d1)
}

/// Materialise a DataFrame to a CSV file (drop-in for comparing node output
/// against the reference CSVs).
async fn write_df_csv(df: datafusion::dataframe::DataFrame, path: &str) {
    use datafusion::common::config::CsvOptions;
    use datafusion::dataframe::DataFrameWriteOptions;
    df.write_csv(
        path,
        DataFrameWriteOptions::new().with_single_file_output(true),
        None::<CsvOptions>,
    )
    .await
    .expect("write csv");
}
