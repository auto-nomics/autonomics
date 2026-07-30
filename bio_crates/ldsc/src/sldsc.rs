//! Stratified LD Score Regression (S-LDSC) result layer.
//!
//! The numerical regression itself is shared with univariate h² —
//! [`crate::regress::Hsq`] already supports `n_annot > 1` and its
//! [`crate::regress::LdScoreRegression`] carries every per-annotation quantity
//! (coef / cat / prop / enrichment / m_prop, with block-jackknife SE). This
//! module turns that fitted [`Hsq`] into the S-LDSC deliverables: a
//! per-annotation result table ([`SldscAnnotResult`]) plus the totals
//! ([`SldscResults`]), and a `.results`-style TSV writer
//! ([`write_results`]) aligned with the Python LDSC output columns.
//!
//! ## Enrichment SE / p convention
//! In the non-overlap regime `enrichment = (cat/M_k) / (tot/M_tot) =
//! prop / m_prop` (see Finucane et al. 2015, Nat Genet). Since `m_prop` is a
//! constant, `enrichment_se = prop_se / m_prop`, and the reported
//! `enrichment_p` is the two-sided test of `H₀: enrichment = 1`:
//! `z = (enrichment − 1) / enrichment_se`, `p = 2·Φ(−|z|)`.
//! `coef_p` is likewise the two-sided test of `H₀: β_k = 0`.

use crate::Result;
use crate::regress::{Hsq, p_z_norm};

/// One row of the S-LDSC per-annotation results table (mirrors the Python
/// `.results` columns, with the two-sided p-values added).
#[derive(Debug, Clone)]
pub struct SldscAnnotResult {
    /// ref_ld column / annotation name (`Category`).
    pub category: String,
    /// Proportion of SNPs in the category `M_k / M_tot` (`Prop._SNPs`).
    pub m_prop: f64,
    /// Per-annotation coefficient `β_k` (`Coefficient`).
    pub coef: f64,
    /// SE of `coef` (`Coefficient_std_error`).
    pub coef_se: f64,
    /// `coef / coef_se` (`Coefficient_z-score`).
    pub coef_z: f64,
    /// Two-sided p-value for `H₀: β_k = 0` (`Coefficient_P_value`).
    pub coef_p: f64,
    /// Per-annotation h² `M_k·β_k` (`Prop._h2`).
    pub cat: f64,
    /// SE of `cat` (`Prop._h2_std_error`).
    pub cat_se: f64,
    /// `(cat/M_k) / (tot/M_tot)` (`Enrichment`).
    pub enrichment: f64,
    /// SE of `enrichment`, derived as `prop_se / m_prop`
    /// (`Enrichment_std_error`).
    pub enrichment_se: f64,
    /// Two-sided p-value for `H₀: enrichment = 1` (`Enrichment_p`).
    pub enrichment_p: f64,
}

/// Full S-LDSC output: a per-annotation table plus the whole-regression totals.
#[derive(Debug, Clone)]
pub struct SldscResults {
    /// One row per (variance-retained) annotation.
    pub annotations: Vec<SldscAnnotResult>,
    /// Total SNP-heritability `Σ_k cat` and its SE.
    pub tot: f64,
    pub tot_se: f64,
    /// Regression intercept (`None` under a constrained intercept).
    pub intercept: Option<f64>,
    pub intercept_se: Option<f64>,
    /// LD Score regression ratio `(intercept − 1) / (meanχ² − 1)`.
    pub ratio: Option<f64>,
    pub ratio_se: Option<f64>,
    /// Mean per-SNP χ².
    pub mean_chisq: f64,
    /// Genomic-control λ: `median(χ²) / 0.4549`.
    pub lambda_gc: f64,
    /// Number of SNPs in the regression (post chisq_max filter).
    pub n_snp: usize,
    /// Number of (variance-retained) annotations.
    pub n_annot: usize,
}

