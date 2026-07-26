//! Bake-off：逆 LD-score 加权 vs 随机剪枝加权，谁更能回收已知参数？
//!
//! 实验设计（控制变量，只让"加权方案"这一个因子变化）：
//!   1. 用真实 HM3 chr21+22 LD 面板（提供真实 LD 拓扑 + N/h）。
//!   2. 选一组已知真值 (π, σ²_β, σ²_zero)，用 `simulate` 采样合成 z。
//!   3. 对**同一份**合成 z，分别用 LdScore 权重和 Randprune 权重构建
//!      `UnivariateSufficient`（m1/m2/tags/totalhet 完全相同，仅 weights 不同），
//!      用**同一个**优化器配置跑 `fit1`。
//!   4. 多个种子重复，统计两种方案回收真值的误差均值±std。
//!
//! 这样把"加权方案"的统计效果干净地孤立出来。LD/sim z/优化器种子都相同，
//! 唯一差别就是 weight 向量。
//!
//! 手动运行：
//! ```sh
//! cargo test -p mixer --test bakeoff_ldscore_vs_randprune -- --ignored --nocapture
//! ```

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use flate2::read::GzDecoder;

use mixer::data::{ChromData, UnivariateSufficient};
use mixer::fit::{FitConfig, fit1};
use mixer::params::UnivariateParams;
use mixer::simulate::simulate;
use mixer::weights::{RandpruneConfig, ldscore_weights, randprune_weights};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

/// 真值参数集（跨不同信号体制）。
const TRUTH_SETS: &[(&str, f64, f64, f64)] = &[
    // (label, π, σ²_β, σ²_zero)
    ("moderate", 0.01, 0.003, 1.0),
    ("dense-weak", 0.05, 0.001, 1.0),
];

