//! Locus construction — faithful port of `process.locus` (`R/input_processing.R`).
//!
//! For a single locus: loads the PLINK LD for the locus SNPs, computes marginal
//! SNP correlations per phenotype (binary via logistic reconstruction,
//! continuous via the Z→r transform), decomposes the LD, and computes the
//! `delta` / `sigma` / `omega` / h² parameters used by all downstream analyses.

use std::collections::HashSet;

use faer::Mat;

use crate::binary::process_binary;
use crate::decompose::decompose_plink;
use crate::error::{LavaError, Result};
use crate::input::{Input, LocusDef, SumStats};
use crate::plink::{load_plink, PlinkFilter};
use crate::stats::{cov2cor, dnorm, qnorm};

/// Options for [`process_locus`] (defaults match LAVA's `process.locus`).
#[derive(Debug, Clone)]
pub struct LocusOptions {
    pub min_k: usize,
    pub prune_thresh: f64, // percent (default 99)
    pub max_prop_k: Option<f64>,
    pub drop_failed: bool,
    pub max_block_size: usize,
    pub cap_estimates: bool,
}

impl Default for LocusOptions {
    fn default() -> Self {
        Self {
            min_k: 2,
            prune_thresh: 99.0,
            max_prop_k: Some(0.75),
            drop_failed: true,
            max_block_size: 3000,
            cap_estimates: true,
        }
    }
}

/// A processed locus — mirrors R's `loc` environment.
#[derive(Debug, Clone)]
pub struct Locus {
    pub id: String,
    pub chr: Option<i64>,
    pub start: Option<i64>,
    pub stop: Option<i64>,
    pub snps: Vec<String>,
    pub n_snps: usize,
    pub k: usize,
    pub nref_scale: f64,
    /// PC-projected joint effects, K × P.
    pub delta: Mat<f64>,
    /// Sampling covariance matrix, P × P.
    pub sigma: Mat<f64>,
    /// Genetic covariance matrix, P × P.
    pub omega: Mat<f64>,
    /// Genetic correlation matrix (cov2cor of omega).
    pub omega_cor: Mat<f64>,
    pub n: Vec<f64>,
    pub phenos: Vec<String>,
    pub binary: Vec<bool>,
    pub h2_obs: Vec<f64>,
    pub h2_latent: Vec<f64>,
    pub ascertained_h2: Vec<bool>,
}

impl Locus {
    pub fn p(&self) -> usize {
        self.phenos.len()
    }
}

/// Subset a phenotype's sum-stats (aligned to `analysis_snps`) to `locus_snps`
/// (a subsequence), returning aligned `(stat, n)` vectors.
fn subset_to_locus(ss: &SumStats, locus_snps: &[String], analysis_index: &std::collections::HashMap<String, usize>) -> (Vec<f64>, Vec<f64>) {
    let mut stat = Vec::with_capacity(locus_snps.len());
    let mut n = Vec::with_capacity(locus_snps.len());
    for s in locus_snps {
        let &pos = analysis_index.get(s).expect("locus snp missing from analysis set");
        stat.push(ss.stat[pos]);
        n.push(ss.n[pos]);
    }
    (stat, n)
}

