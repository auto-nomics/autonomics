//! `genomic_sem` — a pure-Rust port of the R package
//! [GenomicSEM](https://github.com/GenomicSEM/GenomicSEM) (Grotzinger et al.,
//! *Nature Human Behaviour* 2019), for fitting **structural equation models**
//! (SEMs) to GWAS summary statistics.
//!
//! The package uses **multivariate LD Score regression** to estimate the
//! genetic covariance matrix `S` and its sampling covariance `V`, then fits
//! user-specified SEMs to that matrix using **diagonally weighted least
//! squares** (DWLS) or **maximum likelihood** (ML), with sandwich-corrected
//! standard errors.
//!
//! # Pipeline
//!
//! ```text
//! GWAS sumstats → munge → ldsc → S/V matrices → SEM fit → per-SNP GWAS
//! ```
//!
//! # Faithfulness conventions
//!
//! - **NA** is `f64::NAN`; "valid" means `!is_nan()`.
//! - Linear algebra runs on [`faer`] (no LAPACK/MKL).
//! - Normal distribution functions are hand-implemented (reproducing R's
//!   `pnorm`/`qnorm`/`dnorm`); the χ² survival function uses [`statrs`].
//! - The `nearPD` implementation follows Higham (2002) alternating projection.
//!
//! # Modules
//!
//! | Module       | R source                          | Contents                                                |
//! |--------------|-----------------------------------|---------------------------------------------------------|
//! | [`near_pd`]  | `Matrix::nearPD`                  | Nearest positive-definite matrix (Higham 2002)          |
//! | [`stats`]    | `stats::pnorm/qnorm/pchisq`       | Distribution wrappers                                   |
//! | [`linalg`]   | (internal)                        | faer helpers: solve, eigen, vech, cov2cor               |
//! | [`utils`]    | `R/utils.R`                       | V_SNP, V_full, S_full, rearrange, Z_pre                 |
//! | [`ldsc`]     | `R/ldsc.R`                        | Multivariate LD Score regression                        |
//! | [`munge`]    | `R/munge.R`                       | Summary-statistic QC and harmonisation                  |
//! | [`sumstats`] | `R/sumstats.R`                    | Merge multiple sumstats for multivariate GWAS           |
//! | [`sem`]      | `R/usermodel.R`, `commonfactor.R` | SEM fitting engine (DWLS/ML + sandwich SE)              |
//! | [`usermodel`]| `R/usermodel.R`                   | User-specified SEM                                      |
//! | [`commonfactor`] | `R/commonfactor.R`            | Common-factor SEM + CFI                                 |
//! | [`rgmodel`]  | `R/rgmodel.R`                     | Model-implied genetic correlation matrix                |
//! | [`gwas`]     | `R/userGWAS.R`                    | Per-SNP multivariate GWAS                               |
//! | [`sldsc`]    | `R/s_ldsc.R`                      | Stratified/partitioned LDSC                             |
//! | [`enrich`]   | `R/enrich.R`                      | SEM-based enrichment testing                            |
//! | [`paldsc`]   | `R/paLDSC.R`                      | Parallel analysis (Horn's PA)                           |
//! | [`sim`]      | `R/simLDSC.R`                     | Simulate GWAS sumstats                                  |
//! | [`write_model`] | `R/write.model.R`              | Lavaan syntax generation                                |
//! | [`summary_gls`] | `R/summaryGLS.R`              | GLS regression summary                                  |
//! | [`index`]    | `R/indexS.R`, `subSV.R`           | Index/subset helpers                                    |
//! | [`fusion`]   | `R/read_fusion.R`                 | FUSION TWAS file reader                                 |

#![allow(clippy::needless_range_loop)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::doc_lazy_continuation)]

pub mod error;
pub mod linalg;
pub mod near_pd;
pub mod stats;
pub mod utils;

pub mod commonfactor;
pub mod enrich;
pub mod fusion;
pub mod gwas;
pub mod index;
pub mod ldsc;
pub mod munge;
pub mod paldsc;
pub mod rgmodel;
pub mod sem;
pub mod sim;
pub mod sldsc;
pub mod summary_gls;
pub mod sumstats;
pub mod usermodel;
pub mod write_model;

pub use error::{GenomicSemError, Result};

// Re-export key types for convenience.
pub use ldsc::{LdscConfig, LdscOutput};
pub use utils::{Covstruc, GcMode};
