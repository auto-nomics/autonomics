//! Survey design object — the shared metadata for all survey analyses.
//!
//! Mirrors R's `survey.design2` object: cluster IDs, strata, sampling
//! probabilities (or weights), FPC, and degrees-of-freedom bookkeeping.
//!
//! The design is constructed once and passed (by reference) to analysis
//! functions like [`crate::describe::svymean`].

use std::collections::HashMap;

use crate::error::{Result, SurveyError};

/// Policy for handling strata with a single PSU ("lonely PSU").
///
/// Mirrors R's `options(survey.lonely.psu = ...)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LonelyPsu {
    /// Stop with an error.
    Fail,
    /// Remove the lonely PSU's contribution (variance contribution = 0).
    #[default]
    Remove,
    /// Replace the lonely PSU's residual with the average stratum residual.
    Adjust,
    /// Treat the lonely PSU as sampled with certainty (contributes 0).
    Certainty,
    /// Average the variance across strata (divides by fraction of non-lonely).
    Average,
}

impl LonelyPsu {
    /// Parse from a string (matching R option values).
    pub fn from_str(s: &str) -> Self {
        match s {
            "fail" => Self::Fail,
            "remove" => Self::Remove,
            "adjust" => Self::Adjust,
            "certainty" => Self::Certainty,
            "average" => Self::Average,
            _ => Self::default(),
        }
    }
}

/// Finite population correction data per stratum.
#[derive(Debug, Clone, Default)]
pub struct Fpc {
    /// Population size per stratum (keyed by stratum label).
    /// If empty, no FPC is applied (sampling with replacement).
    pub popsize: HashMap<String, f64>,
}

/// Survey design specification (post-construction, immutable).
///
/// Fields mirror R's `survey.design2`:
/// - `cluster`: cluster/PSU id per observation (stage 1).
/// - `strata`: stratum label per observation.
/// - `prob`: sampling probability per observation (= 1/weight).
/// - `fpc`: optional population sizes per stratum.
/// - `n_psu`: original number of PSUs per stratum (for subsetting dof).
/// - `lonely_psu`: lonely-PSU handling policy.
#[derive(Debug, Clone)]
pub struct SurveyDesign {
    /// Stratum label for each observation.
    pub strata: Vec<String>,
    /// Cluster/PSU id for each observation (already nested in strata if
    /// the design was constructed with `nest = true`).
    pub cluster: Vec<String>,
    /// Sampling probability for each observation (= 1/weight).
    pub prob: Vec<f64>,
    /// Optional FPC population sizes per stratum.
    pub fpc: Fpc,
    /// Number of PSUs per stratum in the **full** design (before subsetting).
    pub n_psu: HashMap<String, usize>,
    /// Lonely-PSU handling policy.
    pub lonely_psu: LonelyPsu,
    /// Number of observations.
    pub n_obs: usize,
}

impl SurveyDesign {
    /// Construct a survey design from the fundamental components.
    ///
    /// - `strata`: stratum label per observation.
    /// - `cluster`: cluster/PSU id per observation.
    /// - `prob`: sampling probability per observation (1/weight).
    /// - `fpc_popsize`: optional population size per stratum.
    /// - `lonely_psu`: lonely-PSU policy.
    pub fn new(
        strata: Vec<String>,
        cluster: Vec<String>,
        prob: Vec<f64>,
        fpc_popsize: Option<HashMap<String, f64>>,
        lonely_psu: LonelyPsu,
    ) -> Result<Self> {
        let n = strata.len();
        if cluster.len() != n {
            return Err(SurveyError::LengthMismatch {
                context: "cluster vs strata".into(),
                a: cluster.len(),
                b: n,
            });
        }
        if prob.len() != n {
            return Err(SurveyError::LengthMismatch {
                context: "prob vs strata".into(),
                a: prob.len(),
                b: n,
            });
        }
        for &p in &prob {
            if !p.is_finite() || p <= 0.0 {
                return Err(SurveyError::InvalidDesign(format!(
                    "invalid sampling probability: {p}"
                )));
            }
        }

        // Build n_psu: count unique clusters per stratum.
        let mut n_psu: HashMap<String, usize> = HashMap::new();
        let mut seen: HashMap<(String, String), ()> = HashMap::new();
        for i in 0..n {
            let key = (strata[i].clone(), cluster[i].clone());
            seen.entry(key).or_insert_with(|| {
                *n_psu.entry(strata[i].clone()).or_insert(0) += 1;
            });
        }

        let fpc = Fpc {
            popsize: fpc_popsize.unwrap_or_default(),
        };

        Ok(Self {
            strata,
            cluster,
            prob,
            fpc,
            n_psu,
            lonely_psu,
            n_obs: n,
        })
    }