/// `process.locus`. Returns `Ok(None)` when the locus cannot be analysed
/// (too few SNPs / PCs, or all phenotypes fail), matching R's `NULL` return.
pub fn process_locus(
    locus_def: &LocusDef,
    input: &Input,
    phenos: Option<&[String]>,
    opts: &LocusOptions,
) -> Result<Option<Locus>> {
    let min_k = opts.min_k.max(2);
    let phenos: Vec<String> = match phenos {
        Some(p) => p.to_vec(),
        None => input.phenos.clone(),
    };
    // validate
    for p in &phenos {
        if !input.phenos.contains(p) {
            return Err(LavaError::Input(format!("Invalid phenotype ID: '{p}'")));
        }
    }
    let p = phenos.len();

    // binary flags
    let binary: Vec<bool> = phenos
        .iter()
        .map(|ph| input.info_for(ph).map(|i| i.binary).unwrap_or(false))
        .collect();

    // locus SNPs
    let mut locus_snps: Vec<String> = if let Some(snps) = &locus_def.snps {
        snps.iter().filter_map(|s| {
            if input.analysis_snps.contains(s) { Some(s.clone()) } else { None }
        }).collect()
    } else {
        // by coordinates from the reference bim
        let chr = locus_def.chr.unwrap_or(0);
        let start = locus_def.start.unwrap_or(0);
        let stop = locus_def.stop.unwrap_or(0);
        let analysis_set: HashSet<&String> = input.analysis_snps.iter().collect();
        input
            .reference
            .snp_info
            .snp
            .iter()
            .enumerate()
            .filter(|(_, _s)| true)
            .filter_map(|(i, s)| {
                let in_chr = input.reference.snp_info.chr[i] == chr;
                let pos = input.reference.snp_info.pos[i];
                let in_range = pos >= start && pos <= stop;
                if in_chr && in_range && analysis_set.contains(s) {
                    Some(s.clone())
                } else {
                    None
                }
            })
            .collect()
    };
    // unique preserving order
    let mut seen = HashSet::new();
    locus_snps.retain(|s| seen.insert(s.clone()));

    // bim indices (sorted ascending since reference-ordered)
    let bim_indices: Vec<usize> = locus_snps
        .iter()
        .map(|s| *input.bim_index.get(s).expect("locus snp not in bim"))
        .collect();
    if !bim_indices.windows(2).all(|w| w[0] < w[1]) {
        return Err(LavaError::Locus {
            locus: locus_def.loc.clone(),
            msg: "locus SNP indices not in ascending bim order".into(),
        });
    }

    // load PLINK genotypes for these SNPs
    let require_freq = binary.iter().any(|&b| b);
    let bed = input.reference.prefix.with_extension("bed");
    let ld = load_plink(&bed, input.reference.sample_size, &bim_indices, PlinkFilter::default(), require_freq)?;
    // the kept SNP ids (bim indices → snp ids)
    let kept_ids: Vec<String> = ld.snp_indices.iter().map(|&bi| input.reference.snp_info.snp[bi].clone()).collect();
    let n_indiv = ld.n_indiv();

    // subset sumstats to kept SNPs. Build analysis snp→position map.
    let mut analysis_index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (i, s) in input.analysis_snps.iter().enumerate() {
        analysis_index.insert(s.clone(), i);
    }

    // per-pheno stat / n vectors aligned to kept_ids
    let mut stat_per: Vec<Vec<f64>> = Vec::with_capacity(p);
    let mut n_per: Vec<Vec<f64>> = Vec::with_capacity(p);
    // sum_stats is in input.phenos order; index by pheno position.
    for ph in &phenos {
        let pheno_pos = input.phenos.iter().position(|x| x == ph).unwrap();
        let ss = &input.sum_stats[pheno_pos];
        let (stat, n) = subset_to_locus(ss, &kept_ids, &analysis_index);
        stat_per.push(stat);
        n_per.push(n);
    }
    let n_kept = kept_ids.len();
    let _ = n_indiv;

    if n_kept < min_k {
        return Ok(None);
    }

    // locus N per pheno (mean, with fallback) + mean-impute per-snp N
    let mut locus_n: Vec<f64> = Vec::with_capacity(p);
    for pi in 0..p {
        let valid: Vec<f64> = n_per[pi].iter().copied().filter(|x| !x.is_nan()).collect();
        let mut mean = if valid.is_empty() { f64::NAN } else { valid.iter().sum::<f64>() / valid.len() as f64 };
        if mean.is_nan() {
            // fallback: mean over all analysis snps for this pheno
            let all: Vec<f64> = input.sum_stats[input.phenos.iter().position(|x| x == &phenos[pi]).unwrap()]
                .n
                .iter()
                .copied()
                .filter(|x| !x.is_nan())
                .collect();
            mean = if all.is_empty() { f64::NAN } else { all.iter().sum::<f64>() / all.len() as f64 };
        }
        for v in n_per[pi].iter_mut() {
            if v.is_nan() {
                *v = mean;
            }
        }
        locus_n.push(mean);
    }

    // CORR per pheno; collect SNPs to drop (NA binary corr)
    let mut corr_per: Vec<Vec<f64>> = Vec::with_capacity(p);
    let mut drop_set: HashSet<usize> = HashSet::new();
    for pi in 0..p {
        let corr = if binary[pi] {
            let prop_cases = input.info_for(&phenos[pi]).map(|i| i.prop_cases).unwrap_or(f64::NAN);
            let c = process_binary(&stat_per[pi], &n_per[pi], &ld.freq, prop_cases);
            for (i, &v) in c.iter().enumerate() {
                if v.is_nan() {
                    drop_set.insert(i);
                }
            }
            c
        } else {
            stat_per[pi]
                .iter()
                .zip(n_per[pi].iter())
                .map(|(&z, &n)| z / (z * z + n - 2.0).sqrt())
                .collect()
        };
        corr_per.push(corr);
    }

    // drop SNPs in drop_set
    let (final_ids, final_corr, _final_n): (Vec<String>, Vec<Vec<f64>>, Vec<f64>) = if drop_set.is_empty() {
        (kept_ids.clone(), corr_per.clone(), locus_n.clone())
    } else {
        let keep_idx: Vec<usize> = (0..n_kept).filter(|i| !drop_set.contains(i)).collect();
        let new_ids: Vec<String> = keep_idx.iter().map(|&i| kept_ids[i].clone()).collect();
        let new_corr: Vec<Vec<f64>> = corr_per
            .iter()
            .map(|c| keep_idx.iter().map(|&i| c[i]).collect())
            .collect();
        (new_ids, new_corr, locus_n.clone())
    };
    if final_ids.len() < min_k {
        return Ok(None);
    }

    // reload genotypes restricted to the final SNP set (bim indices), since we
    // may have dropped SNPs with failed binary reconstruction.
    let final_bim: Vec<usize> = final_ids
        .iter()
        .map(|s| *input.bim_index.get(s).unwrap())
        .collect();
    let ld_final = load_plink(&bed, input.reference.sample_size, &final_bim, PlinkFilter::default(), require_freq)?;
    // sanity: kept ids should equal final_ids (filter is deterministic)
    let n_snps = final_ids.len();

    // decompose
    let mut r = decompose_plink(ld_final.genotypes.as_ref(), opts.prune_thresh, opts.max_block_size)?;
    let mut k_raw = r.ncols();
    // cap K
    if let Some(max_prop) = opts.max_prop_k {
        let min_n = locus_n.iter().copied().filter(|x| !x.is_nan()).fold(f64::INFINITY, f64::min);
        let mut max_k = (max_prop * min_n).floor() as usize;
        if max_k < min_k {
            max_k = min_k;
        }
        if k_raw > max_k {
            let nrows = r.nrows();
            let mut r2 = Mat::zeros(nrows, max_k);
            for i in 0..nrows {
                for j in 0..max_k {
                    r2[(i, j)] = r[(i, j)];
                }
            }
            r = r2;
            k_raw = max_k;
        }
    }
    let k = k_raw;
    if k < min_k {
        return Ok(None);
    }

    // CORR_mat (n_snps × P)
    let corr_mat = Mat::from_fn(n_snps, p, |i, j| final_corr[j][i]);
    // delta = R^T · CORR_mat  (K × P)
    let rt = r.transpose();
    let delta = &rt * &corr_mat;

    // per-pheno sigma, h2
    let nref_scale = 1.0;
    let mut sigma_diag = vec![f64::NAN; p];
    let mut h2_obs = vec![f64::NAN; p];
    let mut h2_latent = vec![f64::NAN; p];
    let mut ascertained = vec![false; p];
    for pi in 0..p {
        let mut dtd = 0.0;
        for row in 0..k {
            dtd += delta[(row, pi)] * delta[(row, pi)];
        }
        let ni = locus_n[pi];
        let s = (1.0 - dtd) / (ni - k as f64 - 1.0);
        sigma_diag[pi] = s;
        h2_obs[pi] = (dtd - s * k as f64) * nref_scale;
        if binary[pi] {
            let info = input.info_for(&phenos[pi]).unwrap();
            let case_prop = info.prop_cases;
            let (prevalence, asc) = match info.prevalence {
                Some(prev) if prev.is_finite() => (prev, true),
                _ => (case_prop, false),
            };
            ascertained[pi] = asc;
            // Lee et al. 2011: h2.obs / dnorm(qnorm(prev))^2 * (prev*(1-prev))^2 / (case*(1-case))
            let q = qnorm(prevalence);
            let denom = dnorm(q).powi(2);
            h2_latent[pi] = h2_obs[pi] / denom * (prevalence * (1.0 - prevalence)).powi(2) / (case_prop * (1.0 - case_prop));
        }
    }

    // sigma matrix
    let sigma_mat = build_sigma(&sigma_diag, input, &phenos);

    // omega = delta^T·delta/K - sigma  (dtd built by hand to guarantee symmetry)
    let mut dtd_mat = Mat::zeros(p, p);
    for i in 0..p {
        for j in 0..p {
            let mut s = 0.0;
            for r in 0..k {
                s += delta[(r, i)] * delta[(r, j)];
            }
            dtd_mat[(i, j)] = s;
        }
    }
    let mut omega = Mat::zeros(p, p);
    for i in 0..p {
        for j in 0..p {
            omega[(i, j)] = dtd_mat[(i, j)] / k as f64 - sigma_mat[(i, j)];
        }
    }
    let omega_cor = cov2cor(&omega);

    // cap negative h2
    if opts.cap_estimates {
        for v in h2_obs.iter_mut() {
            if v.is_nan() { /* keep NA */ } else if *v < 0.0 { *v = 0.0; }
        }
        for v in h2_latent.iter_mut() {
            if !v.is_nan() && *v < 0.0 {
                *v = 0.0;
            }
        }
    }
    // K/N > 0.1 → h2 NA
    for pi in 0..p {
        if !locus_n[pi].is_nan() && (k as f64 / locus_n[pi]) > 0.1 {
            h2_obs[pi] = f64::NAN;
            h2_latent[pi] = f64::NAN;
        }
    }

    // failed phenotypes: diag(sigma)<0 | diag(omega)<0, or NA (R `neg.var | is.na(neg.var)`).
    let failed: Vec<bool> = (0..p)
        .map(|pi| {
            let s = sigma_mat[(pi, pi)];
            let o = omega[(pi, pi)];
            if s.is_nan() || o.is_nan() {
                true
            } else {
                s < 0.0 || o < 0.0
            }
        })
        .collect();
    if !failed.iter().any(|&f| !f) {
        return Ok(None);
    }

    // optionally drop failed phenotypes
    let keep: Vec<usize> = if opts.drop_failed && failed.iter().any(|&f| f) {
        (0..p).filter(|&i| !failed[i]).collect()
    } else {
        (0..p).collect()
    };
    let pk = keep.len();
    let pickf = |v: &[f64]| -> Vec<f64> { keep.iter().map(|&i| v[i]).collect() };
    let sub = |m: &Mat<f64>| Mat::from_fn(pk, pk, |i, j| m[(keep[i], keep[j])]);
    let phenos_f: Vec<String> = keep.iter().map(|&i| phenos[i].clone()).collect();
    let binary_f: Vec<bool> = keep.iter().map(|&i| binary[i]).collect();
    let n_f = pickf(&locus_n);
    let h2_obs_f = pickf(&h2_obs);
    let h2_latent_f = pickf(&h2_latent);
    let asc_f: Vec<bool> = keep.iter().map(|&i| ascertained[i]).collect();
    let delta_f = Mat::from_fn(k, pk, |r, c| delta[(r, keep[c])]);
    let sigma_f = sub(&sigma_mat);
    let omega_f = sub(&omega);
    let omega_cor_f = sub(&omega_cor);

    Ok(Some(Locus {
        id: locus_def.loc.clone(),
        chr: locus_def.chr,
        start: locus_def.start,
        stop: locus_def.stop,
        snps: final_ids.clone(),
        n_snps: final_ids.len(),
        k,
        nref_scale,
        delta: delta_f,
        sigma: sigma_f,
        omega: omega_f,
        omega_cor: omega_cor_f,
        n: n_f,
        phenos: phenos_f,
        binary: binary_f,
        h2_obs: h2_obs_f,
        h2_latent: h2_latent_f,
        ascertained_h2: asc_f,
    }))
}

/// Build the sampling covariance matrix from the per-pheno sigma diagonal and
/// the (optional) sample-overlap correlation matrix.
fn build_sigma(sigma_diag: &[f64], input: &Input, phenos: &[String]) -> Mat<f64> {
    let p = phenos.len();
    if p > 1 {
        if let Some(overlap) = &input.sample_overlap {
            // subset overlap to phenos order
            let idx: Vec<usize> = phenos.iter().map(|ph| input.phenos.iter().position(|x| x == ph).unwrap()).collect();
            let mut s = Mat::zeros(p, p);
            for i in 0..p {
                for j in 0..p {
                    let di = sigma_diag[i].max(0.0).sqrt();
                    let dj = sigma_diag[j].max(0.0).sqrt();
                    s[(i, j)] = di * overlap[(idx[i], idx[j])] * dj;
                }
            }
            s
        } else {
            Mat::from_fn(p, p, |i, j| if i == j { sigma_diag[i] } else { 0.0 })
        }
    } else {
        Mat::from_fn(1, 1, |_, _| sigma_diag[0])
    }
}
