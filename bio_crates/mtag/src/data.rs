//! Data loading, allele harmonisation, and result I/O for MTAG.
//!
//! Port of `load_and_merge_data`, `extract_gwas_sumstats`,
//! `save_mtag_results`, and `write_summary` from `mtag.py`.

use statrs::distribution::ContinuousCDF;
use std::collections::HashSet;
use std::path::Path;

use faer::Mat;

use crate::error::{MtagError, Result};
use crate::mtag::MtagResult;

/// Strand-ambiguous SNP allele pairs (matching `allele_info.STRAND_AMBIGUOUS`).
///
/// A pair is strand-ambiguous if it cannot be distinguished from its
/// reverse complement: A/T, T/A, C/G, G/C.
fn is_strand_ambiguous(a1: &str, a2: &str) -> bool {
    matches!((a1, a2), ("A", "T") | ("T", "A") | ("C", "G") | ("G", "C"))
}

/// A single GWAS summary-statistics record as loaded from file.
#[derive(Clone, Debug)]
pub struct GwasRecord {
    pub snp: String,
    pub z: f64,
    pub n: f64,
    pub freq: f64,
    pub a1: String,
    pub a2: String,
    pub chr: Option<i32>,
    pub bp: Option<i64>,
}

/// A trait's GWAS data after loading.
#[derive(Clone, Debug)]
pub struct TraitData {
    pub records: Vec<GwasRecord>,
}

impl TraitData {
    /// Read a whitespace-delimited GWAS sumstats file.
    ///
    /// Expects columns: SNP, Z, N, FRQ, A1, A2 (and optionally CHR, BP).
    pub fn from_tsv(
        path: &Path,
        col_snp: &str,
        col_z: &str,
        col_n: &str,
        col_freq: &str,
        col_a1: &str,
        col_a2: &str,
        col_chr: Option<&str>,
        col_bp: Option<&str>,
    ) -> Result<Self> {
        let content = std::fs::read_to_string(path).map_err(MtagError::Io)?;
        Self::from_str(
            &content, col_snp, col_z, col_n, col_freq, col_a1, col_a2, col_chr, col_bp,
        )
    }

    /// Parse GWAS sumstats from a string (file contents).
    pub fn from_str(
        content: &str,
        col_snp: &str,
        col_z: &str,
        col_n: &str,
        col_freq: &str,
        col_a1: &str,
        col_a2: &str,
        col_chr: Option<&str>,
        col_bp: Option<&str>,
    ) -> Result<Self> {
        let mut lines = content.lines();
        let header = lines
            .next()
            .ok_or_else(|| MtagError::InvalidInput("empty sumstats file".into()))?;

        // Parse header — support both whitespace and tab delimited.
        let headers: Vec<&str> = header.split_whitespace().collect();
        let find_col = |name: &str| -> Result<usize> {
            headers.iter().position(|h| *h == name).ok_or_else(|| {
                MtagError::InvalidInput(format!("column '{name}' not found in sumstats header"))
            })
        };

        let i_snp = find_col(col_snp)?;
        let i_z = find_col(col_z)?;
        let i_n = find_col(col_n)?;
        let i_freq = find_col(col_freq)?;
        let i_a1 = find_col(col_a1)?;
        let i_a2 = find_col(col_a2)?;
        let i_chr = if let Some(cn) = col_chr {
            headers.iter().position(|h| *h == cn)
        } else {
            None
        };
        let i_bp = if let Some(cn) = col_bp {
            headers.iter().position(|h| *h == cn)
        } else {
            None
        };

        let mut records = Vec::new();
        let mut seen = HashSet::new();
        for line in lines {
            if line.trim().is_empty() {
                continue;
            }
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.len() < headers.len() {
                continue;
            }
            let snp = cols[i_snp].to_string();
            if snp == "NA" || snp == "." || snp.is_empty() {
                continue;
            }
            // Drop duplicates (keep first occurrence, like pandas drop_duplicates).
            if !seen.insert(snp.clone()) {
                continue;
            }
            let z: f64 = cols[i_z].parse().unwrap_or(0.0);
            let n: f64 = cols[i_n].parse().unwrap_or(0.0);
            let freq: f64 = cols[i_freq].parse().unwrap_or(0.0);
            let a1 = cols[i_a1].to_uppercase();
            let a2 = cols[i_a2].to_uppercase();

            let chr = i_chr.and_then(|i| cols[i].parse().ok());
            let bp = i_bp.and_then(|i| cols[i].parse().ok());

            records.push(GwasRecord {
                snp,
                z,
                n,
                freq,
                a1,
                a2,
                chr,
                bp,
            });
        }

        Ok(TraitData { records })
    }
}

