#[test]
fn xval_binomial_probit() {
    if !std::path::Path::new("/tmp/svyglm_binom.csv").exists() { return; }
    let (y, x, dnum) = read_csv("/tmp/svyglm_binom.csv");
    let n = y.len();
    let design = make_design(n, &dnum);
    let spec = FamilySpec::new("binomial", Some("probit")).unwrap();
    let fit = svyglm(&y, &[x], &design, true, None, &spec, None).unwrap();
    let se = fit.se();

    // R golden: (Intercept) 0.4277949 SE 0.11403232
    //           x           0.5526210 SE 0.08055959
    assert!((fit.coefficients[0] - 0.4277949).abs() < 0.01,
        "probit intercept: {} (R=0.4278)", fit.coefficients[0]);
    assert!((se[0] - 0.11403232).abs() < 0.01,
        "probit intercept SE: {} (R=0.1140)", se[0]);
    assert!((fit.coefficients[1] - 0.5526210).abs() < 0.01,
        "probit slope: {} (R=0.5526)", fit.coefficients[1]);
    assert!((se[1] - 0.08055959).abs() < 0.01,
        "probit slope SE: {} (R=0.08056)", se[1]);
    eprintln!("Binomial probit xval PASSED ✓");
}

#[test]
fn xval_binomial_cloglog() {
    if !std::path::Path::new("/tmp/svyglm_binom.csv").exists() { return; }
    let (y, x, dnum) = read_csv("/tmp/svyglm_binom.csv");
    let n = y.len();
    let design = make_design(n, &dnum);
    let spec = FamilySpec::new("binomial", Some("cloglog")).unwrap();
    let fit = svyglm(&y, &[x], &design, true, None, &spec, None).unwrap();
    let se = fit.se();

    // R golden: (Intercept) 0.007737354 SE 0.11451197
    //           x           0.587638523 SE 0.08139086
    assert!((fit.coefficients[0] - 0.007737354).abs() < 0.02,
        "cloglog intercept: {} (R=0.00774)", fit.coefficients[0]);
    assert!((se[0] - 0.11451197).abs() < 0.02,
        "cloglog intercept SE: {} (R=0.11451)", se[0]);
    assert!((fit.coefficients[1] - 0.587638523).abs() < 0.02,
        "cloglog slope: {} (R=0.5876)", fit.coefficients[1]);
    assert!((se[1] - 0.08139086).abs() < 0.02,
        "cloglog slope SE: {} (R=0.08139)", se[1]);
    eprintln!("Binomial cloglog xval PASSED ✓");
}

#[test]
fn xval_poisson_identity() {
    if !std::path::Path::new("/tmp/svyglm_pois.csv").exists() { return; }
    let (y, x, dnum) = read_csv("/tmp/svyglm_pois.csv");
    let n = y.len();
    let design = make_design(n, &dnum);
    let spec = FamilySpec::new("poisson", Some("identity")).unwrap();
    let fit = svyglm(&y, &[x], &design, true, None, &spec, None).unwrap();
    let se = fit.se();

    // R golden: (Intercept) 1.5516131 SE 0.17405643
    //           x           0.1875529 SE 0.03842954
    assert!((fit.coefficients[0] - 1.5516131).abs() < 0.02,
        "poisson-identity intercept: {} (R=1.5516)", fit.coefficients[0]);
    assert!((se[0] - 0.17405643).abs() < 0.02,
        "poisson-identity intercept SE: {} (R=0.1741)", se[0]);
    assert!((fit.coefficients[1] - 0.1875529).abs() < 0.02,
        "poisson-identity slope: {} (R=0.1876)", fit.coefficients[1]);
    assert!((se[1] - 0.03842954).abs() < 0.02,
        "poisson-identity slope SE: {} (R=0.03843)", se[1]);
    eprintln!("Poisson identity xval PASSED ✓");
}
// Run with: cargo test --test xval_families -- --nocapture
// Requires: /tmp/svyglm_binom.csv, /tmp/svyglm_pois.csv, /tmp/svyglm_gamma.csv

use survey::*;
use std::collections::HashMap;

