//! `magma` — a pure-Rust port of **MAGMA v1.10** (de Leeuw et al., *PLoS Comp
//! Biol* 2015), the gene-based GWAS analysis tool from CTG Lab, VU Amsterdam.
//!
//! MAGMA performs gene-level association testing by aggregating SNP-level
//! signals within genes, followed by optional gene-set and gene-property
//! analysis. It supports both raw genotype data (PLINK format) and
//! summary statistics (SNP p-values + reference LD panel).
//!
//! # Analysis modes
//!
//! | Mode | Description | Module |
//! |------|-------------|--------|
//! | Annotation | Map SNPs to genes → `.genes.annot` | [`annotation`] |
//! | Gene analysis (pval) | SNP p-values + reference LD → gene p-values | [`geneanalysis`] |
//! | Gene analysis (raw) | PLINK genotypes + phenotype → gene p-values | [`geneanalysis`] |
//! | Gene-set analysis | Competitive regression test on gene sets | _todo_ |
//! | Gene-property analysis | Continuous gene-level covariate regression | _todo_ |
//! | Meta-analysis | Combine multiple gene results | _todo_ |
//!
//! # Faithfulness conventions
//!
//! - Linear algebra runs on [`faer`] (no LAPACK/MKL).
//! - Normal distribution functions reproduce R's `pnorm`/`qnorm` via [`statrs`].
//! - Imhof's method (weighted sum of χ²) is implemented via adaptive quadrature.
//! - PLINK `.bed` decoding follows the SNP-major 2-bit format specification.
//! - P-value truncation defaults match MAGMA: `[1e-50, 1e-5]`.

#![allow(clippy::needless_range_loop)]

pub mod annotation;
pub mod error;
pub mod geneanalysis;
pub mod geneinput;
pub mod plink;
pub mod stats;

pub use error::{MagmaError, Result};
