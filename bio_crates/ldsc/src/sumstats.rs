//! High-level LD-Score-Regression drivers — a port of `ldscore/sumstats.py`,
//! wiring the file readers ([`crate::io`]) to the estimators ([`crate::regress`]).
//!
//! Provides the `--h2` and `--rg` flows: read summary statistics + reference LD
//! Scores + weight LD Scores, inner-join on SNP (in genomic order), drop
//! zero-variance LD columns, apply the χ²ₘₐₓ filter, and run the regression.
//! A [`Logger`] trait replaces Python's `Logger` object.

use std::collections::HashMap;
use std::io::Write;

use faer::Mat;

use crate::io::{self, LdScores, SumStats, read_ldscore, read_m};
use crate::linalg::cond_number;
use crate::regress::{Gencov, Hsq, RG, gencov_obs_to_liab, h2_obs_to_liab};
use crate::{LdscError, Result};

/// Number of autosomes LDSC splits files across (port of `sumstats._N_CHR`).
pub const N_CHR: usize = 22;

/// Receives log lines (like Python `Logger`).
pub trait Logger {
    fn log(&mut self, msg: &str);
}

/// A no-op logger.
pub struct NullLogger;
impl Logger for NullLogger {
    fn log(&mut self, _msg: &str) {}
}

/// A logger that writes to any `Write`.
pub struct WriteLogger<W: Write> {
    pub w: W,
}
impl<W: Write> Logger for WriteLogger<W> {
    fn log(&mut self, msg: &str) {
        let _ = writeln!(self.w, "{msg}");
    }
}

// ---------------------------------------------------------------------------
// Helpers porting sumstats.py internals
// ---------------------------------------------------------------------------

/// `_check_variance` — drop LD-Score columns with zero variance (and their M).
/// Returns `(kept_M, kept_ld_cols, novar_mask)`.
#[allow(clippy::type_complexity)]
pub fn check_variance(
    ld_cols: &[Vec<f64>], // column-major LD scores (excl. SNP)
    m: &[f64],
) -> Result<(Vec<f64>, Vec<Vec<f64>>, Vec<bool>)> {
    let n_annot = ld_cols.len();
    if n_annot == 0 {
        return Err(LdscError::InvalidInput("no LD columns".into()));
    }
    let n = ld_cols[0].len();
    let mut novar = vec![false; n_annot];
    for k in 0..n_annot {
        let col = &ld_cols[k];
        let mean = col.iter().sum::<f64>() / n as f64;
        let var: f64 = col.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n as f64;
        novar[k] = var == 0.0;
    }
    if novar.iter().all(|&x| x) {
        return Err(LdscError::InvalidInput(
            "All LD Scores have zero variance.".into(),
        ));
    }
    let kept_m: Vec<f64> = (0..n_annot).filter(|&k| !novar[k]).map(|k| m[k]).collect();
    let kept_cols: Vec<Vec<f64>> = (0..n_annot)
        .filter(|&k| !novar[k])
        .map(|k| ld_cols[k].clone())
        .collect();
    Ok((kept_m, kept_cols, novar))
}

/// `_check_ld_condnum` — raise (unless `invert_anyway`) when the LD-Score
/// matrix condition number exceeds 1e5.
pub fn check_ld_condnum(ref_ld: &Mat<f64>, invert_anyway: bool) -> Result<()> {
    let cn = cond_number(ref_ld.as_ref())?;
    if cn > 100_000.0 {
        if invert_anyway {
            Ok(())
        } else {
            Err(LdscError::InvalidInput(format!(
                "LD Score matrix condition number is {cn}. Remove collinear LD Scores."
            )))
        }
    } else {
        Ok(())
    }
}

/// `_filter_alleles(alleles)` — keep SNPs whose 4-char allele string is in
/// `MATCH_ALLELES`. `alleles[i] = A1+A2+A1x+A2x`.
pub fn filter_alleles(alleles: &[String]) -> Vec<bool> {
    alleles
        .iter()
        .map(|a| crate::alleles::alleles_match(a))
        .collect()
}

