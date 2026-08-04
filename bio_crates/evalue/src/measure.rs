//! Effect measure types and conversions.
//!
//! Port of `R/effect_measures.R`. Provides typed effect measures (RR, OR, HR,
//! RD, OLS, MD) and approximate conversions to the risk-ratio scale.

use crate::error::{EvalueError, Result};

/// An effect measure estimate with its type and metadata.
///
/// Mirrors the R S3 classes ("RR", "OR", "HR", "RD", "OLS", "MD") and their
/// attributes (`rare`, `sd`). Conversion history is tracked for display.
#[derive(Clone, Debug)]
pub struct Estimate {
    pub measure: MeasureKind,
    pub est: f64,
    /// For OR/HR: whether the rare-outcome assumption holds.
    pub rare: Option<bool>,
    /// For OLS: the outcome standard deviation.
    pub sd: Option<f64>,
    /// Conversion history: list of (measure_name, value) pairs.
    pub history: Vec<(String, f64)>,
}

/// Type of effect measure.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MeasureKind {
    RR,
    OR,
    HR,
    RD,
    OLS,
    MD,
}

impl MeasureKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::RR => "RR",
            Self::OR => "OR",
            Self::HR => "HR",
            Self::RD => "RD",
            Self::OLS => "OLS",
            Self::MD => "MD",
        }
    }

    /// Default true value (null) for this measure kind.
    pub fn default_true(&self) -> f64 {
        match self {
            Self::RD => 0.0,
            Self::OLS | Self::MD => 0.0,
            _ => 1.0,
        }
    }
}

// ── Constructors ──────────────────────────────────────────────────────

impl Estimate {
    pub fn rr(est: f64) -> Self {
        Self {
            measure: MeasureKind::RR,
            est,
            rare: None,
            sd: None,
            history: vec![],
        }
    }

    pub fn or(est: f64, rare: bool) -> Self {
        Self {
            measure: MeasureKind::OR,
            est,
            rare: Some(rare),
            sd: None,
            history: vec![],
        }
    }

    pub fn hr(est: f64, rare: bool) -> Self {
        Self {
            measure: MeasureKind::HR,
            est,
            rare: Some(rare),
            sd: None,
            history: vec![],
        }
    }

    pub fn rd(est: f64) -> Self {
        Self {
            measure: MeasureKind::RD,
            est,
            rare: None,
            sd: None,
            history: vec![],
        }
    }

    pub fn ols(est: f64, sd: f64) -> Self {
        Self {
            measure: MeasureKind::OLS,
            est,
            rare: None,
            sd: Some(sd),
            history: vec![],
        }
    }

    pub fn md(est: f64) -> Self {
        Self {
            measure: MeasureKind::MD,
            est,
            rare: None,
            sd: None,
            history: vec![],
        }
    }

    /// Parse a measure kind from its string name.
    pub fn parse_kind(s: &str) -> Result<MeasureKind> {
        match s {
            "RR" => Ok(MeasureKind::RR),
            "OR" => Ok(MeasureKind::OR),
            "HR" => Ok(MeasureKind::HR),
            "RD" => Ok(MeasureKind::RD),
            "OLS" => Ok(MeasureKind::OLS),
            "MD" => Ok(MeasureKind::MD),
            _ => Err(EvalueError::Invalid(format!("Unknown measure kind: '{s}'"))),
        }
    }
}

// ── Conversions ───────────────────────────────────────────────────────

impl Estimate {
    /// Convert to standardized mean difference (MD scale).
    ///
    /// Port of `toMD.OLS()`: `MD = est * delta / sd`.
    pub fn to_md(&self, delta: f64) -> Result<Estimate> {
        match self.measure {
            MeasureKind::OLS => {
                let sd = self.sd.ok_or_else(|| {
                    EvalueError::Invalid(
                        "Must specify the outcome standard deviation (sd) for OLS conversion"
                            .into(),
                    )
                })?;
                let md_val = self.est * delta / sd;
                let mut history = self.history.clone();
                history.push(("OLS".into(), self.est));
                Ok(Estimate {
                    measure: MeasureKind::MD,
                    est: md_val,
                    rare: None,
                    sd: None,
                    history,
                })
            }
            _ => Err(EvalueError::Invalid(format!(
                "MD conversion is currently available only for OLS estimates (got {})",
                self.measure.as_str()
            ))),
        }
    }

