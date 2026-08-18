//! PLINK `.bed/.bim/.fam` reader — faithful port of `baseinput_plinkinput.cpp`.
//!
//! Decodes the 2-bit SNP-major binary genotype format (magic bytes `0x6c 0x1b 0x01`),
//! providing per-SNP genotype vectors for MAGMA's gene analysis.
//!
//! The PLINK .bed format packs 4 individuals per byte, SNP-major order:
//! each SNP occupies `ceil(n_indiv/4)` bytes starting at byte offset 3.
//! The 2-bit codes are: `00`=hom1, `01`=missing, `10`=het, `11`=hom2.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::error::{MagmaError, Result};

/// Resolve a PLINK file path by appending `.bed`/`.bim`/`.fam` to the prefix.
/// This is the PLINK convention: `1000G.EUR.QC.22` → `1000G.EUR.QC.22.bim`.
pub fn plink_path(prefix: &Path, ext: &str) -> PathBuf {
    let mut s = prefix.as_os_str().to_owned();
    s.push(".");
    s.push(ext);
    PathBuf::from(s)
}

/// SNP metadata from a `.bim` file.
#[derive(Debug, Clone)]
pub struct SnpInfo {
    pub rsid: String,
    pub chr: i32,
    pub pos: u64,
    pub a1: String,
    pub a2: String,
}

/// Individual metadata from a `.fam` file.
#[derive(Debug, Clone)]
pub struct IndivInfo {
    pub fid: String,
    pub iid: String,
    pub gender: i32,        // 1=male, 2=female, 0=unknown
    pub pheno: Option<f64>, // None if missing/NA
}

/// PLINK binary genotype data handle.
///
/// Holds the open `.bed` file, the `.bim` SNP list, and the `.fam` individual
/// list. SNP genotypes are read on demand via [`BedFile::read_snp`] or in
/// batch via [`BedFile::read_snps`].
///
/// For performance with large files, call [`BedFile::cache_all`] to load the
/// entire `.bed` into memory, eliminating per-SNP seek overhead.
pub struct BedFile {
    bed: Option<File>,
    /// In-memory cache of the entire .bed file (optional). When present,
    /// `read_snp` reads from this buffer instead of seeking the file handle.
    bed_cache: Option<Vec<u8>>,
    pub snps: Vec<SnpInfo>,
    pub indivs: Vec<IndivInfo>,
    /// Map rsid → index into `snps`.
    pub snp_index: HashMap<String, usize>,
    /// Map "FID IID" → index into `indivs`.
    pub indiv_index: HashMap<String, usize>,
    block_count: usize,
}

/// Missing genotype code (matches MAGMA's `misscode() = 3` for PLINK input).
pub const MISS: f64 = 3.0;

impl BedFile {
    /// Load a PLINK binary fileset from the given prefix (without `.bed` etc.).
    pub fn open(prefix: &Path) -> Result<Self> {
        let bed_path = plink_path(prefix, "bed");
        let bim_path = plink_path(prefix, "bim");
        let fam_path = plink_path(prefix, "fam");

        let indivs = read_fam(&fam_path)?;
        let n_indiv = indivs.len();
        let snps = read_bim(&bim_path)?;
        let n_snp = snps.len();

        let mut bed = File::open(&bed_path).map_err(MagmaError::Io)?;
        let bed_size = bed.metadata()?.len() as usize;
        let block_count = n_indiv.div_ceil(4);
        let exp_size = 3 + block_count * n_snp;
        if bed_size != exp_size {
            return Err(MagmaError::Input(format!(
                ".bed file size ({bed_size}) inconsistent with .bim ({n_snp} SNPs) and .fam ({n_indiv} indivs); expected {exp_size}"
            )));
        }

        // Validate magic bytes
        let mut magic = [0u8; 3];
        bed.read_exact(&mut magic).map_err(MagmaError::Io)?;
        if magic[0] != 0x6c || magic[1] != 0x1b {
            return Err(MagmaError::Input(
                ".bed file is not a valid PLINK binary file".into(),
            ));
        }
        if magic[2] != 0x01 {
            return Err(MagmaError::Input(
                ".bed file is not in SNP-major format".into(),
            ));
        }

        let snp_index: HashMap<String, usize> = snps
            .iter()
            .enumerate()
            .map(|(i, s)| (s.rsid.clone(), i))
            .collect();
        let indiv_index: HashMap<String, usize> = indivs
            .iter()
            .enumerate()
            .map(|(i, ind)| (format!("{} {}", ind.fid, ind.iid), i))
            .collect();

        Ok(Self {
            bed: Some(bed),
            bed_cache: None,
            snps,
            indivs,
            snp_index,
            indiv_index,
            block_count,
        })
    }

