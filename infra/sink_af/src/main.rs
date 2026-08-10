//! Sink the 1000G EUR allele-frequency tables (PLINK2 `.afreq` TSVs) into a
//! single Iceberg table, partitioned by chromosome.
//!
//! Unlike sink_ld_matrix (one table per chromosome), all chromosomes go into
//! one table `iceberg.af.eur_af`, partitioned by `chrom` (identity transform).
//! Each chromosome is appended independently and idempotently: a partition-
//! pruned `COUNT(*) WHERE chrom = N` skips chromosomes whose partition already
//! has rows, so partial re-runs only fill the gaps.
//!
//! Inserts are **sequential** — they all target the same table, so concurrent
//! appends would race on the Iceberg metadata commit. `.afreq` files are small
//! uncompressed TSVs, so this is fast. Drop the table to force a full rewrite.
//!
//! Run:
//!     cargo run -p sink_af

use std::sync::Arc;

use datafusion::arrow::array::Int64Array;
use datafusion::arrow::datatypes::{DataType, Field, Schema};
use datafusion::catalog::CatalogProvider;
use datafusion::prelude::{CsvReadOptions, SessionContext};
use datalake::Datalake;
use iceberg::arrow::arrow_schema_to_schema_auto_assign_ids;
use iceberg::spec::{PartitionSpecBuilder, Transform};
use iceberg::{NamespaceIdent, TableCreation};

const AF_NAMESPACE: &str = "af";
const AF_TABLE: &str = "eur_af";
/// Source directory for 1000G EUR allele-frequency `.afreq` files.
/// Overridable via `SINK_AF_DATA_PATH` env var or the resource catalog
/// (`sink.af.data_path`) when running inside the autonomics runtime.
const DEFAULT_AF_DATA_PATH: &str = "/mnt/disk2/dataset/1000g_plink/eur/maf/";

fn af_data_path() -> String {
    if let Ok(p) = std::env::var("SINK_AF_DATA_PATH") {
        return p;
    }
    if let Some(cat) = resource_catalog::ResourceCatalog::global() {
        if let Ok(p) = cat.resolve_path("sink.af.data_path") {
            return p.to_string_lossy().to_string();
        }
    }
    DEFAULT_AF_DATA_PATH.to_string()
}

/// Target schema, in source-`.afreq` column order.
///
/// Assigned *positionally* via `CsvReadOptions::schema`, which sidesteps
/// DataFusion name resolution: the TSV's first header is `#CHROM`, and
/// DataFusion treats `#` as a relation/column qualifier separator, so
/// `with_column_renamed`/`col` on it silently fail. The `#` and `?` (in
/// `PROVISIONAL_REF?`) are also rejected by the Iceberg field-name spec, so we
/// normalize to lowercase here. The read schema matches the file column-for-
/// column, so the INSERT can use `SELECT *` (no name resolution involved).
const TABLE_FIELDS: &[(&str, DataType)] = &[
    ("chrom", DataType::Int64),
    ("id", DataType::Utf8),
    ("ref", DataType::Utf8),
    ("alt", DataType::Utf8),
    ("provisional_ref", DataType::Utf8),
    ("alt_freq", DataType::Float64),
    ("obs_ct", DataType::Int64),
];

type AnyError = Box<dyn std::error::Error + Send + Sync>;

/// Build the target Arrow schema (column names assigned positionally).
fn target_schema() -> Schema {
    Schema::new(
        TABLE_FIELDS
            .iter()
            .map(|(name, dt)| Field::new(*name, dt.clone(), true))
            .collect::<Vec<_>>(),
    )
}

/// Discover `1000G.EUR.chr<N>.qc.afreq` files, sorted by chromosome.
fn discover_chromosomes(dir: &str) -> Vec<(u32, String)> {
    let prefix = "1000G.EUR.chr";
    let suffix = ".qc.afreq";
    let mut out: Vec<(u32, String)> = std::fs::read_dir(dir)
        .expect("read afreq data dir")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter_map(|name| {
            let n_str = name.strip_prefix(prefix)?.strip_suffix(suffix)?;
            let n: u32 = n_str.parse().ok()?;
            Some((n, format!("{dir}{name}")))
        })
        .collect();
    out.sort_by_key(|(n, _)| *n);
    out
}

