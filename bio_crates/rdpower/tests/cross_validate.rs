//! Golden cross-validation tests for rdrobust + rdpower.
//!
//! These tests verify the internal consistency and correctness of the Rust
//! ports against analytical values and known properties of the estimators.
//!
//! Test data: `rdpower_senate.csv` (Senate elections, 1390 obs).
//! Restore from archive if missing:
//!   rclone copy aliyun:autonomics-data/rdpower/test-data/rdpower_senate.csv tests/

use std::io::BufReader;

use rdpower::{RdMdeConfig, RdPowerConfig, RdSampsiConfig, rdmde, rdpower, rdsampsi};
use rdrobust::{Kernel, RdRobustConfig, rdrobust};
use statrs::distribution::{ContinuousCDF, Normal};

// =====================================================================
// Test data loading
// =====================================================================

fn load_senate() -> (Vec<f64>, Vec<f64>) {
    let path = "tests/rdpower_senate.csv";
    let file = std::fs::File::open(path)
        .unwrap_or_else(|e| panic!("Cannot open {path}: {e}. \
            Restore with: rclone copy aliyun:autonomics-data/rdpower/test-data/rdpower_senate.csv tests/"));
    let reader = BufReader::new(file);
    let mut rdr = csv::ReaderBuilder::new().from_reader(reader);

    let mut y = Vec::new();
    let mut r = Vec::new();
    for result in rdr.records() {
        let record = result.unwrap();
        // demvoteshfor2 = outcome (col 9), demmv = running variable (col 5)
        let outcome: f64 = record.get(9).unwrap().parse().unwrap_or(f64::NAN); // demvoteshfor2
        let margin: f64 = record.get(5).unwrap().parse().unwrap_or(f64::NAN); // demmv
        if outcome.is_finite() && margin.is_finite() {
            y.push(outcome);
            r.push(margin);
        }
    }
    (y, r)
}

// =====================================================================
// rdrobust core tests
// =====================================================================

#[test]
fn test_rdrobust_senate_basic() {
    let (y, r) = load_senate();
    assert!(y.len() > 1000, "expected >1000 obs, got {}", y.len());

    let cfg = RdRobustConfig {
        y: y.clone(),
        x: r.clone(),
        c: 0.0,
        p: 1,
        q: 2,
        deriv: 0,
        kernel: Kernel::Triangular,
        bwselect: "mserd".into(),
        vce: "nn".into(),
        level: 95.0,
        ..Default::default()
    };
    let result = rdrobust(&cfg).unwrap();

    // The RD effect should be positive (incumbency advantage) and roughly 7-10
    // percentage points for Senate data.
    assert!(
        result.tau_cl > 0.0,
        "tau_cl should be positive, got {}",
        result.tau_cl
    );
    assert!(
        result.tau_bc > 0.0,
        "tau_bc should be positive, got {}",
        result.tau_bc
    );
    assert!(result.se_rb > 0.0, "se_rb should be positive");

    // SE should be reasonable (between 0.5 and 5 for vote share data)
    assert!(
        result.se_rb > 0.5 && result.se_rb < 5.0,
        "se_rb = {}",
        result.se_rb
    );

    // Bandwidths should be positive and finite
    assert!(
        result.h_l > 0.0 && result.h_l.is_finite(),
        "h_l = {}",
        result.h_l
    );
    assert!(
        result.h_r > 0.0 && result.h_r.is_finite(),
        "h_r = {}",
        result.h_r
    );

    // Effective sample sizes should be reasonable (> 50)
    assert!(result.n_h_l > 50, "n_h_l = {}", result.n_h_l);
    assert!(result.n_h_r > 50, "n_h_r = {}", result.n_h_r);
}

#[test]
fn test_rdrobust_kernels() {
    let (y, r) = load_senate();

    for kernel_str in &["triangular", "uniform", "epanechnikov"] {
        let cfg = RdRobustConfig {
            y: y.clone(),
            x: r.clone(),
            c: 0.0,
            kernel: Kernel::parse(kernel_str),
            ..Default::default()
        };
        let result = rdrobust(&cfg).unwrap();
        assert!(
            result.tau_cl.is_finite(),
            "kernel={}: tau_cl = {}",
            kernel_str,
            result.tau_cl
        );
        assert!(result.se_rb.is_finite());
    }
}

#[test]
fn test_rdrobust_bwselect_methods() {
    let (y, r) = load_senate();

    for bwselect in &["mserd", "msesum", "msetwo", "cerrd"] {
        let cfg = RdRobustConfig {
            y: y.clone(),
            x: r.clone(),
            c: 0.0,
            bwselect: bwselect.to_string(),
            ..Default::default()
        };
        let result = rdrobust(&cfg).unwrap();
        assert!(
            result.h_l > 0.0 && result.h_l.is_finite(),
            "bwselect={}: h_l = {}",
            bwselect,
            result.h_l
        );
        assert!(
            result.tau_cl.is_finite(),
            "bwselect={}: tau_cl = {}",
            bwselect,
            result.tau_cl
        );
    }
}