/// Build the S-LDSC result table from a fitted [`Hsq`].
///
/// * `hsq` — the fitted regression (from [`crate::sumstats::estimate_h2_full`]
///   or [`crate::regress::Hsq::new`]).
/// * `cnames` — the variance-retained ref_ld column names, length `n_annot`.
/// * `n_snp` — the regression SNP count (post chisq_max filter).
///
/// Pure / allocation-only: no I/O, no further fitting — extracted so it can be
/// unit-tested without touching files.
pub fn build_sldsc_results(hsq: &Hsq, cnames: &[String], n_snp: usize) -> SldscResults {
    let reg = &hsq.reg;
    let n_annot = reg.n_annot;
    debug_assert_eq!(cnames.len(), n_annot, "cnames length must equal n_annot");

    let annotations = (0..n_annot)
        .map(|k| {
            let category = cnames.get(k).cloned().unwrap_or_else(|| format!("CAT_{k}"));
            let m_prop = reg.m_prop[k];
            let coef = reg.coef[k];
            let coef_se = reg.coef_se[k];
            let (coef_p, coef_z) = p_z_norm(coef, coef_se);

            let cat = reg.cat[k];
            let cat_se = reg.cat_se[k];
            let prop_se = reg.prop_se[k];
            let enrichment = reg.enrichment[k];

            // enrichment = prop / m_prop  ⇒  se = prop_se / m_prop.
            // Guard the degenerate m_prop == 0 case (p_z_norm already handles
            // se == 0 / non-finite by returning (0, ±∞)).
            let enrichment_se = if m_prop > 0.0 && prop_se.is_finite() {
                prop_se / m_prop
            } else {
                0.0
            };
            // Two-sided test of enrichment == 1.
            let (enrichment_p, _enrichment_z) = p_z_norm(enrichment - 1.0, enrichment_se);

            SldscAnnotResult {
                category,
                m_prop,
                coef,
                coef_se,
                coef_z,
                coef_p,
                cat,
                cat_se,
                enrichment,
                enrichment_se,
                enrichment_p,
            }
        })
        .collect();

    SldscResults {
        annotations,
        tot: reg.tot,
        tot_se: reg.tot_se,
        intercept: reg.intercept,
        intercept_se: reg.intercept_se,
        ratio: hsq.ratio,
        ratio_se: hsq.ratio_se,
        mean_chisq: hsq.mean_chisq,
        lambda_gc: hsq.lambda_gc,
        n_snp,
        n_annot,
    }
}

/// `.results` column order, matching Python LDSC's `.results` TSV (with the
/// two-sided p-value columns appended). Keep in sync with [`write_results`].
const RESULTS_COLUMNS: &[&str] = &[
    "Category",
    "Prop._SNPs",
    "Prop._h2",
    "Prop._h2_std_error",
    "Enrichment",
    "Enrichment_std_error",
    "Enrichment_p",
    "Coefficient",
    "Coefficient_std_error",
    "Coefficient_z-score",
    "Coefficient_P_value",
];

/// Write the per-annotation results table as a tab-separated `.results` file,
/// matching the Python LDSC column order (with `Coefficient_P_value` appended).
///
/// Totals (h², intercept, λ_GC, …) are not part of the `.results` format; the
/// caller can log them via [`crate::sumstats::hsq_summary`] if needed.
pub fn write_results(r: &SldscResults, path: &str) -> Result<()> {
    use std::io::Write;
    let mut f = std::fs::File::create(path)?;
    writeln!(f, "{}", RESULTS_COLUMNS.join("\t"))?;
    for a in &r.annotations {
        writeln!(
            f,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            a.category,
            fmt_num(a.m_prop),
            fmt_num(a.cat),
            fmt_num(a.cat_se),
            fmt_num(a.enrichment),
            fmt_num(a.enrichment_se),
            fmt_num(a.enrichment_p),
            fmt_num(a.coef),
            fmt_num(a.coef_se),
            fmt_num(a.coef_z),
            fmt_num(a.coef_p),
        )?;
    }
    Ok(())
}