    /// Build a PLINK fileset from bytes read from a virtualized storage layer.
    ///
    /// VFS object stores do not expose synchronous seekable file handles, so
    /// the BED contents are retained in memory as an already-loaded cache.
    pub fn from_bytes(bed: Vec<u8>, bim: &str, fam: &str) -> Result<Self> {
        let indivs = parse_fam(fam, "<vfs>.fam")?;
        let n_indiv = indivs.len();
        let snps = parse_bim(bim, "<vfs>.bim")?;
        let n_snp = snps.len();
        let block_count = n_indiv.div_ceil(4);
        let exp_size = 3 + block_count * n_snp;
        if bed.len() != exp_size {
            return Err(MagmaError::Input(format!(
                ".bed size ({}) inconsistent with .bim ({n_snp} SNPs) and .fam ({n_indiv} indivs); expected {exp_size}",
                bed.len()
            )));
        }
        if bed[0] != 0x6c || bed[1] != 0x1b {
            return Err(MagmaError::Input(
                ".bed file is not a valid PLINK binary file".into(),
            ));
        }
        if bed[2] != 0x01 {
            return Err(MagmaError::Input(
                ".bed file is not in SNP-major format".into(),
            ));
        }

        let snp_index: HashMap<String, usize> = snps
            .iter()
            .enumerate()
            .map(|(i, s)| (s.rsid.clone(), i))
            .collect();
        let indiv_index: HashMap<String, usize> = indivs
            .iter()
            .enumerate()
            .map(|(i, ind)| (format!("{} {}", ind.fid, ind.iid), i))
            .collect();

        Ok(Self {
            bed: None,
            bed_cache: Some(bed),
            snps,
            indivs,
            snp_index,
            indiv_index,
            block_count,
        })
    }

    pub fn n_indiv(&self) -> usize {
        self.indivs.len()
    }

    pub fn n_snp(&self) -> usize {
        self.snps.len()
    }

    /// Open multiple per-chromosome PLINK filesets and merge into a single
    /// virtual `BedFile`. `template` contains `{N}` which is replaced with
    /// each chromosome number in `chroms`. All chromosomes must share the
    /// same individual set (same `.fam`).
    ///
    /// The `.bim` SNP lists are concatenated, `.bed` genotype bytes are merged
    /// (stripping per-file 3-byte magic headers), and a single in-memory
    /// `bed_cache` is built so `read_snp(global_idx)` works seamlessly across
    /// chromosomes — matching the behaviour of a single genome-wide PLINK file.
    pub fn open_template(template: &str, chroms: &[u32]) -> Result<Self> {
        if chroms.is_empty() {
            return Err(MagmaError::Input(
                "open_template: no chromosomes specified".into(),
            ));
        }

        let mut indivs = Vec::new();
        let mut indiv_index = HashMap::new();
        let mut block_count = 0usize;
        let mut snps: Vec<SnpInfo> = Vec::new();
        let mut snp_index: HashMap<String, usize> = HashMap::new();
        // magic bytes + concatenated genotype blocks (per-file magic stripped)
        let mut bed_data: Vec<u8> = vec![0x6c, 0x1b, 0x01];
        let mut bed_handle: Option<File> = None;

        for &chrom in chroms {
            let prefix = template.replace("{N}", &chrom.to_string());
            let mut bf = BedFile::open(Path::new(&prefix))?;

            if indivs.is_empty() {
                indivs = std::mem::take(&mut bf.indivs);
                indiv_index = std::mem::take(&mut bf.indiv_index);
                block_count = bf.block_count;
            } else if bf.indivs.len() != indivs.len() {
                return Err(MagmaError::Input(format!(
                    "chromosome {chrom} panel has {} individuals, expected {} \
                     (all chromosomes must share the same .fam)",
                    bf.indivs.len(),
                    indivs.len()
                )));
            }

            bf.cache_all()?;
            if let Some(ref cache) = bf.bed_cache {
                bed_data.extend_from_slice(&cache[3..]);
            }

            for s in bf.snps {
                snp_index.insert(s.rsid.clone(), snps.len());
                snps.push(s);
            }

            // Retain the first seekable handle for callers that have not
            // requested BED caching; merged files are always cached below.
            if bed_handle.is_none() {
                bed_handle = bf.bed.take();
            }
        }

        Ok(Self {
            bed: bed_handle,
            bed_cache: Some(bed_data),
            snps,
            indivs,
            snp_index,
            indiv_index,
            block_count,
        })
    }

