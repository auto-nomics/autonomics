//! One-shot inspection of the intercepted upstream sumstats table
//! `iceberg.gwas.ieu_a_2_chr22` — the exact data the TUI pipeline fed into
//! `univariate_mixer`. Prints schema, sample rows, and the overlap with
//! `af.eur_af`, so we can see what columns/rsid format the node really gets.
//!
//! ```sh
//! cargo test -p data-engine --test inspect_intercepted_sumstats -- --ignored --nocapture
//! ```

use datalake::Datalake;

#[tokio::test]
#[ignore]
async fn inspect_intercepted_sumstats() {
    let ctx = Datalake::new()
        .get_ctx()
        .await
        .expect("无法连 Iceberg 数据湖");

    // 1. Schema of the intercepted table.
    let tbl = ctx
        .sql("DESCRIBE iceberg.gwas.ieu_a_2_chr22")
        .await
        .expect("DESCRIBE");
    println!("=== DESCRIBE iceberg.gwas.ieu_a_2_chr22 ===");
    tbl.show().await.unwrap();

    // 2. Row count + sample rows.
    let n: i64 = ctx
        .sql("SELECT COUNT(*) AS n FROM iceberg.gwas.ieu_a_2_chr22")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap()[0]
        .column(0)
        .as_any()
        .downcast_ref::<arrow::array::Int64Array>()
        .unwrap()
        .value(0);
    println!("=== row count: {n} ===");

    let sample = ctx
        .sql("SELECT * FROM iceberg.gwas.ieu_a_2_chr22 LIMIT 5")
        .await
        .expect("sample");
    println!("=== sample 5 rows ===");
    sample.show().await.unwrap();

    // 3. Does it carry a `chrom` column? List column names explicitly.
    let cols: Vec<String> = ctx
        .table("iceberg.gwas.ieu_a_2_chr22")
        .await
        .expect("table")
        .schema()
        .fields()
        .iter()
        .map(|f| f.name().clone())
        .collect();
    println!("=== columns: {cols:?} ===");
    println!("has chrom column: {}", cols.iter().any(|c| c == "chrom"));
    println!("has rsid column:  {}", cols.iter().any(|c| c == "rsid"));

    // 4. Overlap with af.eur_af chr22, joining on whatever the rsid column is.
    //    Try `rsid` first; this is the exact join the node performs.
    if cols.iter().any(|c| c == "rsid") {
        let overlap = ctx
            .sql(
                r#"SELECT COUNT(*) AS n
                   FROM iceberg.af.eur_af AS a
                   INNER JOIN iceberg.gwas.ieu_a_2_chr22 AS s ON a.id = s.rsid
                   WHERE a.chrom = 22"#,
            )
            .await
            .unwrap()
            .collect()
            .await
            .unwrap()[0]
            .column(0)
            .as_any()
            .downcast_ref::<arrow::array::Int64Array>()
            .unwrap()
            .value(0);
        println!("=== overlap (af.chr22 ∩ intercepted on rsid): {overlap} ===");

        // A few matched rsids to eyeball the format.
        let matched = ctx
            .sql(
                r#"SELECT a.id AS af_id, s.rsid AS ss_rsid
                   FROM iceberg.af.eur_af AS a
                   INNER JOIN iceberg.gwas.ieu_a_2_chr22 AS s ON a.id = s.rsid
                   WHERE a.chrom = 22
                   LIMIT 5"#,
            )
            .await
            .unwrap();
        println!("=== matched rsids sample ===");
        matched.show().await.unwrap();
    }
}
