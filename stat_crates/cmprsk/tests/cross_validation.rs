//! Cross-validation of the Rust port against R `cmprsk` 2.2-12.
//!
//! Each test loads a JSON fixture from `fixtures/golden/`, replays the exact
//! inputs R was given, and compares every component of the fitted object.
//!
//! Regenerate the fixtures with (r45 conda env: R 4.5.3 + cmprsk + survival):
//!
//! ```sh
//! export R_HOME=$HOME/miniconda3/envs/r45/lib/R
//! export PATH=$HOME/miniconda3/envs/r45/bin:$PATH
//! export LD_LIBRARY_PATH=$HOME/miniconda3/envs/r45/lib:$LD_LIBRARY_PATH
//! Rscript fixtures/gen_cmprsk_golden.R
//! ```
//!
//! The fixtures carry both inputs and outputs, so the suite is self-contained
//! and does not depend on R's RNG being stable across versions.

use std::path::PathBuf;

use cmprsk::{
    CrrInput, CrrOptions, CumincOptions, TimeFn, TimeFunctions, baseline_cif, crr, cuminc,
    cuminc::CumincInput, predict_crr, summary_crr,
};
use serde_json::Value;

// ── tolerances ──────────────────────────────────────────────────────────────
//
// The kernels are transliterations, so agreement is limited only by
// floating-point association order and by LU (faer vs LAPACK) in the two places
// the driver solves a linear system. In practice every structural quantity —
// coefficients, log-likelihoods, scores, the information and sandwich matrices,
// score residuals, baseline hazard jumps, cumulative incidence curves and
// Gray's statistic — agrees to better than 1e-13 relative.
//
// The one exception is p-values, which are not an algorithmic result at all:
// they are `1 - Φ(|z|)` / `1 - F_χ²(q)` evaluated by `statrs` here and by
// R's `pnorm` / `pchisq` there. Those two implementations of the *same*
// mathematical function differ by ~5e-11 relative, so p-values get their own,
// looser bound. Tightening it would test statrs, not this port.

/// Structural quantities: coefficients, likelihoods, scores, curves, jumps.
const TOL_TIGHT: f64 = 1e-13;
/// Quantities that pass through an LU factorisation or a matrix triple product.
const TOL_MATRIX: f64 = 1e-12;
/// Tail probabilities — limited by the normal/χ² CDF implementation.
const TOL_PVALUE: f64 = 1e-9;

// ── fixture loading ─────────────────────────────────────────────────────────

fn fixture_path(name: &str) -> PathBuf {
    // CARGO_MANIFEST_DIR = <repo>/stat_crates/cmprsk
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/golden")
        .join(format!("cmprsk_{name}.json"))
}

fn load(name: &str) -> Value {
    let path = fixture_path(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "missing fixture {}: {e}\nrun: Rscript fixtures/gen_cmprsk_golden.R",
            path.display()
        )
    });
    serde_json::from_str(&text).expect("fixture is valid JSON")
}

/// jsonlite writes length-1 vectors as one-element arrays; accept either shape.
fn num(v: &Value) -> f64 {
    match v {
        Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
        Value::Array(a) if a.len() == 1 => num(&a[0]),
        Value::Null => f64::NAN,
        _ => f64::NAN,
    }
}

fn flag(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Array(a) if a.len() == 1 => flag(&a[0]),
        _ => false,
    }
}

fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(a) if a.len() == 1 => text(&a[0]),
        _ => String::new(),
    }
}

fn vec_f64(v: &Value) -> Vec<f64> {
    match v {
        Value::Array(a) => a.iter().map(num).collect(),
        Value::Null => Vec::new(),
        other => vec![num(other)],
    }
}

fn vec_str(v: &Value) -> Vec<String> {
    match v {
        Value::Array(a) => a.iter().map(text).collect(),
        Value::Null => Vec::new(),
        other => vec![text(other)],
    }
}