/// Merged GWAS data after allele harmonisation across all traits.
pub struct MergedData {
    /// M×P Z-score matrix.
    pub zs: Mat<f64>,
    /// M×P sample-size matrix.
    pub ns: Mat<f64>,
    /// M×P allele-frequency matrix.
    pub fs: Mat<f64>,
    /// SNP identifiers (length M).
    pub snps: Vec<String>,
    /// A1 allele per SNP (from trait 0).
    pub a1: Vec<String>,
    /// A2 allele per SNP (from trait 0).
    pub a2: Vec<String>,
    /// CHR per SNP (from trait 0, if available).
    pub chr: Vec<Option<i32>>,
    /// BP per SNP (from trait 0, if available).
    pub bp: Vec<Option<i64>>,
    /// Strand-ambiguous flag per SNP.
    pub strand_ambig: Vec<bool>,
}

/// Configuration for data loading and harmonisation.
#[derive(Clone, Debug)]
pub struct DataConfig {
    /// Include strand-ambiguous SNPs (default: exclude).
    pub incld_ambig_snps: bool,
    /// Minimum MAF filter (applied per-trait, default 0.01).
    pub maf_min: f64,
    /// Minimum sample-size filter (applied per-trait, None = no filter).
    pub n_min: Option<f64>,
    /// Maximum sample-size filter (None = no filter).
    pub n_max: Option<f64>,
}

impl Default for DataConfig {
    fn default() -> Self {
        Self {
            incld_ambig_snps: false,
            maf_min: 0.01,
            n_min: None,
            n_max: None,
        }
    }
}