    /// Load the entire `.bed` file into memory. Subsequent `read_snp` calls
    /// will read from this buffer instead of seeking the file handle.
    /// This dramatically improves performance for analyses that read many SNPs.
    pub fn cache_all(&mut self) -> Result<()> {
        use std::io::Read;
        let Some(bed) = self.bed.as_mut() else {
            return Ok(());
        };
        bed.seek(SeekFrom::Start(0)).map_err(MagmaError::Io)?;
        let mut buf = Vec::new();
        bed.read_to_end(&mut buf).map_err(MagmaError::Io)?;
        self.bed_cache = Some(buf);
        Ok(())
    }

    /// Read dosage values for a single SNP (0-based index into `.bim`).
    /// Returns a vector of length `n_indiv`: 0.0, 1.0, 2.0, or 3.0 (missing).
    pub fn read_snp(&mut self, snp_idx: usize) -> Result<Vec<f64>> {
        let offset = 3 + snp_idx * self.block_count;
        let buf = if let Some(ref cache) = self.bed_cache {
            &cache[offset..offset + self.block_count]
        } else {
            // Fall back to file seek
            let Some(bed) = self.bed.as_mut() else {
                return Err(MagmaError::Input(
                    "PLINK reader has no seekable BED backend".into(),
                ));
            };
            bed.seek(SeekFrom::Start(offset as u64))
                .map_err(MagmaError::Io)?;
            let mut tmp = vec![0u8; self.block_count];
            bed.read_exact(&mut tmp).map_err(MagmaError::Io)?;
            // Can't return reference to local, so decode directly
            return Ok(decode_block(&tmp, self.n_indiv()));
        };
        Ok(decode_block(buf, self.n_indiv()))
    }

    /// Read dosage values for multiple SNPs, returning a flattened matrix
    /// (row-major: SNP-major, individuals within each SNP).
    /// Equivalent to MAGMA's `load_snpdata` writing into a flat buffer.
    pub fn read_snps(&mut self, snp_indices: &[usize]) -> Result<Vec<f64>> {
        let n_indiv = self.n_indiv();
        let mut out = Vec::with_capacity(snp_indices.len() * n_indiv);
        for &snp_idx in snp_indices {
            let geno = self.read_snp(snp_idx)?;
            out.extend(geno);
        }
        Ok(out)
    }

    /// Count alleles (hom2 dosage sum + missing count) for a set of SNPs.
    /// Returns `(counts, n_observed)` where counts is `[n_snp][2]` with
    /// `[allele_sum, miss_count]`.
    ///
    /// Faithful port of `PlinkInput::allele_count`.
    pub fn allele_counts(&mut self, snp_indices: &[usize]) -> Result<(Vec<[i32; 2]>, usize)> {
        let n_indiv = self.n_indiv();
        let mut counts = vec![[0i32, 0i32]; snp_indices.len()];
        for (snp_out, &snp_idx) in snp_indices.iter().enumerate() {
            let offset = 3 + snp_idx * self.block_count;
            let mut buf_owned;
            let buf: &[u8] = if let Some(ref cache) = self.bed_cache {
                &cache[offset..offset + self.block_count]
            } else {
                let Some(bed) = self.bed.as_mut() else {
                    return Err(MagmaError::Input(
                        "PLINK reader has no seekable BED backend".into(),
                    ));
                };
                bed.seek(SeekFrom::Start(offset as u64))
                    .map_err(MagmaError::Io)?;
                buf_owned = vec![0u8; self.block_count];
                bed.read_exact(&mut buf_owned).map_err(MagmaError::Io)?;
                &buf_owned
            };
            for (j, &code) in buf.iter().enumerate() {
                for k in 0..4 {
                    let indiv_idx = j * 4 + k;
                    if indiv_idx >= n_indiv {
                        break;
                    }
                    let pair = (code >> (2 * k)) & 3;
                    match pair {
                        0 => {}                       // hom1: dosage 0
                        3 => counts[snp_out][0] += 2, // hom2: dosage 2
                        2 => counts[snp_out][0] += 1, // het: dosage 1
                        1 => counts[snp_out][1] += 1, // missing
                        _ => {}
                    }
                }
            }
        }
        Ok((counts, n_indiv))
    }
}

/// Decode a block of packed genotype bytes into dosage values.
fn decode_block(buf: &[u8], n_indiv: usize) -> Vec<f64> {
    // Lookup table for 2-bit codes → dosage (PLINK convention).
    // 00 → 0.0 (hom ref), 01 → 3.0 (missing), 10 → 1.0 (het), 11 → 2.0 (hom alt)
    const CODE_TO_DOSAGE: [f64; 4] = [0.0, MISS, 1.0, 2.0];
    let mut out = Vec::with_capacity(n_indiv);
    for &byte in buf {
        for k in 0..4 {
            if out.len() >= n_indiv {
                return out;
            }
            let pair = ((byte >> (2 * k)) & 3) as usize;
            out.push(CODE_TO_DOSAGE[pair]);
        }
    }
    out
}