/// `_align_alleles(z, alleles)` — negate Z2 where a reference flip is needed.
pub fn align_alleles(z: &[f64], alleles: &[String]) -> Vec<f64> {
    z.iter()
        .zip(alleles.iter())
        .map(|(&zi, a)| {
            if crate::alleles::flip_alleles(a) {
                -zi
            } else {
                zi
            }
        })
        .collect()
}

/// `smart_merge`-style inner join of two SNP-keyed tables, preserving the left
/// order. Returns the indices of left SNPs present in right, plus the aligned
/// right rows.
fn index_of(snps: &[String]) -> HashMap<String, usize> {
    snps.iter()
        .enumerate()
        .map(|(i, s)| (s.clone(), i))
        .collect()
}

// ---------------------------------------------------------------------------
// Joined arrays
// ---------------------------------------------------------------------------

/// The per-SNP arrays needed for an h² regression, after joining
/// sumstats ⨝ ref_ld ⨝ w_ld on SNP (in ref_ld's genomic order).
pub struct JoinedH2 {
    pub snp: Vec<String>,
    pub chisq: Vec<f64>,
    pub n: Vec<f64>,
    pub ref_ld: Mat<f64>, // n × n_annot
    pub w_ld: Vec<f64>,
    pub ref_ld_cnames: Vec<String>,
}

fn join_h2(sumstats: &SumStats, ref_ld: &LdScores, w_ld: &LdScores) -> Result<JoinedH2> {
    if w_ld.colnames.len() != 1 {
        return Err(LdscError::InvalidInput(
            "--w-ld may only have one LD Score column.".into(),
        ));
    }
    let ss_idx = index_of(&sumstats.snp);
    let wld_idx = index_of(&w_ld.snp);
    let n_annot = ref_ld.colnames.len();

    let mut snp = Vec::new();
    let mut chisq = Vec::new();
    let mut n = Vec::new();
    let mut wld = Vec::new();
    let mut ref_cols: Vec<Vec<f64>> = vec![Vec::new(); n_annot];
    for (i, s) in ref_ld.snp.iter().enumerate() {
        let (Some(&ssi), Some(&wi)) = (ss_idx.get(s), wld_idx.get(s)) else {
            continue;
        };
        snp.push(s.clone());
        let z = sumstats.z[ssi];
        chisq.push(z * z);
        n.push(sumstats.n[ssi]);
        wld.push(w_ld.cols[0][wi]);
        for k in 0..n_annot {
            ref_cols[k].push(ref_ld.cols[k][i]);
        }
    }
    if snp.is_empty() {
        return Err(LdscError::InvalidInput(
            "After merging with reference panel LD, 0 SNPs remain.".into(),
        ));
    }
    let nrows = snp.len();
    let ref_ld_mat = Mat::from_fn(nrows, n_annot, |i, k| ref_cols[k][i]);
    Ok(JoinedH2 {
        snp,
        chisq,
        n,
        ref_ld: ref_ld_mat,
        w_ld: wld,
        ref_ld_cnames: ref_ld.colnames.clone(),
    })
}

// ---------------------------------------------------------------------------
// estimate_h2 from files
// ---------------------------------------------------------------------------

/// Configuration for the `--h2` flow (the Python `estimate_h2` driver).
pub struct H2Config {
    /// `.sumstats` file path.
    pub sumstats: String,
    /// Reference LD prefix (single fileset) — set one of `ref_ld` / `ref_ld_chr`.
    pub ref_ld: Option<String>,
    /// Reference LD prefix split across 22 chromosomes.
    pub ref_ld_chr: Option<String>,
    /// Weight LD prefix (single fileset).
    pub w_ld: Option<String>,
    pub w_ld_chr: Option<String>,
    /// Per-annotation M. If `None`, read from the `.l2.M[_5_50]` next to ref_ld.
    pub m: Option<Vec<f64>>,
    /// Use `.l2.M_5_50` (default) vs `.l2.M`.
    pub not_m_5_50: bool,
    pub n_blocks: usize,
    pub intercept_h2: Option<f64>,
    pub two_step: Option<f64>,
    pub chisq_max: Option<f64>,
    pub invert_anyway: bool,
}

