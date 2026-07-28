//! Input processing — faithful port of `R/input_processing.R` (sumstats
//! read-in / harmonisation / sample overlap / locus file / reference loading).
//!
//! Genotype / LD loading from PLINK lives in [`crate::plink`]; LD decomposition
//! in [`crate::decompose`]; locus construction in [`crate::locus`].

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use faer::Mat;

use crate::align::{com_pair, map_alleles};
use crate::error::{LavaError, Result};
use crate::stats::qnorm;

/// Per-phenotype summary statistics after read-in (pre-alignment). Columns mirror
/// R's `sum.stats`: `[gene?] snp a1 a2 stat n`. `stat` is a Z-statistic; `n` is
/// per-SNP sample size (may contain `NaN`).
#[derive(Debug, Clone)]
pub struct SumStats {
    pub snp: Vec<String>,
    pub a1: Vec<String>,
    pub a2: Vec<String>,
    pub stat: Vec<f64>,
    pub n: Vec<f64>,
    pub gene: Option<Vec<String>>,
}

impl SumStats {
    pub fn len(&self) -> usize {
        self.snp.len()
    }
    pub fn is_empty(&self) -> bool {
        self.snp.is_empty()
    }
}

/// One row of the input-info file (`phenotype, cases, controls, filename` plus
/// the derived `n`, `prop_cases`, `binary` and optional `prevalence`).
#[derive(Debug, Clone)]
pub struct PhenoInfo {
    pub phenotype: String,
    pub cases: f64,
    pub controls: f64,
    pub filename: String,
    pub n: f64,
    pub prop_cases: f64,
    pub binary: bool,
    pub prevalence: Option<f64>,
}

/// SNP info from the reference `.bim` (or `.info`): `snp, chr, pos, a1, a2`.
#[derive(Debug, Clone, Default)]
pub struct SnpInfo {
    pub snp: Vec<String>,
    pub chr: Vec<i64>,
    pub pos: Vec<i64>,
    pub a1: Vec<String>,
    pub a2: Vec<String>,
}

/// PLINK reference data (prefix + `.bim`/`.fam` metadata). Genotypes are loaded
/// per-locus by [`crate::plink::load_plink`].
///
/// `snp_info` is always a **global**, merged view (across all chromosomes, in
/// `.bim` order). `chr_prefix` / `chr_offset` let [`crate::locus::process_locus`]
/// resolve the per-chromosome `.bed` for a locus and translate global `.bim`
/// indices into local `.bed`-row offsets — so a single merged prefix and a set
/// of per-chromosome prefixes are handled by the same code path.
#[derive(Debug, Clone)]
pub struct PlinkRef {
    /// Original / canonical prefix. For per-chromosome references this is the
    /// unresolved template (diagnostics only); resolution goes through
    /// `chr_prefix`.
    pub prefix: PathBuf,
    pub snp_info: SnpInfo,
    pub sample_size: usize,
    /// Chromosome → resolved PLINK prefix for that chromosome's `.bed`/`.bim`.
    /// For a single merged prefix, every chromosome present maps to `prefix`.
    pub chr_prefix: HashMap<i64, PathBuf>,
    /// Chromosome → global `.bim` index of the **first row of the `.bed` file
    /// `chr_prefix[c]` points to**. `process_locus` translates a global `.bim`
    /// index `g` (on chromosome `c`) to a row in that `.bed` via `g - chr_offset[c]`.
    /// For a single merged `.bed` this is `0` for every chromosome (the merged
    /// file is indexed by absolute global row); for per-chromosome files it is
    /// that chromosome's start index within the merged `snp_info`.
    pub chr_offset: HashMap<i64, usize>,
}

/// Processed input object — the result of `process.input`.
#[derive(Debug, Clone)]
pub struct Input {
    pub info: Vec<PhenoInfo>,
    pub phenos: Vec<String>,
    pub p: usize,
    /// Per-phenotype sum-stats, subsetted to common aligned SNPs in reference
    /// order; `stat` sign-aligned to the reference.
    pub sum_stats: Vec<SumStats>,
    /// SNPs shared across all data sets, surviving alignment, in reference order.
    pub analysis_snps: Vec<String>,
    pub unalignable_snps: Vec<String>,
    pub sample_overlap: Option<Mat<f64>>,
    pub reference: PlinkRef,
    /// SNP-id → row index into `reference.snp_info` (the `.bim`), for LD loading.
    pub bim_index: HashMap<String, usize>,
}

