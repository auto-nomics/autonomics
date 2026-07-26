//! Bivariate 交叉验证：用原版 MiXeR 的 HM3 chr21+22 测试数据 + fit2.json 金标准，
//! 验证 Rust 移植的 fit2 数值正确性。
//!
//! 流程对齐原版 `mixer.py fit2 --fit-sequence diffevo-fast neldermead-fast brute1-fast brent1-fast`。
//! LD/AF/权重由 libbgmg dump（与原版同源），univariate 约束来自原版 fit1 输出。
//!
//! 手动运行：`cargo test -p mixer --test cross_validation_bivar -- --ignored --nocapture`

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use flate2::read::GzDecoder;
use mixer::bivariate::{BivariateData, Fit2Config, UnivariateConstraint, fit2};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

fn read_sumstats(path: &Path) -> HashMap<String, (f64, f64)> {
    let f = File::open(path).unwrap();
    let mut lines = BufReader::new(GzDecoder::new(f)).lines();
    let header = lines.next().unwrap().unwrap();
    let cols: Vec<&str> = header.split_whitespace().collect();
    let i_snp = cols.iter().position(|c| *c == "SNP").unwrap();
    let i_z = cols.iter().position(|c| *c == "Z").unwrap();
    let i_n = cols.iter().position(|c| *c == "N").unwrap();
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
    let f = File::open(path).unwrap();
    let lines = BufReader::new(GzDecoder::new(f)).lines();
    let mut m = HashMap::new();
    for line in lines.skip(1) {
        let line = line.unwrap();
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 3 {
            continue;
        }
        m.insert(f[1].to_string(), f[2].parse().unwrap());
    }
    m
}

fn read_ld(path: &Path) -> Vec<(String, String, f64)> {
    let f = File::open(path).unwrap();
    let lines = BufReader::new(GzDecoder::new(f)).lines();
    let mut out = Vec::new();
    for line in lines.skip(1) {
        let line = line.unwrap();
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 5 {
            continue;
        }
        out.push((f[1].to_string(), f[2].to_string(), f[4].parse().unwrap()));
    }
    out
}

/// hm3_bivar_weights.tsv.gz：tag_rsid, weight, n1, n2（仅 tag 子集，11200 个）。
fn read_bivar_weights(path: &Path) -> HashMap<String, (f64, f64, f64)> {
    let f = File::open(path).unwrap();
    let lines = BufReader::new(GzDecoder::new(f)).lines();
    let mut m = HashMap::new();
    for line in lines.skip(1) {
        let line = line.unwrap();
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 4 {
            continue;
        }
        m.insert(
            f[0].to_string(),
            (
                f[1].parse().unwrap(),
                f[2].parse().unwrap(),
                f[3].parse().unwrap(),
            ),
        );
    }
    m
}

/// 从 fit1.json 读 univariate 约束 (pi, sig2_beta, sig2_zero)。
fn read_univariate_constraint(path: &Path) -> UnivariateConstraint {
    let txt = std::fs::read_to_string(path).unwrap();
    let v: serde_json::Value = serde_json::from_str(&txt).unwrap();
    let p = &v["params"];
    UnivariateConstraint {
        pi: p["pi"].as_f64().unwrap(),
        sig2_beta: p["sig2_beta"].as_f64().unwrap(),
        sig2_zero: p["sig2_zero"].as_f64().unwrap(),
    }
}

