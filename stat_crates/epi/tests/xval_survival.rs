//! Cross-validation: Kaplan-Meier + Log-rank vs R survival::survfit + survdiff.
//!
//! Run: `cargo test -p epi --test xval_survival`

use serde_json::Value;
use std::fs;

const TOL_S: f64 = 1e-6; // survival probabilities
const TOL_SE: f64 = 1e-4; // Greenwood SE
const TOL_LR: f64 = 1e-3; // log-rank χ² and p

struct SurvData {
    time: Vec<f64>,
    event: Vec<f64>,
    group: Vec<u64>,
}

fn load_data() -> SurvData {
    let csv = fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/xval/survival_data.csv"
    ))
    .expect("survival_data.csv not found");
    let mut time = Vec::new();
    let mut event = Vec::new();
    let mut group = Vec::new();
    for (i, line) in csv.lines().enumerate() {
        if i == 0 {
            continue;
        }
        let cols: Vec<&str> = line.split(',').collect();
        if cols.len() < 3 {
            continue;
        }
        time.push(cols[0].parse().unwrap());
        event.push(cols[1].parse().unwrap());
        group.push(cols[2].parse::<f64>().unwrap() as u64);
    }
    SurvData { time, event, group }
}

fn load_reference() -> Value {
    let json = fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/xval/survival_reference.json"
    ))
    .expect("survival_reference.json not found");
    serde_json::from_str(&json).expect("invalid JSON")
}

fn approx(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol * b.abs().max(1.0)
}

#[test]
fn xval_kaplan_meier_vs_r_survfit() {
    let data = load_data();
    let ref_val = load_reference();

    let km = epi::survival::kaplan_meier(&data.time, &data.event).expect("KM fit");

    let r_times = ref_val["km"]["times"].as_array().unwrap();
    let r_surv = ref_val["km"]["survival"].as_array().unwrap();
    let r_se = ref_val["km"]["std_error"].as_array().unwrap();
    let r_n_risk = ref_val["km"]["n_at_risk"].as_array().unwrap();
    let r_n_event = ref_val["km"]["n_events"].as_array().unwrap();

    // Same number of event times.
    assert_eq!(
        km.times.len(),
        r_times.len(),
        "event time count mismatch: ours={}, R={}",
        km.times.len(),
        r_times.len()
    );

    // Compare survival, SE, n_at_risk, n_events at each event time.
    let mut max_s_diff = 0.0_f64;
    let mut max_se_diff = 0.0_f64;
    for i in 0..km.times.len() {
        let rs = r_surv[i].as_f64().unwrap();
        let rse = r_se[i].as_f64();
        let rnr = r_n_risk[i].as_f64().unwrap();
        let rne = r_n_event[i].as_f64().unwrap();

        max_s_diff = max_s_diff.max((km.survival[i] - rs).abs());

        assert!(
            approx(km.survival[i], rs, TOL_S),
            "S(t[{i}]): got {:.8}, R {:.8}",
            km.survival[i],
            rs
        );

        // SE: skip when R reports NA (undefined at last event time where n=d).
        if let Some(rse_val) = rse {
            max_se_diff = max_se_diff.max((km.std_error[i] - rse_val).abs());
            assert!(
                approx(km.std_error[i], rse_val, TOL_SE),
                "SE(t[{i}]): got {:.8}, R {:.8}",
                km.std_error[i],
                rse_val
            );
        }

        assert_eq!(km.n_at_risk[i], rnr as usize, "n_at_risk[{i}] mismatch");
        assert_eq!(km.n_events[i], rne as usize, "n_events[{i}] mismatch");
    }

    println!(
        "✅ Kaplan-Meier: {} event times, max S diff={:.2e}, max SE diff={:.2e}",
        km.times.len(),
        max_s_diff,
        max_se_diff
    );
}

#[test]
fn xval_log_rank_vs_r_survdiff() {
    let data = load_data();
    let ref_val = load_reference();

    let lr =
        epi::survival::log_rank_test(&data.time, &data.event, &data.group).expect("log-rank test");

    let r_chi2 = ref_val["logrank"]["chi_squared"].as_f64().unwrap();
    let r_p = ref_val["logrank"]["p_value"].as_f64().unwrap();
    let r_df = ref_val["logrank"]["df"].as_u64().unwrap() as usize;

    assert_eq!(lr.df, r_df, "df mismatch");
    assert!(
        approx(lr.chi_squared, r_chi2, TOL_LR),
        "Log-rank χ²: got {:.6}, R {:.6}",
        lr.chi_squared,
        r_chi2
    );
    assert!(
        approx(lr.p_value, r_p, TOL_LR),
        "Log-rank p: got {:.6e}, R {:.6e}",
        lr.p_value,
        r_p
    );

    // Compare observed and expected per group.
    let r_obs = ref_val["logrank"]["observed"].as_array().unwrap();
    let r_exp = ref_val["logrank"]["expected"].as_array().unwrap();
    for g in 0..2 {
        assert!(
            approx(lr.observed[g], r_obs[g].as_f64().unwrap(), 0.5),
            "O[{}]: got {:.1}, R {:.1}",
            g,
            lr.observed[g],
            r_obs[g].as_f64().unwrap()
        );
        assert!(
            approx(lr.expected[g], r_exp[g].as_f64().unwrap(), TOL_LR),
            "E[{}]: got {:.4}, R {:.4}",
            g,
            lr.expected[g],
            r_exp[g].as_f64().unwrap()
        );
    }

    println!(
        "✅ Log-rank: χ²={:.4} (R {:.4}), p={:.6} (R {:.6})",
        lr.chi_squared, r_chi2, lr.p_value, r_p
    );
    println!("  O = {:?}, E = {:?}", lr.observed, lr.expected);
}
