//! Regression discontinuity DAG node bundle.
//!
//! Wraps the pure-Rust [`rdrobust`] and [`rdpower`] crates (faithful ports
//! of the R `rdrobust` and `rdpower` packages by Calonico, Cattaneo,
//! Titiunik & Vázquez-Bare).
//!
//! ## Nodes
//!
//! | Node kind      | Description                                                  |
//! |----------------|--------------------------------------------------------------|
//! | `rdrobust`     | Local-polynomial RD estimation with robust bias correction. |
//! | `rdpower`      | Power calculations for RD designs.                           |
//! | `rdsampsi`     | Sample size calculations for RD designs.                     |
//! | `rdmde`        | Minimum detectable effect (MDE) for RD designs.             |

pub mod common;
pub mod rdmde;
pub mod rdpower;
pub mod rd_extra;
pub mod rdrobust_node;
pub mod rdsampsi;

pub use rdmde::*;
pub use rdpower::*;
pub use rd_extra::*;
pub use rdrobust_node::*;
pub use rdsampsi::*;
