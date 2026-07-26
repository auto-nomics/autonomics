//! 诊断：扫描 bivariate cost 沿 pi12（固定 rg、rho_zero），看山脊形状。
//! `cargo test -p mixer --test scan_pi12 -- --nocapture`
use flate2::read::GzDecoder;
use mixer::bivariate::cost::bivariate_cost_gaussian;
use mixer::bivariate::{BivariateData, BivariateParams, UnivariateConstraint};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

fn read_sumstats(path: &Path) -> HashMap<String, (f64, f64)> {
    let f = File::open(path).unwrap();
    let mut lines = BufReader::new(GzDecoder::new(f)).lines();
    let header = lines.next().unwrap().unwrap();
    let cols: Vec<&str> = header.split_whitespace().collect();
    let (i_snp, i_z, i_n) = (
        cols.iter().position(|c| *c == "SNP").unwrap(),
        cols.iter().position(|c| *c == "Z").unwrap(),
        cols.iter().position(|c| *c == "N").unwrap(),
    );
    let mut m = HashMap::new();
    for line in lines {
        let line = line.unwrap();
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() <= i_z.max(i_n) {
            continue;
        }
        m.insert(
            f[i_snp].to_string(),
            (f[i_z].parse().unwrap(), f[i_n].parse().unwrap()),
        );
    }
    m
}
fn read_af(path: &Path) -> HashMap<String, f64> {
    let lines = BufReader::new(GzDecoder::new(File::open(path).unwrap())).lines();
    let mut m = HashMap::new();
    for line in lines.skip(1) {
        let line = line.unwrap();
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() >= 3 {
            m.insert(f[1].to_string(), f[2].parse().unwrap());
        }
    }
    m
}
fn read_ld(path: &Path) -> Vec<(String, String, f64)> {
    let lines = BufReader::new(GzDecoder::new(File::open(path).unwrap())).lines();
    let mut out = Vec::new();
    for line in lines.skip(1) {
        let line = line.unwrap();
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() >= 5 {
            out.push((f[1].to_string(), f[2].to_string(), f[4].parse().unwrap()));
        }
    }
    out
}
fn read_bivar_weights(path: &Path) -> HashMap<String, (f64, f64, f64)> {
    let lines = BufReader::new(GzDecoder::new(File::open(path).unwrap())).lines();
    let mut m = HashMap::new();
    for line in lines.skip(1) {
        let line = line.unwrap();
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() >= 4 {
            m.insert(
                f[0].to_string(),
                (
                    f[1].parse().unwrap(),
                    f[2].parse().unwrap(),
                    f[3].parse().unwrap(),
                ),
            );
        }
    }
    m
}
fn read_constraint(path: &Path) -> UnivariateConstraint {
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let p = &v["params"];
    UnivariateConstraint {
        pi: p["pi"].as_f64().unwrap(),
        sig2_beta: p["sig2_beta"].as_f64().unwrap(),
        sig2_zero: p["sig2_zero"].as_f64().unwrap(),
    }
}

#[test]
fn scan_pi12_along_rg() {
    let (t1, t2) = (
        read_sumstats(&Path::new(FIXTURES).join("trait1.sumstats.gz")),
        read_sumstats(&Path::new(FIXTURES).join("trait2.sumstats.gz")),
    );
    let af = read_af(&Path::new(FIXTURES).join("hm3_af.tsv.gz"));
    let ld = read_ld(&Path::new(FIXTURES).join("hm3_ld.tsv.gz"));
    let tag_w = read_bivar_weights(&Path::new(FIXTURES).join("hm3_bivar_weights.tsv.gz"));
    let c1 = read_constraint(&Path::new(FIXTURES).join("trait1.fit1.json"));
    let c2 = read_constraint(&Path::new(FIXTURES).join("trait2.fit1.json"));

    let mut idx: HashMap<String, u32> = HashMap::new();
    let (mut z1, mut z2, mut n1, mut n2, mut h) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for (rsid, &(z1v, n1v)) in &t1 {
        if let (Some(&freq), Some(&(z2v, n2v))) = (af.get(rsid), t2.get(rsid)) {
            idx.insert(rsid.clone(), idx.len() as u32);
            z1.push(z1v);
            z2.push(z2v);
            n1.push(n1v);
            n2.push(n2v);
            h.push(2.0 * freq * (1.0 - freq));
        }
    }
    let mut triples = Vec::new();
    for (a, b, r2) in &ld {
        if let (Some(&ta), Some(&tb)) = (idx.get(a), idx.get(b)) {
            triples.push((ta, tb, *r2));
        }
    }
    let mut tags = Vec::new();
    let mut weights = vec![0.0; z1.len()];
    for (rsid, &(w, _, _)) in &tag_w {
        if let Some(&i) = idx.get(rsid) {
            tags.push(i);
            weights[i as usize] = w;
        }
    }
    let mut data = BivariateData::new(z1, z2, n1, n2, h, &triples);
    data.tags = tags;
    data.weights = weights;

    let max_pi12 = c1.pi.min(c2.pi);
    let rg = -0.47314519108538067_f64;
    let rho_zero = -0.08575274965811708_f64;
    let min_pi12 = rg.abs() * (c1.pi * c2.pi).sqrt();
    println!(
        "min_pi12={:.6e} max_pi12={:.6e}  (ref pi12=7.8576e-4)",
        min_pi12, max_pi12
    );
    println!(
        "{:>10} {:>10} {:>10} {:>14}",
        "pi12", "rho_beta", "rg", "cost"
    );
    for frac in [
        0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 0.95, 0.99, 1.0,
    ] {
        let pi12 = min_pi12 + frac * (max_pi12 - min_pi12);
        let rho_beta = rg * (c1.pi * c2.pi).sqrt() / pi12;
        let p = BivariateParams {
            pi: [c1.pi - pi12, c2.pi - pi12, pi12],
            sig2_beta: [c1.sig2_beta, c2.sig2_beta],
            sig2_zero: [c1.sig2_zero, c2.sig2_zero],
            rho_beta,
            rho_zero,
        };
        let cost = bivariate_cost_gaussian(&data, &p);
        println!(
            "{:>10.6e} {:>10.5} {:>10.5} {:>14.5}",
            pi12,
            rho_beta,
            p.rg(),
            cost
        );
    }
}
