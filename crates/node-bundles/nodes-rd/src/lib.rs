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
pub mod rd_extra;
pub mod rdmde;
pub mod rdpower;
pub mod rdrobust_node;
pub mod rdsampsi;

pub use rd_extra::*;
pub use rdmde::*;
pub use rdpower::*;
pub use rdrobust_node::*;
pub use rdsampsi::*;

use dag_core::{NodePlugin, NodeRegistry};

/// Plugin entry point for all regression-discontinuity DAG nodes.
pub struct Plugin;

impl NodePlugin for Plugin {
    fn name(&self) -> &'static str {
        "rd"
    }

    fn register(&self, registry: &mut NodeRegistry) {
        registry.register(Box::new(rdrobust_node::RdRobustNodeFactory));
        registry.register(Box::new(rdpower::RdPowerNodeFactory));
        registry.register(Box::new(rdsampsi::RdSampsiNodeFactory));
        registry.register(Box::new(rdmde::RdMdeNodeFactory));
        registry.register(Box::new(rd_extra::RdMcNodeFactory));
        registry.register(Box::new(rd_extra::RdDensityNodeFactory));
        registry.register(Box::new(rd_extra::RdRandInfNodeFactory));
    }
}
