//! 交叉验证：用原版 MiXeR 的 HM3 chr21+22 测试数据 + 金标准参考输出，
//! 验证 Rust 移植的 fit1 数值正确性。
//!
//! 数据来自 `tests/fixtures/`（见 README）。原版同源 LD/AF，由 libbgmg.so
//! 从 `g1000_eur_hm3_chr{N}.ld` 解压导出，完全忠实。
//!
//! 手动运行：`cargo test -p mixer --test cross_validation -- --ignored --nocapture`

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use flate2::read::GzDecoder;
use mixer::data::ChromData;
use mixer::fit::{fit1, FitConfig};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

/// 金标准参考值（trait1.fit1.json，原版 univariate fit1）。
/// ⚠️ 原版用了 randprune 权重 + extract 子集(11200 tags) + downsample=50，
/// 我们用 weights=1 + 全 SNP，所以 pi/sig2_beta/h2 会有系统偏差；
/// 但 sig2_zero（null 方差膨胀，对权重相对稳健）应接近 REF_SIG2_ZERO。
const REF_PI: f64 = 0.0013072336834414333;
const REF_SIG2_BETA: f64 = 0.040229480662896805;
const REF_SIG2_ZERO: f64 = 0.9981262704964816;
const REF_H2: f64 = 0.5880186818033747;

/// 读 trait1.sumstats.gz → rsid → (z, n)。列：A1 A2 BETA BP CHR N P SE SNP Z
fn read_sumstats(path: &Path) -> HashMap<String, (f64, f64)> {
    let f = File::open(path).unwrap();
    let dec = GzDecoder::new(f);
    let mut lines = BufReader::new(dec).lines();
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
        let rsid = f[i_snp].to_string();
        let z: f64 = f[i_z].parse().unwrap();
        let n: f64 = f[i_n].parse().unwrap();
        m.insert(rsid, (z, n));
    }
    m
}

/// 读 hm3_af.tsv.gz → rsid → alt_freq。
fn read_af(path: &Path) -> HashMap<String, f64> {
    let f = File::open(path).unwrap();
    let dec = GzDecoder::new(f);
    let lines = BufReader::new(dec).lines();
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

/// 读 hm3_ld.tsv.gz → Vec<(tag_rsid, snp_rsid, r2)>。
fn read_ld(path: &Path) -> Vec<(String, String, f64)> {
    let f = File::open(path).unwrap();
    let dec = GzDecoder::new(f);
    let lines = BufReader::new(dec).lines();
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

#[test]
#[ignore]
fn cross_validate_vs_reference() {
    let sumstats = read_sumstats(&Path::new(FIXTURES).join("trait1.sumstats.gz"));
    let af = read_af(&Path::new(FIXTURES).join("hm3_af.tsv.gz"));
    let ld = read_ld(&Path::new(FIXTURES).join("hm3_ld.tsv.gz"));

    // universe = sumstats ∩ af（同时有 z 和 h 的 SNP），分配连续 index
    let mut rsid_to_idx: HashMap<String, u32> = HashMap::new();
    let mut z_vec = Vec::new();
    let mut n_vec = Vec::new();
    let mut h_vec = Vec::new();
    for (rsid, &(z, n)) in &sumstats {
        if let Some(&freq) = af.get(rsid) {
            let idx = rsid_to_idx.len() as u32;
            rsid_to_idx.insert(rsid.clone(), idx);
            z_vec.push(z);
            n_vec.push(n);
            h_vec.push(2.0 * freq * (1.0 - freq));
        }
    }
    let n_snp = z_vec.len();
    assert!(n_snp > 1000, "universe too small: {n_snp}");

    // LD 三元组 → index 空间（两端点必须在 universe 内）
    let mut triples: Vec<(u32, u32, f64)> = Vec::new();
    for (a, b, r2) in &ld {
        if let (Some(&ta), Some(&tb)) = (rsid_to_idx.get(a), rsid_to_idx.get(b)) {
            triples.push((ta, tb, *r2));
        }
    }
    assert!(!triples.is_empty());

    let data = ChromData::new(z_vec, n_vec, h_vec, &triples);

    // 拟合：diffevo_repeats=1 + 适度 DE 规模（平衡速度与全局搜索）
    let cfg = FitConfig {
        diffevo_repeats: 1,
        diffevo_popsize: 10,
        diffevo_max_gen: 40,
        nm_max_iter: 1200,
        ..Default::default()
    };
    let result = fit1(&data, &cfg);

    println!("=== 交叉验证（Rust 移植 vs 原版金标准）===");
    println!("SNP 数: {}  LD 对: {}", n_snp, triples.len());
    println!(
        "{:<12} {:>12} {:>12}   (参考)",
        "参数", "Rust拟合", "原版"
    );
    println!(
        "{:<12} {:>12.6} {:>12.6}",
        "pi", result.params.pi, REF_PI
    );
    println!(
        "{:<12} {:>12.6} {:>12.6}",
        "sig2_beta", result.params.sig2_beta, REF_SIG2_BETA
    );
    println!(
        "{:<12} {:>12.6} {:>12.6}   ← 首要校验量（对权重稳健）",
        "sig2_zero", result.params.sig2_zero, REF_SIG2_ZERO
    );
    println!("{:<12} {:>12.6} {:>12.6}", "h2", result.h2, REF_H2);

    // sig2_zero 是对权重/子集最稳健的量，应接近 0.998（容差宽松，因 weights=1）
    let sig2_zero_err = (result.params.sig2_zero - REF_SIG2_ZERO).abs();
    assert!(
        sig2_zero_err < 0.3,
        "sig2_zero 偏差过大: Rust={} vs 参考={}, |err|={}",
        result.params.sig2_zero, REF_SIG2_ZERO, sig2_zero_err
    );
    // 参数合法
    assert!(result.params.pi > 0.0 && result.params.pi < 1.0);
    assert!(result.params.sig2_beta > 0.0);
}
