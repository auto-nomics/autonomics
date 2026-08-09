//! `mice` — a pure-Rust port of the R package
//! [`mice`](https://github.com/amices/mice) (van Buuren, Groothuis-Oudshoorn
//! et al.) which performs **Multivariate Imputation by Chained Equations**.
//!
//! The package implements a Gibbs sampler that, for each incomplete column
//! `y_j`, fits a univariate conditional imputation model on the
//! observed-plus-currently-imputed predictors and draws `m` imputed values.
//! Five commonly-used imputation methods are exposed here, matching the R
//! reference implementations to within ~1e-10:
//!
//! | Method    | Module                  | R function              |
//! |-----------|-------------------------|-------------------------|
//! | `pmm`     | [`pmm`]                 | `mice.impute.pmm`       |
//! | `norm`    | [`norm`]                | `mice.impute.norm`      |
//! | `mean`    | [`mean`]                | `mice.impute.mean`      |
//! | `sample`  | [`sample`]              | `mice.impute.sample`    |
//! | `logreg`  | [`logreg`]              | `mice.impute.logreg`    |
//!
//! The orchestrator [`mice::mice`] runs the full Gibbs loop and returns a
//! [`Mids`] object, from which completed datasets can be extracted via
//! [`complete`]. Numeric results reproduce R's `mice::mice()` output to
//! ~1e-10 in the cross-validation harness.
//!
//! # References
//!
//! - van Buuren S, Groothuis-Oudshoorn K (2011). `mice`: Multivariate
//!   Imputation by Chained Equations in `R`. *Journal of Statistical
//!   Software* 45(3):1-67. doi:10.18637/jss.v045.i03
//! - van Buuren S (2018). *Flexible Imputation of Missing Data. Second
//!   Edition.* Chapman & Hall/CRC.

#![allow(clippy::needless_range_loop)]

pub mod augment;
pub mod complete;
pub mod error;
pub mod estimice;
pub mod linalg;
pub mod logreg;
pub mod mean;
pub mod mids;
pub mod norm;
pub mod orchestrator;
pub mod pmm;
pub mod sample;

pub use complete::{CompleteFormat, complete};
pub use error::{MiceError, Result};
pub use mids::{ImpList, MethodSpec, Mids};
pub use orchestrator::{MiceConfig, mice};