impl Input {
    /// Phenotype info lookup helper.
    pub fn info_for(&self, pheno: &str) -> Option<&PhenoInfo> {
        self.info.iter().find(|p| p.phenotype == pheno)
    }
}

/// A locus definition row (`LOC` + `CHR/START/STOP` and/or `SNPS`).
#[derive(Debug, Clone)]
pub struct LocusDef {
    pub loc: String,
    pub chr: Option<i64>,
    pub start: Option<i64>,
    pub stop: Option<i64>,
    pub snps: Option<Vec<String>>,
}

// ---------------------------------------------------------------------------
// Table reader (whitespace-delimited, like data.table::fread on these files).
// ---------------------------------------------------------------------------

/// Read a whitespace-delimited table with a header row into column-major form.
pub fn read_table(path: &Path) -> Result<(Vec<String>, Vec<Vec<String>>)> {
    use std::io::{BufRead, BufReader};
    let file = std::fs::File::open(path)?;
    let mut lines = BufReader::new(file).lines();
    let header_line = lines
        .next()
        .ok_or_else(|| LavaError::Input(format!("empty file: {}", path.display())))??;
    let header: Vec<String> = split_ws(&header_line);
    let ncol = header.len();
    let mut cols: Vec<Vec<String>> = vec![Vec::new(); ncol];
    for line in lines {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let parts = split_ws(&line);
        for (c, val) in parts.into_iter().enumerate().take(ncol) {
            cols[c].push(val);
        }
    }
    Ok((header, cols))
}

/// Split on runs of ASCII whitespace (space / tab), dropping empties.
fn split_ws(s: &str) -> Vec<String> {
    s.split_whitespace().map(|x| x.to_string()).collect()
}

// ---------------------------------------------------------------------------
// read.sumstats.file
// ---------------------------------------------------------------------------

const SNP_ALIASES: &[&str] = &[
    "SNP",
    "ID",
    "SNPID_UKB",
    "SNPID",
    "MarkerName",
    "RSID",
    "RSID_UKB",
];
const STAT_ALIASES: &[&str] = &["Z", "T", "STAT", "Zscore"];
const B_ALIASES: &[&str] = &["B", "BETA"];
const A1_ALIASES: &[&str] = &["A1", "ALT"];
const A2_ALIASES: &[&str] = &["A2", "REF"];
const N_ALIASES: &[&str] = &["N", "NMISS", "N_analyzed"];