fn mat_f64(v: &Value) -> Vec<Vec<f64>> {
    match v {
        Value::Array(a) => a.iter().map(vec_f64).collect(),
        _ => Vec::new(),
    }
}

fn is_null(v: &Value) -> bool {
    matches!(v, Value::Null)
}

// ── comparison ──────────────────────────────────────────────────────────────

/// Relative-or-absolute closeness: `|got - want| <= tol * max(1, |want|)`.
fn close(got: f64, want: f64, tol: f64) -> bool {
    if got.is_nan() && want.is_nan() {
        return true;
    }
    (got - want).abs() <= tol * want.abs().max(1.0)
}

#[track_caller]
fn check(scenario: &str, label: &str, got: f64, want: f64, tol: f64) {
    assert!(
        close(got, want, tol),
        "{scenario}/{label}: rust={got:.17e} R={want:.17e} \
         (abs diff {:.3e}, tol {tol:.1e})",
        (got - want).abs()
    );
}

#[track_caller]
fn check_vec(scenario: &str, label: &str, got: &[f64], want: &[f64], tol: f64) {
    assert_eq!(
        got.len(),
        want.len(),
        "{scenario}/{label}: length rust={} R={}",
        got.len(),
        want.len()
    );
    let mut worst = 0.0_f64;
    let mut worst_i = 0usize;
    for (i, (&g, &w)) in got.iter().zip(want.iter()).enumerate() {
        let d = (g - w).abs() / w.abs().max(1.0);
        if d > worst {
            worst = d;
            worst_i = i;
        }
    }
    assert!(
        worst <= tol,
        "{scenario}/{label}[{worst_i}]: rust={:.17e} R={:.17e} \
         (max rel diff {worst:.3e}, tol {tol:.1e})",
        got[worst_i],
        want[worst_i]
    );
}

#[track_caller]
fn check_mat(scenario: &str, label: &str, got: &[Vec<f64>], want: &[Vec<f64>], tol: f64) {
    assert_eq!(
        got.len(),
        want.len(),
        "{scenario}/{label}: rows rust={} R={}",
        got.len(),
        want.len()
    );
    for (i, (g, w)) in got.iter().zip(want.iter()).enumerate() {
        check_vec(scenario, &format!("{label}[{i}]"), g, w, tol);
    }
}

// ── driver ──────────────────────────────────────────────────────────────────

fn parse_tf(names: &[String]) -> Vec<TimeFn> {
    names
        .iter()
        .map(|s| match s.as_str() {
            "identity" => TimeFn::Identity,
            "log" => TimeFn::Log,
            "sqrt" => TimeFn::Sqrt,
            "square" => TimeFn::Square,
            "log1p" => TimeFn::Log1p,
            other => panic!("unknown time function in fixture: {other}"),
        })
        .collect()
}

