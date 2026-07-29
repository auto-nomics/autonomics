//! PLINK `.bed/.bim/.fam` reader — faithful port of `src/load_plink.cpp`.
//!
//! Decodes the 2-bit SNP-major binary genotype format, applies the
//! maf/mac/missing SNP filter, and returns the genotype matrix (individuals ×
//! kept SNPs) plus per-SNP allele frequency (needed for binary phenotypes).
//! Only the genotype-loading path (`load_plink` / `Genotypes` mode) is needed
//! for LAVA's PLINK reference: the LD is derived from the genotypes via SVD in
//! [`crate::decompose`], not from a precomputed correlation matrix.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use faer::Mat;

/// Resolve a PLINK file path by **appending** `.bed`/`.bim`/`.fam` to the
/// prefix, not replacing its last extension.
///
/// This is the PLINK convention: a prefix like `1000G.EUR.chr1.qc` maps to
/// `1000G.EUR.chr1.qc.bim` etc. Using [`Path::with_extension`] instead would
/// strip the trailing `.qc`, yielding `1000G.EUR.chr1.bim` (a nonexistent
/// file) whenever the prefix itself contains dots.
pub(crate) fn plink_file(prefix: &Path, ext: &str) -> PathBuf {
    let mut s = prefix.as_os_str().to_owned();
    s.push(".");
    s.push(ext);
    PathBuf::from(s)
}

use crate::error::{LavaError, Result};
use crate::input::{PlinkRef, SnpInfo};

/// Reference genotype data for a set of SNPs after maf/mac/missing filtering.
#[derive(Debug, Clone)]
pub struct PlinkLd {
    /// Genotype dosages, individuals × kept SNPs. Missing → `NAN`.
    pub genotypes: Mat<f64>,
    /// Indices (into the reference `.bim`) of the SNPs actually loaded, in order.
    pub snp_indices: Vec<usize>,
    /// Allele frequency per kept SNP (hom2-allele freq: `Σ dosage / (2·nobs)`).
    pub freq: Vec<f64>,
}

impl PlinkLd {
    pub fn n_indiv(&self) -> usize {
        self.genotypes.nrows()
    }
    pub fn n_snps(&self) -> usize {
        self.genotypes.ncols()
    }
}

/// PLINK filter thresholds (defaults match LAVA's `read.ld`: maf=0, mac=1, missing=0.05).
#[derive(Debug, Clone, Copy)]
pub struct PlinkFilter {
    pub maf: f64,
    pub mac: f64,
    pub missing: f64,
}

impl Default for PlinkFilter {
    fn default() -> Self {
        Self {
            maf: 0.0,
            mac: 1.0,
            missing: 0.05,
        }
    }
}

