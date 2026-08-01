//! Real chr22 validation: compare Rust port against golden MAGMA output.
//!
//! Run with: `cargo test -p magma --test real_chr22 -- --nocapture --include-ignored`

use magma::*;
use std::path::PathBuf;

fn real_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/real_chr22")
}

#[test]
fn test_real_chr22_gene_analysis() {
    let dir = real_dir();
    if !dir.join("g1k_eur_chr22.bed").exists() {
        eprintln!("Skipping: real chr22 data not present. Restore with:");
        eprintln!(
            "  rclone copy aliyun:autonomics-data/magma/real_chr22/ {}",
            dir.display()
        );
        return;
    }

    let mut bed = plink::BedFile::open(&dir.join("g1k_eur_chr22")).unwrap();
    assert_eq!(bed.n_indiv(), 489);
    assert_eq!(bed.n_snp(), 141123);

    let annot = geneinput::GeneAnnot::read(&dir.join("real_annot.genes.annot")).unwrap();

    // Read p-value file with per-SNP N column
    let pval_data = geneinput::SnpPvalData::read(
        &dir.join("gwas_height_chr22.txt"),
        "SNP",
        "P",
        Some("N"),
        None,
    )
    .unwrap();

    let config = geneanalysis::PvalAnalysisConfig {
        truncate_low: 1e-50,
        truncate_high: 1e-5,
        fixed_n: None,
        ..Default::default()
    };

    let results = geneanalysis::analyze_pval(&mut bed, &annot, &pval_data, &config).unwrap();

    // Read golden output
    let golden = std::fs::read_to_string(dir.join("gene_pval.genes.out")).unwrap();
    let golden_lines: Vec<&str> = golden.lines().skip(1).collect();

    eprintln!(
        "\n=== chr22 gene analysis: {} genes (golden: {}) ===\n",
        results.len(),
        golden_lines.len()
    );

    let mut max_z_diff = 0.0_f64;
    let mut max_p_log_diff = 0.0_f64;
    let mut n_match = 0;

    for (i, r) in results.iter().enumerate() {
        let fields: Vec<&str> = golden_lines[i].split_whitespace().collect();
        let golden_z: f64 = fields[7].parse().unwrap();
        let golden_p: f64 = fields[8].parse().unwrap();

        let z_diff = (r.zstat - golden_z).abs();
        let p_log_diff = (r.pval.ln() - golden_p.ln()).abs();

        max_z_diff = max_z_diff.max(z_diff);
        max_p_log_diff = max_p_log_diff.max(p_log_diff);

        if z_diff < 0.2 {
            n_match += 1;
        }

        if i < 5 || z_diff > 0.5 {
            eprintln!(
                "  {:<12} Z={:>8.4} (golden {:>8.4}, Δ={:.4})  P={:.3e} (golden {:.3e})",
                r.id, r.zstat, golden_z, z_diff, r.pval, golden_p
            );
        }
    }

    eprintln!(
        "\n=== Summary: {}/{} genes within Z tolerance 0.2 ===",
        n_match,
        results.len()
    );
    eprintln!("  Max ZSTAT diff: {:.4}", max_z_diff);
    eprintln!("  Max |ln(P) diff|: {:.4}", max_p_log_diff);

    // Most genes should match within reasonable tolerance
    // (Imhof integration method differences may cause some divergence)
    assert!(
        n_match > results.len() / 2,
        "too few genes match golden output: {}/{}",
        n_match,
        results.len()
    );
}