fn run_crr_fixture(name: &str) {
    let g = load(name);
    let inp = &g["input"];
    let fit_ref = &g["fit"];

    let ftime = vec_f64(&inp["ftime"]);
    let fstatus = vec_f64(&inp["fstatus"]);
    let cov1 = mat_f64(&inp["cov1"]);
    let cov2 = mat_f64(&inp["cov2"]);
    let cov1_names = vec_str(&inp["cov1_names"]);
    let cov2_names = vec_str(&inp["cov2_names"]);
    let tf_names = vec_str(&inp["tf"]);
    let tf_fns = parse_tf(&tf_names);
    let cengroup = if is_null(&inp["cengroup"]) {
        None
    } else {
        Some(vec_f64(&inp["cengroup"]))
    };
    let init = if is_null(&inp["init"]) {
        None
    } else {
        Some(vec_f64(&inp["init"]))
    };

    let opts = CrrOptions {
        failcode: num(&inp["failcode"]),
        cencode: num(&inp["cencode"]),
        gtol: num(&inp["gtol"]),
        maxiter: num(&inp["maxiter"]) as usize,
        init,
        variance: flag(&inp["variance"]),
    };

    let tf = if tf_fns.is_empty() {
        TimeFunctions::None
    } else {
        TimeFunctions::Fns(&tf_fns)
    };
    let input = CrrInput {
        ftime: &ftime,
        fstatus: &fstatus,
        cov1: &cov1,
        cov1_names: &cov1_names,
        cov2: &cov2,
        cov2_names: &cov2_names,
        tf,
        cengroup: cengroup.as_deref(),
    };

    let fit = crr(&input, &opts).unwrap_or_else(|e| panic!("{name}: crr failed: {e}"));

    // ── point estimates and likelihood ──────────────────────────────────────
    check_vec(name, "coef", &fit.coef, &vec_f64(&fit_ref["coef"]), TOL_TIGHT);
    check(name, "loglik", fit.loglik, num(&fit_ref["loglik"]), TOL_TIGHT);
    check(
        name,
        "loglik_null",
        fit.loglik_null,
        num(&fit_ref["loglik_null"]),
        TOL_TIGHT,
    );
    check_vec(
        name,
        "score",
        &fit.score,
        &vec_f64(&fit_ref["score"]),
        TOL_TIGHT,
    );
    check_vec(
        name,
        "uftime",
        &fit.uftime,
        &vec_f64(&fit_ref["uftime"]),
        TOL_TIGHT,
    );
    check_vec(
        name,
        "bfitj",
        &fit.bfitj,
        &vec_f64(&fit_ref["bfitj"]),
        TOL_TIGHT,
    );
    assert_eq!(
        fit.converged,
        flag(&fit_ref["converged"]),
        "{name}: converged flag"
    );
    assert_eq!(fit.n, num(&fit_ref["n"]) as usize, "{name}: n");
    assert_eq!(
        fit.n_missing,
        num(&fit_ref["n_missing"]) as usize,
        "{name}: n_missing"
    );

    // Term labels only match R verbatim when there are no cov2 terms — R names
    // those from `colnames(tf(uft))`, which has no analogue in the TimeFn
    // vocabulary.
    if cov2.is_empty() {
        assert_eq!(
            fit.terms,
            vec_str(&fit_ref["terms"]),
            "{name}: term labels"
        );
    }

    // ── tfs matrix ──────────────────────────────────────────────────────────
    if !is_null(&fit_ref["tfs"]) {
        check_mat(name, "tfs", &fit.tfs, &mat_f64(&fit_ref["tfs"]), TOL_TIGHT);
    }

    // ── variance components ─────────────────────────────────────────────────
    if opts.variance {
        check_mat(name, "inf", &fit.inf, &mat_f64(&fit_ref["inf"]), TOL_MATRIX);
        check_mat(name, "var", &fit.var, &mat_f64(&fit_ref["var"]), TOL_MATRIX);
        check_mat(
            name,
            "invinf",
            &fit.invinf,
            &mat_f64(&fit_ref["invinf"]),
            TOL_MATRIX,
        );
        let res = fit.res.as_ref().expect("res present when variance = true");
        check_mat(name, "res", res, &mat_f64(&fit_ref["res"]), TOL_MATRIX);
    } else {
        assert!(fit.res.is_none(), "{name}: res must be None");
        assert!(
            fit.var.iter().flatten().all(|v| v.is_nan()),
            "{name}: var must be all-NaN when variance = false"
        );
    }

    // ── summary() ───────────────────────────────────────────────────────────
    if !is_null(&g["summary"]) {
        let s = &g["summary"];
        let smry = summary_crr(&fit, 0.95);
        let got = |f: fn(&cmprsk::CoefRow) -> f64| -> Vec<f64> {
            smry.coefficients.iter().map(f).collect()
        };
        check_vec(name, "se", &got(|c| c.se), &vec_f64(&s["se"]), TOL_MATRIX);
        check_vec(name, "z", &got(|c| c.z), &vec_f64(&s["z"]), TOL_MATRIX);
        check_vec(
            name,
            "p_value",
            &got(|c| c.p_value),
            &vec_f64(&s["p_value"]),
            TOL_PVALUE,
        );
        check_vec(
            name,
            "exp_coef",
            &got(|c| c.exp_coef),
            &vec_f64(&s["exp_coef"]),
            TOL_TIGHT,
        );
        check_vec(
            name,
            "exp_neg_coef",
            &got(|c| c.exp_neg_coef),
            &vec_f64(&s["exp_neg_coef"]),
            TOL_TIGHT,
        );
        check_vec(
            name,
            "ci_lower",
            &got(|c| c.ci_lower),
            &vec_f64(&s["ci_lower"]),
            TOL_MATRIX,
        );
        check_vec(
            name,
            "ci_upper",
            &got(|c| c.ci_upper),
            &vec_f64(&s["ci_upper"]),
            TOL_MATRIX,
        );
        // logtest = -2 · (loglik_null − loglik) subtracts two large, nearly
        // equal log-likelihoods, so its absolute error inherits |loglik|'s
        // scale rather than its own. Bound it by the inputs' magnitude instead
        // of by the (much smaller) result.
        let want_logtest = num(&s["logtest"]);
        let logtest_tol = 2.0 * TOL_TIGHT * fit.loglik.abs().max(1.0);
        assert!(
            (smry.logtest - want_logtest).abs() <= logtest_tol,
            "{name}/logtest: rust={:.17e} R={want_logtest:.17e} \
             (abs diff {:.3e}, tol {logtest_tol:.3e})",
            smry.logtest,
            (smry.logtest - want_logtest).abs()
        );
        assert_eq!(smry.df, num(&s["df"]) as usize, "{name}: logtest df");

        // summary()'s own conf.int matrix: columns are
        // exp(coef), exp(-coef), lower, upper
        let ci = mat_f64(&s["conf_int"]);
        for (i, row) in ci.iter().enumerate() {
            let c = &smry.coefficients[i];
            check(name, "conf.int[,1]", c.exp_coef, row[0], TOL_TIGHT);
            check(name, "conf.int[,2]", c.exp_neg_coef, row[1], TOL_TIGHT);
            check(name, "conf.int[,3]", c.ci_lower, row[2], TOL_MATRIX);
            check(name, "conf.int[,4]", c.ci_upper, row[3], TOL_MATRIX);
        }
    }

    // ── predict() ───────────────────────────────────────────────────────────
    if !is_null(&g["predict"]) {
        let p = &g["predict"];
        let pc1 = mat_f64(&p["cov1"]);
        let pc2 = mat_f64(&p["cov2"]);
        let pred = predict_crr(&fit, &pc1, &pc2).expect("predict");
        let want = mat_f64(&p["curves"]);
        assert_eq!(pred.curves.len(), want.len(), "{name}: predict curve count");
        for (j, w) in want.iter().enumerate() {
            check_vec(name, &format!("predict[{j}]"), &pred.curves[j], w, TOL_TIGHT);
        }
    }

    // ── baseline CIF: predict at all-zero covariates ────────────────────────
    let base = baseline_cif(&fit);
    assert_eq!(base.len(), fit.uftime.len(), "{name}: baseline CIF length");
}

