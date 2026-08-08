//! GLM family + link function specifications.
//!
//! Mirrors R's `stats::family` — each family provides the variance function
//! `V(μ)`, the link functions (`linkfun`, `linkinv`, `mu.eta`), initial values
//! for `μ`, and deviance residuals.
//!
//! Used by [`crate::model::svyglm`] to drive the IRLS (iteratively reweighted
//! least squares) algorithm for survey-weighted generalised linear models.

use statrs::distribution::{ContinuousCDF, Normal};

// =====================================================================
// Enums
// =====================================================================

/// GLM family — the random component of the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Gaussian,
    Binomial,
    Poisson,
    Gamma,
    InverseGaussian,
    QuasiBinomial,
    QuasiPoisson,
}

impl Family {
    /// Parse from a string (case-insensitive, matching R family names).
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "gaussian" => Some(Self::Gaussian),
            "binomial" | "quasibinomial" => Some(Self::QuasiBinomial),
            "poisson" | "quasipoisson" => Some(Self::QuasiPoisson),
            "gamma" => Some(Self::Gamma),
            "inverse.gaussian" | "inverse_gaussian" => Some(Self::InverseGaussian),
            _ => None,
        }
    }

    /// Whether this family estimates dispersion from the data (quasi families
    /// and Gaussian / Gamma / Inverse Gaussian).  Binomial and Poisson with
    /// fixed n assume dispersion = 1.
    pub fn estimate_dispersion(&self) -> bool {
        match self {
            Self::Gaussian
            | Self::Gamma
            | Self::InverseGaussian
            | Self::QuasiBinomial
            | Self::QuasiPoisson => true,
            Self::Binomial | Self::Poisson => false,
        }
    }

    /// The canonical (default) link for this family.
    pub fn canonical_link(&self) -> Link {
        match self {
            Self::Gaussian => Link::Identity,
            Self::Binomial | Self::QuasiBinomial => Link::Logit,
            Self::Poisson | Self::QuasiPoisson => Link::Log,
            Self::Gamma => Link::Inverse,
            Self::InverseGaussian => Link::InverseMuSquared,
        }
    }

    /// Resolve a family from a string, mapping "quasibinomial"/"quasipoisson"
    /// to their exact enum variants.
    pub fn parse_exact(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "gaussian" => Some(Self::Gaussian),
            "binomial" => Some(Self::Binomial),
            "quasibinomial" => Some(Self::QuasiBinomial),
            "poisson" => Some(Self::Poisson),
            "quasipoisson" => Some(Self::QuasiPoisson),
            "gamma" => Some(Self::Gamma),
            "inverse.gaussian" | "inverse_gaussian" => Some(Self::InverseGaussian),
            _ => None,
        }
    }
}

/// Link function — the deterministic transformation `g(μ) = η`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    Identity,
    Log,
    Inverse,
    InverseMuSquared, // 1/μ²
    Logit,
    Probit,
    Cloglog,
    Cauchit,
    Sqrt,
}

impl Link {
    /// Parse from a string.
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "identity" => Some(Self::Identity),
            "log" => Some(Self::Log),
            "inverse" => Some(Self::Inverse),
            "1/mu^2" | "inverse-mu-squared" => Some(Self::InverseMuSquared),
            "logit" => Some(Self::Logit),
            "probit" => Some(Self::Probit),
            "cloglog" => Some(Self::Cloglog),
            "cauchit" => Some(Self::Cauchit),
            "sqrt" => Some(Self::Sqrt),
            _ => None,
        }
    }
}

/// Combined family + link specification.
#[derive(Debug, Clone, Copy)]
pub struct FamilySpec {
    pub family: Family,
    pub link: Link,
}

impl FamilySpec {
    /// Create from family + optional link string. If `link` is `None`,
    /// the family's canonical link is used.
    pub fn new(family_name: &str, link: Option<&str>) -> Option<Self> {
        let family = Family::parse_exact(family_name)?;
        let link = match link {
            Some(l) => Link::from_str(l)?,
            None => family.canonical_link(),
        };
        Some(Self { family, link })
    }