/// Read one phenotype's sum-stats file. Faithful port of `read.sumstats.file`.
pub fn read_sumstats_file(
    filename: &Path,
    _pheno_label: &str,
    min_pval: f64,
    n_override: Option<f64>,
) -> Result<SumStats> {
    let (header, cols) = read_table(filename)?;

    // Build the alias map. SNP/A1/A2 required; N required unless overridden;
    // STAT and B are optional at this stage. Pick the FIRST matching column per
    // canonical name (R keeps the first and warns on extras).
    let pick = |aliases: &[&str]| -> Option<usize> {
        header.iter().position(|h| aliases.contains(&h.as_str()))
    };

    let col_snp = pick(SNP_ALIASES)
        .ok_or_else(|| LavaError::Input(format!("No valid SNP header: {}", filename.display())))?;
    let col_a1 = pick(A1_ALIASES)
        .ok_or_else(|| LavaError::Input(format!("No valid A1 header: {}", filename.display())))?;
    let col_a2 = pick(A2_ALIASES)
        .ok_or_else(|| LavaError::Input(format!("No valid A2 header: {}", filename.display())))?;
    let col_stat = pick(STAT_ALIASES);
    let col_b = pick(B_ALIASES);
    let col_n = if n_override.is_none() {
        Some(pick(N_ALIASES).ok_or_else(|| {
            LavaError::Input(format!("No valid N header: {}", filename.display()))
        })?)
    } else {
        pick(N_ALIASES)
    };
    let col_or = pick(&["OR"]);
    let col_logor = pick(&["logOdds"]);
    let col_p = pick(&["P"]);
    let col_gene = pick(&["GENE"]);

    let nrows = cols[col_snp].len();

    // Determine STAT vector
    let mut stat: Vec<f64> = if let Some(c) = col_stat {
        cols[c].iter().map(|s| parse_f64_lossy(s)).collect()
    } else {
        // Derive Z from an effect-size column + P
        let param = [col_b, col_or, col_logor].into_iter().flatten().next();
        let pc = col_p.ok_or_else(|| {
            LavaError::Input(format!(
                "Lack of valid statistics (Z, or BETA/OR/logOdds + P): {}",
                filename.display()
            ))
        })?;
        let param = param.ok_or_else(|| {
            LavaError::Input(format!(
                "No STAT and no effect-size column (B/OR/logOdds): {}",
                filename.display()
            ))
        })?;
        // is the chosen column OR/logOdds/B? Determine sign rule
        let is_or = header.get(param).map(|h| h == "OR").unwrap_or(false);
        (0..nrows)
            .map(|i| {
                let p = format_pvalue(&cols[pc][i], min_pval);
                let raw = parse_f64_lossy(&cols[param][i]);
                let sign = if is_or {
                    if raw > 1.0 { 1.0 } else { -1.0 }
                } else {
                    raw.signum()
                };
                -qnorm(p / 2.0) * sign
            })
            .collect()
    };

    // format.z: truncate out-of-range Z
    let bound = qnorm(min_pval / 2.0).abs();
    for v in stat.iter_mut() {
        if v.abs() > bound {
            *v = bound * v.signum();
        }
    }

    let n: Vec<f64> = match n_override {
        Some(nv) => vec![nv; nrows],
        None => col_n
            .map(|c| cols[c].iter().map(|s| parse_f64_lossy(s)).collect())
            .unwrap_or_default(),
    };

    let mut snp: Vec<String> = cols[col_snp].iter().map(|s| s.to_lowercase()).collect();
    let a1: Vec<String> = cols[col_a1].clone();
    let a2: Vec<String> = cols[col_a2].clone();

    let gene = col_gene.map(|c| cols[c].clone());

    // filter rows with any NA or n<=0 (keep GENE even if NA? R: any NA in kept.cols which excludes GENE)
    let mut keep_snp = Vec::with_capacity(nrows);
    let mut keep_a1 = Vec::with_capacity(nrows);
    let mut keep_a2 = Vec::with_capacity(nrows);
    let mut keep_stat = Vec::with_capacity(nrows);
    let mut keep_n = Vec::with_capacity(nrows);
    let mut keep_gene = gene.as_ref().map(|_| Vec::with_capacity(nrows));
    for i in 0..nrows {
        let s = stat[i];
        let nv = n[i];
        if s.is_nan() || nv.is_nan() || nv <= 0.0 || snp[i].is_empty() {
            continue;
        }
        keep_snp.push(std::mem::take(&mut snp[i]));
        keep_a1.push(a1[i].clone());
        keep_a2.push(a2[i].clone());
        keep_stat.push(s);
        keep_n.push(nv);
        if let Some(g) = gene.as_ref() {
            keep_gene.as_mut().unwrap().push(g[i].clone());
        }
    }

    Ok(SumStats {
        snp: keep_snp,
        a1: keep_a1,
        a2: keep_a2,
        stat: keep_stat,
        n: keep_n,
        gene: keep_gene,
    })
}

/// `format.pvalues`: coerce to numeric; clamp p < min_pval up to min_pval.
fn format_pvalue(s: &str, min_pval: f64) -> f64 {
    let p = parse_f64_lossy(s);
    if p.is_nan() {
        return min_pval;
    }
    if p < min_pval { min_pval } else { p }
}

/// Parse a float the way R `as.numeric` does: empty / non-numeric → NaN.
fn parse_f64_lossy(s: &str) -> f64 {
    let t = s.trim();
    if t.is_empty() {
        return f64::NAN;
    }
    t.parse::<f64>().unwrap_or(f64::NAN)
}