/// Format a float like numpy's default `str()`: plain decimal, no trailing
/// noise. Good enough for a TSV the user reads / diffs against Python.
fn fmt_num(x: f64) -> String {
    if !x.is_finite() {
        // Match the LDSC "NA" sentinel for non-finite values.
        "NA".to_string()
    } else {
        format!("{x}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linalg::build_mat_row_major;
    use crate::regress::Hsq;
    use crate::sumstats::{H2Config, NullLogger, estimate_sldsc};

    /// Build a deterministic 2-annotation `Hsq` whose per-annotation h² is
    /// known, reusing the synthesis pattern from
    /// `regress::tests::hsq_coef_cat_tot_prop_enrichment`.
    fn fitted_two_annot(hsq1: f64, hsq2: f64) -> Hsq {
        let m = vec![1e7 / 2.0; 2];
        let n = 400usize;
        let rows: Vec<Vec<f64>> = (0..n)
            .map(|i| {
                let a = ((i as f64 * 12.9898).sin() * 43758.5453).fract().abs() + 1.0;
                let b = ((i as f64 * 78.233).sin() * 43758.5453).fract().abs() + 1.0;
                vec![a, b]
            })
            .collect();
        let ld = build_mat_row_major(&rows);
        let nvec = vec![1e5; n];
        let chisq: Vec<f64> = (0..n)
            .map(|i| 1.0 + 1e5 * (ld[(i, 0)] * hsq1 / m[0] + ld[(i, 1)] * hsq2 / m[1]))
            .collect();
        let wld = vec![1.0; n];
        // constrained intercept, old_weights (the n_annot>1 path)
        Hsq::new(&chisq, &ld, &wld, &nvec, &m, 3, Some(1.0), None, true).unwrap()
    }

    #[test]
    fn build_results_recovers_per_annot_quantities() {
        let hsq1 = 0.2_f64;
        let hsq2 = 0.7_f64;
        let hsq = fitted_two_annot(hsq1, hsq2);
        let cnames = vec!["AL2".to_string(), "BL2".to_string()];
        let r = build_sldsc_results(&hsq, &cnames, 400);

        assert_eq!(r.n_annot, 2);
        assert_eq!(r.n_snp, 400);
        // per-annot h² recovered
        assert!(
            (r.annotations[0].cat - hsq1).abs() < 1e-6,
            "cat0={}",
            r.annotations[0].cat
        );
        assert!(
            (r.annotations[1].cat - hsq2).abs() < 1e-6,
            "cat1={}",
            r.annotations[1].cat
        );
        // tot == Σ cat
        assert!((r.tot - (hsq1 + hsq2)).abs() < 1e-6, "tot={}", r.tot);
        assert!((r.tot - r.annotations.iter().map(|a| a.cat).sum::<f64>()).abs() < 1e-9);
        // m_prop sums to 1 (equal M ⇒ 0.5 each)
        let mp_sum: f64 = r.annotations.iter().map(|a| a.m_prop).sum();
        assert!((mp_sum - 1.0).abs() < 1e-9);
        assert!((r.annotations[0].m_prop - 0.5).abs() < 1e-9);
        // coef_z == coef / coef_se
        let a = &r.annotations[0];
        assert!((a.coef_z - a.coef / a.coef_se).abs() < 1e-6);
        // enrichment_se ≈ prop_se / m_prop
        // (prop = cat/tot ⇒ prop_se from jackknife; enrichment = prop/m_prop)
        let expected_enrich = (a.cat / 1e7 * 2.0) / ((hsq1 + hsq2) / 1e7);
        assert!(
            (a.enrichment - expected_enrich).abs() < 1e-6,
            "enrich={}",
            a.enrichment
        );
        // equal M ⇒ enrichment == 2·cat/tot == prop/m_prop
        assert!((a.enrichment - (a.cat / r.tot) / a.m_prop).abs() < 1e-9);
        // p-values in [0,1] and finite
        for a in &r.annotations {
            assert!(
                a.coef_p.is_finite() && (0.0..=1.0).contains(&a.coef_p),
                "coef_p={}",
                a.coef_p
            );
            assert!(
                a.enrichment_p.is_finite() && (0.0..=1.0).contains(&a.enrichment_p),
                "enrichment_p={}",
                a.enrichment_p
            );
        }
    }

    #[test]
    fn enrichment_se_equals_prop_se_over_m_prop() {
        let hsq = fitted_two_annot(0.2, 0.7);
        let r = build_sldsc_results(&hsq, &["A".to_string(), "B".to_string()], 400);
        for (k, a) in r.annotations.iter().enumerate() {
            let prop = hsq.reg.prop[k];
            let prop_se = hsq.reg.prop_se[k];
            let expected = prop_se / hsq.reg.m_prop[k];
            assert!((a.enrichment_se - expected).abs() < 1e-12, "k={k}");
            // sanity: enrichment ≈ prop / m_prop
            assert!((a.enrichment - prop / hsq.reg.m_prop[k]).abs() < 1e-12);
        }
    }

    #[test]
    fn write_results_header_matches_python() {
        let hsq = fitted_two_annot(0.2, 0.7);
        let r = build_sldsc_results(&hsq, &["AL2".to_string(), "BL2".to_string()], 400);
        // process-unique path under the system temp dir (avoids a `tempfile`
        // dev-dependency and parallel-test collisions).
        let path =
            std::env::temp_dir().join(format!("sldsc_results_{}.results", std::process::id()));
        write_results(&r, path.to_str().unwrap()).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        let header = content.lines().next().unwrap();
        assert_eq!(
            header,
            "Category\tProp._SNPs\tProp._h2\tProp._h2_std_error\tEnrichment\t\
             Enrichment_std_error\tEnrichment_p\tCoefficient\t\
             Coefficient_std_error\tCoefficient_z-score\tCoefficient_P_value"
        );
        // one header + two annotation rows
        assert_eq!(content.lines().count(), 3);
        assert!(content.lines().nth(1).unwrap().starts_with("AL2\t"));
        assert!(content.lines().nth(2).unwrap().starts_with("BL2\t"));
    }

    // -----------------------------------------------------------------
    // File-driver end-to-end: write tiny 2-annotation fixtures to a temp
    // dir, run estimate_sldsc, and assert per-annotation h² / enrichment
    // recover the known linear model.
    // -----------------------------------------------------------------

    /// Deterministic pseudo-random in [1, 2) (no RNG dependency).
    fn ldet(i: usize, seed: f64) -> f64 {
        ((i as f64 * 12.9898 + seed).sin() * 43758.5453)
            .fract()
            .abs()
            + 1.0
    }

    /// Write a whitespace-delimited table with a header row.
    fn write_table(path: &std::path::Path, header: &str, rows: &[String]) {
        let mut body = String::from(header);
        body.push('\n');
        for r in rows {
            body.push_str(r);
            body.push('\n');
        }
        std::fs::write(path, body).unwrap();
    }

    /// A process-unique scratch directory under the system temp dir.
    fn scratch_dir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ldsc_sldsc_test_{}_{}", std::process::id(), name));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Build a 2-annotation fixture set under `dir`:
    /// `sumstats`, `ref.l2.ldscore`, `ref.l2.M_5_50`, `w.l2.ldscore`.
    /// The χ² signal follows the S-LDSC mean model exactly
    /// `χ² = 1 + (N/M_k)·h2_k·ld_k`, so a constrained-intercept fit recovers
    /// `cat[k] = h2_k`. Returns `(ref_prefix, w_prefix, m1, m2)`.
    fn write_two_annot_fixture(
        dir: &std::path::Path,
        n_snp: usize,
        n_samp: f64,
        m1: f64,
        m2: f64,
        h2_1: f64,
        h2_2: f64,
    ) -> (std::path::PathBuf, std::path::PathBuf) {
        let mut sumstats_rows = Vec::with_capacity(n_snp);
        let mut ref_rows = Vec::with_capacity(n_snp);
        let mut w_rows = Vec::with_capacity(n_snp);
        for i in 0..n_snp {
            let snp = format!("rs{}", 1 + i);
            let al2 = ldet(i, 1.0);
            let bl2 = ldet(i, 2.0);
            let chisq = 1.0 + n_samp * (h2_1 * al2 / m1 + h2_2 * bl2 / m2);
            let z = chisq.sqrt();
            sumstats_rows.push(format!("{snp}\t{n_samp}\t{z}\tA\tT"));
            // CHR SNP BP CM MAF AL2 BL2
            ref_rows.push(format!("1\t{snp}\t{}\t0\t0.5\t{al2}\t{bl2}", 1 + i));
            // single weight LD column = AL2 + BL2
            let wld = al2 + bl2;
            w_rows.push(format!("1\t{snp}\t{}\t0\t0.5\t{wld}", 1 + i));
        }
        write_table(&dir.join("sumstats"), "SNP\tN\tZ\tA1\tA2", &sumstats_rows);
        write_table(
            &dir.join("ref.l2.ldscore"),
            "CHR\tSNP\tBP\tCM\tMAF\tAL2\tBL2",
            &ref_rows,
        );
        write_table(
            &dir.join("w.l2.ldscore"),
            "CHR\tSNP\tBP\tCM\tMAF\tWLD",
            &w_rows,
        );
        std::fs::write(dir.join("ref.l2.M_5_50"), format!("{m1}\t{m2}")).unwrap();
        (dir.join("ref"), dir.join("w"))
    }

    #[test]
    fn estimate_sldsc_from_files_recovers_per_annot_h2() {
        let dir = scratch_dir("e2e");
        let n_snp = 400usize;
        let n_samp = 1e5_f64;
        let m1 = 5e6_f64;
        let m2 = 5e6_f64;
        let h2_1 = 0.2_f64;
        let h2_2 = 0.7_f64;
        let (ref_prefix, w_prefix) =
            write_two_annot_fixture(&dir, n_snp, n_samp, m1, m2, h2_1, h2_2);

        let cfg = H2Config {
            sumstats: dir.join("sumstats").to_string_lossy().into_owned(),
            ref_ld: Some(ref_prefix.to_string_lossy().into_owned()),
            ref_ld_chr: None,
            w_ld: Some(w_prefix.to_string_lossy().into_owned()),
            w_ld_chr: None,
            m: None,
            not_m_5_50: false,
            n_blocks: 20,
            intercept_h2: Some(1.0),
            two_step: None,
            chisq_max: None,
            invert_anyway: false,
        };
        let r = estimate_sldsc(&cfg, &mut NullLogger).unwrap();

        assert_eq!(r.n_annot, 2);
        assert_eq!(r.n_snp, n_snp);
        // Per-annotation h² recovered (cat = M_k · β_k).
        assert!(
            (r.annotations[0].cat - h2_1).abs() < 1e-6,
            "cat0={}",
            r.annotations[0].cat
        );
        assert!(
            (r.annotations[1].cat - h2_2).abs() < 1e-6,
            "cat1={}",
            r.annotations[1].cat
        );
        // tot == Σ cat == h2_1 + h2_2
        assert!((r.tot - (h2_1 + h2_2)).abs() < 1e-6, "tot={}", r.tot);
        // equal M ⇒ m_prop = 0.5 each, sums to 1
        assert!((r.annotations[0].m_prop - 0.5).abs() < 1e-9);
        assert!((r.annotations.iter().map(|a| a.m_prop).sum::<f64>() - 1.0).abs() < 1e-9);
        // enrichment = (cat/M_k)/(tot/M_tot); equal M ⇒ (cat·2)/(tot)
        let tot = h2_1 + h2_2;
        assert!(
            (r.annotations[0].enrichment - (h2_1 / m1) / (tot / (m1 + m2))).abs() < 1e-6,
            "enrich0={}",
            r.annotations[0].enrichment
        );
        assert!(
            (r.annotations[1].enrichment - (h2_2 / m2) / (tot / (m1 + m2))).abs() < 1e-6,
            "enrich1={}",
            r.annotations[1].enrichment
        );
        // constrained intercept reported as the fixed value
        assert!((r.intercept.unwrap() - 1.0).abs() < 1e-9);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
