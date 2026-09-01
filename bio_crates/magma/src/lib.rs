//! `magma` — a pure-Rust port of **MAGMA v1.10** (de Leeuw et al., *PLoS Comp
//! Biol* 2015), the gene-based GWAS analysis tool from CTG Lab, VU Amsterdam.
//!
//! MAGMA performs gene-level association testing by aggregating SNP-level
//! signals within genes, followed by optional gene-set and gene-property
//! analysis. It supports both raw genotype data (PLINK format) and
//! summary statistics (SNP p-values + reference LD panel).
//!
//! # Summary-stats pipeline
//!
//! | Step | CLI | Module | Status |
//! |------|-----|--------|--------|
//! | Annotation | `--annotate` | external `magma_annotate_container` | replaced by the official runtime |
//! | Gene analysis (pval) | `--bfile --pval --gene-annot` | [`geneanalysis`] | ✅ |
//! | Gene-set analysis | `--gene-results --set-annot` | [`setanalysis`] | ✅ |
//! | Gene-property analysis | `--gene-results --gene-covar` | [`setanalysis`] | ✅ |
//! | Meta-analysis | `--meta` | [`meta`] | ✅ |
//! | Merge | `--merge` | [`meta`] | ✅ |
//!
//! # Faithfulness conventions
//!
//! - Linear algebra runs on [`faer`] (no LAPACK/MKL).
//! - Normal distribution functions reproduce R's `pnorm`/`qnorm` via [`statrs`].
//! - Imhof's method (weighted sum of χ²) via midpoint quadrature with u=t/(1-t).
//! - PLINK `.bed` decoding follows the SNP-major 2-bit format specification.
//! - P-value truncation: `[1e-50, 1-1e-5]` (MAGMA's `set_bounds(low, 1-high)`).

#![allow(clippy::needless_range_loop)]

pub(crate) mod chromosome;
pub mod error;
pub mod geneanalysis;
pub mod geneinput;
pub mod meta;
pub mod plink;
pub mod setanalysis;
pub mod stats;

pub use error::{MagmaError, Result};