/// Merge and harmonise GWAS sumstats across traits.
///
/// Port of `load_and_merge_data` + `extract_gwas_sumstats` in `mtag.py`.
///
/// Steps:
/// 1. Take the intersection of SNPs across all traits.
/// 2. For each SNP, check allele consistency with trait 0:
///    - Same orientation → keep.
///    - Flipped (A1↔A2 swapped) → flip Z sign and adjust FRQ.
///    - Inconsistent → drop.
/// 3. Drop strand-ambiguous SNPs (unless `incld_ambig_snps`).
/// 4. Apply MAF and N filters.
pub fn load_and_merge(traits: &[TraitData], cfg: &DataConfig) -> Result<MergedData> {
    if traits.is_empty() {
        return Err(MtagError::InvalidInput("no traits provided".into()));
    }
    let p = traits.len();

    // Build index maps for each trait: SNP → record index.
    let maps: Vec<std::collections::HashMap<&str, usize>> = traits
        .iter()
        .map(|t| {
            t.records
                .iter()
                .enumerate()
                .map(|(i, r)| (r.snp.as_str(), i))
                .collect()
        })
        .collect();

    // Intersection of SNPs across all traits (in trait-0 file order).
    let common_snps: Vec<&str> = traits[0]
        .records
        .iter()
        .map(|r| r.snp.as_str())
        .filter(|&snp| maps.iter().all(|m| m.contains_key(snp)))
        .collect();

    let mut zs_rows: Vec<Vec<f64>> = Vec::new();
    let mut ns_rows: Vec<Vec<f64>> = Vec::new();
    let mut fs_rows: Vec<Vec<f64>> = Vec::new();
    let mut snps: Vec<String> = Vec::new();
    let mut a1_out: Vec<String> = Vec::new();
    let mut a2_out: Vec<String> = Vec::new();
    let mut chr_out: Vec<Option<i32>> = Vec::new();
    let mut bp_out: Vec<Option<i64>> = Vec::new();
    let mut strand_ambig: Vec<bool> = Vec::new();

    for snp in &common_snps {
        // Get trait 0 record.
        let r0 = &traits[0].records[maps[0][snp]];
        let a1_0 = &r0.a1;
        let a2_0 = &r0.a2;

        let mut z_row = Vec::with_capacity(p);
        let mut n_row = Vec::with_capacity(p);
        let mut f_row = Vec::with_capacity(p);

        let mut keep = true;

        for (t_idx, m) in maps.iter().enumerate() {
            let rec = &traits[t_idx].records[m[snp]];

            if t_idx == 0 {
                z_row.push(rec.z);
                n_row.push(rec.n);
                f_row.push(rec.freq);
            } else {
                // Check allele orientation.
                let same = rec.a1 == *a1_0 && rec.a2 == *a2_0;
                let flipped = rec.a1 == *a2_0 && rec.a2 == *a1_0;
                if same {
                    z_row.push(rec.z);
                    n_row.push(rec.n);
                    f_row.push(rec.freq);
                } else if flipped {
                    z_row.push(-rec.z);
                    n_row.push(rec.n);
                    f_row.push(1.0 - rec.freq);
                } else {
                    keep = false;
                    break;
                }
            }
        }

        if !keep {
            continue;
        }

        // Strand ambiguity check.
        let sa = is_strand_ambiguous(a1_0, a2_0);
        if sa && !cfg.incld_ambig_snps {
            continue;
        }

        // MAF filter.
        let maf_pass = f_row.iter().all(|&freq| {
            let maf = freq.min(1.0 - freq);
            maf >= cfg.maf_min
        });
        if !maf_pass {
            continue;
        }

        // N filters.
        if let Some(n_min) = cfg.n_min {
            if !n_row.iter().all(|&n| n >= n_min) {
                continue;
            }
        }
        if let Some(n_max) = cfg.n_max {
            if !n_row.iter().all(|&n| n <= n_max) {
                continue;
            }
        }

        zs_rows.push(z_row);
        ns_rows.push(n_row);
        fs_rows.push(f_row);
        snps.push(snp.to_string());
        a1_out.push(a1_0.clone());
        a2_out.push(a2_0.clone());
        chr_out.push(r0.chr);
        bp_out.push(r0.bp);
        strand_ambig.push(sa);
    }

    let m = snps.len();
    if m == 0 {
        return Err(MtagError::InvalidInput(
            "no SNPs remain after merging and filtering".into(),
        ));
    }

    let zs = row_vecs_to_mat(&zs_rows, m, p);
    let ns = row_vecs_to_mat(&ns_rows, m, p);
    let fs = row_vecs_to_mat(&fs_rows, m, p);

    Ok(MergedData {
        zs,
        ns,
        fs,
        snps,
        a1: a1_out,
        a2: a2_out,
        chr: chr_out,
        bp: bp_out,
        strand_ambig,
    })
}

/// Convert a Vec of row-vectors into a faer Mat (row-major).
fn row_vecs_to_mat(rows: &[Vec<f64>], m: usize, p: usize) -> Mat<f64> {
    let mut mat = Mat::zeros(m, p);
    for i in 0..m {
        for j in 0..p {
            mat[(i, j)] = rows[i][j];
        }
    }
    mat
}

/// Subset the Z/N matrices to non-strand-ambiguous SNPs.
pub fn filter_strand_ambig(data: &MergedData) -> (Mat<f64>, Mat<f64>) {
    let keep: Vec<bool> = data.strand_ambig.iter().map(|&sa| !sa).collect();
    let m_new = keep.iter().filter(|&&b| b).count();
    let p = data.zs.ncols();
    let mut zs = Mat::zeros(m_new, p);
    let mut ns = Mat::zeros(m_new, p);
    let mut row = 0;
    for i in 0..data.zs.nrows() {
        if keep[i] {
            for j in 0..p {
                zs[(row, j)] = data.zs[(i, j)];
                ns[(row, j)] = data.ns[(i, j)];
            }
            row += 1;
        }
    }
    (zs, ns)
}

/// Output row for a single SNP–trait MTAG result.
#[derive(Clone, Debug)]
pub struct OutputRow {
    pub snp: String,
    pub chr: Option<i32>,
    pub bp: Option<i64>,
    pub a1: String,
    pub a2: String,
    pub z: f64,
    pub n: f64,
    pub freq: f64,
    pub mtag_beta: f64,
    pub mtag_se: f64,
    pub mtag_z: f64,
    pub mtag_pval: f64,
}