fn run_cuminc_fixture(name: &str) {
    let g = load(name);
    let inp = &g["input"];

    let ftime = vec_f64(&inp["ftime"]);
    let fstatus = vec_f64(&inp["fstatus"]);
    let group = if is_null(&inp["group"]) {
        None
    } else {
        Some(vec_f64(&inp["group"]))
    };
    let strata = if is_null(&inp["strata"]) {
        None
    } else {
        Some(vec_f64(&inp["strata"]))
    };
    let opts = CumincOptions {
        rho: num(&inp["rho"]),
        cencode: num(&inp["cencode"]),
    };

    let res = cuminc(
        &CumincInput {
            ftime: &ftime,
            fstatus: &fstatus,
            group: group.as_deref(),
            group_labels: None,
            strata: strata.as_deref(),
        },
        &opts,
    )
    .unwrap_or_else(|e| panic!("{name}: cuminc failed: {e}"));

    // ── curves, in R's list order (cause-major, then group) ─────────────────
    let want_curves = g["curves"].as_array().expect("curves array");
    assert_eq!(
        res.curves.len(),
        want_curves.len(),
        "{name}: curve count rust={} R={}",
        res.curves.len(),
        want_curves.len()
    );
    for (i, w) in want_curves.iter().enumerate() {
        let c = &res.curves[i];
        let label = format!("{} {}", c.group, c.cause);
        assert_eq!(label, text(&w["name"]), "{name}: curve {i} name");
        check_vec(name, &format!("{label}/time"), &c.time, &vec_f64(&w["time"]), TOL_TIGHT);
        check_vec(name, &format!("{label}/est"), &c.est, &vec_f64(&w["est"]), TOL_TIGHT);
        check_vec(name, &format!("{label}/var"), &c.var, &vec_f64(&w["var"]), TOL_MATRIX);
    }

    // ── Gray's k-sample test ────────────────────────────────────────────────
    if is_null(&g["tests"]) {
        assert!(res.tests.is_empty(), "{name}: unexpected tests");
    } else {
        let want_tests = g["tests"].as_array().expect("tests array");
        assert_eq!(res.tests.len(), want_tests.len(), "{name}: test count");
        for (i, w) in want_tests.iter().enumerate() {
            let t = &res.tests[i];
            assert_eq!(t.cause, text(&w["cause"]), "{name}: test {i} cause");
            check(name, "gray stat", t.stat, num(&w["stat"]), TOL_MATRIX);
            check(name, "gray p", t.p_value, num(&w["p_value"]), TOL_PVALUE);
            assert_eq!(t.df, num(&w["df"]) as usize, "{name}: test {i} df");
        }
    }
}

