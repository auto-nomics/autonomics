//! Model specification: covariate indicator vectors + parameter layout.

use serde::{Deserialize, Serialize};

/// Indicator-vector specification of an hlme model.
///
/// Each covariate column `k` in the design matrix `X0` (length `nv`) is tagged
/// with up to four roles:
/// * `idprob[k] = 1` — enters the class-membership multinomial logit
/// * `idea[k]  = 1` — has a random effect
/// * `idg[k]   = 0/1/2` — no effect / overall fixed / class-specific fixed
/// * `idcor[k] = 1` — is the time variable used by `cor`
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelSpec {
    pub ng: usize,
    pub idiag: bool,
    pub nwg: bool,
    /// 0 = none, 1 = Brownian motion, 2 = AR(1).
    pub ncor: usize,
    /// Length nv, values 0/1.
    pub idprob: Vec<u8>,
    /// Length nv, values 0/1.
    pub idea: Vec<u8>,
    /// Length nv, values 0/1/2.
    pub idg: Vec<u8>,
    /// Length nv, values 0/1.
    pub idcor: Vec<u8>,
}

/// Pre-computed parameter counts and offsets into `B`.
///
/// All offsets are 0-based byte offsets into the parameter vector. The layout
/// matches R `hlme` exactly so that initial values and golden-test vectors are
/// interchangeable.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ParamLayout {
    pub ng: usize,
    pub nv: usize,
    pub nea: usize,        // # random-effect design columns (sum idea==1)
    pub nvarprob: usize,   // # classmb covariates (sum idprob==1)
    pub nprob: usize,      // nvarprob * (ng-1)
    pub ncssig: usize,     // # idg==1 covars
    pub ncg: usize,        // # idg==2 covars
    pub nef_count: usize,  // ncssig + ncg*ng
    pub nvc: usize,        // nea (idiag) or nea*(nea+1)/2 (full)
    pub nw: usize,         // ng-1 if nwg else 0
    pub ncor: usize,       // 0/1/2
    pub npm: usize,        // total parameter count
    pub i_nef: usize,      // start of fixed-effect block
    pub i_nvc: usize,      // start of variance-Cholesky block
    pub i_nw: usize,       // start of nwg block
    pub i_ncor: usize,     // start of correlation block
    pub i_stderr: usize,   // index of stderr (= npm - 1)
}

impl ModelSpec {
    pub fn layout(&self) -> ParamLayout {
        let nv = self.idg.len();
        debug_assert_eq!(self.idprob.len(), nv);
        debug_assert_eq!(self.idea.len(), nv);
        debug_assert_eq!(self.idcor.len(), nv);

        let nea = self.idea.iter().filter(|&&x| x == 1).count();
        let nvarprob = self.idprob.iter().filter(|&&x| x == 1).count();
        let ncssig = self.idg.iter().filter(|&&x| x == 1).count();
        let ncg = self.idg.iter().filter(|&&x| x == 2).count();

        let nprob = if self.ng > 1 { nvarprob * (self.ng - 1) } else { 0 };
        let nef_count = ncssig + ncg * self.ng;
        let nvc = if self.idiag { nea } else { nea * (nea + 1) / 2 };
        let nw = if self.nwg && self.ng > 1 { self.ng - 1 } else { 0 };
        let ncor = self.ncor;

        let i_nef = nprob;
        let i_nvc = i_nef + nef_count;
        let i_nw = i_nvc + nvc;
        let i_ncor = i_nw + nw;
        let i_stderr = i_ncor + ncor;
        let npm = i_stderr + 1;

        ParamLayout {
            ng: self.ng,
            nv,
            nea,
            nvarprob,
            nprob,
            ncssig,
            ncg,
            nef_count,
            nvc,
            nw,
            ncor,
            npm,
            i_nef,
            i_nvc,
            i_nw,
            i_ncor,
            i_stderr,
        }
    }
}

impl ParamLayout {
    /// Reconstruct the Cholesky factor `Ut` (upper-triangular, nea×nea) of the
    /// random-effect covariance B, where B = Ut'·Ut. This mirrors Fortran
    /// `hetmixlin.f90` lines 149–168.
    pub fn cholesky_ut(&self, b: &[f64], idiag: bool) -> Vec<f64> {
        // Returns row-major nea*nea matrix.
        let n = self.nea;
        let mut ut = vec![0.0; n * n];
        if n == 0 {
            return ut;
        }
        if idiag {
            // Ut[j][j] = b[i_nvc + j], off-diagonal = 0  (0-indexed j in 0..nea)
            for j in 0..n {
                ut[j * n + j] = b[self.i_nvc + j];
            }
        } else {
            // Fortran: Ut(j,k) = b(nef + k + j*(j-1)/2), 1-indexed, k≤j.
            // 0-indexed: row i=j-1, col l=k-1 (l≤i), position i_nvc + l + i*(i+1)/2
            for i in 0..n {
                for l in 0..=i {
                    ut[i * n + l] = b[self.i_nvc + l + i * (i + 1) / 2];
                }
            }
        }
        ut
    }

    /// Apply class-specific nwg scaling to the Cholesky factor.
    /// For class g < ng (1-indexed), Ut_g = Ut * |b[i_nw + g-1]|.
    /// For the reference class g = ng, Ut_g = Ut.
    /// `class1` is 1-indexed.
    pub fn scale_ut(&self, ut: &[f64], b: &[f64], class1: usize, nwg: bool) -> Vec<f64> {
        if !nwg || class1 == self.ng {
            return ut.to_vec();
        }
        let scale = b[self.i_nw + class1 - 1].abs();
        ut.iter().map(|&x| x * scale).collect()
    }
}