impl Default for H2Config {
    fn default() -> Self {
        H2Config {
            sumstats: String::new(),
            ref_ld: None,
            ref_ld_chr: None,
            w_ld: None,
            w_ld_chr: None,
            m: None,
            not_m_5_50: false,
            n_blocks: 200,
            intercept_h2: None,
            two_step: None,
            chisq_max: None,
            invert_anyway: false,
        }
    }
}

fn resolve_ldscore(
    prefix_single: &Option<String>,
    prefix_chr: &Option<String>,
) -> Result<LdScores> {
    if let Some(p) = prefix_single {
        read_ldscore(p, None)
    } else if let Some(p) = prefix_chr {
        read_ldscore(p, Some(N_CHR as u32))
    } else {
        Err(LdscError::InvalidInput("no ref_ld / w_ld prefix".into()))
    }
}

/// Run the `--h2` driver: read files, merge, filter, regress. Returns the
/// fitted [`Hsq`] (whose `.reg` carries coef/cat/tot/prop/... and `.mean_chisq`,
/// `.lambda_gc`, `.ratio`).
pub fn estimate_h2_from_files(cfg: &H2Config, log: &mut dyn Logger) -> Result<Hsq> {
    Ok(estimate_h2_full(cfg, log)?.0)
}

/// Same pipeline as [`estimate_h2_from_files`], but additionally returns the
/// (variance-filtered) ref_ld column names and the regression SNP count — the
/// pieces the S-LDSC result layer ([`crate::sldsc::build_sldsc_results`]) needs
/// to label per-annotation rows.
///
/// Returns `(Hsq, ref_ld_cnames, n_snp)`.
fn estimate_h2_full(cfg: &H2Config, log: &mut dyn Logger) -> Result<(Hsq, Vec<String>, usize)> {
    let sumstats = io::read_sumstats(&cfg.sumstats, false, true)?;
    log.log(&format!(
        "Read summary statistics for {} SNPs.",
        sumstats.len()
    ));
    let ref_ld = resolve_ldscore(&cfg.ref_ld, &cfg.ref_ld_chr)?;
    let n_annot = ref_ld.colnames.len();
    let m = match &cfg.m {
        Some(m) => m.clone(),
        None => {
            let prefix = cfg
                .ref_ld
                .clone()
                .or_else(|| cfg.ref_ld_chr.clone())
                .unwrap();
            read_m(
                &prefix,
                cfg.ref_ld_chr.as_ref().map(|_| N_CHR as u32),
                2,
                !cfg.not_m_5_50,
            )?
        }
    };
    if m.len() != n_annot {
        return Err(LdscError::DimensionMismatch(format!(
            "# terms in M ({}) must match # of LD Scores ({n_annot})",
            m.len()
        )));
    }
    let w_ld = resolve_ldscore(&cfg.w_ld, &cfg.w_ld_chr)?;

    // join
    let mut joined = join_h2(&sumstats, &ref_ld, &w_ld)?;
    let nrows = joined.snp.len();
    log.log(&format!(
        "After merging with reference panel LD, {nrows} SNPs remain."
    ));

    // check_variance (operate per-column)
    let (m_keep, ref_cols_keep, _novar) = check_variance(
        &(0..joined.ref_ld.ncols())
            .map(|k| joined.ref_ld.col_as_vec(k))
            .collect::<Vec<_>>(),
        &m,
    )?;
    // rebuild ref_ld from kept columns
    let n_keep = ref_cols_keep.len();
    joined.ref_ld = Mat::from_fn(nrows, n_keep, |i, k| ref_cols_keep[k][i]);
    joined.ref_ld_cnames = joined
        .ref_ld_cnames
        .iter()
        .zip(_novar.iter())
        .filter(|(_, nv)| !**nv)
        .map(|(c, _)| c.clone())
        .collect();

    check_ld_condnum(&joined.ref_ld, cfg.invert_anyway)?;
    let n_blocks = cfg.n_blocks.min(nrows);
    let n_annot_now = n_keep;

    let mut chisq_max = cfg.chisq_max;
    let mut old_weights = false;
    let mut two_step = cfg.two_step;
    if n_annot_now == 1 {
        if two_step.is_none() && cfg.intercept_h2.is_none() {
            two_step = Some(30.0);
        }
    } else {
        old_weights = true;
        if chisq_max.is_none() {
            let nmax = joined.n.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            chisq_max = Some((0.001 * nmax).max(80.0));
        }
    }

    // apply chisq_max
    if let Some(cm) = chisq_max {
        let mask: Vec<bool> = joined.chisq.iter().map(|c| *c < cm).collect();
        let kept: usize = mask.iter().filter(|x| **x).count();
        let removed = nrows - kept;
        log.log(&format!(
            "Removed {removed} SNPs with chi^2 > {cm} ({kept} SNPs remain)"
        ));
        filter_in_place(&mut joined, &mask);
    }

    if let Some(ts) = two_step {
        log.log(&format!("Using two-step estimator with cutoff at {ts}."));
    }

    let n_snp = joined.snp.len();
    let cnames = joined.ref_ld_cnames.clone();
    let chisq = joined.chisq.clone();
    let hsq = Hsq::new(
        &chisq,
        &joined.ref_ld,
        &joined.w_ld,
        &joined.n,
        &m_keep,
        n_blocks,
        cfg.intercept_h2,
        two_step,
        old_weights,
    )?;
    Ok((hsq, cnames, n_snp))
}