/// Append one chromosome's `.afreq` into the shared table.
///
/// Skips if the chromosome's partition already has rows — the `WHERE chrom = N`
/// predicate is partition-pruned, so the COUNT only touches that one
/// partition's files (cheap). Each call owns its own `SessionContext`; the
/// `Datalake` (catalog) handle is shared via `Arc`.
async fn sink_chromosome(
    datalake: Arc<Datalake>,
    fqn: String,
    chrom: u32,
    file_path: String,
) -> Result<f64, AnyError> {
    let arrow_schema = target_schema();
    let ctx = SessionContext::new();

    // Fresh provider so the planner sees the table created in main().
    let provider = datalake.get_provider().await?;
    ctx.register_catalog("iceberg", Arc::new(provider) as Arc<dyn CatalogProvider>);

    // Idempotent: skip if this chrom partition already has data (partition-pruned).
    let existing: i64 = ctx
        .sql(&format!("SELECT COUNT(*) FROM {fqn} WHERE chrom = {chrom}"))
        .await?
        .collect()
        .await?
        .into_iter()
        .next()
        .and_then(|b| {
            b.column(0)
                .as_any()
                .downcast_ref::<Int64Array>()
                .map(|a| a.value(0))
        })
        .unwrap_or(0);
    if existing > 0 {
        return Ok(0.0);
    }

    let started = std::time::Instant::now();

    // Read the .afreq TSV with the positional schema (no compression).
    // `.file_extension` is required: DataFusion's CSV reader rejects any file
    // whose suffix isn't the configured extension (default ".csv").
    let opts = CsvReadOptions::new()
        .has_header(true)
        .schema(&arrow_schema)
        .delimiter(b'\t')
        .file_extension("afreq");
    let df = ctx.read_csv(&file_path, opts).await?;

    // Register the source as a temp view and INSERT. Schema matches the file
    // column-for-column, so SELECT * needs no name resolution.
    let src_name = format!("__sink_src_{chrom}_{AF_TABLE}");
    ctx.register_table(&src_name, df.into_view())?;
    let sql = format!("INSERT INTO {fqn} SELECT * FROM {src_name}");
    ctx.sql(&sql).await?.collect().await?;
    ctx.deregister_table(&src_name)?;

    Ok(started.elapsed().as_secs_f64())
}

#[tokio::main]
async fn main() -> Result<(), AnyError> {
    let datalake = Arc::new(Datalake::new());
    let namespace = NamespaceIdent::from_vec(vec![AF_NAMESPACE.to_string()])?;

    let data_path = af_data_path();
    let chromosomes = discover_chromosomes(&data_path);
    if chromosomes.is_empty() {
        return Err(format!("no .afreq files found in {data_path}").into());
    }

    // Create the single partitioned table once: partitioned by chrom (identity).
    let arrow_schema = target_schema();
    let iceberg_schema = arrow_schema_to_schema_auto_assign_ids(&arrow_schema)?;
    let partition_spec = PartitionSpecBuilder::new(iceberg_schema.clone())
        .with_spec_id(0)
        .add_partition_field("chrom", "chrom", Transform::Identity)?
        .build()? // PartitionSpec (bound)
        .into_unbound(); // UnboundPartitionSpec, as TableCreation expects
    let creation = TableCreation::builder()
        .name(AF_TABLE.to_string())
        .schema(iceberg_schema)
        .partition_spec(partition_spec)
        .build();
    datalake
        .create_table_if_not_exist(&namespace, creation)
        .await?;
    let fqn = format!("iceberg.{AF_NAMESPACE}.{AF_TABLE}");
    println!("{fqn} ready (partitioned by chrom)");

    // Sequential inserts: one shared table → concurrent appends would race on
    // the Iceberg commit.
    let mut written = 0usize;
    let mut skipped = 0usize;
    let mut failed = Vec::new();
    for (chrom, file_path) in chromosomes {
        print!("chr{chrom}: ");
        use std::io::Write;
        let _ = std::io::stdout().flush();
        match sink_chromosome(datalake.clone(), fqn.clone(), chrom, file_path).await {
            Ok(0.0) => {
                skipped += 1;
                println!("skip (exists)");
            }
            Ok(secs) => {
                written += 1;
                println!("done in {secs:.1}s");
            }
            Err(e) => {
                let msg = format!("chr{chrom}: {e}");
                eprintln!("FAIL {msg}");
                failed.push(msg);
            }
        }
    }

    println!(
        "\nsummary: {written} written, {skipped} skipped, {} failed",
        failed.len()
    );
    for msg in &failed {
        println!("  - {msg}");
    }
    if !failed.is_empty() {
        return Err(format!("{} chromosome(s) failed", failed.len()).into());
    }
    Ok(())
}