    /// Default (canonical) link for the given family.
    pub fn canonical(family: Family) -> Self {
        let link = family.canonical_link();
        Self { family, link }
    }
}

// =====================================================================
// Family functions
// =====================================================================

/// Clamp for fitted values to avoid division by zero.
const EPS: f64 = 1.0e-10;

impl FamilySpec {
    /// Variance function `V(μ)`.
    pub fn variance(&self, mu: f64) -> f64 {
        match self.family {
            Family::Gaussian => 1.0,
            Family::Binomial | Family::QuasiBinomial => {
                let m = mu.clamp(EPS, 1.0 - EPS);
                m * (1.0 - m)
            }
            Family::Poisson | Family::QuasiPoisson => mu.max(EPS),
            Family::Gamma => mu.max(EPS).powi(2),
            Family::InverseGaussian => mu.max(EPS).powi(3),
        }
    }

    /// Link function `g(μ) = η`.
    pub fn linkfun(&self, mu: f64) -> f64 {
        match self.link {
            Link::Identity => mu,
            Link::Log => mu.max(EPS).ln(),
            Link::Inverse => 1.0 / mu.max(EPS),
            Link::InverseMuSquared => 1.0 / mu.max(EPS).powi(2),
            Link::Logit => {
                let m = mu.clamp(EPS, 1.0 - EPS);
                (m / (1.0 - m)).ln()
            }
            Link::Probit => {
                let m = mu.clamp(EPS, 1.0 - EPS);
                normal_inv_cdf(m)
            }
            Link::Cloglog => {
                let m = mu.clamp(EPS, 1.0 - EPS);
                (-((1.0 - m).ln())).ln()
            }
            Link::Cauchit => {
                let m = mu.clamp(EPS, 1.0 - EPS);
                (std::f64::consts::PI * (m - 0.5)).tan()
            }
            Link::Sqrt => mu.max(0.0).sqrt(),
        }
    }

    /// Inverse link `g⁻¹(η) = μ`.
    pub fn linkinv(&self, eta: f64) -> f64 {
        match self.link {
            Link::Identity => eta,
            Link::Log => eta.exp(),
            Link::Inverse => 1.0 / eta.max(EPS),
            Link::InverseMuSquared => (1.0 / eta.max(EPS)).sqrt(),
            Link::Logit => sigmoid(eta),
            Link::Probit => normal_cdf(eta),
            Link::Cloglog => 1.0 - (-eta.exp().max(f64::MIN_POSITIVE)).exp(),
            Link::Cauchit => 0.5 + eta.atan() / std::f64::consts::PI,
            Link::Sqrt => eta.max(0.0).powi(2),
        }
    }

    /// Derivative `dμ/dη = g⁻¹'(η)`.
    pub fn mu_eta(&self, eta: f64) -> f64 {
        match self.link {
            Link::Identity => 1.0,
            Link::Log => eta.exp(),
            Link::Inverse => {
                let e = eta.max(EPS);
                -1.0 / (e * e)
            }
            Link::InverseMuSquared => {
                let e = eta.max(EPS);
                -0.5 / (e * e.abs().sqrt())
            }
            Link::Logit => {
                // d/dη [1/(1+e^{-η})] = e^{-η} / (1+e^{-η})² = μ(1-μ)
                let mu = sigmoid(eta);
                mu * (1.0 - mu)
            }
            Link::Probit => normal_pdf(eta),
            Link::Cloglog => {
                // d/dη [1 - e^{-e^{η}}] = e^{η} · e^{-e^{η}}
                let e = eta.exp();
                e * (-e.max(f64::MIN_POSITIVE)).exp()
            }
            Link::Cauchit => {
                // d/dη [0.5 + arctan(η)/π] = 1/(π(1+η²))
                1.0 / (std::f64::consts::PI * (1.0 + eta * eta))
            }
            Link::Sqrt => 2.0 * eta.max(0.0),
        }
    }

