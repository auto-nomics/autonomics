//! Example: estimate SNP-heritability (h²) for BMI GWAS (ieu-b-40).
//!
//! ```sh
//! cargo run --release --example bmi_h2
//! ```

use ldsc::munge::{MungeConfig, write_sumstats_gz};
use ldsc::sumstats::{H2Config, WriteLogger, estimate_h2_from_files, hsq_summary};

fn main() -> ldsc::Result<()> {
    let ldsc_data = "/mnt/disk3/autonomics_tree/port_sldsc/reference/ldsc_data";

    // ── Step 1: Munge ────────────────────────────────────────────────────
    let munge_cfg = MungeConfig {
        sumstats: "/mnt/disk3/test/bmi_sumstats.tsv".into(),
        signed_sumstats: Some(("Z".into(), 0.0)), // Z computed directly from ES/SE
        n: None,                                  // per-SNP SS column present
        n_col: Some("N".into()),
        a1: Some("A1".into()),
        a2: Some("A2".into()),
        frq: Some("FRQ".into()),
        maf_min: 0.01,
        keep_maf: true,
        ..Default::default()
    };

    eprintln!("Munging summary statistics …");
    let munged = ldsc::munge::munge_sumstats(&munge_cfg)?;
    eprintln!("  {} SNPs after munging.", munged.len());

    let munged_path = "/mnt/disk3/test/bmi_munged.sumstats.gz";
    write_sumstats_gz(&munged, munged_path)?;
    eprintln!("  Written to {munged_path}");

    // ── Step 2: h² regression ────────────────────────────────────────────
    let h2_cfg = H2Config {
        sumstats: munged_path.into(),
        ref_ld_chr: Some(format!("{ldsc_data}/LDscore/LDscore.@")),
        w_ld_chr: Some(format!(
            "{ldsc_data}/1000G_Phase3_weights_hm3_no_MHC/weights.hm3_noMHC.@"
        )),
        ..Default::default()
    };

    eprintln!("\nRunning LD Score Regression h² …");
    let mut log = WriteLogger {
        w: std::io::stderr(),
    };
    let hsq = estimate_h2_from_files(&h2_cfg, &mut log)?;

    // ── Print results ─────────────────────────────────────────────────────
    let summary = hsq_summary(&hsq, &["baseL2".to_string()], None, None)?;
    println!("\n{summary}");
    println!(
        "lambda_gc: {:.4}  mean_chi^2: {:.4}",
        hsq.lambda_gc, hsq.mean_chisq
    );
    if let (Some(ratio), Some(ratio_se)) = (hsq.ratio, hsq.ratio_se) {
        println!("Ratio: {:.4} ({:.4})", ratio, ratio_se);
    }

    Ok(())
}
