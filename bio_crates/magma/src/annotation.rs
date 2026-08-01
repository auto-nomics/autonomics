//! Gene-SNP annotation — faithful port of `annotation.cpp`.
//!
//! Maps SNPs to genes based on genomic location, producing a `.genes.annot`
//! file. A SNP is assigned to a gene if it falls within the gene's transcription
//! region ± an optional window.
//!
//! # Input formats
//!
//! **Gene location file** (one gene per line):
//! ```text
//! gene_id  chr  start  end  [strand]
//! ```
//! `strand` is only needed if the upstream/downstream window is asymmetric.
//!
//! **SNP location file**: either a plain `.bim` file or a 3-column file
//! (`rsid chr pos`).
//!
//! # Output format (`.genes.annot`)
//! ```text
//! # window_up = 35000
//! # window_down = 35000
//! gene_id  chr:start:end  rsid1  rsid2  ...
//! ```

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use crate::error::{MagmaError, Result};

/// A gene location entry from the gene-loc file.
#[derive(Debug, Clone)]
pub struct GeneLoc {
    pub id: String,
    pub chr: i32,
    pub start: u64,
    pub end: u64,
    pub strand: Strand,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strand {
    Plus,
    Minus,
    Unknown,
}

impl Strand {
    fn parse(s: &str) -> Self {
        match s {
            "+" => Strand::Plus,
            "-" => Strand::Minus,
            _ => Strand::Unknown,
        }
    }
}

/// A SNP location entry.
#[derive(Debug, Clone)]
pub struct SnpLoc {
    pub rsid: String,
    pub chr: i32,
    pub pos: u64,
}

/// Annotation result: per-gene list of mapped SNP IDs.
#[derive(Debug, Clone)]
pub struct GeneAnnotation {
    pub genes: Vec<GeneAnnotEntry>,
    pub window_up: i64,
    pub window_down: i64,
}

/// One gene's annotation: ID, location, and the SNPs mapped to it.
#[derive(Debug, Clone)]
pub struct GeneAnnotEntry {
    pub id: String,
    pub chr: i32,
    pub start: u64,
    pub end: u64,
    pub snps: Vec<String>,
}

impl GeneAnnotation {
    /// Total number of non-empty genes.
    pub fn n_genes(&self) -> usize {
        self.genes.iter().filter(|g| !g.snps.is_empty()).count()
    }

    /// Total number of unique SNPs mapped to at least one gene.
    pub fn n_mapped_snps(&self) -> usize {
        let mut all: HashSet<&str> = HashSet::new();
        for g in &self.genes {
            for s in &g.snps {
                all.insert(s);
            }
        }
        all.len()
    }
}

/// Read a gene location file.
///
/// Each line: `gene_id  chr  start  end  [strand]`.
/// Strands `+`/`-` are only used if window_up ≠ window_down.
pub fn read_gene_loc(path: &Path) -> Result<Vec<GeneLoc>> {
    let content = std::fs::read_to_string(path).map_err(MagmaError::Io)?;
    let mut genes = Vec::new();
    for (lineno, line) in content.lines().enumerate() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.is_empty() {
            continue;
        }
        if fields.len() < 4 {
            return Err(MagmaError::Input(format!(
                "{path:?}: line {}: expected at least 4 fields (gene_id chr start end), got {}",
                lineno + 1,
                fields.len()
            )));
        }
        let id = fields[0].to_string();
        let chr = parse_chr(fields[1]).ok_or_else(|| {
            MagmaError::Input(format!(
                "{path:?}: line {}: chromosome '{}' not recognised",
                lineno + 1,
                fields[1]
            ))
        })?;
        let start: u64 = fields[2].parse().map_err(|_| {
            MagmaError::Input(format!(
                "{path:?}: line {}: start '{}' not a valid integer",
                lineno + 1,
                fields[2]
            ))
        })?;
        let end: u64 = fields[3].parse().map_err(|_| {
            MagmaError::Input(format!(
                "{path:?}: line {}: end '{}' not a valid integer",
                lineno + 1,
                fields[3]
            ))
        })?;
        let strand = if fields.len() >= 5 {
            Strand::parse(fields[4])
        } else {
            Strand::Plus
        };
        genes.push(GeneLoc {
            id,
            chr,
            start,
            end,
            strand,
        });
    }
    if genes.is_empty() {
        return Err(MagmaError::Input(format!("{path:?}: found no valid genes")));
    }
    Ok(genes)
}

