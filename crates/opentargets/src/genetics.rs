//! Genetics (fine-mapping / Locus-to-Gene) types over the Platform
//! `credibleSets` query family.
//!
//! The standalone Open Targets Genetics Portal API was retired and its
//! content folded into the Platform GraphQL API: fine-mapped credible sets
//! are exposed via `credibleSets(filter, page)` and every locus carries its
//! Locus-to-Gene (L2G) predictions, each with a score and the predicted
//! target gene.

use serde::{Deserialize, Serialize};

/// GraphQL selection set for one `credibleSets` row (kept next to the types
/// it deserializes into; re-exported for the client).
pub(crate) const CREDIBLE_SET_FRAGMENT: &str = r#"
studyLocusId studyId studyType purityMeanR2 pValueMantissa pValueExponent beta
study { traitFromSource pubmedId nCases nControls }
variant { id rsIds }
l2GPredictions { count rows { score target { id approvedSymbol } } }
"#;

/// Minimal study projection carried on a credible-set locus.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredibleSetStudy {
    /// Reported trait label, e.g. `"LDL cholesterol"`.
    #[serde(default)]
    pub trait_from_source: Option<String>,
    /// PubMed identifier, when the study is published.
    #[serde(default)]
    pub pubmed_id: Option<String>,
    /// Case sample size (0 for quantitative traits).
    #[serde(default)]
    pub n_cases: Option<i64>,
    /// Control sample size (0 for quantitative traits).
    #[serde(default)]
    pub n_controls: Option<i64>,
}

/// Lead-variant projection carried on a credible-set locus.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredibleSetVariant {
    /// `chr_pos_ref_alt` (GRCh38) identifier of the lead variant.
    #[serde(default)]
    pub id: Option<String>,
    /// rsID synonyms of the lead variant.
    #[serde(default)]
    pub rs_ids: Option<Vec<String>>,
}

/// A gene predicted to be causally linked to a locus, with its L2G score.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct L2GPrediction {
    /// Locus-to-Gene posterior score in `[0, 1]`.
    #[serde(default)]
    pub score: f64,
    /// The predicted target gene.
    #[serde(default)]
    pub target: L2GTarget,
}

/// Target-gene identity inside an L2G prediction.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct L2GTarget {
    /// Ensembl gene ID.
    #[serde(default)]
    pub id: String,
    /// HGNC approved symbol, e.g. `"APOH"`.
    #[serde(default)]
    pub approved_symbol: Option<String>,
}

/// L2G prediction list attached to a credible set (unpaginated, small).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct L2GPredictionPage {
    #[serde(default)]
    pub count: i64,
    #[serde(default)]
    pub rows: Vec<L2GPrediction>,
}

/// One fine-mapped study–locus: a credible set with its lead variant,
/// study context, and L2G gene predictions.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredibleSetLocus {
    /// Stable hash ID of the study–locus pair.
    #[serde(default)]
    pub study_locus_id: String,
    /// Owning study ID (e.g. GCST…).
    #[serde(default)]
    pub study_id: String,
    /// `gwas`, `molqtl`, …
    #[serde(default)]
    pub study_type: Option<String>,
    /// Mean LD `r²` of the credible set (purity).
    #[serde(default)]
    pub purity_mean_r2: Option<f64>,
    /// p-value mantissa of the lead variant.
    #[serde(default)]
    pub p_value_mantissa: Option<f64>,
    /// p-value decimal exponent of the lead variant.
    #[serde(default)]
    pub p_value_exponent: Option<i32>,
    /// Effect size of the lead variant.
    #[serde(default)]
    pub beta: Option<f64>,
    #[serde(default)]
    pub study: Option<CredibleSetStudy>,
    #[serde(default)]
    pub variant: Option<CredibleSetVariant>,
    /// GraphQL spells this `l2GPredictions`; serde's camelCase of
    /// `l2g_predictions` would be `l2gPredictions`, so pin the exact name.
    #[serde(default, rename = "l2GPredictions")]
    pub l2g_predictions: L2GPredictionPage,
}

/// One page of `credibleSets` results.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredibleSetPage {
    /// Total matching loci across all pages.
    #[serde(default)]
    pub count: i64,
    #[serde(default)]
    pub rows: Vec<CredibleSetLocus>,
}