// ---------------------------------------------------------------------------
// input.info + sample overlap + loci
// ---------------------------------------------------------------------------

/// Read the input-info file and build `PhenoInfo` rows (subsetting to `phenos`
/// if given, else all). Faithful port of `get.input.info` minus reference load.
pub fn read_input_info(
    input_info_file: &Path,
    phenos: Option<&[String]>,
    input_dir: Option<&str>,
) -> Result<Vec<PhenoInfo>> {
    let (header, cols) = read_table(input_info_file)?;
    for required in &["phenotype", "cases", "controls", "filename"] {
        if !header.iter().any(|h| h == *required) {
            return Err(LavaError::Input(format!(
                "input.info missing required header '{required}'"
            )));
        }
    }
    let i_phen = header.iter().position(|h| h == "phenotype").unwrap();
    let i_cases = header.iter().position(|h| h == "cases").unwrap();
    let i_ctrl = header.iter().position(|h| h == "controls").unwrap();
    let i_file = header.iter().position(|h| h == "filename").unwrap();
    let i_prev = header.iter().position(|h| h == "prevalence");

    let nrows = cols[i_phen].len();
    let mut all: Vec<PhenoInfo> = Vec::with_capacity(nrows);
    for r in 0..nrows {
        let phenotype = cols[i_phen][r].clone();
        let cases = parse_f64_lossy(&cols[i_cases][r]);
        let controls = parse_f64_lossy(&cols[i_ctrl][r]);
        let mut filename = cols[i_file][r].clone();
        if let Some(dir) = input_dir {
            filename = format!("{dir}/{filename}");
        }
        let n = if cases.is_nan() || controls.is_nan() {
            f64::NAN
        } else {
            cases + controls
        };
        let prop_cases = if n.is_nan() || n == 0.0 {
            f64::NAN
        } else {
            cases / n
        };
        // binary = !is.na(prop_cases) & prop_cases != 1
        let binary = !prop_cases.is_nan() && prop_cases != 1.0;
        let prevalence = i_prev.and_then(|i| {
            let v = parse_f64_lossy(&cols[i][r]);
            if v.is_nan() { None } else { Some(v) }
        });
        all.push(PhenoInfo {
            phenotype,
            cases,
            controls,
            filename,
            n,
            prop_cases,
            binary,
            prevalence,
        });
    }
    // subset / order by phenos
    if let Some(want) = phenos {
        let mut out = Vec::with_capacity(want.len());
        for p in want {
            let found = all
                .iter()
                .find(|x| &x.phenotype == p)
                .cloned()
                .ok_or_else(|| {
                    LavaError::Input(format!("Phenotype(s) not listed in input info file: '{p}'"))
                })?;
            out.push(found);
        }
        Ok(out)
    } else {
        Ok(all)
    }
}

/// `process.sample.overlap`: read the matrix (first column = row names), subset
/// to `phenos`, `cov2cor`. Matches R `read.table(..., check.names=F)` + `cov2cor`.
pub fn process_sample_overlap(file: &Path, phenos: &[String]) -> Result<Mat<f64>> {
    use std::io::{BufRead, BufReader};
    let f = std::fs::File::open(file)?;
    let mut lines = BufReader::new(f).lines();
    let header_line = lines
        .next()
        .ok_or_else(|| LavaError::Input("empty overlap file".into()))??;
    let col_names: Vec<String> = split_ws(&header_line);
    // value-column index for phenotype pj = its position in col_names + 1 (offset for row-name col)
    let mut raw: std::collections::HashMap<(String, String), f64> =
        std::collections::HashMap::new();
    for line in lines {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let parts = split_ws(&line);
        if parts.len() != col_names.len() + 1 {
            return Err(LavaError::Input(format!(
                "malformed sample overlap row: {line}"
            )));
        }
        let row_name = parts[0].clone();
        for (j, pname) in col_names.iter().enumerate() {
            raw.insert(
                (row_name.clone(), pname.clone()),
                parse_f64_lossy(&parts[j + 1]),
            );
        }
    }
    let n = phenos.len();
    let mut m = Mat::zeros(n, n);
    for (i, pi) in phenos.iter().enumerate() {
        for (j, pj) in phenos.iter().enumerate() {
            let v = *raw.get(&(pi.clone(), pj.clone())).ok_or_else(|| {
                LavaError::Input(format!(
                    "Phenotype not in sample overlap file: '{pi}'/'{pj}'"
                ))
            })?;
            m[(i, j)] = v;
        }
    }
    Ok(crate::stats::cov2cor(&m))
}

