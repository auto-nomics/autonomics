//! `hdl` — a pure-Rust port of **HDL-L** (Li, Pawitan & Shen, *Nat Genet* 2025,
//! doi:10.1038/s41588-025-02123-3), the *local* version of the High-Definition
//! Likelihood method for **local genetic correlation** analysis.
//!
//! HDL-L partitions the genome into ~2,468 approximately independent LD
//! segments and, within each segment, estimates local heritabilities h²₁/h²₂,
//! the local genetic covariance h₁₂, and the local genetic correlation
//! r_G = h₁₂ / √(h²₁h²₂) — with a **likelihood-based confidence interval** and
//! a **likelihood-ratio test (LRT) P value**, all derived from the full HDL
//! likelihood.
//!
//! This is a **library only**. It is a 1:1 faithful port of the R reference
//! implementation in the `HDL` package:
//!   - `HDL/R/HDL.L.R` — the per-region `HDL.L` driver (one call = one region),
//!   - `HDL/R/llfun.R` + `HDL/R/llfun.gcov.part.2.R` — the eigen-space HDL
//!     log-likelihoods.
//!
//! # Relationship to LAVA
//!
//! HDL-L is a "swap-the-estimator" enhancement of LAVA (Werme et al. 2022):
//! both share the genome partitioning, the GWAS-summary harmonisation, the
//! PLINK LD reference, and the per-region eigen-decomposition with a 99%
//! variance cutoff. HDL-L replaces LAVA's method-of-moments estimator +
//! non-central-Wishart simulation inference with an **MLE on the full HDL
//! likelihood** + **LRT P value** + **profile-likelihood CI**. Consequently
//! this crate reuses [`lava`]'s leaf primitives (PLINK loader, allele
//! alignment, distribution functions, symmetric eigen-decomposition) but ports
//! the HDL-L estimator and inference afresh.
//!
//! # Faithfulness conventions
//!
//! - **NA** is `f64::NAN`; "valid" means `!is_nan()` (matching R's `!is.na()`).
//! - Linear algebra runs on [`faer`] (no LAPACK/MKL), reusing
//!   [`lava::decompose::sym_eigen`] for the LD eigen-decomposition.
//! - χ² survival (`pchisq`) reuses [`lava::stats::pchisq_sf`]; the χ² quantile
//!   (`qchisq`, for the CI cutoff) uses [`statrs`].
//! - The MLE uses a hand-rolled faithful **L-BFGS-B** with the same multi-start
//!   strategy as R's `optim(method = "L-BFGS-B")`; point estimates match R to
//!   ~6 significant figures.
//! - HDL-L is fully deterministic (no RNG), so golden comparisons are exact
//!   value-by-value alignments — no Monte-Carlo tolerance is needed.
//!
//! | Module          | R source (`HDL/R/`)              | Contents                                                       |
//! |-----------------|----------------------------------|----------------------------------------------------------------|
//! | [`stats`]       | `pchisq`/`qchisq`                | χ² quantile (CI cutoff); survival re-exported from `lava::stats`|
//! | [`likelihood`]  | `HDL.L.R` llfun/llfun0 +         | the four eigen-space negative log-likelihoods (univariate h²,  |
//! |                 | `llfun.gcov.part.2.R`            | conditional genetic covariance) + their null forms             |
//! | [`optimize`]    | R `optim(L-BFGS-B)`              | hand-rolled bounded L-BFGS + multi-start driver                |
//! | [`wls`]         | `HDL.L.R` § LD-score regression  | OLS/WLS starting values (h²/gcov) + sample z-z correlation     |
//! | [`input`]       | `HDL.L.R` § sumstat harmonise    | filter→ref SNPs, dedup, allele align, `bhat = Z/√N`            |
//! | [`reference`]   | `build_ld_ref/` + `eigen.R`      | PLINK→(lam,V,LDsc) eigen reference + portable-array loader     |
//! | [`locus`]       | `HDL.L` driver                   | per-region full pipeline: bstar→eigen-cut→MLE→LRT→profile CI   |

#![allow(clippy::needless_range_loop)]

pub mod error;
pub mod input;
pub mod likelihood;
pub mod locus;
pub mod optimize;
pub mod reference;
pub mod stats;
pub mod wls;

pub use error::{HdlError, Result};