    /// Initialize `μ` from response `y` and prior weights.
    ///
    /// Matches R's `family$initialize` expressions.
    pub fn initialize_mu(&self, y: &[f64], weights: &[f64]) -> Vec<f64> {
        match self.family {
            Family::Gaussian => y.to_vec(),
            Family::Binomial | Family::QuasiBinomial => {
                // mustart = (weights * y + 0.5) / (weights + 1)
                y.iter()
                    .zip(weights)
                    .map(|(&yi, &wi)| (wi * yi + 0.5) / (wi + 1.0))
                    .map(|m| m.clamp(EPS, 1.0 - EPS))
                    .collect()
            }
            Family::Poisson | Family::QuasiPoisson => {
                // mustart = y + 0.1
                y.iter().map(|&yi| yi + 0.1).collect()
            }
            Family::Gamma => {
                // mustart = y, but clamp small values
                y.iter().map(|&yi| yi.max(0.1)).collect()
            }
            Family::InverseGaussian => {
                // mustart = y, clamp
                y.iter().map(|&yi| yi.max(0.1)).collect()
            }
        }
    }

    /// Deviance residual contribution (squared) for one observation.
    ///
    /// Returns the per-observation contribution to the deviance `D = Σ dᵢ`.
    /// This is the actual deviance term, not the signed residual.
    pub fn dev_resid(&self, y: f64, mu: f64, wt: f64) -> f64 {
        let m = mu.max(EPS);
        match self.family {
            Family::Gaussian => wt * (y - mu) * (y - mu),
            Family::Binomial | Family::QuasiBinomial => {
                let mu_c = mu.clamp(EPS, 1.0 - EPS);
                if y == 1.0 {
                    wt * (y / mu_c).ln()
                } else if y == 0.0 {
                    wt * ((1.0 - y) / (1.0 - mu_c)).ln()
                } else {
                    // For binomial with proportions (y in (0,1) with weights > 1)
                    wt * (y * (y / mu_c).ln() + (1.0 - y) * ((1.0 - y) / (1.0 - mu_c)).ln())
                }
            }
            Family::Poisson | Family::QuasiPoisson => {
                if y == 0.0 {
                    wt * m
                } else {
                    wt * (y * (y / m).ln() - (y - m))
                }
            }
            Family::Gamma => {
                // deviance = 2 * wt * (-log(y/mu) + (y-mu)/mu)
                wt * 2.0 * (-((y / m).ln()) + (y - mu) / m)
            }
            Family::InverseGaussian => {
                // deviance = wt * (y-mu)^2 / (mu^2 * y)
                if y <= 0.0 {
                    0.0
                } else {
                    wt * (y - mu) * (y - mu) / (m * m * y)
                }
            }
        }
    }

    /// Validate that the response `y` is valid for this family.
    pub fn validate_y(&self, y: &[f64]) -> std::result::Result<(), String> {
        match self.family {
            Family::Binomial | Family::QuasiBinomial => {
                for (i, &v) in y.iter().enumerate() {
                    if !(0.0..=1.0).contains(&v) {
                        return Err(format!(
                            "y[{i}] = {v} is outside [0,1] for binomial family"
                        ));
                    }
                }
            }
            Family::Poisson | Family::QuasiPoisson => {
                for (i, &v) in y.iter().enumerate() {
                    if v < 0.0 {
                        return Err(format!("y[{i}] = {v} < 0 for poisson family"));
                    }
                }
            }
            Family::Gamma => {
                for (i, &v) in y.iter().enumerate() {
                    if v <= 0.0 {
                        return Err(format!("y[{i}] = {v} <= 0 for Gamma family"));
                    }
                }
            }
            Family::InverseGaussian => {
                for (i, &v) in y.iter().enumerate() {
                    if v <= 0.0 {
                        return Err(format!(
                            "y[{i}] = {v} <= 0 for inverse.gaussian family"
                        ));
                    }
                }
            }
            Family::Gaussian => {}
        }
        Ok(())
    }
}