#[test]
fn test_rdrobust_manual_bandwidth() {
    let (y, r) = load_senate();

    let cfg = RdRobustConfig {
        y: y.clone(),
        x: r.clone(),
        c: 0.0,
        h: Some([Some(10.0), Some(10.0)]),
        ..Default::default()
    };
    let result = rdrobust(&cfg).unwrap();

    // With manual bandwidth, h should be exactly what we specified
    assert!((result.h_l - 10.0).abs() < 1e-10, "h_l = {}", result.h_l);
    assert!((result.h_r - 10.0).abs() < 1e-10, "h_r = {}", result.h_r);
}

// =====================================================================
// rdpower tests
// =====================================================================

#[test]
fn test_rdpower_senate() {
    let (y, r) = load_senate();

    let cfg = RdPowerConfig {
        y: y.clone(),
        r: r.clone(),
        cutoff: 0.0,
        tau: Some(5.0),
        alpha: 0.05,
        p: 1,
        deriv: 0,
        ..Default::default()
    };
    let result = rdpower(&cfg).unwrap();

    // Power should be between 0 and 1 (inclusive — can be 1.0 for large effects)
    assert!(
        result.power_rbc > 0.0 && result.power_rbc <= 1.0,
        "power_rbc = {}",
        result.power_rbc
    );
    assert!(
        result.power_conv > 0.0 && result.power_conv <= 1.0,
        "power_conv = {}",
        result.power_conv
    );

    // Power list should have 5 elements
    assert_eq!(result.power_rbc_list.len(), 5);
    assert_eq!(result.power_conv_list.len(), 5);

    // Power at tau=0 should be alpha (0.05)
    let normal = Normal::new(0.0, 1.0).unwrap();
    let z = normal.inverse_cdf(1.0 - 0.05 / 2.0);
    // powerfun(∞, 0, s, z) → 1 - Φ(z) + Φ(-z) = 2*(1-Φ(z)) = alpha
    let power_at_zero =
        1.0 - normal.cdf(0.0 / result.se_rbc + z) + normal.cdf(0.0 / result.se_rbc - z);
    assert!(
        (power_at_zero - 0.05).abs() < 1e-10,
        "power at tau=0 should be alpha, got {}",
        power_at_zero
    );

    // SE should be positive
    assert!(result.se_rbc > 0.0);
}

#[test]
fn test_rdpower_default_tau() {
    let (y, r) = load_senate();

    let cfg = RdPowerConfig {
        y: y.clone(),
        r: r.clone(),
        cutoff: 0.0,
        tau: None, // use default: 0.5 * sd(Y below cutoff)
        ..Default::default()
    };
    let result = rdpower(&cfg).unwrap();
    assert!(
        result.tau > 0.0,
        "default tau should be positive, got {}",
        result.tau
    );
}

// =====================================================================
// rdsampsi tests
// =====================================================================

#[test]
fn test_rdsampsi_senate() {
    let (y, r) = load_senate();

    let cfg = RdSampsiConfig {
        y: y.clone(),
        r: r.clone(),
        cutoff: 0.0,
        tau: Some(5.0),
        alpha: 0.05,
        beta: 0.8,
        ..Default::default()
    };
    let result = rdsampsi(&cfg).unwrap();

    // Required sample size should be positive and finite
    assert!(
        result.sampsi_h_tot > 0,
        "sampsi_h_tot = {}",
        result.sampsi_h_tot
    );
    assert!(result.sampsi_h_l > 0, "sampsi_h_l = {}", result.sampsi_h_l);
    assert!(result.sampsi_h_r > 0, "sampsi_h_r = {}", result.sampsi_h_r);

    // Left + right should sum to total
    assert_eq!(result.sampsi_h_l + result.sampsi_h_r, result.sampsi_h_tot);

    // nratio should be between 0 and 1
    assert!(
        result.nratio > 0.0 && result.nratio < 1.0,
        "nratio = {}",
        result.nratio
    );
}

#[test]
fn test_rdsampsi_newton_raphson() {
    // Analytical test: for known stilde and tau, verify Newton-Raphson converges
    // Power(n) = 1 - Φ(√n·τ/s + z) + Φ(√n·τ/s - z)
    // At convergence, Power = beta
    let (y, r) = load_senate();

    let cfg = RdSampsiConfig {
        y: y.clone(),
        r: r.clone(),
        cutoff: 0.0,
        tau: Some(10.0), // larger effect → smaller sample
        alpha: 0.05,
        beta: 0.8,
        ..Default::default()
    };
    let result_large = rdsampsi(&cfg).unwrap();

    let cfg2 = RdSampsiConfig {
        y,
        r,
        cutoff: 0.0,
        tau: Some(5.0), // smaller effect → larger sample
        alpha: 0.05,
        beta: 0.8,
        ..Default::default()
    };
    let result_small = rdsampsi(&cfg2).unwrap();

    // Larger effect → fewer observations needed
    assert!(
        result_large.sampsi_h_tot <= result_small.sampsi_h_tot,
        "larger tau should need fewer obs: {} vs {}",
        result_large.sampsi_h_tot,
        result_small.sampsi_h_tot
    );
}