/// Run the stratified LD Score Regression (`--h2` with multiple ref_ld
/// annotations) driver and return a per-annotation result table
/// ([`crate::sldsc::SldscResults`]).
///
/// This shares the exact read/merge/filter/regress pipeline of
/// [`estimate_h2_from_files`] (so the S-LDSC defaults — `old_weights` and
/// `chisq_max = max(0.001·Nmax, 80)` for `n_annot > 1` — are applied
/// automatically) and then derives the partitioned quantities (per-annotation
/// h², proportion, enrichment, and their SE / z / p) via
/// [`crate::sldsc::build_sldsc_results`].
pub fn estimate_sldsc(cfg: &H2Config, log: &mut dyn Logger) -> Result<crate::sldsc::SldscResults> {
    let (hsq, cnames, n_snp) = estimate_h2_full(cfg, log)?;
    Ok(crate::sldsc::build_sldsc_results(&hsq, &cnames, n_snp))
}

/// Port of `_print_cov` / `_print_delete_values` — write the jackknife delete
/// values / coef covariance for an [`Hsq`].
pub fn print_hsq_outputs(
    hsq: &Hsq,
    out_prefix: &str,
    print_cov: bool,
    print_delete: bool,
) -> Result<()> {
    if print_cov {
        let mut f = std::fs::File::create(format!("{out_prefix}.cov"))?;
        write_mat(&mut f, &hsq.reg.coef_cov, hsq.reg.n_annot)?;
    }
    if print_delete {
        let mut f = std::fs::File::create(format!("{out_prefix}.delete"))?;
        for v in &hsq.reg.tot_delete_values {
            use std::io::Write;
            writeln!(f, "{v}")?;
        }
    }
    Ok(())
}

fn write_mat(f: &mut std::fs::File, flat: &[f64], n: usize) -> Result<()> {
    use std::io::Write;
    for r in 0..n {
        let row: Vec<String> = (0..n).map(|c| format!("{}", flat[r * n + c])).collect();
        writeln!(f, "{}", row.join("\t"))?;
    }
    Ok(())
}

/// Liability-scale h² for an [`Hsq`], given sample/pop prevalence.
pub fn hsq_to_liability(hsq: &Hsq, samp_prev: Option<f64>, pop_prev: Option<f64>) -> Result<f64> {
    match (samp_prev, pop_prev) {
        (Some(p), Some(k)) => h2_obs_to_liab(hsq.reg.tot, p, k),
        _ => Ok(hsq.reg.tot),
    }
}