#[test]
#[ignore]
fn cross_validate_bivariate_vs_reference() {
    let t1 = read_sumstats(&Path::new(FIXTURES).join("trait1.sumstats.gz"));
    let t2 = read_sumstats(&Path::new(FIXTURES).join("trait2.sumstats.gz"));
    let af = read_af(&Path::new(FIXTURES).join("hm3_af.tsv.gz"));
    let ld = read_ld(&Path::new(FIXTURES).join("hm3_ld.tsv.gz"));
    let tag_w = read_bivar_weights(&Path::new(FIXTURES).join("hm3_bivar_weights.tsv.gz"));
    let c1 = read_univariate_constraint(&Path::new(FIXTURES).join("trait1.fit1.json"));
    let c2 = read_univariate_constraint(&Path::new(FIXTURES).join("trait2.fit1.json"));

    // universe = trait1 ∩ trait2 ∩ af
    let mut idx: HashMap<String, u32> = HashMap::new();
    let mut z1 = Vec::new();
    let mut z2 = Vec::new();
    let mut n1 = Vec::new();
    let mut n2 = Vec::new();
    let mut h = Vec::new();
    for (rsid, &(z1v, n1v)) in &t1 {
        if let (Some(&freq), Some(&(z2v, n2v))) = (af.get(rsid), t2.get(rsid)) {
            let i = idx.len() as u32;
            idx.insert(rsid.clone(), i);
            z1.push(z1v);
            z2.push(z2v);
            n1.push(n1v);
            n2.push(n2v);
            h.push(2.0 * freq * (1.0 - freq));
        }
    }
    let n_snp = z1.len();
    assert!(n_snp > 1000, "universe too small: {n_snp}");

    // LD 三元组 → index 空间
    let mut triples: Vec<(u32, u32, f64)> = Vec::new();
    for (a, b, r2) in &ld {
        if let (Some(&ta), Some(&tb)) = (idx.get(a), idx.get(b)) {
            triples.push((ta, tb, *r2));
        }
    }
    assert!(!triples.is_empty());

    // tag 子集 + 权重（精确对齐原版 randprune）。非 tag 处权重 0。
    let mut tags: Vec<u32> = Vec::new();
    let mut weights = vec![0.0_f64; n_snp];
    for (rsid, &(w, _, _)) in &tag_w {
        if let Some(&i) = idx.get(rsid) {
            tags.push(i);
            weights[i as usize] = w;
        }
    }
    assert!(!tags.is_empty(), "tag 子集为空");

    let mut data = BivariateData::new(z1, z2, n1, n2, h, &triples);
    data.tags = tags;
    data.weights = weights;

    // 约束 sanity：与 fit2.json 的 pi1u/pi2u/sig2 应一致
    println!(
        "约束: c1(pi={:.6e}, sb={:.6}, sz={:.6})  c2(pi={:.6e}, sb={:.6}, sz={:.6})",
        c1.pi, c1.sig2_beta, c1.sig2_zero, c2.pi, c2.sig2_beta, c2.sig2_zero
    );

    let cfg = Fit2Config {
        diffevo_repeats: 2,
        ..Default::default()
    };
    let r = fit2(&data, c1, c2, &cfg);

    // 金标准（fit2.json 最终参数）
    let ref_pi12 = 0.0007857607399747815_f64;
    let ref_rho_beta = -0.6088339374212625_f64;
    let ref_rho_zero = -0.08575274965811708_f64;
    let ref_rg = -0.47314519108538067_f64;
    let ref_dice = 0.7530869822934179_f64;
    let ref_h2_t1 = 0.5880131152031144_f64;
    let ref_h2_t2 = 0.7340249433191081_f64;
    let ref_cost = 8329.7158_f64;

    println!("=== Bivariate 交叉验证（Rust fit2 vs 原版 fit2.json 金标准）===");
    println!(
        "SNP:{} LD对:{} tag:{} sum_w:{:.2}",
        n_snp,
        triples.len(),
        data.tags.len(),
        data.weights.iter().sum::<f64>()
    );
    println!(
        "{:<10} {:>12} {:>12}   {:>8}",
        "量", "Rust", "原版", "相对误差"
    );
    let row = |name: &str, got: f64, refv: f64| {
        let rel = (got - refv).abs() / refv.abs().max(1e-12);
        println!(
            "{:<10} {:>12.6} {:>12.6}   {:>7.2}%",
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
    row("h2_T1", r.h2_t1, ref_h2_t1);
    row("h2_T2", r.h2_t2, ref_h2_t2);
    println!("{:<10} {:>12.4} {:>12.4}", "cost", r.loglike, ref_cost);

    // ── 可识别性说明 ──
    // bivariate 高斯 cost 对 (pi12, rho_beta) 的分解存在精确退化：沿 rg=const 曲线
    //（rho_beta = rg·√(pi1u·pi2u)/pi12）cost 恒定（见 scan_pi12 测试，全区間 8329.71589）。
    // 因此高斯 cost 只识别 rg、rho_zero、cost；pi12/rho_beta/dice 各自不可识别。
    // 原版 production 用 sampling cost（brute1/brent1 非 -fast）才打破退化识别 pi12；
    // 本测试对齐 -fast（gaussian）序列，故只断言可识别量。
    // 下方 pi12/rho_beta/dice 仅展示（会落在山脊上某个等价点，与原版不必相同）。

    // 断言可识别量（核心校验）
    let cost_err = (r.loglike - ref_cost).abs() / ref_cost;
    assert!(
        cost_err < 1e-3,
        "cost 相对偏差 {:.4}% > 0.1%",
        cost_err * 100.0
    );
    let rg_err = (r.rg - ref_rg).abs() / ref_rg.abs();
    assert!(rg_err < 0.01, "rg 相对偏差 {:.2}% > 1%", rg_err * 100.0);
    let rz_err = (r.rho_zero - ref_rho_zero).abs();
    assert!(rz_err < 0.01, "rho_zero 偏差 {} > 0.01", rz_err);
    // h2_T1/T2 由 univariate 约束决定，应几乎精确
    assert!((r.h2_t1 - ref_h2_t1).abs() < 1e-6);
    assert!((r.h2_t2 - ref_h2_t2).abs() < 1e-6);
    // pi12/rho_beta 在合法区间
    assert!(r.pi12 > 0.0 && r.pi12 <= c1.pi.min(c2.pi) + 1e-12);
    assert!(r.rho_beta.abs() <= 1.0 + 1e-6);
}
