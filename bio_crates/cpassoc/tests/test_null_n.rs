//! Test null/NaN sample-size (n) behavior in the cpassoc library.
//!
//! The library's `weight_vector` has no NaN guard — a null n propagates
//! silently. This file documents the current behavior.

use cpassoc::stats::{self, weight_vector, ShetOptions};
use faer::Mat;

#[test]
fn weight_vector_with_nan_n() {
    // One trait has NaN sample size (null n from Arrow → push_numeric → NAN).
    let ss = vec![1000.0, f64::NAN, 800.0];
    let w = weight_vector(&ss);
    // sum_sq = 1000² + NaN² + 800² = NaN → sum_w = NaN → all weights NaN
    println!("weights with NaN n: {:?}", w);
    assert!(w.iter().all(|x| x.is_nan()), "NaN n should poison all weights");
}

#[test]
fn weight_vector_with_zero_n() {
    // One trait has n=0 → weight 0 (excluded from analysis).
    let ss = vec![1000.0, 0.0, 800.0];
    let w = weight_vector(&ss);
    println!("weights with zero n: {:?}", w);
    assert!((w[0] - 1000.0 / (1000.0_f64.powi(2) + 800.0_f64.powi(2)).sqrt()).abs() < 1e-10);
    assert!(w[1].abs() < 1e-15, "zero-n trait should have ~0 weight");
}

#[test]
fn weight_vector_all_zero_n() {
    let ss = vec![0.0, 0.0];
    let w = weight_vector(&ss);
    // 0/0 = NaN
    println!("weights with all-zero n: {:?}", w);
    assert!(w.iter().all(|x| x.is_nan()), "all-zero n should give NaN weights");
}

#[test]
fn shom_with_nan_weight() {
    // If n is NaN → weight is NaN → SHom result is NaN (silent corruption).
    let corr = Mat::identity(2, 2);
    let ss = vec![1000.0, f64::NAN];
    let w = weight_vector(&ss);
    let x = vec![2.0, 1.5];
    let stat = stats::shom_single(&x, &w, corr.as_ref());
    println!("SHom with NaN weight: {}", stat);
    assert!(stat.is_nan(), "NaN n should silently corrupt SHom to NaN");
}