/// Format the `Hsq.summary()` text (port of `Hsq.summary`).
pub fn hsq_summary(
    hsq: &Hsq,
    ref_ld_cnames: &[String],
    samp_prev: Option<f64>,
    pop_prev: Option<f64>,
) -> Result<String> {
    let liability = matches!((samp_prev, pop_prev), (Some(_), Some(_)));
    let scale = if liability { "Liability" } else { "Observed" };
    let c = if liability {
        h2_obs_to_liab(1.0, samp_prev.unwrap(), pop_prev.unwrap())?
    } else {
        1.0
    };
    let mut out = vec![format!(
        "Total {scale} scale h2: {:.4} ({:.4})",
        c * hsq.reg.tot,
        c * hsq.reg.tot_se
    )];
    if hsq.reg.n_annot > 1 {
        out.push(format!("Categories: {}", ref_ld_cnames.join(" ")));
        let fmt_v = |v: &[f64]| {
            v.iter()
                .map(|x| format!("{:.4}", x))
                .collect::<Vec<_>>()
                .join(" ")
        };
        out.push(format!("{} scale h2: {}", scale, fmt_v(&hsq.reg.cat)));
        out.push(format!("{} scale h2 SE: {}", scale, fmt_v(&hsq.reg.cat_se)));
        out.push(format!("Proportion of SNPs: {}", fmt_v(&hsq.reg.m_prop)));
        out.push(format!("Proportion of h2g: {}", fmt_v(&hsq.reg.prop)));
        out.push(format!("Enrichment: {}", fmt_v(&hsq.reg.enrichment)));
        out.push(format!("Coefficients: {}", fmt_v(&hsq.reg.coef)));
        out.push(format!("Coefficient SE: {}", fmt_v(&hsq.reg.coef_se)));
    }
    out.push(format!("Lambda GC: {:.4}", hsq.lambda_gc));
    out.push(format!("Mean Chi^2: {:.4}", hsq.mean_chisq));
    match hsq.reg.intercept {
        Some(ic) => {
            out.push(format!(
                "Intercept: {:.4} ({:.4})",
                ic,
                hsq.reg.intercept_se.unwrap_or(f64::NAN)
            ));
            if hsq.mean_chisq > 1.0 {
                if let Some(r) = hsq.ratio {
                    out.push(if r < 0.0 {
                        "Ratio < 0 (usually indicates GC correction).".into()
                    } else {
                        format!("Ratio: {:.4} ({:.4})", r, hsq.ratio_se.unwrap_or(f64::NAN))
                    });
                }
            } else {
                out.push("Ratio: NA (mean chi^2 < 1)".into());
            }
        }
        None => out.push("Intercept: constrained".into()),
    }
    Ok(out.join("\n"))
}

/// Port of the `_get_rg_table` genetic-correlation summary row for one pair.
pub fn rg_summary(rg: &RG) -> String {
    let na = |x: f64| {
        if x.is_nan() {
            "NA".into()
        } else {
            format!("{:.4}", x)
        }
    };
    format!(
        "rg {} se {} z {} p {}",
        na(rg.rg_ratio),
        na(rg.rg_se),
        na(rg.z),
        na(rg.p)
    )
}

/// Liability-scaled gencov (helper exposing [`gencov_obs_to_liab`]).
pub fn gencov_to_liability(
    gencov: &Gencov,
    p1: Option<f64>,
    p2: Option<f64>,
    k1: Option<f64>,
    k2: Option<f64>,
) -> Result<f64> {
    gencov_obs_to_liab(gencov.reg.tot, p1, p2, k1, k2)
}

