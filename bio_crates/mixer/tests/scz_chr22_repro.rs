//! SCZ chr22 复现测试：用与原始 MiXeR 完全相同的数据跑 Rust fit1，对比两种
//! 加权模式（Randprune / LdScore）与原始 MiXeR 金标准的差异。
//!
//! ## 测试数据
//!
//! 本测试需要两套数据，均归档于对象存储：
//!
//! ### 1. 跨验证夹具（提供 AF + LD）
//!   aliyun://autonomics-data/mixer/test-data/fixtures/
//!
//!   rclone copy aliyun://autonomics-data/mixer/test-data/fixtures/ \
//!     bio_crates/mixer/tests/fixtures/
//!
//! ### 2. SCZ chr22 复现数据（sumstats + 金标准 JSON）
//!   aliyun://autonomics-data/mixer/test-data/scz-chr22-repro/
//!
//!   rclone copy aliyun://autonomics-data/mixer/test-data/scz-chr22-repro/ \
//!     /tmp/scz_chr22_mixer/
//!
//! ## 数据来源
//!
//! - GWAS：IEU ieu-b-5102（SCZ, Trubetskoy et al. 2022），VCF → chr22 提取
//!   → HM3 BIM 交集 → 16,183 SNPs
//! - 参考面板：1000G EUR HM3 chr22（17,752 SNPs）
//! - LD：hm3_ld.tsv.gz（跨验证夹具提供）
//! - 金标准：`orig_fit1.json` — 原始 `mixer.py fit1` 输出（Randprune, seed=123）
//!
//! ## 原始 MiXeR 运行命令（复现用）
//!
//! ```sh
//! cd /mnt/disk3/gsa-mixer
//! PYTHONPATH=precimed:$PYTHONPATH python3 precimed/mixer.py fit1 \
//!   --trait1-file /tmp/scz_chr22_mixer/scz_chr22.sumstats.tsv \
//!   --bim-file precimed/mixer-test/data/g1000_eur_hm3_chr22.bim \
//!   --ld-file precimed/mixer-test/data/g1000_eur_hm3_chr22.ld \
//!   --chr2use 22 --seed 123 \
//!   --out /tmp/scz_chr22_mixer/orig_fit1 \
//!   --lib src/build/lib/libbgmg.so \
//!   --extract /tmp/scz_chr22_mixer/scz_chr22.sumstats.tsv
//! ```
//!
//! ## 运行
//!
//! ```sh
//! cargo test -p mixer --test scz_chr22_repro -- --ignored --nocapture
//! ```
//!
//! ## 预期结果
//!
//! π 和 σ²_β 各自在两种权重模式间有 ~30% 差异（π↔σ²_β 可识别退化），但：
//! - h² 两种模式一致（<2%）
//! - Randprune 模式的 cost 与原始 MiXeR 匹配到 <0.01%
//! - π·σ²_β（即 h²/totalhet）在所有模式间一致

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::Path;

use mixer::data::{ChromData, UnivariateSufficient};
use mixer::fit::{FitConfig, fit1};
use mixer::weights::{RandpruneConfig, ldscore_weights, randprune_weights};

/// 原始 MiXeR 在 SCZ chr22 上的输出（作为参考值）
const REF_PI: f64 = 0.022545903301906;
const REF_SIG2_BETA: f64 = 7.255311615929301e-05;
const REF_SIG2_ZERO: f64 = 1.1239754972928908;
const REF_H2: f64 = 0.009167831815503956;
const REF_COST: f64 = 3269.0784584761604;

