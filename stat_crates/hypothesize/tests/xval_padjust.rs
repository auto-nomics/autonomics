//! Cross-validation of `hypothesize::p_adjust_raw` against R `stats::p.adjust`.
//!
//! Golden vectors were generated in R 4.5 and saved to
//! `fixtures/padjust_golden.json`. To regenerate:
//!
//! ```bash
//! Rscript -e '<gen script>' > stat_crates/hypothesize/fixtures/padjust_golden.json
//! ```

use hypothesize::{AdjustMethod, p_adjust_raw};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Case {
    p: Vec<f64>,
    method: String,
    expected: Vec<f64>,
}

fn load_golden() -> Vec<Case> {
    let json = include_str!("../fixtures/padjust_golden.json");
    serde_json::from_str(json).expect("failed to parse padjust_golden.json")
}

#[test]
fn matches_r_p_adjust() {
    let cases = load_golden();
    assert!(
        cases.len() >= 100,
        "expected ≥100 golden cases, got {}",
        cases.len()
    );

    let mut tested = 0;
    for case in &cases {
        let method = AdjustMethod::parse(&case.method)
            .unwrap_or_else(|e| panic!("unknown method '{}': {e}", case.method));
        let got = p_adjust_raw(&case.p, method, None).expect("adjust failed");
        assert_eq!(
            got.len(),
            case.expected.len(),
            "length mismatch for method '{}' with p = {:?}",
            case.method,
            case.p
        );
        for (i, (g, e)) in got.iter().zip(case.expected.iter()).enumerate() {
            assert!(
                (g - e).abs() < 1e-8,
                "method='{}' p[{:?}] idx[{}]: got {g:.10}, expected {e:.10}",
                case.method,
                case.p,
                i
            );
        }
        tested += 1;
    }
    assert!(tested >= 100);
}

#[test]
fn power_ordering_holds() {
    // For every golden case: Hommel ≤ Hochberg ≤ Holm ≤ Bonferroni and
    // BH ≤ BY component-wise.
    let cases = load_golden();
    for case in &cases {
        if case.p.len() <= 2 {
            continue; // Hommel=Hochberg fallback at n=2
        }
        let bonf = p_adjust_raw(&case.p, AdjustMethod::Bonferroni, None).unwrap();
        let holm = p_adjust_raw(&case.p, AdjustMethod::Holm, None).unwrap();
        let hoc = p_adjust_raw(&case.p, AdjustMethod::Hochberg, None).unwrap();
        let hom = p_adjust_raw(&case.p, AdjustMethod::Hommel, None).unwrap();
        let bh = p_adjust_raw(&case.p, AdjustMethod::Bh, None).unwrap();
        let by = p_adjust_raw(&case.p, AdjustMethod::By, None).unwrap();

        for i in 0..case.p.len() {
            assert!(holm[i] <= bonf[i] + 1e-9, "Holm > Bonferroni at {i}");
            assert!(hoc[i] <= holm[i] + 1e-9, "Hochberg > Holm at {i}");
            assert!(hom[i] <= hoc[i] + 1e-9, "Hommel > Hochberg at {i}");
            assert!(bh[i] <= by[i] + 1e-9, "BH > BY at {i}");
        }
    }
}
