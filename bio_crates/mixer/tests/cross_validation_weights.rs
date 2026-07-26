//! 随机剪枝权重 bit-exact 交叉验证：Rust `randprune_weights` vs libbgmg dump 的
//! `hm3_weights.tsv.gz`（sum_weights=2839.7188，11200 tags）。
//!
//! 验证纯 Rust 的 mt19937_64 + Lemire uniform_int + 贪心剪枝算法与原版 C++
//! `set_weights_randprune(64, 0.1, 0, False)` 逐位一致。
//!
//! 手动运行：`cargo test -p mixer --test cross_validation_weights -- --ignored --nocapture`

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use flate2::read::GzDecoder;
use mixer::ld_matrix::LdBlock;
use mixer::weights::{RandpruneConfig, randprune_weights};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

fn read_af(path: &Path) -> Vec<String> {
    let lines = BufReader::new(GzDecoder::new(File::open(path).unwrap())).lines();
    let mut v = Vec::new();
    for line in lines.skip(1) {
        let line = line.unwrap();
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() >= 3 {
            v.push(f[1].to_string());
        }
    }
    v
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

/// hm3_weights.tsv.gz：tag_rsid, weight（原版 randprune 输出，金标准）。
fn read_ref_weights(path: &Path) -> Vec<(String, f64)> {
    let lines = BufReader::new(GzDecoder::new(File::open(path).unwrap())).lines();
    let mut out = Vec::new();
    for line in lines.skip(1) {
        let line = line.unwrap();
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() >= 2 {
            out.push((f[0].to_string(), f[1].parse().unwrap()));
        }
    }
    out
}

#[test]
#[ignore]
fn randprune_matches_libbgmg_dump() {
    let af = read_af(&Path::new(FIXTURES).join("hm3_af.tsv.gz"));
    let ld = read_ld(&Path::new(FIXTURES).join("hm3_ld.tsv.gz"));
    let ref_w = read_ref_weights(&Path::new(FIXTURES).join("hm3_weights.tsv.gz"));

    // universe = af（34958 SNP），分配连续 index
    let mut idx: HashMap<String, u32> = HashMap::new();
    for rsid in &af {
        let i = idx.len() as u32;
        idx.insert(rsid.clone(), i);
    }
    let n_snp = idx.len();

    // tags = ref_weights 的 tag_rsid（11200），保持 dump 顺序
    let tags: Vec<u32> = ref_w
        .iter()
        .map(|(rsid, _)| *idx.get(rsid).expect("tag rsid 不在 af universe"))
        .collect();
    let num_tag = tags.len();

    // LD 三元组 → unified index
    let mut triples: Vec<(u32, u32, f64)> = Vec::new();
    for (a, b, r2) in &ld {
        if let (Some(&ta), Some(&tb)) = (idx.get(a), idx.get(b)) {
            triples.push((ta, tb, *r2));
        }
    }
    let ld_block = LdBlock::from_coo(&triples, n_snp);

    // Rust randprune（与原版同参数：n=64, r2=0.1, use_w_ld=false, seed=123）
    let cfg = RandpruneConfig::default();
    let rust_w = randprune_weights(&ld_block, n_snp, &tags, None, &cfg);

    // 对比
    let ref_sum: f64 = ref_w.iter().map(|(_, w)| *w).sum();
    let rust_sum: f64 = rust_w.iter().sum();
    println!("=== randprune bit-exact 交叉验证 ===");
    println!("num_tag={num_tag}  n_snp={n_snp}  LD对={}", triples.len());
    println!("sum_weights: Rust={rust_sum:.6}  原版={ref_sum:.6}  (参考 2839.7188)");

    // 逐 tag 最大绝对/相对误差
    let mut max_abs = 0.0f64;
    let mut max_rel = 0.0f64;
    let mut exact_count = 0usize;
    for (i, (_, rw)) in ref_w.iter().enumerate() {
        let diff = (rust_w[i] - rw).abs();
        let rel = diff / rw.abs().max(1e-12);
        if diff < 1e-9 {
            exact_count += 1;
        }
        if diff > max_abs {
            max_abs = diff;
        }
        if rel > max_rel {
            max_rel = rel;
        }
    }
    println!(
        "逐 tag: 精确匹配 {exact_count}/{num_tag}，max_abs={max_abs:.3e}，max_rel={max_rel:.3e}"
    );

    // bit-exact 断言（f32 精度内）
    assert!(
        (rust_sum - ref_sum).abs() < 1e-3,
        "sum 偏差 {}",
        (rust_sum - ref_sum).abs()
    );
    assert!(max_abs < 1e-4, "max_abs={max_abs} 应近 0（bit-exact）");
    assert!(
        (rust_sum - 2839.7188).abs() < 1.0,
        "sum {rust_sum} 应接近参考 2839.7188"
    );
}