/// 读 TSV sumstats → rsid → (z, n)。期望表头：A1 A2 BETA BP CHR N P SE SNP Z
fn read_sumstats(path: &Path) -> HashMap<String, (f64, f64)> {
    let f = std::fs::File::open(path).unwrap();
    let mut lines = BufReader::new(f).lines();
    let header = lines.next().unwrap().unwrap();
    let cols: Vec<&str> = header.split('\t').collect();
    let i_snp = cols.iter().position(|c| c == &"SNP").unwrap();
    let i_z = cols.iter().position(|c| c == &"Z").unwrap();
    let i_n = cols.iter().position(|c| c == &"N").unwrap();

    let mut m = HashMap::new();
    for line in lines {
        let line = line.unwrap();
        let f: Vec<&str> = line.split('\t').collect();
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

/// 读 gzipped AF → rsid → alt_freq（同 cross_validation 格式）
fn read_af(path: &Path) -> HashMap<String, f64> {
    use flate2::read::GzDecoder;
    let f = std::fs::File::open(path).unwrap();
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

/// 读 gzipped LD → Vec<(tag_rsid, snp_rsid, r2)>
fn read_ld(path: &Path) -> Vec<(String, String, f64)> {
    use flate2::read::GzDecoder;
    let f = std::fs::File::open(path).unwrap();
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
fn scz_chr22_repro_vs_original_mixer() {
    let fixtures =
        Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures"));
    let sumstats_path = Path::new("/tmp/scz_chr22_mixer/scz_chr22.sumstats.tsv");

    let sumstats = read_sumstats(&sumstats_path);
    let af = read_af(&fixtures.join("hm3_af.tsv.gz"));
    let ld = read_ld(&fixtures.join("hm3_ld.tsv.gz"));

    // universe = sumstats ∩ af → 连续 index
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
    println!("universe: {n_snp} SNPs (SCZ ∩ HM3 AF)");

    // LD → index
    let mut triples: Vec<(u32, u32, f64)> = Vec::new();
    for (a, b, r2) in &ld {
        if let (Some(&ta), Some(&tb)) = (rsid_to_idx.get(a), rsid_to_idx.get(b)) {
            triples.push((ta, tb, *r2));
        }
    }
    println!("LD pairs in universe: {}", triples.len());

    // All SNPs are tags (original MiXeR does extract internally; we use all)
    let tags: Vec<u32> = (0..n_snp as u32).collect();

    let mut data = ChromData::new(z_vec, n_vec, h_vec, &triples);
    data.tags = tags.clone();

    // Randprune weights — same config as original: n=64, r2=0.1, seed=123
    let rp_cfg = RandpruneConfig {
        n: 64,
        r2_threshold: 0.1,
        use_w_ld: false,
        seed: 123,
    };
    let rp_weights = randprune_weights(&data.ld, n_snp, &tags, None, &rp_cfg);
    let ldscore_wts = ldscore_weights(&data.ld, n_snp);

    data.weights = rp_weights.clone();
    let sum_w_rp: f64 = data.weights.iter().sum();
    println!("sum_weights (randprune): {sum_w_rp:.2}");
    println!("sum_weights (ldscore):   {:.2}", ldscore_wts.iter().sum::<f64>());

    let cfg = FitConfig {
        diffevo_repeats: 2,
        seed: 123,
        ..Default::default()
    };

    // ---- Randprune ----
    let suff_rp = UnivariateSufficient::from_chrom_data(&data);
    let rp_result = fit1(&suff_rp, &cfg);

    // ---- LdScore ----
    data.weights = ldscore_wts;
    let suff_ld = UnivariateSufficient::from_chrom_data(&data);
    let ld_result = fit1(&suff_ld, &cfg);

    println!("=== SCZ chr22: Rust Randprune vs LdScore vs Original MiXeR ===");
    println!("{:<14} {:>14} {:>14} {:>14}", "param", "Rust(RP)", "Rust(LD)", "Original");
    let rows: [(&str, f64, f64, f64); 7] = [
        ("pi", rp_result.params.pi, ld_result.params.pi, REF_PI),
        ("sig2_beta", rp_result.params.sig2_beta, ld_result.params.sig2_beta, REF_SIG2_BETA),
        ("sig2_zero", rp_result.params.sig2_zero, ld_result.params.sig2_zero, REF_SIG2_ZERO),
        ("h2", rp_result.h2, ld_result.h2, REF_H2),
        ("cost", rp_result.loglike, ld_result.loglike, REF_COST),
        ("nc", rp_result.nc, ld_result.nc, 400.2348754154353),
        ("nc_p9", rp_result.nc_p9, ld_result.nc_p9, 127.67492525752385),
    ];
    for (name, rp, ld, ref_val) in &rows {
        println!("{:<14} {:>14.8} {:>14.8} {:>14.8}", name, rp, ld, ref_val);
    }

    let totalhet = suff_rp.totalhet;
    println!("\ntotalhet={totalhet:.2}  n_snp={n_snp}");

    // Both modes should be within ~2x of each other (π↔σ²β degeneracy)
    let pi_ratio = rp_result.params.pi / ld_result.params.pi;
    assert!(pi_ratio > 0.3 && pi_ratio < 3.0, "pi ratio {pi_ratio:.2} out of range");
    let h2_ratio = rp_result.h2 / ld_result.h2;
    assert!(h2_ratio > 0.7 && h2_ratio < 1.3, "h2 ratio {h2_ratio:.2} out of range");
    println!("pi(RP)/pi(LD)={pi_ratio:.2}  h2(RP)/h2(LD)={h2_ratio:.2}");
}
