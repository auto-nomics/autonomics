//! Gene annotation (`.genes.annot`) reader.
//!
//! Format:
//! ```text
//! # window_up = 35000
//! # window_down = 35000
//! gene_id  chr:start:end  rsid1  rsid2  ...
//! ```

use std::collections::HashMap;
use std::path::Path;

use crate::error::{MagmaError, Result};

/// A gene definition from the `.genes.annot` file.
#[derive(Debug, Clone)]
pub struct GeneDef {
    pub id: String,
    pub chr: i32,
    pub start: u64,
    pub end: u64,
    pub snps: Vec<String>,
}

/// Parsed `.genes.annot` file: all gene definitions with their mapped SNPs.
#[derive(Debug, Clone)]
pub struct GeneAnnot {
    pub genes: Vec<GeneDef>,
    pub window_up: i64,
    pub window_down: i64,
}

impl GeneAnnot {
    /// Read a `.genes.annot` file.
    pub fn read(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path).map_err(MagmaError::Io)?;
        let mut window_up = 0i64;
        let mut window_down = 0i64;
        let mut genes = Vec::new();

        for (lineno, line) in content.lines().enumerate() {
            if line.starts_with('#') {
                // Parse header parameters
                let parts: Vec<&str> = line.split('=').collect();
                if parts.len() == 2 {
                    let key = parts[0].trim().trim_start_matches('#').trim();
                    let val = parts[1].trim();
                    match key {
                        "window_up" => window_up = val.parse().unwrap_or(0),
                        "window_down" => window_down = val.parse().unwrap_or(0),
                        _ => {}
                    }
                }
                continue;
            }

            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() < 2 {
                if fields.is_empty() {
                    continue;
                }
                return Err(MagmaError::Input(format!(
                    "{path:?}: line {}: expected at least 2 fields (gene_id location [snps...])",
                    lineno + 1
                )));
            }

            let id = fields[0].to_string();
            // Parse location "chr:start:end"
            let (chr, start, end) = parse_location(fields[1]).ok_or_else(|| {
                MagmaError::Input(format!(
                    "{path:?}: line {}: cannot parse location '{}'",
                    lineno + 1,
                    fields[1]
                ))
            })?;

            let snps: Vec<String> = fields[2..].iter().map(|s| s.to_string()).collect();
            genes.push(GeneDef {
                id,
                chr,
                start,
                end,
                snps,
            });
        }

        if genes.is_empty() {
            return Err(MagmaError::Input(format!(
                "{path:?}: no gene definitions found"
            )));
        }

        Ok(GeneAnnot {
            genes,
            window_up,
            window_down,
        })
    }

    /// Build a map from rsid → index into the PLINK .bim for SNP lookup.
    pub fn snp_set(&self) -> HashMap<String, ()> {
        let mut set = HashMap::new();
        for g in &self.genes {
            for s in &g.snps {
                set.insert(s.clone(), ());
            }
        }
        set
    }
}

/// Parse "chr:start:end" into (chr, start, end).
fn parse_location(s: &str) -> Option<(i32, u64, u64)> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 3 {
        return None;
    }
    let chr: i32 = parts[0].parse().ok()?;
    let start: u64 = parts[1].parse().ok()?;
    let end: u64 = parts[2].parse().ok()?;
    Some((chr, start, end))
}

/// Read SNP p-values from a file.
///
/// Expected format (header required):
/// ```text
/// SNP  P  N
/// rs1  0.05  50000
/// ```
/// The `snp_col` and `pval_col` parameters name the SNP ID and p-value columns.
/// The `n_col` optionally names a per-SNP sample size column.
pub struct SnpPvalData {
    /// Map rsid → (p-value, optional N).
    pub snps: HashMap<String, (f64, Option<i64>)>,
}

impl SnpPvalData {
    /// Read a SNP p-value file.
    ///
    /// - `snp_col`: column name for SNP ID
    /// - `pval_col`: column name for p-value
    /// - `n_col`: optional column name for per-SNP sample size
    /// - `fixed_n`: optional fixed sample size for all SNPs
    pub fn read(
        path: &Path,
        snp_col: &str,
        pval_col: &str,
        n_col: Option<&str>,
        fixed_n: Option<i64>,
    ) -> Result<Self> {
        let content = std::fs::read_to_string(path).map_err(MagmaError::Io)?;
        let mut lines = content.lines();

        let header = lines
            .next()
            .ok_or_else(|| MagmaError::Input(format!("{path:?}: file is empty")))?;
        let headers: Vec<&str> = header.split_whitespace().collect();

        let snp_idx = headers.iter().position(|&h| h == snp_col).ok_or_else(|| {
            MagmaError::Input(format!(
                "{path:?}: SNP column '{snp_col}' not found in header: {header}"
            ))
        })?;
        let pval_idx = headers.iter().position(|&h| h == pval_col).ok_or_else(|| {
            MagmaError::Input(format!(
                "{path:?}: p-value column '{pval_col}' not found in header: {header}"
            ))
        })?;
        let n_idx = n_col.and_then(|nc| headers.iter().position(|&h| h == nc));

        let mut snps = HashMap::new();
        for (_lineno, line) in lines.enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() <= snp_idx.max(pval_idx) {
                continue; // skip malformed lines silently (MAGMA warns)
            }
            let rsid = fields[snp_idx].to_string();
            let pval: f64 = match fields[pval_idx].parse() {
                Ok(v) => v,
                Err(_) => continue,
            };
            let n = if let Some(ni) = n_idx {
                fields.get(ni).and_then(|s| s.parse().ok())
            } else {
                fixed_n
            };
            snps.insert(rsid, (pval, n));
        }

        Ok(SnpPvalData { snps })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_annot() {
        let annot = GeneAnnot::read(std::path::Path::new("tests/data/annot.genes.annot")).unwrap();
        assert_eq!(annot.window_up, 35000);
        assert_eq!(annot.window_down, 35000);
        assert_eq!(annot.genes.len(), 20);

        let g0 = &annot.genes[0];
        assert_eq!(g0.id, "10000");
        assert_eq!(g0.chr, 1);
        assert_eq!(g0.snps.len(), 13);
        assert_eq!(g0.snps[0], "rs1");
    }

    #[test]
    fn test_read_pval_file() {
        let data = SnpPvalData::read(
            std::path::Path::new("tests/data/gwas_pval.txt"),
            "SNP",
            "P",
            Some("N"),
            None,
        )
        .unwrap();
        assert_eq!(data.snps.len(), 500);
        let (p, n) = data.snps.get("rs1").unwrap();
        assert!(*p > 0.0 && *p <= 1.0);
        assert_eq!(*n, Some(50000));
    }
}