    /// Sampling weights = 1/prob.
    pub fn weights(&self) -> Vec<f64> {
        self.prob.iter().map(|&p| 1.0 / p).collect()
    }

    /// Degrees of freedom: Σ(n_h - 1) over strata.
    pub fn degf(&self) -> usize {
        self.n_psu.values().map(|&n| n.saturating_sub(1)).sum()
    }

    /// Return the unique stratum labels in order of first appearance.
    pub fn strata_labels(&self) -> Vec<String> {
        let mut labels = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for s in &self.strata {
            if seen.insert(s.clone()) {
                labels.push(s.clone());
            }
        }
        labels
    }
}

// =====================================================================
// Builder
// =====================================================================

/// Builder for [`SurveyDesign`] — accepts either weights or probabilities.
pub struct SurveyDesignBuilder {
    pub strata: Vec<String>,
    pub cluster: Vec<String>,
    pub weights: Option<Vec<f64>>,
    pub probs: Option<Vec<f64>>,
    pub fpc_popsize: Option<HashMap<String, f64>>,
    pub lonely_psu: LonelyPsu,
}

impl SurveyDesignBuilder {
    pub fn new() -> Self {
        Self {
            strata: Vec::new(),
            cluster: Vec::new(),
            weights: None,
            probs: None,
            fpc_popsize: None,
            lonely_psu: LonelyPsu::default(),
        }
    }

    pub fn strata(mut self, strata: Vec<String>) -> Self {
        self.strata = strata;
        self
    }

    pub fn cluster(mut self, cluster: Vec<String>) -> Self {
        self.cluster = cluster;
        self
    }

    pub fn weights(mut self, weights: Vec<f64>) -> Self {
        self.weights = Some(weights);
        self
    }

    pub fn probs(mut self, probs: Vec<f64>) -> Self {
        self.probs = Some(probs);
        self
    }

    pub fn fpc_popsize(mut self, fpc: HashMap<String, f64>) -> Self {
        self.fpc_popsize = Some(fpc);
        self
    }

    pub fn lonely_psu(mut self, policy: LonelyPsu) -> Self {
        self.lonely_psu = policy;
        self
    }

    pub fn build(self) -> Result<SurveyDesign> {
        let prob = match (self.probs, self.weights) {
            (Some(p), _) => p,
            (None, Some(w)) => w.iter().map(|&w| 1.0 / w).collect(),
            (None, None) => vec![1.0; self.strata.len()],
        };
        SurveyDesign::new(
            self.strata,
            self.cluster,
            prob,
            self.fpc_popsize,
            self.lonely_psu,
        )
    }
}

impl Default for SurveyDesignBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_from_weights() {
        let d = SurveyDesignBuilder::new()
            .strata(vec!["A".into(), "A".into(), "B".into()])
            .cluster(vec!["1".into(), "2".into(), "1".into()])
            .weights(vec![3.0, 3.0, 4.0])
            .build()
            .unwrap();
        assert_eq!(d.n_obs, 3);
        assert_eq!(d.weights(), vec![3.0, 3.0, 4.0]);
        assert_eq!(d.n_psu.get("A"), Some(&2));
        assert_eq!(d.n_psu.get("B"), Some(&1));
        assert_eq!(d.degf(), 1); // (2-1) + (1-1) = 1
    }

    #[test]
    fn build_from_probs() {
        let d = SurveyDesignBuilder::new()
            .strata(vec!["A".into()])
            .cluster(vec!["1".into()])
            .probs(vec![0.5])
            .build()
            .unwrap();
        assert_eq!(d.weights(), vec![2.0]);
    }
}
