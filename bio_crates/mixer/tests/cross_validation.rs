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
use mixer::data::{ChromData, UnivariateSufficient};
use mixer::fit::{FitConfig, fit1};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

/// 金标准参考值（trait1.fit1.json，原版 univariate fit1）。
/// 现已对齐原版预处理：extract 子集(11200 tags) + randprune 权重(n=64,r2=0.1,seed=123)，
/// 均由 libbgmg dump（hm3_weights.tsv.gz），sum_weights=2839.72 与原版一致。
/// 因此参数与 cost 都应高度一致（残余差异来自 sig2_zeroL=0 简化 + DE 实现差异）。
const REF_PI: f64 = 0.0013072336834414333;
const REF_SIG2_BETA: f64 = 0.040229480662896805;
const REF_SIG2_ZERO: f64 = 0.9981262704964816;
const REF_H2: f64 = 0.5880186818033747;
const REF_COST: f64 = 4107.199188007042;

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

/// 读 hm3_weights.tsv.gz → rsid → randprune 权重（仅 tag 子集，11200 个）。
fn read_weights(path: &Path) -> HashMap<String, f64> {
    let f = File::open(path).unwrap();
    let dec = GzDecoder::new(f);
    let lines = BufReader::new(dec).lines();
    let mut m = HashMap::new();
    for line in lines.skip(1) {
        let line = line.unwrap();
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 2 {
            continue;
        }
        m.insert(f[0].to_string(), f[1].parse().unwrap());
    }
    m
}

#[test]
#[ignore]
fn cross_validate_vs_reference() {
    let sumstats = read_sumstats(&Path::new(FIXTURES).join("trait1.sumstats.gz"));
    let af = read_af(&Path::new(FIXTURES).join("hm3_af.tsv.gz"));
    let ld = read_ld(&Path::new(FIXTURES).join("hm3_ld.tsv.gz"));
    let tag_weights = read_weights(&Path::new(FIXTURES).join("hm3_weights.tsv.gz"));

    // universe = sumstats ∩ af（全部 SNP 作 LD 邻居 + h 来源），分配连续 index
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

    // LD 三元组 → index 空间（两端点在 universe 内；LD 覆盖全部 SNP 作邻居）
    let mut triples: Vec<(u32, u32, f64)> = Vec::new();
    for (a, b, r2) in &ld {
        if let (Some(&ta), Some(&tb)) = (rsid_to_idx.get(a), rsid_to_idx.get(b)) {
            triples.push((ta, tb, *r2));
        }
    }
    assert!(!triples.is_empty());

    // tag 子集 + 权重（原版 extract + randprune，精确对齐）
    let mut tags: Vec<u32> = Vec::new();
    let mut weights = vec![0.0_f64; n_snp];
    for (rsid, w) in &tag_weights {
        if let Some(&idx) = rsid_to_idx.get(rsid) {
            tags.push(idx);
            weights[idx as usize] = *w;
        }
    }
    assert!(
        !tags.is_empty(),
        "tag 子集为空，weights fixture 与 universe 不匹配"
    );

    let mut data = ChromData::new(z_vec, n_vec, h_vec, &triples);
    data.tags = tags;
    data.weights = weights;

    // 拟合：diffevo_repeats=2（对齐原版 diffevo_fast_repeats=2）
    let cfg = FitConfig {
        diffevo_repeats: 2,
        ..Default::default()
    };
    // 压缩成充分统计量：单趟扫 LD 折成 m1/m2，之后拟合不再触碰 CSR。
    let suff = UnivariateSufficient::from_chrom_data(&data);
    let result = fit1(&suff, &cfg);

    println!("=== 交叉验证（Rust 移植 vs 原版金标准，已对齐 extract+randprune）===");
    println!(
        "SNP 数: {}  LD 对: {}  tag 数: {}  sum_weights: {:.2}",
        n_snp,
        triples.len(),
        data.tags.len(),
        data.weights.iter().sum::<f64>()
    );
    println!("{:<12} {:>12} {:>12}   (参考)", "参数", "Rust拟合", "原版");
    println!("{:<12} {:>12.6} {:>12.6}", "pi", result.params.pi, REF_PI);
    println!(
        "{:<12} {:>12.6} {:>12.6}",
        "sig2_beta", result.params.sig2_beta, REF_SIG2_BETA
    );
    println!(
        "{:<12} {:>12.6} {:>12.6}",
        "sig2_zero", result.params.sig2_zero, REF_SIG2_ZERO
    );
    println!("{:<12} {:>12.6} {:>12.6}", "h2", result.h2, REF_H2);
    println!("{:<12} {:>12.4} {:>12.4}", "cost", result.loglike, REF_COST);

    // 对齐预处理后，所有量都应接近（残余差异来自 sig2_zeroL=0 + DE 实现细节）
    let sig2_zero_err = (result.params.sig2_zero - REF_SIG2_ZERO).abs();
    assert!(sig2_zero_err < 0.05, "sig2_zero 偏差: {}", sig2_zero_err);
    let cost_err = (result.loglike - REF_COST).abs() / REF_COST;
    assert!(cost_err < 0.02, "cost 相对偏差: {:.2}%", cost_err * 100.0);
    assert!(result.params.pi > 0.0 && result.params.pi < 1.0);
    assert!(result.params.sig2_beta > 0.0);
}