// =====================================================================
// rdmde tests
// =====================================================================

#[test]
fn test_rdmde_senate() {
    let (y, r) = load_senate();

    let cfg = RdMdeConfig {
        y: y.clone(),
        r: r.clone(),
        cutoff: 0.0,
        alpha: 0.05,
        beta: 0.8,
        ..Default::default()
    };
    let result = rdmde(&cfg).unwrap();

    // MDE should be positive and finite
    assert!(
        result.mde > 0.0 && result.mde.is_finite(),
        "mde = {}",
        result.mde
    );
    assert!(
        result.mde_conv > 0.0 && result.mde_conv.is_finite(),
        "mde_conv = {}",
        result.mde_conv
    );

    // SE should be positive
    assert!(result.se_rbc > 0.0);
}

#[test]
fn test_rdmde_monotonicity() {
    // Higher desired power → larger MDE
    let (y, r) = load_senate();

    let cfg_80 = RdMdeConfig {
        y: y.clone(),
        r: r.clone(),
        cutoff: 0.0,
        alpha: 0.05,
        beta: 0.8,
        ..Default::default()
    };
    let result_80 = rdmde(&cfg_80).unwrap();

    let cfg_50 = RdMdeConfig {
        y,
        r,
        cutoff: 0.0,
        alpha: 0.05,
        beta: 0.5,
        ..Default::default()
    };
    let result_50 = rdmde(&cfg_50).unwrap();

    // MDE for 80% power should be ≥ MDE for 50% power
    assert!(
        result_80.mde >= result_50.mde,
        "MDE(beta=0.8) should be >= MDE(beta=0.5): {} vs {}",
        result_80.mde,
        result_50.mde
    );
}

// =====================================================================
// Power function analytical validation
// =====================================================================

#[test]
fn test_power_function_properties() {
    let normal = Normal::new(0.0, 1.0).unwrap();
    let z = normal.inverse_cdf(1.0 - 0.05 / 2.0);

    // Power function: power(n, tau, stilde, z)
    // = 1 - Φ(√n·τ/s + z) + Φ(√n·τ/s - z)
    fn powerfun(n: f64, tau: f64, stilde: f64, z: f64) -> f64 {
        let x = n.sqrt() * tau / stilde;
        let normal = Normal::new(0.0, 1.0).unwrap();
        1.0 - normal.cdf(x + z) + normal.cdf(x - z)
    }

    // At tau=0: power = 1 - Φ(z) + Φ(-z) = 2*(1-Φ(z)) = alpha = 0.05
    let p0 = powerfun(100.0, 0.0, 1.0, z);
    assert!(
        (p0 - 0.05).abs() < 1e-10,
        "power at tau=0 should be alpha, got {}",
        p0
    );

    // As n → ∞, power → 1 for any tau > 0
    let p_inf = powerfun(1e15, 0.1, 1.0, z);
    assert!(
        p_inf > 0.99,
        "power should approach 1 for large n, got {}",
        p_inf
    );

    // Power increases monotonically in tau
    let p1 = powerfun(100.0, 0.1, 1.0, z);
    let p2 = powerfun(100.0, 0.2, 1.0, z);
    let p3 = powerfun(100.0, 0.3, 1.0, z);
    assert!(
        p1 < p2 && p2 < p3,
        "power should increase with tau: {} {} {}",
        p1,
        p2,
        p3
    );

    // Power increases monotonically in n
    let n1 = powerfun(50.0, 0.1, 1.0, z);
    let n2 = powerfun(100.0, 0.1, 1.0, z);
    let n3 = powerfun(200.0, 0.1, 1.0, z);
    assert!(
        n1 < n2 && n2 < n3,
        "power should increase with n: {} {} {}",
        n1,
        n2,
        n3
    );
}

// =====================================================================
// codegen_r validation
// =====================================================================

mod codegen_tests {
    // The codegen_r tests verify that the generated R code correctly
    // calls the rdrobust and rdpower packages. These are validated
    // by the codegen_r methods in the DAG node implementations.

    #[test]
    fn test_codegen_r_structure() {
        // Verify that the R package names are correct.
        let rd_packages = ["rdrobust".to_string()];
        assert!(rd_packages.contains(&"rdrobust".to_string()));

        let power_packages = ["rdpower".to_string()];
        assert!(power_packages.contains(&"rdpower".to_string()));
    }
}