/// Load genotypes for a set of SNPs (0-based indices into the `.bim`, sorted
/// ascending, unique) from a PLINK `.bed`. Faithful port of `load_plink` /
/// `BinaryPlink::read_data` + the FREQ computation in R `read.ld`.
pub fn load_plink(
    bed_path: &Path,
    n_indiv: usize,
    snp_indices: &[usize],
    filt: PlinkFilter,
    require_freq: bool,
) -> Result<PlinkLd> {
    if snp_indices.is_empty() {
        return Err(LavaError::Input("SNP index is empty".into()));
    }
    let line_size = n_indiv.div_ceil(4); // ceil(n_indiv/4): bytes per SNP in SNP-major .bed

    let mut bed = std::fs::File::open(bed_path)?;
    // Validate magic: 0x6c 0x1e 0x01 (SNP-major).
    let mut magic = [0u8; 3];
    bed.read_exact(&mut magic)?;
    if magic[0] != 108 || magic[1] != 27 {
        return Err(LavaError::Input("file is not a valid .bed file".into()));
    }
    if magic[2] != 1 {
        return Err(LavaError::Input(
            ".bed is not SNP-major (individual-major unsupported)".into(),
        ));
    }

    // Step indices (differences), as the C++ process_index(as_steps=true).
    let mut step_index: Vec<usize> = Vec::with_capacity(snp_indices.len());
    for (k, &s) in snp_indices.iter().enumerate() {
        if k > 0 {
            let prev = snp_indices[k - 1];
            if s <= prev {
                return Err(LavaError::Input(
                    "SNP index is not ordered / duplicate".into(),
                ));
            }
            step_index.push(s - prev);
        } else {
            step_index.push(s);
        }
    }

    let n_used = snp_indices.len();
    let mut geno = Mat::zeros(n_indiv, n_used);
    // counts: per SNP [hom1, het, hom2, miss]
    let mut counts: Vec<[u32; 4]> = vec![[0; 4]; n_used];

    let mut file_offset: u64 = 3;
    let mut raw = vec![0u8; line_size];
    for (i_snp, &step) in step_index.iter().enumerate() {
        file_offset += (line_size as u64) * (step as u64);
        bed.seek(SeekFrom::Start(file_offset))?;
        bed.read_exact(&mut raw)?;
        for k in 0..n_indiv {
            let byte = raw[k / 4];
            let v = (byte >> (2 * (k % 4))) & 0b11;
            let g = match v {
                0 => 0.0,
                1 => f64::NAN,
                2 => 1.0,
                3 => 2.0,
                _ => unreachable!(),
            };
            geno[(k, i_snp)] = g;
            let ci = match v {
                0 => 0,
                2 => 1,
                3 => 2,
                _ => 3, // 1 (miss) or unreachable
            };
            counts[i_snp][ci] += 1;
        }
    }

    // check_snp filter
    let mut include: Vec<usize> = Vec::new();
    for (i, c) in counts.iter().enumerate() {
        let nmiss = c[3] as f64;
        if nmiss / (n_indiv as f64) > filt.missing {
            continue;
        }
        let nobs = n_indiv as f64 - nmiss;
        let mut mac = (c[1] + 2 * c[2]) as f64; // het + 2*hom2
        if mac > nobs {
            mac = 2.0 * nobs - mac;
        }
        if mac >= filt.mac && (nobs > 0.0 && mac / (2.0 * nobs) >= filt.maf) {
            include.push(i);
        }
    }
    if include.is_empty() {
        return Err(LavaError::Input("no SNPs remaining after filtering".into()));
    }

    // Build kept genotype matrix + freq.
    let nk = include.len();
    let mut genotypes = Mat::zeros(n_indiv, nk);
    let mut freq = Vec::with_capacity(nk);
    let mut snp_out = Vec::with_capacity(nk);
    for (out_col, &i) in include.iter().enumerate() {
        let mut s = 0.0;
        let mut nmiss = 0usize;
        for k in 0..n_indiv {
            let g = geno[(k, i)];
            genotypes[(k, out_col)] = g;
            if g.is_nan() {
                nmiss += 1;
            } else {
                s += g;
            }
        }
        let nobs = n_indiv - nmiss;
        let f = if require_freq && nobs > 0 {
            s / (2.0 * nobs as f64)
        } else {
            f64::NAN
        };
        freq.push(f);
        snp_out.push(snp_indices[i]);
    }

    Ok(PlinkLd {
        genotypes,
        snp_indices: snp_out,
        freq,
    })
}

/// Load the `.bim` (SNP info) and `.fam` (sample size) for a PLINK prefix.
/// Faithful port of `load.reference` (plink mode): snp.info = (SNP, CHR, POS, A1, A2),
/// SNP lower-cased; sample.size = nrow(.fam).
///
/// Builds `chr_prefix`/`chr_offset` from the (merged) `.bim` so that
/// [`crate::locus::process_locus`] resolves the right `.bed` per chromosome
/// through the same code path used for per-chromosome references.
pub fn load_reference(prefix: &Path) -> Result<PlinkRef> {
    let bim = plink_file(prefix, "bim");
    let fam = plink_file(prefix, "fam");
    let bed = plink_file(prefix, "bed");
    if !bed.exists() || !bim.exists() || !fam.exists() {
        return Err(LavaError::Input(format!(
            "missing PLINK files for prefix {}",
            prefix.display()
        )));
    }
    let sample_size = count_lines(&fam)?;
    let snp_info = read_bim(&bim)?;
    let prefix = prefix.to_path_buf();
    // Single merged `.bed`: every chromosome lives in the same file, indexed by
    // its global `.bim` row. So the per-`.bed` row offset for every chromosome is
    // 0 (load_plink seeks by absolute global index), and every chromosome
    // resolves to this one prefix.
    let mut chr_prefix: HashMap<i64, PathBuf> = HashMap::new();
    let mut chr_offset: HashMap<i64, usize> = HashMap::new();
    for &c in snp_info.chr.iter() {
        chr_offset.entry(c).or_insert(0);
        chr_prefix.entry(c).or_insert_with(|| prefix.clone());
    }
    Ok(PlinkRef {
        prefix,
        snp_info,
        sample_size,
        chr_prefix,
        chr_offset,
    })
}

