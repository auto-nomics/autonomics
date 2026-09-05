use serde::{Deserialize, Serialize};

// ===========================================================================
// Journal rank (normalized)
// ===========================================================================

/// Journal ranking metrics for a single publication, normalized from the
/// EasyScholar wire format.
///
/// Numeric impact factors are parsed to `f64` at this boundary; quartile and
/// CAS classification fields stay as cleaned strings. Every field is
/// `Option` — EasyScholar only knows journals it covers, and even covered
/// journals may miss individual dimensions (e.g. SSCI quartile for a
/// SCI-only journal).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct JournalRank {
    /// JCR impact factor (2-year window).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub impact_factor: Option<f64>,
    /// JCR 5-year impact factor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub impact_factor_5: Option<f64>,
    /// JCR quartile (`"Q1"`–`"Q4"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jcr_quartile: Option<String>,
    /// SSCI quartile (`"Q1"`–`"Q4"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ssci_quartile: Option<String>,
    /// CAS (中科院) upgraded quartile — 大类.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cas_quartile: Option<String>,
    /// CAS (中科院) base-edition quartile — 基础版.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cas_quartile_base: Option<String>,
    /// CAS (中科院) small-class quartile — 小类.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cas_small: Option<String>,
    /// CAS (中科院) top-journal flag (顶级期刊).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cas_top: Option<bool>,
    /// CAS (中科院) early-warning list membership (预警期刊).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cas_warning: Option<String>,
}

impl JournalRank {
    /// Whether any dimension carries a value.
    ///
    /// EasyScholar answers `code: 200` with an empty rank block for unknown
    /// journal names; callers use this to distinguish "no data" from
    /// "partial data".
    pub fn has_any(&self) -> bool {
        self.impact_factor.is_some()
            || self.impact_factor_5.is_some()
            || self.jcr_quartile.is_some()
            || self.ssci_quartile.is_some()
            || self.cas_quartile.is_some()
            || self.cas_quartile_base.is_some()
            || self.cas_small.is_some()
            || self.cas_top.is_some()
            || self.cas_warning.is_some()
    }
}

// ===========================================================================
// Wire types (private to the crate via client.rs re-export control)
// ===========================================================================

/// EasyScholar response envelope.
#[derive(Debug, Deserialize)]
pub(crate) struct EasyScholarResponse {
    /// Business status code (`200` = success; `40002` = invalid key).
    pub code: i32,
    /// Error message when `code != 200`.
    pub msg: Option<String>,
    /// Payload, present on success.
    pub data: Option<EasyScholarData>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct EasyScholarData {
    #[serde(rename = "officialRank")]
    pub official_rank: Option<OfficialRank>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OfficialRank {
    #[serde(rename = "all")]
    pub all: Option<RankData>,
}

/// Raw per-dimension rank strings, exactly as EasyScholar names them:
///
/// | wire field      | normalized field        |
/// |-----------------|-------------------------|
/// | `sciif`         | `impact_factor`         |
/// | `sciif5`        | `impact_factor_5`       |
/// | `sci`           | `jcr_quartile`          |
/// | `ssci`          | `ssci_quartile`         |
/// | `sciBase`       | `cas_quartile_base`     |
/// | `sciUp`         | `cas_quartile`          |
/// | `sciUpSmall`    | `cas_small`             |
/// | `sciUpTop`      | `cas_top`               |
/// | `sciwarn`       | `cas_warning`           |
#[derive(Debug, Deserialize)]
pub(crate) struct RankData {
    pub sciif: Option<String>,
    pub sciif5: Option<String>,
    pub sci: Option<String>,
    pub ssci: Option<String>,
    #[serde(rename = "sciBase")]
    pub sci_base: Option<String>,
    #[serde(rename = "sciUp")]
    pub sci_up: Option<String>,
    #[serde(rename = "sciUpSmall")]
    pub sci_up_small: Option<String>,
    #[serde(rename = "sciUpTop")]
    pub sci_up_top: Option<String>,
    pub sciwarn: Option<String>,
}
