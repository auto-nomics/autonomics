//! Boolean algebra over hypothesis tests.
//!
//! - [`intersection_test`] — AND: p-value is `max(pᵢ)`. Rejects only when
//!   every component rejects. The intersection-union test (IUT, Berger
//!   1982); no multiplicity correction needed.
//! - [`union_test`] — OR: p-value is `min(pᵢ)`. Rejects when any component
//!   rejects (De Morgan's dual of intersection).
//! - [`complement_test`] — NOT: p-value is `1 − p`. Double complement
//!   recovers the original p.
//!
//! Together these form a complete Boolean algebra in which De Morgan's laws
//! hold by construction. Port of `hypothesize::intersection_test`,
//! `union_test`, `complement_test`.

use serde_json::json;

use super::PvalSource;
use crate::{
    Alternative, HypoError, HypothesisTest, KEY_COMPONENT_PVALS, KEY_KIND, KEY_N_TESTS,
    KEY_ORIGINAL_PVAL, Result, extras,
};

/// Validate p-values lie in `[0, 1]` (Boolean combinators accept 0).
fn validate_loose(pvals: &[f64]) -> Result<()> {
    for &p in pvals {
        if !(0.0..=1.0).contains(&p) {
            return Err(HypoError::PvalOutOfRange(p));
        }
    }
    Ok(())
}

fn extract_loose<S: PvalSource>(srcs: &[S]) -> Result<Vec<f64>> {
    let pvals: Vec<f64> = srcs.iter().map(|s| s.pval()).collect();
    validate_loose(&pvals)?;
    Ok(pvals)
}

/// AND: reject only when every component test rejects.
pub fn intersection_test<S: PvalSource>(srcs: &[S]) -> Result<HypothesisTest> {
    let pvals = extract_loose(srcs)?;
    let k = pvals.len();
    if k == 0 {
        return Err(HypoError::InvalidInput("no p-values supplied".into()));
    }
    let p_value = pvals.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    Ok(HypothesisTest::new(
        f64::NAN,
        p_value,
        f64::NAN,
        Alternative::TwoSided,
        "Intersection-Union Test",
        extras([
            (KEY_KIND, json!("intersection_test")),
            (KEY_N_TESTS, json!(k as u64)),
            (KEY_COMPONENT_PVALS, json!(pvals)),
        ]),
    ))
}

/// OR: reject when any component rejects. Anti-conservative for multiple
/// comparisons; pair with [`crate::adjust_pvals`] if FWER control is needed.
pub fn union_test<S: PvalSource>(srcs: &[S]) -> Result<HypothesisTest> {
    let pvals = extract_loose(srcs)?;
    let k = pvals.len();
    if k == 0 {
        return Err(HypoError::InvalidInput("no p-values supplied".into()));
    }
    let p_value = pvals.iter().copied().fold(f64::INFINITY, f64::min);
    Ok(HypothesisTest::new(
        f64::NAN,
        p_value,
        f64::NAN,
        Alternative::TwoSided,
        "Union-Intersection Test",
        extras([
            (KEY_KIND, json!("union_test")),
            (KEY_N_TESTS, json!(k as u64)),
            (KEY_COMPONENT_PVALS, json!(pvals)),
        ]),
    ))
}

/// NOT: p-value is `1 − p`. The complement test rejects when the original
/// fails to reject. Connects to equivalence testing via TOST.
pub fn complement_test(t: &HypothesisTest) -> HypothesisTest {
    let original_pval = t.p_value;
    HypothesisTest::new(
        t.stat,
        1.0 - original_pval,
        t.dof,
        t.alternative,
        "Complemented Test",
        {
            let mut e = t.extras.clone();
            e.insert(KEY_KIND.into(), json!("complemented_test"));
            e.insert(KEY_ORIGINAL_PVAL.into(), json!(original_pval));
            e
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{wald_uni, z_test};

    #[test]
    fn intersection_is_max_p() {
        let a = wald_uni(2.5, 0.8, 0.0).unwrap();
        let b = wald_uni(1.2, 0.5, 0.0).unwrap();
        let p_a = a.p_value;
        let p_b = b.p_value;
        let t = intersection_test(&[a, b]).unwrap();
        assert!((t.p_value - p_a.max(p_b)).abs() < 1e-12);
    }

    #[test]
    fn union_is_min_p() {
        let t = union_test(&[0.01_f64, 0.80]).unwrap();
        assert!((t.p_value - 0.01).abs() < 1e-12);
    }

    #[test]
    fn complement_roundtrip() {
        let z = z_test(&[1.0; 5], 0.0, 1.0, Alternative::TwoSided).unwrap();
        let c = complement_test(&z);
        assert!((c.p_value - (1.0 - z.p_value)).abs() < 1e-12);
        let cc = complement_test(&c);
        assert!((cc.p_value - z.p_value).abs() < 1e-12);
    }

    #[test]
    fn de_morgan_law() {
        let a = wald_uni(2.0, 1.0, 0.0).unwrap();
        let b = wald_uni(1.5, 0.8, 0.0).unwrap();
        let or_ab = union_test(&[a.clone(), b.clone()]).unwrap();
        // NOT(AND(NOT(a), NOT(b)))
        let na = complement_test(&a);
        let nb = complement_test(&b);
        let and_neg = intersection_test(&[na, nb]).unwrap();
        let de_morgan = complement_test(&and_neg);
        assert!((or_ab.p_value - de_morgan.p_value).abs() < 1e-9);
    }

    #[test]
    fn boolean_accepts_zero_p() {
        let t = union_test(&[0.0_f64, 0.5]).unwrap();
        assert_eq!(t.p_value, 0.0);
    }
}
