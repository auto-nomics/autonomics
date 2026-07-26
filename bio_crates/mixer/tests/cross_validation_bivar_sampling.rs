//! Bivariate **sampling** 交叉验证：brute1/brent1 用 Monte Carlo sampling cost（打破 pi12/rho_beta 退化），
//! 对比原版 `fit2_sampling.json`（`--fit-sequence diffevo-fast neldermead-fast brute1 brent1 --kmax 20000`）。
//!
//! 手动运行：`cargo test -p mixer --test cross_validation_bivar_sampling -- --ignored --nocapture`

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use flate2::read::GzDecoder;
use mixer::bivariate::{BivariateData, Fit2Config, UnivariateConstraint, fit2};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

fn read_sumstats(path: &Path) -> HashMap<String, (f64, f64)> {
    let mut lines = BufReader::new(GzDecoder::new(File::open(path).unwrap())).lines();
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
#[ignore]
fn cross_validate_bivariate_sampling() {
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
    let n_snp = z1.len();
    let mut triples = Vec::new();
    for (a, b, r2) in &ld {
        if let (Some(&ta), Some(&tb)) = (idx.get(a), idx.get(b)) {
            triples.push((ta, tb, *r2));
        }
    }
    let mut tags = Vec::new();
    let mut weights = vec![0.0; n_snp];
    for (rsid, &(w, _, _)) in &tag_w {
        if let Some(&i) = idx.get(rsid) {
            tags.push(i);
            weights[i as usize] = w;
        }
    }
    let mut data = BivariateData::new(z1, z2, n1, n2, h, &triples);
    data.tags = tags;
    data.weights = weights;

    // sampling fit2：brute1/brent1 用 MC sampling cost
    let cfg = Fit2Config {
        diffevo_repeats: 2,
        sampling: true,
        k_max: 20000,
        ..Default::default()
    };
    let r = fit2(&data, c1, c2, &cfg);

    // 金标准（fit2_sampling.json，brute1/brent1 sampling）
    let ref_pi12 = 0.0004833758397701709_f64;
    let ref_rho_beta = -0.989701476482257_f64;
    let ref_rho_zero = -0.08575275074408606_f64;
    let ref_rg = -0.4731451684630205_f64;
    let ref_dice = 0.46327594898384444_f64;
    let ref_cost = 8294.91492_f64;

    println!("=== Bivariate SAMPLING 交叉验证（Rust vs 原版 fit2_sampling.json）===");
    println!(
        "SNP:{} LD:{} tag:{} sum_w:{:.2}  kmax={}",
        n_snp,
        triples.len(),
        data.tags.len(),
        data.weights.iter().sum::<f64>(),
        cfg.k_max
    );
    let row = |name: &str, got: f64, refv: f64| {
        let rel = (got - refv).abs() / refv.abs().max(1e-12);
        println!(
            "{:<10} {:>14.6} {:>14.6}   {:>7.2}%",
            name,
            got,
            refv,
            rel * 100.0
        );
    };
    row("pi12", r.pi12, ref_pi12);
    row("rho_beta", r.rho_beta, ref_rho_beta);
    row("rho_zero", r.rho_zero, ref_rho_zero);
    row("rg", r.rg, ref_rg);
    row("dice", r.dice, ref_dice);
    println!(
        "{:<10} {:>14.4} {:>14.4}",
        "cost(samp)", r.loglike, ref_cost
    );

    // 可识别量（rg、rho_zero 由 gaussian NM 决定，应紧密一致）
    let rg_err = (r.rg - ref_rg).abs() / ref_rg.abs();
    assert!(rg_err < 0.01, "rg 相对偏差 {:.2}% > 1%", rg_err * 100.0);
    let rz_err = (r.rho_zero - ref_rho_zero).abs();
    assert!(rz_err < 0.01, "rho_zero 偏差 {} > 0.01", rz_err);
    // sampling cost：两独立 MC 估计应高度一致（<0.5%）
    let cost_err = (r.loglike - ref_cost).abs() / ref_cost;
    assert!(
        cost_err < 0.005,
        "sampling cost 相对偏差 {:.2}% > 0.5%",
        cost_err * 100.0
    );
    // pi12/rho_beta：sampling 打破退化后应落在同一区域（rho_beta 接近 −1 边界）
    assert!(
        r.rho_beta < -0.5,
        "rho_beta 应在负边界附近，got {}",
        r.rho_beta
    );
    assert!(r.pi12 > 0.0 && r.pi12 <= c1.pi.min(c2.pi));
    assert!(r.dice > 0.0 && r.dice < 1.0);
}