/// Read SNP locations from either a `.bim` file or a 3-column text file.
pub fn read_snp_loc(path: &Path) -> Result<Vec<SnpLoc>> {
    let content = std::fs::read_to_string(path).map_err(MagmaError::Io)?;
    let is_bim = path.extension().is_some_and(|e| e == "bim");

    let mut snps = Vec::new();
    for (lineno, line) in content.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        let (rsid, chr_str, pos_str) = if is_bim {
            // .bim: chr rsID 0 pos a1 a2
            if fields.len() < 6 {
                return Err(MagmaError::Input(format!(
                    "{path:?}: line {}: .bim file has fewer than 6 fields",
                    lineno + 1
                )));
            }
            (fields[1], fields[0], fields[3])
        } else {
            // Plain: rsid chr pos
            if fields.len() < 3 {
                return Err(MagmaError::Input(format!(
                    "{path:?}: line {}: expected at least 3 fields (rsid chr pos)",
                    lineno + 1
                )));
            }
            (fields[0], fields[1], fields[2])
        };
        let chr = parse_chr(chr_str).ok_or_else(|| {
            MagmaError::Input(format!(
                "{path:?}: line {}: chromosome '{}' not recognised",
                lineno + 1,
                chr_str
            ))
        })?;
        let pos: u64 = pos_str.parse().map_err(|_| {
            MagmaError::Input(format!(
                "{path:?}: line {}: position '{}' not a valid integer",
                lineno + 1,
                pos_str
            ))
        })?;
        snps.push(SnpLoc {
            rsid: rsid.to_string(),
            chr,
            pos,
        });
    }
    if snps.is_empty() {
        return Err(MagmaError::Input(format!("{path:?}: found no valid SNPs")));
    }
    Ok(snps)
}

/// Perform the annotation: assign SNPs to genes based on location + window.
///
/// Faithful port of the `Annotation` constructor in `annotation.cpp`.
/// Genes are sorted by start position per chromosome, then each SNP is tested
/// against overlapping gene ranges.
pub fn annotate(
    genes: &[GeneLoc],
    snps: &[SnpLoc],
    window_up_bp: i64,
    window_down_bp: i64,
) -> Result<GeneAnnotation> {
    let do_window = window_up_bp > 0 || window_down_bp > 0;
    let read_strand = do_window && window_up_bp != window_down_bp;

    // Apply window and build per-chromosome gene lists (sorted by start).
    let mut by_chr: BTreeMap<i32, Vec<(usize, u64, u64)>> = BTreeMap::new();
    let mut gene_entries: Vec<GeneAnnotEntry> = Vec::with_capacity(genes.len());

    for (gi, g) in genes.iter().enumerate() {
        let (win_up, win_down) = if do_window {
            if read_strand {
                match g.strand {
                    Strand::Minus => (window_down_bp, window_up_bp),
                    _ => (window_up_bp, window_down_bp),
                }
            } else {
                (window_up_bp, window_down_bp)
            }
        } else {
            (0, 0)
        };

        let start = if (g.start as i64) < win_up {
            0
        } else {
            g.start - win_up as u64
        };
        let end = g.end.saturating_add(win_down as u64);

        gene_entries.push(GeneAnnotEntry {
            id: g.id.clone(),
            chr: g.chr,
            start,
            end,
            snps: Vec::new(),
        });

        by_chr.entry(g.chr).or_default().push((gi, start, end));
    }

    // Sort gene ranges by start within each chromosome.
    for ranges in by_chr.values_mut() {
        ranges.sort_by_key(|&(_, start, _)| start);
    }

    // Assign SNPs to genes. For each SNP, find all genes on the same
    // chromosome whose [start, end] range contains the SNP position.
    for s in snps {
        if let Some(ranges) = by_chr.get(&s.chr) {
            let pos = s.pos;
            // Scan all genes whose start <= pos (ranges sorted by start).
            let end_scan = ranges.partition_point(|&(_, gstart, _)| gstart <= pos);
            for &(gi, _gstart, gend) in &ranges[..end_scan] {
                if gend >= pos {
                    gene_entries[gi].snps.push(s.rsid.clone());
                }
            }
        }
    }

    Ok(GeneAnnotation {
        genes: gene_entries,
        window_up: window_up_bp,
        window_down: window_down_bp,
    })
}

