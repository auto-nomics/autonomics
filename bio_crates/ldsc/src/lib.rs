//! `ldsc` — a pure-Rust port of LD Score Regression (Bulik-Sullivan & Finucane,
//! the `/mnt/disk3/ldsc` Python suite), covering SNP-heritability (h²), genetic
//! correlation (rg), cell-type-specific analysis, LD-score computation from
//! PLINK genotypes, summary-statistic munging, and annotation building.
//!
//! Linear algebra runs on [`faer`] (no LAPACK/MKL backend). Numeric results are
//! validated against the Python reference's test suite (ported to
//! `tests/`) and its golden fixtures (vendored under `tests/data/`).
//!
//! This is a **library** only — there are no CLI binaries. Every Python
//! "argument" is a field on a Rust config struct.
//!
//! | Module        | Python source                           | Contents                                                       |
//! |---------------|-----------------------------------------|----------------------------------------------------------------|
//! | [`linalg`]    | (internal)                              | faer WLS / SPD / general solve, condition number               |
//! | [`jackknife`] | `ldscore/jackknife.py`                  | block jackknife + `RatioJackknife`                             |
//! | [`irwls`]     | `ldscore/irwls.py`                      | two-pass IRWLS + Hsq/Gencov weight functions                   |
//! | [`ingest`]    | (internal)                              | DataFusion `DataFrame` → faer vectors (h² path)                |
//! | [`hsq`]       | `ldscore/regressions.py: Hsq`           | the h² driver + result type                                    |
//! | [`alleles`]   | `ldscore/sumstats.py:24-48`             | allele tables, strand/ref flip logic                          |
//! | [`io`]        | `ldscore/parse.py`                      | LDSC file-format readers (ldscore/M/annot/frq/sumstats/bim/fam)|
//! | [`bedio`]     | `ldscore/ldscore.py: PlinkBEDFile`      | PLINK `.bed` genotype reader + MAF filtering                   |
//! | [`ldscore`]   | `ldscore/ldscore.py`                    | LD-score computation (`corSumVarBlocks`)                       |
//! | [`regress`]   | `ldscore/regressions.py`                | `LD_Score_Regression`, `Gencov`, `RG`, liability conversions   |
//! | [`sldsc`]     | (new)                                   | S-LDSC per-annotation result table + `.results` writer         |
//! | [`sumstats`]  | `ldscore/sumstats.py`                   | h²/rg/cts pipeline drivers (read files → regress)              |
//! | [`munge`]     | `munge_sumstats.py`                     | summary-statistic munging                                      |
//! | [`bed`]       | (new)                                   | pure-Rust BED interval sort/merge/intersect                    |
//! | [`make_annot`]| `make_annot.py`                         | gene-set / BED → `.annot`                                      |
//
// The numerical code is a faithful port of vectorised numpy/scipy, and uses
// explicit index loops (`for i in 0..n`) to mirror the Python element
// ordering exactly. Suppress the clippy styles that conflict with that.
#![allow(clippy::needless_range_loop)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::neg_cmp_op_on_partial_ord)]
#![allow(clippy::doc_lazy_continuation)]

pub mod alleles;
pub mod bed;
pub mod bedio;
pub mod hsq;
pub mod ingest;
pub mod io;
pub mod irwls;
pub mod jackknife;
pub mod ldscore;
pub mod linalg;
pub mod make_annot;
pub mod munge;
pub mod regress;
pub mod sldsc;
pub mod stats;
pub mod sumstats;

use datafusion::error::DataFusionError;

/// Error type for the `ldsc` crate.
#[derive(Debug, thiserror::Error)]
pub enum LdscError {
    /// Shape / size mismatch between vectors or matrices.
    #[error("dimension mismatch: {0}")]
    DimensionMismatch(String),
    /// An input value is invalid (empty data, non-numeric column, etc.).
    #[error("invalid input: {0}")]
    InvalidInput(String),
    /// A numerical linear-algebra operation failed (e.g. LLᵀ factorisation).
    #[error("linear algebra error: {0}")]
    Linalg(String),
    /// An arithmetic operation produced a non-finite value (NaN/Inf) where
    /// numpy would have raised (`np.seterr(divide='raise', invalid='raise')`).
    #[error("numerical error: {0}")]
    Numerical(String),
    /// File / stream I/O failure.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// Could not detect / handle a compression suffix.
    #[error("compression error: {0}")]
    Compression(String),
    /// A file did not parse according to its LDSC format.
    #[error("parse error in {context}: {reason}")]
    Parse { context: String, reason: String },
    /// PLINK `.bed` / `.bim` / `.fam` structural problem.
    #[error("plink error: {0}")]
    Plink(String),
    /// Transparent passthrough for DataFusion errors.
    #[error(transparent)]
    DataFusion(#[from] DataFusionError),
}

/// Crate-local `Result` alias.
pub type Result<T> = std::result::Result<T, LdscError>;