// =====================================================================
// Helper functions
// =====================================================================

/// Logistic sigmoid `1/(1+e^{-x})` — numerically stable.
fn sigmoid(x: f64) -> f64 {
    if x >= 0.0 {
        1.0 / (1.0 + (-x).exp())
    } else {
        let e = x.exp();
        e / (1.0 + e)
    }
}

/// Standard normal CDF `Φ(x)`.
fn normal_cdf(x: f64) -> f64 {
    Normal::new(0.0, 1.0)
        .map(|d| d.cdf(x))
        .unwrap_or(if x > 0.0 { 1.0 } else { 0.0 })
}

/// Standard normal PDF `φ(x)`.
fn normal_pdf(x: f64) -> f64 {
    (-0.5 * x * x).exp() / (2.0 * std::f64::consts::PI).sqrt()
}

/// Standard normal inverse CDF `Φ⁻¹(p)` via rational approximation
/// (Acklam's algorithm — accurate to ~1e-9).
fn normal_inv_cdf(p: f64) -> f64 {
    let p = p.clamp(1e-15, 1.0 - 1e-15);
    // Acklam's inverse normal CDF approximation.
    let a = [
        -3.969683028665376e+01,
        2.209460984245205e+02,
        -2.759285104469687e+02,
        1.383_577_518_672_69e2,
        -3.066479806614716e+01,
        2.506628277459239e+00,
    ];
    let b = [
        -5.447609879822406e+01,
        1.615858368580409e+02,
        -1.556989798598866e+02,
        6.680131188771972e+01,
        -1.328068155288572e+01,
    ];
    let c = [
        -7.784894002430293e-03,
        -3.223964580411365e-01,
        -2.400758277161838e+00,
        -2.549732539343734e+00,
        4.374664141464968e+00,
        2.938163982698783e+00,
    ];
    let d = [
        7.784695709041462e-03,
        3.224671290700398e-01,
        2.445134137142996e+00,
        3.754408661907416e+00,
    ];

    let plow = 0.02425;
    let phigh = 1.0 - plow;

    if p < plow {
        let q = (-2.0 * p.ln()).sqrt();
        let r = ((((c[0] * q + c[1]) * q + c[2]) * q + c[3]) * q + c[4]) * q + c[5];
        let s = (((d[0] * q + d[1]) * q + d[2]) * q + d[3]) * q + 1.0;
        r / s
    } else if p <= phigh {
        let q = p - 0.5;
        let r = q * q;
        let num = ((((a[0] * r + a[1]) * r + a[2]) * r + a[3]) * r + a[4]) * r + a[5];
        let den = ((((b[0] * r + b[1]) * r + b[2]) * r + b[3]) * r + b[4]) * r + 1.0;
        q * num / den
    } else {
        let q = (-2.0 * (1.0 - p).ln()).sqrt();
        let num = ((((c[0] * q + c[1]) * q + c[2]) * q + c[3]) * q + c[4]) * q + c[5];
        let den = (((d[0] * q + d[1]) * q + d[2]) * q + d[3]) * q + 1.0;
        -num / den
    }
}

