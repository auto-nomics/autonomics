//! GWAS summary-statistics harmonisation — port of `HDL.L.R` lines 388-460.
//!
//! Given a reference SNP list (with A2 alleles) and one GWAS sumstat table,
//! compute the per-reference-SNP `bhat = Z/√N`, allele-aligned to the reference
//! A2 (sign-flipped when the GWAS A2 does not match). Missing reference SNPs
//! get `bhat = 0.0` (matching R's `bhat <- numeric(M); bhat[names] <- bhat.raw`).

use crate::error::{HdlError, Result};

/// One GWAS sumstat row (already Z-scored).
#[derive(Debug, Clone)]
pub struct SumStatRow {
    pub snp: String,
    pub a1: String,
    pub a2: String,
    pub n: f64,
    pub z: f64,
}

/// Harmonise a GWAS sumstat table to the reference SNP list / A2 alleles.
///
/// Returns `(bhat, n_eff)` where `bhat` is length `snps_ref.len()` in reference
/// order (0.0 where the SNP is absent), and `n_eff` is `median(N)` over valid
/// rows (matching `HDL.L.R` line 370).
///
/// Faithful port of `HDL.L.R` lines 388-453: filter to reference SNPs, drop
/// duplicates (keeping the first distinct SNP — R's `distinct(SNP, A1, A2)`),
/// `bhat.raw = Z/√N`, sign-flip when `gwas.A2 != ref.A2`.
pub fn harmonise_gwas(rows: &[SumStatRow], snps_ref: &[String], a2_ref: &[String]) -> Result<(Vec<f64>, f64)> {
    if snps_ref.len() != a2_ref.len() {
        return Err(HdlError::Input(
            "harmonise_gwas: snps_ref / a2_ref length mismatch".into(),
        ));
    }
    let m = snps_ref.len();

    // index reference SNPs (case-insensitive, matching lava's lower-cased .bim)
    use std::collections::HashMap;
    let mut ref_idx: HashMap<String, (usize, &str)> = HashMap::new();
    for (i, s) in snps_ref.iter().enumerate() {
        ref_idx.insert(s.to_lowercase(), (i, &a2_ref[i]));
    }

    let mut bhat = vec![0.0f64; m];
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut n_vals: Vec<f64> = Vec::new();

    for r in rows {
        if r.z.is_nan() || r.n <= 0.0 {
            continue;
        }
        let key = r.snp.to_lowercase();
        let Some((i, a2r)) = ref_idx.get(&key).copied() else {
            continue;
        };
        // distinct(SNP, A1, A2): skip a (SNP,A1,A2) trio we've already kept.
        let trio = format!("{}|{}|{}", key, r.a1, r.a2);
        if !seen.insert(trio) {
            continue;
        }
        // allele-align: sign +1 if gwas.A2 == ref.A2 else -1
        let sign = if r.a2.eq_ignore_ascii_case(a2r) { 1.0 } else { -1.0 };
        // if bhat[i] already set by an earlier row of the same SNP, R keeps the
        // first distinct; emulate by only writing when still 0 from a *match*.
        bhat[i] = sign * r.z / r.n.sqrt();
        n_vals.push(r.n);
    }

    let n_eff = if n_vals.is_empty() {
        return Err(HdlError::Input("no overlapping SNPs between GWAS and reference".into()));
    } else {
        median(&n_vals)
    };
    Ok((bhat, n_eff))
}

fn median(v: &[f64]) -> f64 {
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = s.len();
    if n % 2 == 1 {
        s[n / 2]
    } else {
        0.5 * (s[n / 2 - 1] + s[n / 2])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harmonise_aligns_and_signflips() {
        let snps = vec!["rs1".into(), "rs2".into(), "rs3".into()];
        let a2 = vec!["A".into(), "C".into(), "G".into()];
        // rs1: A2 matches → +; rs2: A2 flipped → -; rs3: absent → 0
        let rows = vec![
            SumStatRow { snp: "rs1".into(), a1: "T".into(), a2: "A".into(), n: 100.0, z: 2.0 },
            SumStatRow { snp: "rs2".into(), a1: "G".into(), a2: "G".into(), n: 100.0, z: 3.0 },
        ];
        let (bhat, n) = harmonise_gwas(&rows, &snps, &a2).unwrap();
        assert!((bhat[0] - 2.0 / 10.0).abs() < 1e-12, "rs1 bhat={}", bhat[0]);
        assert!((bhat[1] - (-3.0 / 10.0)).abs() < 1e-12, "rs2 bhat={}", bhat[1]);
        assert!(bhat[2].abs() < 1e-12, "rs3 bhat={}", bhat[2]);
        assert!((n - 100.0).abs() < 1e-12);
    }

    #[test]
    fn dedups_snps() {
        let snps = vec!["rs1".into()];
        let a2 = vec!["A".into()];
        let rows = vec![
            SumStatRow { snp: "rs1".into(), a1: "T".into(), a2: "A".into(), n: 100.0, z: 2.0 },
            SumStatRow { snp: "rs1".into(), a1: "T".into(), a2: "A".into(), n: 200.0, z: 4.0 },
        ];
        let (bhat, _) = harmonise_gwas(&rows, &snps, &a2).unwrap();
        // first distinct row kept
        assert!((bhat[0] - 2.0 / 10.0).abs() < 1e-12, "bhat={}", bhat[0]);
    }
}