/// `read.loci`: read the locus file into `LocusDef`s.
pub fn read_loci(loc_file: &Path) -> Result<Vec<LocusDef>> {
    let (header, cols) = read_table(loc_file)?;
    let has_coord = ["LOC", "CHR", "START", "STOP"]
        .iter()
        .all(|r| header.iter().any(|h| h == *r));
    let has_snps = ["LOC", "SNPS"]
        .iter()
        .all(|r| header.iter().any(|h| h == *r));
    if !has_coord && !has_snps {
        return Err(LavaError::Input(
            "Locus file missing required headers (LOC + CHR/START/STOP and/or SNPS)".into(),
        ));
    }
    let i_loc = header.iter().position(|h| h == "LOC").unwrap();
    let i_chr = header.iter().position(|h| h == "CHR");
    let i_start = header.iter().position(|h| h == "START");
    let i_stop = header.iter().position(|h| h == "STOP");
    let i_snps = header.iter().position(|h| h == "SNPS");

    let nrows = cols[i_loc].len();
    let mut out = Vec::with_capacity(nrows);
    for r in 0..nrows {
        let loc = cols[i_loc][r].clone();
        let chr = i_chr.map(|i| cols[i][r].parse::<i64>().unwrap_or(0));
        let start = i_start.map(|i| cols[i][r].parse::<i64>().unwrap_or(0));
        let stop = i_stop.map(|i| cols[i][r].parse::<i64>().unwrap_or(0));
        let snps = i_snps.map(|i| {
            cols[i][r]
                .split(';')
                .filter(|s| !s.trim().is_empty())
                .map(|s| s.trim().to_lowercase())
                .collect::<Vec<_>>()
        });
        out.push(LocusDef {
            loc,
            chr,
            start,
            stop,
            snps,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Harmonisation + alignment (process.sumstats core)
// ---------------------------------------------------------------------------

/// Harmonise sum-stats to the SNPs shared across the reference and all
/// phenotypes, in reference order. Returns the shared SNP list and reindexes
/// each phenotype's sum-stats to it (rows matched by SNP; unmatched → dropped).
/// Faithful port of `harmonize.snps`.
pub fn harmonize_snps(
    reference_snps: &[String],
    sum_stats: &mut [SumStats],
) -> Result<Vec<String>> {
    // Start from the intersection of reference and the first phenotype.
    let ref_set: HashSet<&String> = reference_snps.iter().collect();
    let mut analysis: Vec<String> = sum_stats[0]
        .snp
        .iter()
        .filter(|s| ref_set.contains(*s))
        .cloned()
        .collect();
    for ss in sum_stats.iter().skip(1) {
        let set: HashSet<&String> = ss.snp.iter().collect();
        analysis.retain(|s| set.contains(s));
    }
    if analysis.len() < 3 {
        return Err(LavaError::Input(format!(
            "Less than 3 SNPs shared across data sets ({} shared)",
            analysis.len()
        )));
    }
    // Verify analysis order matches reference subsequence (it must, by construction).
    // Subset each phenotype's sum-stats to `analysis`, in that order.
    for ss in sum_stats.iter_mut() {
        let mut idx = std::collections::HashMap::new();
        for (k, s) in ss.snp.iter().enumerate() {
            idx.insert(s.clone(), k); // first occurrence wins
        }
        let mut snp = Vec::with_capacity(analysis.len());
        let mut a1 = Vec::with_capacity(analysis.len());
        let mut a2 = Vec::with_capacity(analysis.len());
        let mut stat = Vec::with_capacity(analysis.len());
        let mut n = Vec::with_capacity(analysis.len());
        let mut gene = ss.gene.as_ref().map(|_| Vec::with_capacity(analysis.len()));
        for s in &analysis {
            let &k = idx
                .get(s)
                .ok_or_else(|| LavaError::Input("SNP missing after harmonise".into()))?;
            snp.push(ss.snp[k].clone());
            a1.push(ss.a1[k].clone());
            a2.push(ss.a2[k].clone());
            stat.push(ss.stat[k]);
            n.push(ss.n[k]);
            if let Some(g) = ss.gene.as_ref() {
                gene.as_mut().unwrap().push(g[k].clone());
            }
        }
        ss.snp = snp;
        ss.a1 = a1;
        ss.a2 = a2;
        ss.stat = stat;
        ss.n = n;
        ss.gene = gene;
    }
    Ok(analysis)
}

/// Align effect alleles to the reference, sign-flipping `stat` and dropping
/// unalignable SNPs from `analysis_snps` and every phenotype. Faithful port of
/// `align`.
pub fn align(
    ref_a1: &[String],
    ref_a2: &[String],
    analysis_snps: &mut Vec<String>,
    sum_stats: &mut [SumStats],
) -> Vec<String> {
    // reference allele map for the analysis SNPs (ref is ordered like analysis_snps)
    let ref_map: Vec<Option<i8>> = (0..analysis_snps.len())
        .map(|i| map_alleles(&ref_a1[i], &ref_a2[i]))
        .collect();

    let mut remove: Vec<usize> = Vec::new();
    for ss in sum_stats.iter_mut() {
        for (i, _) in analysis_snps.iter().enumerate() {
            let sum_map = map_alleles(&ss.a1[i], &ss.a2[i]);
            let cmp = com_pair(ref_map[i], sum_map);
            match cmp {
                Some(s) => ss.stat[i] *= s,
                None => {
                    ss.stat[i] = f64::NAN;
                    if !remove.contains(&i) {
                        remove.push(i);
                    }
                }
            }
        }
    }

    let mut unalignable = Vec::new();
    if !remove.is_empty() {
        remove.sort_unstable();
        remove.dedup();
        // build keep mask
        let remove_set: HashSet<usize> = remove.iter().copied().collect();
        for (i, s) in analysis_snps.iter().enumerate() {
            if remove_set.contains(&i) {
                unalignable.push(s.clone());
            }
        }
        // drop from analysis_snps and each sumstat
        drop_indices(analysis_snps, &remove_set);
        for ss in sum_stats.iter_mut() {
            drop_indices_ss(ss, &remove_set);
        }
    }
    unalignable
}

fn drop_indices(v: &mut Vec<String>, remove: &HashSet<usize>) {
    let kept: Vec<String> = v
        .iter()
        .enumerate()
        .filter(|(i, _)| !remove.contains(i))
        .map(|(_, s)| s.clone())
        .collect();
    *v = kept;
}

fn drop_indices_ss(ss: &mut SumStats, remove: &HashSet<usize>) {
    let keep: Vec<usize> = (0..ss.snp.len()).filter(|i| !remove.contains(i)).collect();
    let pick = |v: &mut Vec<String>| {
        let kept: Vec<String> = keep.iter().map(|&i| v[i].clone()).collect();
        *v = kept;
    };
    let pickf = |v: &mut Vec<f64>| {
        let kept: Vec<f64> = keep.iter().map(|&i| v[i]).collect();
        *v = kept;
    };
    pick(&mut ss.snp);
    pick(&mut ss.a1);
    pick(&mut ss.a2);
    pickf(&mut ss.stat);
    pickf(&mut ss.n);
    if let Some(g) = ss.gene.as_mut() {
        pick(g);
    }
}

/// `process.input`: read info + sample overlap + all sum-stats, load the PLINK
/// reference, harmonise to common SNPs, align effect alleles to the reference,
/// and build the SNP→`.bim` index. Faithful port of `process.input` (PLINK mode).
pub fn process_input(
    input_info_file: &Path,
    sample_overlap_file: Option<&Path>,
    ref_prefix: &Path,
    phenos: Option<&[String]>,
    input_dir: Option<&str>,
) -> Result<Input> {
    let info = read_input_info(input_info_file, phenos, input_dir)?;
    let phenos: Vec<String> = info.iter().map(|p| p.phenotype.clone()).collect();
    let p = phenos.len();

    // sample overlap (cov2cor) — NULL if single phenotype
    let sample_overlap = if p > 1 {
        sample_overlap_file
            .map(|f| process_sample_overlap(f, &phenos))
            .transpose()?
    } else {
        None
    };

    // read sumstats for each phenotype
    let mut sum_stats: Vec<SumStats> = Vec::with_capacity(p);
    for pi in &info {
        sum_stats.push(read_sumstats_file(
            Path::new(&pi.filename),
            &pi.phenotype,
            1e-300,
            None,
        )?);
    }

    finish_input(info, phenos, sum_stats, sample_overlap, ref_prefix)
}

/// Build a processed [`Input`] from in-memory per-phenotype sum-stats + sample
/// overlap + PLINK reference, performing the harmonise/align/index steps. Used by
/// the DAG node (which receives sum-stats via its input port rather than files).
///
/// The PLINK reference is loaded from a single merged prefix here. For a
/// per-chromosome reference (prefix template), build the [`PlinkRef`] with
/// [`crate::plink::load_reference_template`] and call [`finish_input_with_ref`].
pub fn finish_input(
    info: Vec<PhenoInfo>,
    phenos: Vec<String>,
    sum_stats: Vec<SumStats>,
    sample_overlap: Option<Mat<f64>>,
    ref_prefix: &Path,
) -> Result<Input> {
    let reference = crate::plink::load_reference(ref_prefix)?;
    finish_input_with_ref(info, phenos, sum_stats, sample_overlap, reference)
}

/// Like [`finish_input`] but takes a pre-built [`PlinkRef`] (single-prefix or
/// per-chromosome template), so the harmonise/align/index steps run against any
/// reference shape.
pub fn finish_input_with_ref(
    info: Vec<PhenoInfo>,
    phenos: Vec<String>,
    mut sum_stats: Vec<SumStats>,
    sample_overlap: Option<Mat<f64>>,
    reference: PlinkRef,
) -> Result<Input> {
    let p = phenos.len();
    let ref_snps: Vec<String> = reference.snp_info.snp.clone();
    let mut analysis_snps = harmonize_snps(&ref_snps, &mut sum_stats)?;
    let ref_alleles: HashMap<String, (String, String)> = reference
        .snp_info
        .snp
        .iter()
        .zip(
            reference
                .snp_info
                .a1
                .iter()
                .zip(reference.snp_info.a2.iter()),
        )
        .map(|(s, (a1, a2))| (s.clone(), (a1.clone(), a2.clone())))
        .collect();
    let ref_a1: Vec<String> = analysis_snps
        .iter()
        .map(|s| ref_alleles.get(s).unwrap().0.clone())
        .collect();
    let ref_a2: Vec<String> = analysis_snps
        .iter()
        .map(|s| ref_alleles.get(s).unwrap().1.clone())
        .collect();
    let unalignable = align(&ref_a1, &ref_a2, &mut analysis_snps, &mut sum_stats);
    let mut bim_index: HashMap<String, usize> = HashMap::new();
    for (i, s) in reference.snp_info.snp.iter().enumerate() {
        bim_index.entry(s.clone()).or_insert(i);
    }
    Ok(Input {
        info,
        phenos,
        p,
        sum_stats,
        analysis_snps,
        unalignable_snps: unalignable,
        sample_overlap,
        reference,
        bim_index,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allele_pair_coding() {
        // AC vs reference AC -> same direction
        assert_eq!(
            com_pair(map_alleles("A", "C"), map_alleles("A", "C")),
            Some(1.0)
        );
        // AC vs CA -> flipped
        assert_eq!(
            com_pair(map_alleles("A", "C"), map_alleles("C", "A")),
            Some(-1.0)
        );
        // strand-ambiguous AT -> None
        assert_eq!(map_alleles("A", "T"), None);
        // mismatched
        assert_eq!(com_pair(map_alleles("A", "C"), map_alleles("A", "G")), None);
    }

    /// `load_reference_template` must merge per-chromosome `.bim`s into one
    /// global `snp_info` with correct `chr_offset` (per-chr start index) and
    /// `chr_prefix` (resolved per-chr prefix), skipping absent chromosomes and
    /// enforcing a uniform sample size. (`.bed` content is not read here, only
    /// its existence checked, so an empty file suffices.)
    #[test]
    fn load_reference_template_merges_per_chrom() {
        use std::fs;
        let dir = std::env::temp_dir().join(format!("lava_plink_test_{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();

        // write a fake prefix: .bim (CHR SNP cM BP A1 A2) + .fam (N lines) + empty .bed
        let write_prefix = |prefix: &std::path::Path, chr: i64, snps: &[(i64, &str)]| {
            let bim = prefix.with_extension("bim");
            let fam = prefix.with_extension("fam");
            let bed = prefix.with_extension("bed");
            let bim_txt: String = snps
                .iter()
                .map(|(pos, id)| format!("{chr} {id} 0 {pos} A C\n"))
                .collect();
            fs::write(&bim, bim_txt).unwrap();
            fs::write(&fam, "F1 I1 0 0 1 -9\nF2 I2 0 0 1 -9\nF3 I3 0 0 1 -9\n").unwrap();
            fs::write(&bed, []).unwrap(); // exists, unread at load time
        };
        write_prefix(&dir.join("chr1"), 1, &[(100, "rs1a"), (200, "rs1b")]);
        write_prefix(&dir.join("chr2"), 2, &[(50, "rs2a"), (60, "rs2b")]);

        let template = dir.join("chr{N}");
        let tpl = template.to_string_lossy().into_owned();
        // include chr3 (absent) to confirm graceful skip
        let r = crate::plink::load_reference_template(&tpl, &[1, 2, 3]).unwrap();

        assert_eq!(r.sample_size, 3, "sample size from .fam line count");
        assert_eq!(r.snp_info.snp, vec!["rs1a", "rs1b", "rs2a", "rs2b"]);
        assert_eq!(r.snp_info.chr, vec![1, 1, 2, 2]);
        assert_eq!(r.chr_offset.get(&1).copied(), Some(0), "chr1 offset");
        assert_eq!(r.chr_offset.get(&2).copied(), Some(2), "chr2 offset");
        assert!(!r.chr_offset.contains_key(&3), "chr3 skipped");
        assert_eq!(
            r.chr_prefix
                .get(&1)
                .map(|p| p.file_name().unwrap().to_str().unwrap().to_string()),
            Some("chr1".into())
        );
        assert_eq!(
            r.chr_prefix
                .get(&2)
                .map(|p| p.file_name().unwrap().to_str().unwrap().to_string()),
            Some("chr2".into())
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// A single merged prefix must produce `chr_offset[c] == 0` for every
    /// chromosome (the merged `.bed` is indexed by absolute global row, so
    /// `process_locus` passes global indices to `load_plink` unchanged).
    /// Regression guard for the per-chromosome refactor.
    #[test]
    fn load_reference_merged_has_zero_offsets() {
        use std::fs;
        let dir = std::env::temp_dir().join(format!("lava_plink_merged_{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        // one merged .bim spanning chr1 + chr2
        let bim = "1 rs1a 0 100 A C\n1 rs1b 0 200 A C\n2 rs2a 0 50 A C\n2 rs2b 0 60 A C\n";
        fs::write(dir.join("merged.bim"), bim).unwrap();
        fs::write(dir.join("merged.fam"), "F1 I1 0 0 1 -9\nF2 I2 0 0 1 -9\n").unwrap();
        fs::write(dir.join("merged.bed"), []).unwrap();

        let r = crate::plink::load_reference(&dir.join("merged")).unwrap();
        assert_eq!(r.snp_info.chr, vec![1, 1, 2, 2]);
        assert_eq!(
            r.chr_offset.get(&1).copied(),
            Some(0),
            "merged chr1 offset must be 0"
        );
        assert_eq!(
            r.chr_offset.get(&2).copied(),
            Some(0),
            "merged chr2 offset must be 0 (NOT its bim start)"
        );
        assert_eq!(
            r.chr_prefix.get(&1),
            r.chr_prefix.get(&2),
            "merged: both chrs → same prefix"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