// =====================================================================
// Tests
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn family_parse() {
        assert_eq!(Family::parse_exact("gaussian"), Some(Family::Gaussian));
        assert_eq!(Family::parse_exact("binomial"), Some(Family::Binomial));
        assert_eq!(
            Family::parse_exact("quasibinomial"),
            Some(Family::QuasiBinomial)
        );
        assert_eq!(
            Family::parse_exact("inverse.gaussian"),
            Some(Family::InverseGaussian)
        );
        assert_eq!(Family::parse_exact("Gamma"), Some(Family::Gamma));
    }

    #[test]
    fn link_parse() {
        assert_eq!(Link::from_str("identity"), Some(Link::Identity));
        assert_eq!(Link::from_str("logit"), Some(Link::Logit));
        assert_eq!(Link::from_str("cloglog"), Some(Link::Cloglog));
        assert_eq!(Link::from_str("1/mu^2"), Some(Link::InverseMuSquared));
    }

    #[test]
    fn canonical_links() {
        assert_eq!(Family::Gaussian.canonical_link(), Link::Identity);
        assert_eq!(Family::Binomial.canonical_link(), Link::Logit);
        assert_eq!(Family::Poisson.canonical_link(), Link::Log);
        assert_eq!(Family::Gamma.canonical_link(), Link::Inverse);
        assert_eq!(
            Family::InverseGaussian.canonical_link(),
            Link::InverseMuSquared
        );
    }

    #[test]
    fn identity_link_roundtrip() {
        let spec = FamilySpec::canonical(Family::Gaussian);
        for &mu in &[0.5, 1.0, 2.0, 100.0] {
            let eta = spec.linkfun(mu);
            assert!((spec.linkinv(eta) - mu).abs() < 1e-10);
        }
    }

    #[test]
    fn log_link_roundtrip() {
        let spec = FamilySpec::new("poisson", Some("log")).unwrap();
        for &mu in &[0.1, 1.0, 5.0, 100.0] {
            let eta = spec.linkfun(mu);
            assert!((spec.linkinv(eta) - mu).abs() < 1e-10);
        }
    }

    #[test]
    fn logit_link_roundtrip() {
        let spec = FamilySpec::new("binomial", Some("logit")).unwrap();
        for &mu in &[0.1, 0.3, 0.5, 0.7, 0.9] {
            let eta = spec.linkfun(mu);
            assert!((spec.linkinv(eta) - mu).abs() < 1e-8);
        }
    }

    #[test]
    fn probit_link_roundtrip() {
        let spec = FamilySpec::new("binomial", Some("probit")).unwrap();
        for &mu in &[0.1, 0.25, 0.5, 0.75, 0.9] {
            let eta = spec.linkfun(mu);
            let mu_back = spec.linkinv(eta);
            assert!((mu_back - mu).abs() < 1e-6, "mu={mu}, back={mu_back}");
        }
    }

    #[test]
    fn cloglog_link_roundtrip() {
        let spec = FamilySpec::new("binomial", Some("cloglog")).unwrap();
        for &mu in &[0.1, 0.25, 0.5, 0.75, 0.9] {
            let eta = spec.linkfun(mu);
            assert!((spec.linkinv(eta) - mu).abs() < 1e-6, "mu={mu}");
        }
    }

    #[test]
    fn cauchit_link_roundtrip() {
        let spec = FamilySpec::new("binomial", Some("cauchit")).unwrap();
        for &mu in &[0.2, 0.4, 0.5, 0.6, 0.8] {
            let eta = spec.linkfun(mu);
            assert!((spec.linkinv(eta) - mu).abs() < 1e-6, "mu={mu}");
        }
    }

    #[test]
    fn inverse_link_roundtrip() {
        let spec = FamilySpec::new("Gamma", Some("inverse")).unwrap();
        for &mu in &[0.5, 1.0, 5.0, 100.0] {
            let eta = spec.linkfun(mu);
            assert!((spec.linkinv(eta) - mu).abs() < 1e-6, "mu={mu}");
        }
    }

    #[test]
    fn sqrt_link_roundtrip() {
        let spec = FamilySpec::new("poisson", Some("sqrt")).unwrap();
        for &mu in &[0.0, 1.0, 4.0, 100.0] {
            let eta = spec.linkfun(mu);
            assert!((spec.linkinv(eta) - mu).abs() < 1e-8, "mu={mu}");
        }
    }

    #[test]
    fn inverse_mu_squared_link() {
        let spec = FamilySpec::new("inverse.gaussian", None).unwrap();
        for &mu in &[0.5, 1.0, 5.0, 100.0] {
            let eta = spec.linkfun(mu);
            let mu_back = spec.linkinv(eta);
            assert!((mu_back - mu).abs() < 1e-4, "mu={mu}, back={mu_back}");
        }
    }

    #[test]
    fn variance_functions() {
        let g = FamilySpec::canonical(Family::Gaussian);
        assert_eq!(g.variance(5.0), 1.0);

        let b = FamilySpec::canonical(Family::Binomial);
        assert!((b.variance(0.3) - 0.21).abs() < 1e-10);

        let p = FamilySpec::canonical(Family::Poisson);
        assert!((p.variance(5.0) - 5.0).abs() < 1e-10);

        let ga = FamilySpec::canonical(Family::Gamma);
        assert!((ga.variance(3.0) - 9.0).abs() < 1e-10);

        let ig = FamilySpec::canonical(Family::InverseGaussian);
        assert!((ig.variance(2.0) - 8.0).abs() < 1e-10);
    }

    #[test]
    fn gaussian_dev_resid() {
        let s = FamilySpec::canonical(Family::Gaussian);
        // deviance = wt * (y - mu)^2
        let d = s.dev_resid(3.0, 1.0, 2.0);
        assert!((d - 2.0 * 4.0).abs() < 1e-10);
    }

    #[test]
    fn poisson_dev_resid() {
        let s = FamilySpec::canonical(Family::Poisson);
        // y=0: deviance contribution = wt * mu
        assert!((s.dev_resid(0.0, 2.0, 1.0) - 2.0).abs() < 1e-10);
        // y=5, mu=3: wt * [y*log(y/mu) - (y-mu)]
        let d = s.dev_resid(5.0, 3.0, 1.0);
        let expected: f64 = 1.0 * (5.0 * (5.0_f64 / 3.0).ln() - (5.0 - 3.0));
        assert!((d - expected).abs() < 1e-10);
    }

    #[test]
    fn gamma_dev_resid() {
        let s = FamilySpec::canonical(Family::Gamma);
        let d = s.dev_resid(2.0, 3.0, 1.0);
        let expected: f64 = 1.0 * 2.0 * (-((2.0_f64 / 3.0).ln()) + (2.0 - 3.0) / 3.0);
        assert!((d - expected).abs() < 1e-10);
    }

    #[test]
    fn binomial_initialize() {
        let s = FamilySpec::canonical(Family::Binomial);
        let y = vec![0.0, 1.0, 0.0, 1.0];
        let w = vec![1.0; 4];
        let mu = s.initialize_mu(&y, &w);
        // mustart = (w*y + 0.5)/(w+1) = 0.25 or 0.75
        assert!((mu[0] - 0.25).abs() < 1e-10);
        assert!((mu[1] - 0.75).abs() < 1e-10);
    }

    #[test]
    fn poisson_initialize() {
        let s = FamilySpec::canonical(Family::Poisson);
        let y = vec![0.0, 1.0, 5.0];
        let w = vec![1.0; 3];
        let mu = s.initialize_mu(&y, &w);
        assert!((mu[0] - 0.1).abs() < 1e-10);
        assert!((mu[1] - 1.1).abs() < 1e-10);
        assert!((mu[2] - 5.1).abs() < 1e-10);
    }

    #[test]
    fn validate_y_rejects_invalid() {
        let s = FamilySpec::canonical(Family::Poisson);
        assert!(s.validate_y(&[-1.0, 2.0]).is_err());
        assert!(s.validate_y(&[1.0, 2.0]).is_ok());

        let s = FamilySpec::canonical(Family::Gamma);
        assert!(s.validate_y(&[0.0, 1.0]).is_err());
        assert!(s.validate_y(&[1.0, 2.0]).is_ok());
    }
}
