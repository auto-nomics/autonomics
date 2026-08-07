//! Node abstractions and built-in implementations.
//!
//! Core traits ([`DagNode`], [`NodePorts`], [`NodeInput`]) and shared helpers
//! ([`numeric_util`], [`sink_common`]) are re-exported from [`dag_core`] via
//! thin shim modules so existing `super::meta::*` / `super::numeric_util::*`
//! paths keep working.

// ── Re-export shims for modules now living in dag-core ────────────────────
pub mod meta {
    pub use dag_core::node::*;
}
pub mod numeric_util {
    pub use dag_core::arrow_util::*;
}
pub mod sink_common {
    pub use dag_core::sink::*;
}

// ── Phase 4 inline nodes (LDSC + genetics) ────────────────────────────────
pub mod bivariate_mixer;
pub mod cpassoc;
pub mod hdl_l;
pub mod hdl_l_scan;
pub mod lava;
pub mod lcv;
pub mod ldsc_common;
pub mod ldsc_hsq;
pub mod ldsc_rg;
pub mod ldsc_sldsc;
pub mod liability;
pub mod magma;
pub mod mtag;
pub mod susie_rss;
pub mod univariate_mixer;

// ── Re-exports ────────────────────────────────────────────────────────────
pub use bivariate_mixer::{BivariateMixerNode, BivariateMixerNodeFactory, BivariateMixerNodeSpec};
pub use cpassoc::{CpassocConfig, CpassocNode, CpassocNodeFactory};
pub use hdl_l::{HdlLNode, HdlLNodeFactory, HdlLSpec};
pub use lava::{
    LavaBivarNode, LavaBivarNodeFactory, LavaLocusNode, LavaLocusNodeFactory, LavaMultiregNode,
    LavaMultiregNodeFactory, LavaPcorNode, LavaPcorNodeFactory, LavaUnivNode, LavaUnivNodeFactory,
};
pub use lcv::{LcvConfig, LcvNode, LcvNodeFactory};
pub use ldsc_hsq::{LdscHsqConfig, LdscHsqNode, LdscHsqNodeFactory};
pub use ldsc_rg::{LdscRgConfig, LdscRgNode, LdscRgNodeFactory};
pub use ldsc_sldsc::{LdscSldscConfig, LdscSldscNode, LdscSldscNodeFactory};
pub use liability::{LiabilityConfig, LiabilityNode, LiabilityNodeFactory};
pub use meta::{DEFAULT_PORT, DagNode, NodeId, NodeInput, NodePorts, Port};
pub use mtag::{MtagConfig, MtagNode, MtagNodeFactory};
pub use susie_rss::{SusieRssNode, SusieRssNodeFactory, SusieRssSpec};
pub use univariate_mixer::{
    UnivariateMixerNode, UnivariateMixerNodeFactory, UnivariateMixerNodeSpec,
};
