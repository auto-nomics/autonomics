//! Long-format longitudinal data, grouped by subject.
//!
//! Mirrors the R `hlme` pre-processing: after NA-removal and subject-sorting,
//! observations are packed contiguously per subject. The design matrix `X0`
//! contains all covariate columns (the union of `fixed`/`mixture`/`random`/
//! `classmb`/`cor` terms), in the same order used by the indicator vectors in
//! [`crate::spec::ModelSpec`].

use serde::{Deserialize, Serialize};

/// Packed long-format data for `hlme`.
///
/// `x` is row-major `nobs × nv`: element `(r, k)` is at `x[r * nv + k]`.
/// `pprior` is row-major `ns × ng`: element `(i, g)` is at `pprior[i * ng + g]`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LongData {
    pub ns: usize,
    pub ng: usize,
    pub nv: usize,
    pub nobs: usize,
    pub y: Vec<f64>,
    pub x: Vec<f64>,
    /// `ns` entries: number of observations per subject.
    pub nmes: Vec<usize>,
    /// `ns + 1` prefix sums: subject `i` occupies rows `[offsets[i], offsets[i+1])`.
    pub offsets: Vec<usize>,
    /// `ns` entries: 0 = no prior, else 1..ng fixes the subject's class.
    pub prior: Vec<i32>,
    /// `ns * ng` entries: prior class-membership probabilities (default all 1).
    pub pprior: Vec<f64>,
}

impl LongData {
    /// Build a `LongData` from per-subject observation blocks, defaulting
    /// `prior = 0` (no fixed class) and `pprior = 1` (uniform).
    pub fn new(y: Vec<f64>, x: Vec<f64>, nv: usize, nmes: Vec<usize>, ng: usize) -> Self {
        let ns = nmes.len();
        let nobs = y.len();
        debug_assert_eq!(x.len(), nobs * nv);
        let mut offsets = Vec::with_capacity(ns + 1);
        offsets.push(0);
        let mut acc = 0;
        for &n in &nmes {
            acc += n;
            offsets.push(acc);
        }
        debug_assert_eq!(acc, nobs);
        let prior = vec![0; ns];
        let pprior = vec![1.0; ns * ng];
        Self {
            ns,
            ng,
            nv,
            nobs,
            y,
            x,
            nmes,
            offsets,
            prior,
            pprior,
        }
    }

    /// Set the `prior` vector (subject-fixed class assignments).
    pub fn with_prior(mut self, prior: Vec<i32>) -> Self {
        debug_assert_eq!(prior.len(), self.ns);
        self.prior = prior;
        self
    }

    /// Set the `pprior` matrix (row-major ns×ng).
    pub fn with_pprior(mut self, pprior: Vec<f64>) -> Self {
        debug_assert_eq!(pprior.len(), self.ns * self.ng);
        self.pprior = pprior;
        self
    }

    /// Slice of Y for subject `i`.
    pub fn y_subject(&self, i: usize) -> &[f64] {
        &self.y[self.offsets[i]..self.offsets[i + 1]]
    }

    /// Covariate `k` for subject `i` at local observation index `j`.
    #[inline]
    pub fn x_at(&self, i: usize, j: usize, k: usize) -> f64 {
        self.x[(self.offsets[i] + j) * self.nv + k]
    }

    /// Maximum number of observations over all subjects (= Fortran `maxmes`).
    pub fn maxmes(&self) -> usize {
        self.nmes.iter().copied().max().unwrap_or(0)
    }
}