/// 每个真值用多少个种子采样合成 trait（默认 6，可用环境变量覆盖）。
fn n_seeds() -> usize {
    std::env::var("BAKEOFF_SEEDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(6)
}

/// 差分进化重复数（默认 4，越高越准越慢）。
fn diffevo_repeats() -> usize {
    std::env::var("BAKEOFF_REPEATS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(4)
}

// ---------- fixture 读取（与 cross_validation.rs 同源） ----------

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

/// 单次拟合的回收结果（用于误差统计）。
struct Fit {
    pi: f64,
    sig2_beta: f64,
    sig2_zero: f64,
}

fn run_fit(suff: &UnivariateSufficient) -> Fit {
    let cfg = FitConfig {
        diffevo_repeats: diffevo_repeats(),
        ..Default::default()
    };
    let r = fit1(suff, &cfg);
    Fit {
        pi: r.params.pi,
        sig2_beta: r.params.sig2_beta,
        sig2_zero: r.params.sig2_zero,
    }
}

/// 均值±std（总体标准差）。
fn mean_std(xs: &[f64]) -> (f64, f64) {
    let n = xs.len() as f64;
    let mean = xs.iter().sum::<f64>() / n;
    let var = xs.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n;
    (mean, var.sqrt())
}

#[test]
#[ignore]
fn bakeoff_ldscore_vs_randprune_recovery() {
    // 1. 装配真实 HM3 LD 面板。
    let sumstats = read_sumstats(&Path::new(FIXTURES).join("trait1.sumstats.gz"));
    let af = read_af(&Path::new(FIXTURES).join("hm3_af.tsv.gz"));
    let ld = read_ld(&Path::new(FIXTURES).join("hm3_ld.tsv.gz"));

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
    let mut triples: Vec<(u32, u32, f64)> = Vec::new();
    for (a, b, r2) in &ld {
        if let (Some(&ta), Some(&tb)) = (rsid_to_idx.get(a), rsid_to_idx.get(b)) {
            triples.push((ta, tb, *r2));
        }
    }
    let data = ChromData::new(z_vec, n_vec, h_vec, &triples);
    let n_snp = data.n_snp();
    let tags: Vec<u32> = (0..n_snp as u32).collect();
    println!("LD 面板：{n_snp} SNP，{} LD 对", triples.len());

    // 2. m1/m2/totalhet 只依赖 LD/n/h，与 z/weights 无关 → 预算一次，两个方案共用。
    let template = UnivariateSufficient::from_chrom_data(&data);

    // 3. 两套权重（在完整 tag 集上算；tags = 全部 SNP，与节点默认一致）。
    let w_ldscore = ldscore_weights(&data.ld, n_snp);
    let rp_cfg = RandpruneConfig::default(); // n=64, r2=0.1, seed=123
    let w_randprune = randprune_weights(&data.ld, n_snp, &tags, None, &rp_cfg);
    println!(
        "权重：LdScore Σ={:.1}（均 {:.4}）  Randprune Σ={:.1}（均 {:.4}）",
        w_ldscore.iter().sum::<f64>(),
        w_ldscore.iter().sum::<f64>() / n_snp as f64,
        w_randprune.iter().sum::<f64>(),
        w_randprune.iter().sum::<f64>() / n_snp as f64,
    );

    let seeds = n_seeds();
    println!(
        "\n每个真值 {} 个种子，diffevo_repeats={}，优化器种子固定（两方案共享）",
        seeds,
        diffevo_repeats()
    );

    for &(label, pi_t, sb_t, sz_t) in TRUTH_SETS {
        let truth = UnivariateParams::new(pi_t, sb_t, sz_t);

        let mut abs_ld = Vec::new(); // (π, σ²_β, σ²_zero) 绝对误差 —— LdScore
        let mut abs_rp = Vec::new(); // Randprune
        let mut rel_h2_ld = Vec::new();
        let mut rel_h2_rp = Vec::new();
        let totalhet = template.totalhet;
        let h2_t = sb_t * pi_t * totalhet;

        for seed in 0..seeds as u64 {
            // 同一份合成 z 喂两个方案 → 唯一变量是 weights。
            let z_sim = simulate(&data, &truth, seed);

            let s_ld = UnivariateSufficient {
                z: z_sim.clone(),
                weights: w_ldscore.clone(),
                m1: template.m1.clone(),
                m2: template.m2.clone(),
                tags: template.tags.clone(),
                totalhet: template.totalhet,
                n_snp,
            };
            let s_rp = UnivariateSufficient {
                z: z_sim,
                weights: w_randprune.clone(),
                m1: template.m1.clone(),
                m2: template.m2.clone(),
                tags: template.tags.clone(),
                totalhet: template.totalhet,
                n_snp,
            };

            let f_ld = run_fit(&s_ld);
            let f_rp = run_fit(&s_rp);

            abs_ld.push((
                (f_ld.pi - pi_t).abs(),
                (f_ld.sig2_beta - sb_t).abs(),
                (f_ld.sig2_zero - sz_t).abs(),
            ));
            abs_rp.push((
                (f_rp.pi - pi_t).abs(),
                (f_rp.sig2_beta - sb_t).abs(),
                (f_rp.sig2_zero - sz_t).abs(),
            ));
            let h2_ld = f_ld.sig2_beta * f_ld.pi * totalhet;
            let h2_rp = f_rp.sig2_beta * f_rp.pi * totalhet;
            rel_h2_ld.push((h2_ld - h2_t).abs() / h2_t);
            rel_h2_rp.push((h2_rp - h2_t).abs() / h2_t);
        }

        // 汇总。
        let pi_ld: Vec<f64> = abs_ld.iter().map(|t| t.0).collect();
        let pi_rp: Vec<f64> = abs_rp.iter().map(|t| t.0).collect();
        let sb_ld: Vec<f64> = abs_ld.iter().map(|t| t.1).collect();
        let sb_rp: Vec<f64> = abs_rp.iter().map(|t| t.1).collect();
        let sz_ld: Vec<f64> = abs_ld.iter().map(|t| t.2).collect();
        let sz_rp: Vec<f64> = abs_rp.iter().map(|t| t.2).collect();

        let (pi_ld_m, pi_ld_s) = mean_std(&pi_ld);
        let (pi_rp_m, pi_rp_s) = mean_std(&pi_rp);
        let (sb_ld_m, sb_ld_s) = mean_std(&sb_ld);
        let (sb_rp_m, sb_rp_s) = mean_std(&sb_rp);
        let (sz_ld_m, sz_ld_s) = mean_std(&sz_ld);
        let (sz_rp_m, sz_rp_s) = mean_std(&sz_rp);
        let (h2_ld_m, _) = mean_std(&rel_h2_ld);
        let (h2_rp_m, _) = mean_std(&rel_h2_rp);

        println!(
            "\n================ 真值 [{label}] π={pi_t} σ²_β={sb_t} σ²_zero={sz_t}  (h²={h2_t:.4}) ================"
        );
        println!(
            "{:<14} {:>22} {:>22}",
            "指标(均值±std)", "LdScore", "Randprune"
        );
        println!(
            "{:<14} {:>22} {:>22}",
            "π 绝对误差",
            format!("{:.5} ± {:.5}", pi_ld_m, pi_ld_s),
            format!("{:.5} ± {:.5}", pi_rp_m, pi_rp_s),
        );
        println!(
            "{:<14} {:>22} {:>22}",
            "σ²_β 绝对误差",
            format!("{:.5} ± {:.5}", sb_ld_m, sb_ld_s),
            format!("{:.5} ± {:.5}", sb_rp_m, sb_rp_s),
        );
        println!(
            "{:<14} {:>22} {:>22}",
            "σ²_zero 误差",
            format!("{:.5} ± {:.5}", sz_ld_m, sz_ld_s),
            format!("{:.5} ± {:.5}", sz_rp_m, sz_rp_s),
        );
        println!(
            "{:<14} {:>22} {:>22}",
            "h² 相对误差",
            format!("{:.4}", h2_ld_m),
            format!("{:.4}", h2_rp_m),
        );
        // 每行标出胜者（误差更小者）。
        println!(
            "  胜者 → π:{}  σ²_β:{}  σ²_zero:{}  h²:{}",
            if pi_ld_m <= pi_rp_m {
                "LdScore"
            } else {
                "Randprune"
            },
            if sb_ld_m <= sb_rp_m {
                "LdScore"
            } else {
                "Randprune"
            },
            if sz_ld_m <= sz_rp_m {
                "LdScore"
            } else {
                "Randprune"
            },
            if h2_ld_m <= h2_rp_m {
                "LdScore"
            } else {
                "Randprune"
            },
        );
    }
    println!(
        "\n（注：误差越小越好；±std 反映跨种子波动。两方案共享同一合成 z、同一优化器种子，差别仅来自 weights。）"
    );
}
