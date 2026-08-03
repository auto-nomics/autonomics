//! Read-only verification: per-chromosome row counts + grand total for all
//! populations present in the ld_matrix namespace.
use datafusion::arrow::array::Int64Array;
use datalake::Datalake;
use iceberg::{Catalog, NamespaceIdent};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let dl = Datalake::new();
    let catalog = dl.get_catalog().await?;
    let ns = NamespaceIdent::from_vec(vec!["ld_matrix".to_string()])?;

    let mut tables = catalog.list_tables(&ns).await?;
    tables.sort_by(|a, b| a.name.cmp(&b.name));

    // Group table names by population prefix.
    let mut pops: Vec<String> = Vec::new();
    for t in &tables {
        let pop = t.name.split("_chr").next().unwrap_or("").to_string();
        if !pops.contains(&pop) {
            pops.push(pop);
        }
    }

    let ctx = dl.get_ctx().await?;
    let mut grand_total: i64 = 0;
    for pop in &pops {
        let mut pop_total: i64 = 0;
        let mut count = 0u32;
        for chr in 1..=22u32 {
            let fqn = format!("iceberg.ld_matrix.{pop}_chr{chr}");
            let batches = ctx
                .sql(format!("SELECT COUNT(*) FROM {fqn}").as_str())
                .await?
                .collect()
                .await?;
            let n = batches[0]
                .column(0)
                .as_any()
                .downcast_ref::<Int64Array>()
                .unwrap()
                .value(0);
            println!("  {fqn}: {n}");
            pop_total += n;
            count += 1;
        }
        println!("  ── {pop}: {count} chromosomes, {pop_total} rows\n");
        grand_total += pop_total;
    }
    println!(
        "GRAND TOTAL across {} populations: {} rows",
        pops.len(),
        grand_total
    );
    Ok(())
}
