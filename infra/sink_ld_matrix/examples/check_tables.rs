//! Read-only, metadata-only: does each ld_matrix table have committed data?
//! Fast — inspects snapshots, does NOT scan data files.
use datalake::Datalake;
use iceberg::{Catalog, NamespaceIdent};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let dl = Datalake::new();
    let catalog = dl.get_catalog().await?;
    let ns = NamespaceIdent::from_vec(vec!["ld_matrix".to_string()])?;

    let mut tables = catalog.list_tables(&ns).await?;
    tables.sort_by(|a, b| a.name.cmp(&b.name));

    let mut with_data = 0usize;
    let mut empty = 0usize;
    let mut total_records: i64 = 0;

    // Group by population prefix for readable output.
    let mut pops: Vec<String> = Vec::new();
    for t in &tables {
        let pop = t.name.split("_chr").next().unwrap_or("").to_string();
        if !pops.contains(&pop) {
            pops.push(pop);
        }
    }

    for pop in &pops {
        println!("── {pop} ──");
        let mut pop_rows: i64 = 0;
        let mut pop_with_data = 0usize;
        for t in tables.iter().filter(|t| t.name.starts_with(&format!("{pop}_chr"))) {
            let table = catalog.load_table(t).await?;
            let md = table.metadata();
            let n_snaps = md.snapshots().count();
            let rows = md
                .current_snapshot()
                .and_then(|s| s.summary().additional_properties.get("total-records"))
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or(-1);
            let state = if n_snaps > 0 {
                with_data += 1;
                pop_with_data += 1;
                if rows > 0 {
                    total_records += rows;
                    pop_rows += rows;
                }
                "HAS DATA"
            } else {
                empty += 1;
                "EMPTY"
            };
            println!(
                "  ld_matrix.{:<11} snapshots={n_snaps}  rows={rows:>12}  {state}",
                t.name
            );
        }
        println!("  {pop}: {pop_with_data} tables with data, {pop_rows} rows\n");
    }

    println!(
        "{} tables total: {with_data} with data, {empty} empty | TOTAL rows = {total_records}",
        tables.len()
    );
    Ok(())
}