/// Load a **per-chromosome** PLINK reference from a prefix template containing
/// the literal `{N}` (e.g. `/data/chr{N}/panel.chr{N}.qc`), resolved for each
/// chromosome in `chroms`. Chromosomes whose files are absent are silently
/// skipped (loci on them later yield no SNPs); present chromosomes are
/// concatenated in `chroms` order into one merged `snp_info`. All loaded
/// chromosomes must share the same `.fam` sample size.
pub fn load_reference_template(template: &str, chroms: &[i64]) -> Result<PlinkRef> {
    if !template.contains("{N}") {
        return Err(LavaError::Input(format!(
            "ref_prefix_template must contain '{{N}}' (chromosome placeholder): got {template:?}"
        )));
    }
    let mut snp = Vec::new();
    let mut chr = Vec::new();
    let mut pos = Vec::new();
    let mut a1 = Vec::new();
    let mut a2 = Vec::new();
    let mut chr_prefix: HashMap<i64, PathBuf> = HashMap::new();
    let mut chr_offset: HashMap<i64, usize> = HashMap::new();
    let mut sample_size: Option<usize> = None;
    for c in chroms {
        let resolved = PathBuf::from(template.replace("{N}", &c.to_string()));
        let bim = plink_file(&resolved, "bim");
        let fam = plink_file(&resolved, "fam");
        let bed = plink_file(&resolved, "bed");
        if !bed.exists() || !bim.exists() || !fam.exists() {
            // graceful skip — locus on this chr will find no SNPs
            continue;
        }
        let n = count_lines(&fam)?;
        match sample_size {
            Some(s) if s != n => {
                return Err(LavaError::Input(format!(
                    "sample-size mismatch for {prefix}: {s} vs {n} (per-locus n_indiv must be uniform)",
                    prefix = resolved.display()
                )));
            }
            None => sample_size = Some(n),
            _ => {}
        }
        let info = read_bim(&bim)?;
        chr_offset.insert(*c, snp.len());
        chr_prefix.insert(*c, resolved);
        snp.extend(info.snp);
        chr.extend(info.chr);
        pos.extend(info.pos);
        a1.extend(info.a1);
        a2.extend(info.a2);
    }
    let sample_size = sample_size.ok_or_else(|| {
        LavaError::Input(format!(
            "ref_prefix_template {template:?} matched no PLINK files for chromosomes {chroms:?}"
        ))
    })?;
    let snp_info = SnpInfo {
        snp,
        chr,
        pos,
        a1,
        a2,
    };
    Ok(PlinkRef {
        prefix: PathBuf::from(template),
        snp_info,
        sample_size,
        chr_prefix,
        chr_offset,
    })
}

fn count_lines(path: &Path) -> Result<usize> {
    use std::io::{BufRead, BufReader};
    let f = std::fs::File::open(path)?;
    Ok(BufReader::new(f).lines().count())
}

fn read_bim(path: &Path) -> Result<SnpInfo> {
    use std::io::{BufRead, BufReader};
    let f = std::fs::File::open(path)?;
    let mut snp = Vec::new();
    let mut chr = Vec::new();
    let mut pos = Vec::new();
    let mut a1 = Vec::new();
    let mut a2 = Vec::new();
    for line in BufReader::new(f).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 6 {
            return Err(LavaError::Input(format!("malformed .bim line: {line}")));
        }
        // .bim: CHR SNP cM BP A1 A2
        chr.push(parts[0].parse::<i64>().unwrap_or(0));
        snp.push(parts[1].to_lowercase());
        pos.push(parts[3].parse::<i64>().unwrap_or(0));
        a1.push(parts[4].to_string());
        a2.push(parts[5].to_string());
    }
    Ok(SnpInfo {
        snp,
        chr,
        pos,
        a1,
        a2,
    })
}