// ── crr scenarios ───────────────────────────────────────────────────────────

#[test]
fn crr_basic() {
    run_crr_fixture("crr_basic");
}

#[test]
fn crr_failcode2() {
    run_crr_fixture("crr_failcode2");
}

#[test]
fn crr_cencode2() {
    run_crr_fixture("crr_cencode2");
}

#[test]
fn crr_cengroup() {
    run_crr_fixture("crr_cengroup");
}

#[test]
fn crr_cov2_tf_quadratic() {
    run_crr_fixture("crr_cov2_tf_quadratic");
}

#[test]
fn crr_cov2_only() {
    run_crr_fixture("crr_cov2_only");
}

#[test]
fn crr_cov1_cov2_cengroup() {
    run_crr_fixture("crr_cov1_cov2_cengroup");
}

#[test]
fn crr_ties_heavy() {
    run_crr_fixture("crr_ties_heavy");
}

#[test]
fn crr_events_at_zero() {
    run_crr_fixture("crr_events_at_zero");
}

#[test]
fn crr_missing() {
    run_crr_fixture("crr_missing");
}

#[test]
fn crr_maxiter0_init() {
    run_crr_fixture("crr_maxiter0_init");
}

#[test]
fn crr_variance_false() {
    run_crr_fixture("crr_variance_false");
}

#[test]
fn crr_single_cov() {
    run_crr_fixture("crr_single_cov");
}

// ── cuminc scenarios ────────────────────────────────────────────────────────

#[test]
fn cuminc_basic() {
    run_cuminc_fixture("cuminc_basic");
}

#[test]
fn cuminc_group_only() {
    run_cuminc_fixture("cuminc_group_only");
}

#[test]
fn cuminc_group_strata_rho() {
    run_cuminc_fixture("cuminc_group_strata_rho");
}

#[test]
fn cuminc_ties() {
    run_cuminc_fixture("cuminc_ties");
}