fn read_csv(path: &str) -> (Vec<f64>, Vec<f64>, Vec<String>) {
    let data = std::fs::read_to_string(path).unwrap();
    let mut lines = data.lines();
    let header = lines.next().unwrap();
    let idx: HashMap<&str, usize> = header.split(',')
        .enumerate()
        .map(|(i, n)| (n.trim_matches('"'), i))
        .collect();
    let mut y = Vec::new();
    let mut x = Vec::new();
    let mut dnum = Vec::new();
    for line in lines {
        let cols: Vec<&str> = line.split(',').collect();
        y.push(cols[idx["y"]].parse::<f64>().unwrap());
        x.push(cols[idx["x"]].parse::<f64>().unwrap());
        dnum.push(cols[idx["dnum"]].trim_matches('"').to_string());
    }
    (y, x, dnum)
}

fn make_design(n: usize, dnum: &[String]) -> SurveyDesign {
    SurveyDesignBuilder::new()
        .strata(vec!["1".to_string(); n])
        .cluster(dnum.to_vec())
        .weights(vec![1.0; n])
        .lonely_psu(LonelyPsu::Remove)
        .build()
        .unwrap()
}

#[test]
fn xval_binomial_logit() {
    if !std::path::Path::new("/tmp/svyglm_binom.csv").exists() { return; }
    let (y, x, dnum) = read_csv("/tmp/svyglm_binom.csv");
    let n = y.len();
    let design = make_design(n, &dnum);
    let spec = FamilySpec::canonical(Family::Binomial);
    let fit = svyglm(&y, &[x], &design, true, None, &spec, None).unwrap();
    let se = fit.se();

    // R golden: (Intercept) 0.7070138 SE 0.1998821
    //           x           0.9154584 SE 0.1481062
    assert!((fit.coefficients[0] - 0.7070138).abs() < 0.01,
        "intercept: {} (R=0.707)", fit.coefficients[0]);
    assert!((se[0] - 0.1998821).abs() < 0.01,
        "intercept SE: {} (R=0.1999)", se[0]);
    assert!((fit.coefficients[1] - 0.9154584).abs() < 0.01,
        "slope: {} (R=0.9155)", fit.coefficients[1]);
    assert!((se[1] - 0.1481062).abs() < 0.01,
        "slope SE: {} (R=0.1481)", se[1]);
    eprintln!("Binomial xval PASSED ✓");
}

#[test]
fn xval_poisson_log() {
    if !std::path::Path::new("/tmp/svyglm_pois.csv").exists() { return; }
    let (y, x, dnum) = read_csv("/tmp/svyglm_pois.csv");
    let n = y.len();
    let design = make_design(n, &dnum);
    let spec = FamilySpec::canonical(Family::Poisson);
    let fit = svyglm(&y, &[x], &design, true, None, &spec, None).unwrap();
    let se = fit.se();

    // R golden: (Intercept) 0.48679905 SE 0.08042010
    //           x           0.07981035 SE 0.01313388
    assert!((fit.coefficients[0] - 0.48679905).abs() < 0.01,
        "intercept: {} (R=0.4868)", fit.coefficients[0]);
    assert!((se[0] - 0.08042010).abs() < 0.01,
        "intercept SE: {} (R=0.08042)", se[0]);
    assert!((fit.coefficients[1] - 0.07981035).abs() < 0.005,
        "slope: {} (R=0.07981)", fit.coefficients[1]);
    assert!((se[1] - 0.01313388).abs() < 0.005,
        "slope SE: {} (R=0.01313)", se[1]);
    eprintln!("Poisson xval PASSED ✓");
}

#[test]
fn xval_gamma_log() {
    if !std::path::Path::new("/tmp/svyglm_gamma.csv").exists() { return; }
    let (y, x, dnum) = read_csv("/tmp/svyglm_gamma.csv");
    let n = y.len();
    let design = make_design(n, &dnum);
    let spec = FamilySpec::new("Gamma", Some("log")).unwrap();
    let fit = svyglm(&y, &[x], &design, true, None, &spec, None).unwrap();

    // R golden: (Intercept) 1.0, x 0.5 (no noise → exact)
    assert!((fit.coefficients[0] - 1.0).abs() < 1e-6,
        "intercept: {} (R=1.0)", fit.coefficients[0]);
    assert!((fit.coefficients[1] - 0.5).abs() < 1e-6,
        "slope: {} (R=0.5)", fit.coefficients[1]);
    eprintln!("Gamma xval PASSED ✓");
}