/// Write the `.genes.annot` file.
///
/// Format:
/// ```text
/// # window_up = <bp>
/// # window_down = <bp>
/// gene_id  chr:start:end  rsid1 rsid2 ...
/// ```
/// Only non-empty genes (with ≥1 mapped SNP) are written.
pub fn write_annot(annot: &GeneAnnotation, path: &Path) -> Result<()> {
    use std::io::Write;
    let f = std::fs::File::create(path).map_err(MagmaError::Io)?;
    let mut w = std::io::BufWriter::new(f);

    writeln!(w, "# window_up = {}", annot.window_up).map_err(MagmaError::Io)?;
    writeln!(w, "# window_down = {}", annot.window_down).map_err(MagmaError::Io)?;

    for g in &annot.genes {
        if g.snps.is_empty() {
            continue;
        }
        write!(w, "{}\t{}:{}:{}", g.id, g.chr, g.start, g.end).map_err(MagmaError::Io)?;
        for snp in &g.snps {
            write!(w, "\t{}", snp).map_err(MagmaError::Io)?;
        }
        writeln!(w).map_err(MagmaError::Io)?;
    }
    Ok(())
}

/// Parse a chromosome string to an integer.
/// Handles "1"-"22", "X"=23, "Y"=24, "XY"=25, "MT"/"MT_"/"0"=0.
fn parse_chr(s: &str) -> Option<i32> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // Try integer first
    if let Ok(n) = s.parse::<i32>() {
        return Some(n);
    }
    // Try sex chromosomes
    match s.to_uppercase().as_str() {
        "X" => Some(23),
        "Y" => Some(24),
        "XY" => Some(25),
        "MT" | "M" => Some(26),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir() -> &'static str {
        "tests/data"
    }

    #[test]
    fn test_read_gene_loc() {
        let genes = read_gene_loc(&Path::new(test_dir()).join("gene_loc.txt")).unwrap();
        assert_eq!(genes.len(), 20);
        assert_eq!(genes[0].id, "10000");
        assert_eq!(genes[0].chr, 1);
    }

    #[test]
    fn test_annotate_sim() {
        let genes = read_gene_loc(&Path::new(test_dir()).join("gene_loc.txt")).unwrap();
        let snps = read_snp_loc(&Path::new(test_dir()).join("sim_geno.bim")).unwrap();
        let annot = annotate(&genes, &snps, 35000, 35000).unwrap();

        // Should match the golden output: 20 genes with SNPs
        let nonempty = annot.genes.iter().filter(|g| !g.snps.is_empty()).count();
        assert_eq!(nonempty, 20, "all 20 genes should have mapped SNPs");

        // Verify total mapped SNPs matches golden (329 with multiplicity,
        // 274 unique as reported in MAGMA log)
        let total: usize = annot.genes.iter().map(|g| g.snps.len()).sum();
        assert_eq!(total, 329, "should match golden output SNP count");
    }

    #[test]
    fn test_annotate_matches_golden() {
        let genes = read_gene_loc(&Path::new(test_dir()).join("gene_loc.txt")).unwrap();
        let snps = read_snp_loc(&Path::new(test_dir()).join("sim_geno.bim")).unwrap();
        let annot = annotate(&genes, &snps, 35000, 35000).unwrap();

        // Compare first gene with golden annot.genes.annot
        let first = annot.genes.iter().find(|g| !g.snps.is_empty()).unwrap();
        assert_eq!(first.id, "10000");
        assert_eq!(first.snps.len(), 13);
        assert_eq!(first.snps[0], "rs1");
    }

    #[test]
    fn test_parse_chr() {
        assert_eq!(parse_chr("1"), Some(1));
        assert_eq!(parse_chr("22"), Some(22));
        assert_eq!(parse_chr("X"), Some(23));
        assert_eq!(parse_chr("Y"), Some(24));
        assert_eq!(parse_chr("0"), Some(0));
        assert_eq!(parse_chr("abc"), None);
    }
}