    /// Convert to risk ratio (RR scale).
    ///
    /// Ports `toRR.OR()`, `toRR.HR()`, `toRR.MD()`, `toRR.OLS()`.
    pub fn to_rr(&self) -> Result<Estimate> {
        match self.measure {
            MeasureKind::RR => Ok(self.clone()),
            MeasureKind::OR => {
                let rare = self.rare.ok_or_else(|| {
                    EvalueError::Invalid(
                        "Must specify rare for OR conversion. Use OR(est, rare).".into(),
                    )
                })?;
                let rr = if rare { self.est } else { self.est.sqrt() };
                let mut history = self.history.clone();
                history.push(("OR".into(), self.est));
                Ok(Estimate {
                    measure: MeasureKind::RR,
                    est: rr,
                    rare: None,
                    sd: None,
                    history,
                })
            }
            MeasureKind::HR => {
                let rare = self.rare.ok_or_else(|| {
                    EvalueError::Invalid(
                        "Must specify rare for HR conversion. Use HR(est, rare).".into(),
                    )
                })?;
                let rr = if rare {
                    self.est
                } else {
                    (1.0 - 0.5_f64.powf(self.est.sqrt()))
                        / (1.0 - 0.5_f64.powf(1.0 / self.est.sqrt()))
                };
                let mut history = self.history.clone();
                history.push(("HR".into(), self.est));
                Ok(Estimate {
                    measure: MeasureKind::RR,
                    est: rr,
                    rare: None,
                    sd: None,
                    history,
                })
            }
            MeasureKind::MD => {
                // RR = exp(0.91 * MD)
                let rr = (0.91 * self.est).exp();
                let mut history = self.history.clone();
                history.push(("MD".into(), self.est));
                Ok(Estimate {
                    measure: MeasureKind::RR,
                    est: rr,
                    rare: None,
                    sd: None,
                    history,
                })
            }
            MeasureKind::OLS => {
                // OLS → MD → RR, using delta = 1 (callers handle delta via to_md first)
                let md = self.to_md(1.0)?;
                md.to_rr()
            }
            MeasureKind::RD => Err(EvalueError::Invalid(
                "RR conversion is not available for RD estimates".into(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_or_rare_conversion() {
        let or = Estimate::or(3.0, true);
        let rr = or.to_rr().unwrap();
        assert!((rr.est - 3.0).abs() < 1e-10);
    }

    #[test]
    fn test_or_nonrare_conversion() {
        let or = Estimate::or(3.0, false);
        let rr = or.to_rr().unwrap();
        assert!((rr.est - 3.0_f64.sqrt()).abs() < 1e-10);
    }

    #[test]
    fn test_hr_nonrare_conversion() {
        // HR = 0.56, rare = FALSE
        let hr = Estimate::hr(0.56, false);
        let rr = hr.to_rr().unwrap();
        let expected =
            (1.0 - 0.5_f64.powf(0.56_f64.sqrt())) / (1.0 - 0.5_f64.powf(1.0 / 0.56_f64.sqrt()));
        assert!(
            (rr.est - expected).abs() < 1e-10,
            "got {}, expected {expected}",
            rr.est
        );
    }

    #[test]
    fn test_ols_to_rr() {
        let ols = Estimate::ols(3.0, 1.2);
        let rr = ols.to_rr().unwrap();
        // OLS → MD = 3*1/1.2 = 2.5 → RR = exp(0.91 * 2.5)
        let expected = (0.91_f64 * (3.0_f64 / 1.2)).exp();
        assert!((rr.est - expected).abs() < 1e-10);
    }

    #[test]
    fn test_md_to_rr() {
        let md = Estimate::md(0.5);
        let rr = md.to_rr().unwrap();
        let expected = (0.91_f64 * 0.5_f64).exp();
        assert!((rr.est - expected).abs() < 1e-10);
    }
}