fn filter_in_place(j: &mut JoinedH2, mask: &[bool]) {
    let keep: Vec<usize> = (0..j.snp.len()).filter(|&i| mask[i]).collect();
    let new_n = keep.len();
    let n_annot = j.ref_ld.ncols();
    let new_ref = Mat::from_fn(new_n, n_annot, |r, k| j.ref_ld[(keep[r], k)]);
    j.ref_ld = new_ref;
    j.snp = keep.iter().map(|&i| j.snp[i].clone()).collect();
    j.chisq = keep.iter().map(|&i| j.chisq[i]).collect();
    j.n = keep.iter().map(|&i| j.n[i]).collect();
    j.w_ld = keep.iter().map(|&i| j.w_ld[i]).collect();
}

/// Helper: materialize a faer column into a Vec.
trait ColAsVec {
    fn col_as_vec(&self, k: usize) -> Vec<f64>;
}
impl ColAsVec for Mat<f64> {
    fn col_as_vec(&self, k: usize) -> Vec<f64> {
        (0..self.nrows()).map(|i| self[(i, k)]).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linalg::build_mat_row_major;

    #[test]
    fn check_variance_drops_zero_var_col() {
        // LD1 all ones (zero var), LD2 arange. Keep LD2.
        let ld = vec![vec![1.0, 1.0, 1.0], vec![0.0, 1.0, 2.0]];
        let m = vec![1.0, 2.0];
        let (mk, lk, novar) = check_variance(&ld, &m).unwrap();
        assert_eq!(mk, vec![2.0]);
        assert_eq!(lk, vec![vec![0.0, 1.0, 2.0]]);
        assert_eq!(novar, vec![true, false]);
    }

    #[test]
    fn check_variance_all_zero_errors() {
        let ld = vec![vec![1.0, 1.0]];
        assert!(check_variance(&ld, &[1.0]).is_err());
    }

    #[test]
    fn check_condnum_raises_on_ill_conditioned() {
        let a = build_mat_row_major(&[vec![1.0, 1.0], vec![1.0, 1.0 + 1e-5]]);
        assert!(check_ld_condnum(&a, false).is_err());
        assert!(check_ld_condnum(&a, true).is_ok());
    }

    #[test]
    fn filter_and_align_alleles() {
        // test_align_alleles: align → [1,1,1,-1,1,1]
        let alleles = vec![
            "ACAC".into(),
            "TGTG".into(),
            "GTGT".into(),
            "AGCT".into(),
            "AGTC".into(),
            "TCTC".into(),
        ];
        let beta = vec![1.0; 6];
        let aligned = align_alleles(&beta, &alleles);
        assert_eq!(aligned, vec![1.0, 1.0, 1.0, -1.0, 1.0, 1.0]);

        // test_filter_bad_alleles: ATAT, ATAG, DIID invalid; ACAC valid.
        let bad = vec!["ATAT".into(), "ATAG".into(), "DIID".into(), "ACAC".into()];
        assert_eq!(filter_alleles(&bad), vec![false, false, false, true]);
    }

    #[test]
    fn match_alleles_set_matches_python() {
        // The exact 32-element MATCH_ALLELES set from test_match_alleles.
        let expected: Vec<&str> = vec![
            "ACAC", "ACCA", "ACGT", "ACTG", "AGAG", "AGCT", "AGGA", "AGTC", "CAAC", "CACA", "CAGT",
            "CATG", "CTAG", "CTCT", "CTGA", "CTTC", "GAAG", "GACT", "GAGA", "GATC", "GTAC", "GTCA",
            "GTGT", "GTTG", "TCAG", "TCCT", "TCGA", "TCTC", "TGAC", "TGCA", "TGGT", "TGTG",
        ];
        let valid = crate::alleles::valid_snps();
        let mut got: Vec<String> = Vec::new();
        for s1 in &valid {
            for s2 in &valid {
                let four = format!("{s1}{s2}");
                if crate::alleles::alleles_match(&four) {
                    got.push(four);
                }
            }
        }
        let mut exp: Vec<String> = expected.iter().map(|s| s.to_string()).collect();
        exp.sort();
        got.sort();
        assert_eq!(got, exp);
    }
}