/// Read a `.fam` file: `FID IID Father Mother Sex Pheno` (whitespace-separated).
fn read_fam(path: &Path) -> Result<Vec<IndivInfo>> {
    let content = std::fs::read_to_string(path).map_err(MagmaError::Io)?;
    parse_fam(&content, &path.to_string_lossy())
}

fn parse_fam(content: &str, source: &str) -> Result<Vec<IndivInfo>> {
    let mut indivs = Vec::new();
    for (lineno, line) in content.lines().enumerate() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 6 {
            return Err(MagmaError::Input(format!(
                "{source}: line {} has fewer than 6 fields",
                lineno + 1
            )));
        }
        let fid = fields[0].to_string();
        let iid = fields[1].to_string();
        let gender: i32 = fields[4].parse().unwrap_or(0);
        let gender = match gender {
            1 | 2 => gender,
            _ => 0,
        };
        let pheno = if fields[5] == "NA" || fields[5] == "-9" {
            None
        } else {
            fields[5].parse::<f64>().ok()
        };
        indivs.push(IndivInfo {
            fid,
            iid,
            gender,
            pheno,
        });
    }
    if indivs.is_empty() {
        return Err(MagmaError::Input(format!(
            "{source}: no individuals in file"
        )));
    }
    Ok(indivs)
}

/// Read a `.bim` file: `Chr rsID 0(Pos_cM) Pos A1 A2` (whitespace-separated).
/// Also handles MAGMA's own .bim format which may have a different column 3.
fn read_bim(path: &Path) -> Result<Vec<SnpInfo>> {
    let content = std::fs::read_to_string(path).map_err(MagmaError::Io)?;
    parse_bim(&content, &path.to_string_lossy())
}

fn parse_bim(content: &str, source: &str) -> Result<Vec<SnpInfo>> {
    let mut snps = Vec::new();
    for (lineno, line) in content.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 6 {
            return Err(MagmaError::Input(format!(
                "{source}: line {} has fewer than 6 fields",
                lineno + 1
            )));
        }
        let chr: i32 = fields[0].parse().map_err(|_| {
            MagmaError::Input(format!(
                "{source}: line {}: chromosome '{}' not a valid integer",
                lineno + 1,
                fields[0]
            ))
        })?;
        let pos: u64 = fields[3].parse().map_err(|_| {
            MagmaError::Input(format!(
                "{source}: line {}: position '{}' not a valid integer",
                lineno + 1,
                fields[3]
            ))
        })?;
        snps.push(SnpInfo {
            rsid: fields[1].to_string(),
            chr,
            pos,
            a1: fields[4].to_string(),
            a2: fields[5].to_string(),
        });
    }
    if snps.is_empty() {
        return Err(MagmaError::Input(format!("{source}: no SNPs in file")));
    }
    Ok(snps)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_data_dir() -> &'static str {
        "tests/data"
    }

    #[test]
    fn test_open_sim_geno() {
        let prefix = Path::new(test_data_dir()).join("sim_geno");
        let bed = BedFile::open(&prefix).unwrap();
        assert_eq!(bed.n_indiv(), 200);
        assert_eq!(bed.n_snp(), 500);
        assert_eq!(bed.snps[0].rsid, "rs1");
        assert_eq!(bed.snps[0].chr, 1);
        assert_eq!(bed.snps[0].pos, 100000);
        assert_eq!(bed.snps[499].rsid, "rs500");
        assert_eq!(bed.snps[499].pos, 5090000);
    }

    #[test]
    fn test_read_snp_dosages() {
        let prefix = Path::new(test_data_dir()).join("sim_geno");
        let mut bed = BedFile::open(&prefix).unwrap();
        let geno = bed.read_snp(0).unwrap();
        assert_eq!(geno.len(), 200);
        // All dosages should be 0.0, 1.0, 2.0, or 3.0 (missing)
        for &d in &geno {
            assert!(
                d == 0.0 || d == 1.0 || d == 2.0 || d == 3.0,
                "unexpected dosage {d}"
            );
        }
    }

    #[test]
    fn test_allele_counts() {
        let prefix = Path::new(test_data_dir()).join("sim_geno");
        let mut bed = BedFile::open(&prefix).unwrap();
        let (counts, n_obs) = bed.allele_counts(&[0, 1, 2]).unwrap();
        assert_eq!(n_obs, 200);
        assert_eq!(counts.len(), 3);
        // allele_sum should be between 0 and 2*200
        for c in &counts {
            assert!(c[0] >= 0 && c[0] <= 400);
            assert!(c[0] + c[1] <= 400);
        }
    }
}