/// Build output rows for all SNPs for a given trait index.
///
/// Port of `save_mtag_results` in `mtag.py`.
pub fn build_output_rows(
    data: &MergedData,
    result: &MtagResult,
    trait_idx: usize,
    std_betas: bool,
) -> Vec<OutputRow> {
    let n = standard_normal_cdf_helper();
    let m = data.zs.nrows();
    let mut rows = Vec::with_capacity(m);

    for i in 0..m {
        let freq = data.fs[(i, trait_idx)];
        let weight = if std_betas {
            1.0
        } else {
            (2.0 * freq * (1.0 - freq)).sqrt()
        };
        let mtag_beta = result.mtag_betas[(i, trait_idx)] / weight;
        let mtag_se = result.mtag_se[(i, trait_idx)] / weight;
        let mtag_z = result.mtag_betas[(i, trait_idx)] / result.mtag_se[(i, trait_idx)];
        let mtag_pval = 2.0 * n.sf(mtag_z.abs());

        rows.push(OutputRow {
            snp: data.snps[i].clone(),
            chr: data.chr[i],
            bp: data.bp[i],
            a1: data.a1[i].clone(),
            a2: data.a2[i].clone(),
            z: data.zs[(i, trait_idx)],
            n: data.ns[(i, trait_idx)],
            freq,
            mtag_beta,
            mtag_se,
            mtag_z,
            mtag_pval,
        });
    }
    rows
}

/// Write output rows to a TSV file.
pub fn write_output_rows(rows: &[OutputRow], path: &Path) -> Result<()> {
    use std::io::Write;
    let mut f = std::fs::File::create(path)?;
    writeln!(
        f,
        "SNP\tCHR\tBP\tA1\tA2\tZ\tN\tFRQ\tmtag_beta\tmtag_se\tmtag_z\tmtag_pval"
    )?;
    for r in rows {
        writeln!(
            f,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            r.snp,
            r.chr.map_or("NA".to_string(), |c| c.to_string()),
            r.bp.map_or("NA".to_string(), |b| b.to_string()),
            r.a1,
            r.a2,
            r.z,
            r.n,
            r.freq,
            r.mtag_beta,
            r.mtag_se,
            r.mtag_z,
            r.mtag_pval,
        )?;
    }
    Ok(())
}

/// Write a P×P matrix to a tab-delimited file.
pub fn write_matrix(mat: &Mat<f64>, path: &Path) -> Result<()> {
    use std::io::Write;
    let mut f = std::fs::File::create(path)?;
    for i in 0..mat.nrows() {
        let row: Vec<String> = (0..mat.ncols()).map(|j| mat[(i, j)].to_string()).collect();
        writeln!(f, "{}", row.join("\t"))?;
    }
    Ok(())
}

fn standard_normal_cdf_helper() -> statrs::distribution::Normal {
    statrs::distribution::Normal::new(0.0, 1.0).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_strand_ambiguous() {
        assert!(is_strand_ambiguous("A", "T"));
        assert!(is_strand_ambiguous("C", "G"));
        assert!(!is_strand_ambiguous("A", "G"));
        assert!(!is_strand_ambiguous("A", "C"));
    }

    #[test]
    fn test_load_and_merge_simple() {
        let content1 =
            "SNP\tZ\tN\tFRQ\tA1\tA2\nrs1\t2.0\t100\t0.3\tA\tG\nrs2\t1.5\t100\t0.4\tA\tG\n";
        let content2 =
            "SNP\tZ\tN\tFRQ\tA1\tA2\nrs1\t1.8\t200\t0.3\tA\tG\nrs2\t-1.5\t200\t0.6\tG\tA\n";

        let t1 =
            TraitData::from_str(content1, "SNP", "Z", "N", "FRQ", "A1", "A2", None, None).unwrap();
        let t2 =
            TraitData::from_str(content2, "SNP", "Z", "N", "FRQ", "A1", "A2", None, None).unwrap();

        let cfg = DataConfig {
            maf_min: 0.0,
            ..Default::default()
        };
        let merged = load_and_merge(&[t1, t2], &cfg).unwrap();

        assert_eq!(merged.snps.len(), 2);
        // rs2 is flipped: Z should be -(-1.5) = 1.5
        assert!((merged.zs[(1, 1)] - 1.5).abs() < 1e-10);
    }
}
