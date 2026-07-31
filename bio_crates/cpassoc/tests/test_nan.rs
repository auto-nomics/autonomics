//! Test NaN/missing Z-score behavior in the cpassoc library.
//!
//! The R reference states: "the current version assumes no missing summary
//! statistics." The library functions should not panic on NaN input — they
//! should return NaN gracefully (the DAG node is responsible for filtering NaN
//! rows before calling the library).

use cpassoc::stats::{self, ShetOptions};
use cpassoc::input;
use faer::Mat;

#[test]
fn nan_in_shom_returns_nan_not_panic() {
    let corr = Mat::from_fn(3, 3, |i, j| if i == j { 1.0 } else { 0.3 });
    let ss = vec![1000.0, 1500.0, 800.0];

    // Row 0: all valid. Row 1: trait 2 is NaN.
    let x = Mat::from_fn(2, 3, |i, j| {
        if i == 1 && j == 1 {
            return f64::NAN;
        }
        match (i, j) {
            (0, _) => [2.0, 1.5, 0.5][j],
            (1, _) => [1.0, -2.0, 3.0][j],
            _ => unreachable!(),
        }
    });

    // SHom should return NaN for the NaN row, NOT panic.
    let shom = stats::shom(&x, &ss, &corr);
    assert!(shom[0].is_finite(), "valid row should give finite SHom");
    assert!(shom[1].is_nan(), "NaN row should give NaN SHom, got {}", shom[1]);
}

#[test]
fn nan_in_shet_does_not_panic() {
    let corr = Mat::from_fn(3, 3, |i, j| if i == j { 1.0 } else { 0.3 });
    let ss = vec![1000.0, 1500.0, 800.0];

    // Single row with one NaN.
    let x = Mat::from_fn(1, 3, |_, j| match j {
        0 => 2.0,
        1 => f64::NAN,
        2 => 0.5,
        _ => unreachable!(),
    });

    // Should NOT panic — the NaN-safe sort prevents it.
    let shet = stats::shet(&x, &ss, &corr, ShetOptions::default());
    assert_eq!(shet.len(), 1);
    // NaN elements corrupt every subvector's weighted_score to NaN.
    // Since `ttt < NaN` is always false, the initial TTT=-1 is returned.
    // This is an invalid statistic (SHet ≥ 0 for valid input), signaling
    // bad data. The DAG node filters NaN rows before calling the library.
    assert!(
        shet[0] < 0.0 || shet[0].is_nan(),
        "NaN row SHet should be invalid (negative or NaN), got {}",
        shet[0]
    );
}

#[test]
fn corr_matrix_handles_nan_pairwise() {
    // 4 rows × 2 cols; row 2 has NaN in col 1.
    let x = Mat::from_fn(4, 2, |i, j| match (i, j) {
        (0, 0) => 1.0,
        (1, 0) => 2.0,
        (2, 0) => 3.0,
        (3, 0) => 4.0,
        (0, 1) => 2.0,
        (1, 1) => 4.0,
        (2, 1) => f64::NAN, // missing
        (3, 1) => 8.0,
        _ => unreachable!(),
    });

    let r = input::corr_matrix(&x);
    // With pairwise-complete (rows 0,1,3), col0=[1,2,4] col1=[2,4,8]
    // are perfectly correlated.
    assert!(
        (r[(0, 1)] - 1.0).abs() < 1e-10,
        "pairwise corr should be 1.0, got {}",
        r[(0, 1)]
    );
    // Diagonal should be 1.0.
    assert!((r[(0, 0)] - 1.0).abs() < 1e-10);
    assert!((r[(1, 1)] - 1.0).abs() < 1e-10);
}
