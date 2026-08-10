//! Sink the 1000G LD matrix (zstd-compressed TSV, produced by
//! `infra/thousand_genomes/ld_matrix_unphased_r2.sh`) into Iceberg — one table
//! per chromosome under the `ld_matrix` namespace, named `<pop>_chr<N>`.
//!
//! Each chromosome is written independently, so the whole batch fans out across
//! all CPU cores: up to `available_parallelism()` chromosomes are sunk
//! concurrently, each driving a single-threaded zstd-decode → parquet-encode
//! pipeline.
//!
//! Tables that already have a committed snapshot are skipped (idempotent
//! re-runs). Drop a table manually to force a rewrite.
//!
//! Run:
//!     cargo run -p sink_ld_matrix                    # sink all populations
//!     cargo run -p sink_ld_matrix -- EAS             # sink one population
//!     cargo run -p sink_ld_matrix -- EAS AFR SAS     # sink specific populations

use std::sync::Arc;

use datafusion::arrow::datatypes::{DataType, Field, Schema};
use datafusion::catalog::CatalogProvider;
use datafusion::datasource::file_format::file_compression_type::FileCompressionType;
use datafusion::prelude::{CsvReadOptions, SessionContext};
use datalake::Datalake;
use iceberg::arrow::arrow_schema_to_schema_auto_assign_ids;
use iceberg::{Catalog, NamespaceIdent, TableCreation, TableIdent};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

const LD_NAMESPACE: &str = "ld_matrix";
/// Data root for source LD matrix TSV files.
/// Overridable via `SINK_LD_MATRIX_DATA_ROOT` env var or the resource catalog
/// (`sink.ld_matrix.data_root`) when running inside the autonomics runtime.
const DEFAULT_DATA_ROOT: &str = "/mnt/disk2/dataset/1000g_plink/";

fn data_root() -> String {
    if let Ok(root) = std::env::var("SINK_LD_MATRIX_DATA_ROOT") {
        return root;
    }
    // Try resource catalog global (set by SharedInfra::open when in-runtime)
    if let Some(cat) = resource_catalog::ResourceCatalog::global() {
        if let Ok(p) = cat.resolve_path("sink.ld_matrix.data_root") {
            return p.to_string_lossy().to_string();
        }
    }
    DEFAULT_DATA_ROOT.to_string()
}

/// All 1000G super-populations.
const ALL_POPS: &[&str] = &["EUR", "EAS", "AFR", "SAS", "AMR"];

/// Target schema, in source-TSV column order.
///
/// Assigned *positionally* via `CsvReadOptions::schema`, which sidesteps
/// DataFusion name resolution: the TSV's first header is `#CHROM_A`, and
/// DataFusion treats `#` as a relation/column qualifier separator, so
/// `with_column_renamed`/`col` on it silently fail. The `#` is also rejected
/// by the Iceberg field-name spec, so we normalize to lowercase here.
/// `pos_a`/`pos_b` are safe — iceberg-rust reserves only the *bare* `pos`.
const TABLE_FIELDS: &[(&str, DataType)] = &[
    ("chrom_a", DataType::Int64),
    ("pos_a", DataType::Int64),
    ("id_a", DataType::Utf8),
    ("chrom_b", DataType::Int64),
    ("pos_b", DataType::Int64),
    ("id_b", DataType::Utf8),
    ("unphased_r2", DataType::Float64),
];

type AnyError = Box<dyn std::error::Error + Send + Sync>;

/// Resolve the LD data directory for a population.
///
/// Prefers the canonical `unphased_r2/<POP>/ld/` layout (produced by
/// `ld_matrix_unphased_r2.sh`). Falls back to the legacy `<pop_lower>/ld/`
/// layout used by the original EUR run.
fn resolve_data_dir(pop: &str) -> String {
    let root = data_root();
    let canonical = format!("{root}unphased_r2/{pop}/ld/");
    if std::path::Path::new(&canonical).read_dir().is_ok() {
        return canonical;
    }
    let legacy = format!("{root}{}/ld/", pop.to_lowercase());
    if std::path::Path::new(&legacy).read_dir().is_ok() {
        return legacy;
    }
    canonical // fall through → discover_chromosomes will report "no files"
}

/// Build the target Arrow schema (column names assigned positionally).
fn target_schema() -> Schema {
    Schema::new(
        TABLE_FIELDS
            .iter()
            .map(|(name, dt)| Field::new(*name, dt.clone(), true))
            .collect::<Vec<_>>(),
    )
}

/// Outcome of sinking one chromosome, for the final summary.
enum SinkResult {
    Written,
    Skipped,
    Failed(String),
}

/// Sink a single chromosome file into `iceberg.ld_matrix.<table>`.
///
/// Skips silently if the table already has committed data. Each call owns its
/// own `SessionContext` so concurrent calls never share planner state; the
/// `Datalake` (catalog) handle is shared via `Arc`.
async fn sink_chromosome(
    datalake: Arc<Datalake>,
    namespace: NamespaceIdent,
    file_path: String,
    table_name: String,
) -> Result<(String, f64), AnyError> {
    let arrow_schema = target_schema();

    // Each chromosome gets its own context; the iceberg catalog is shared.
    let ctx = SessionContext::new();
    let opts = CsvReadOptions::new()
        .has_header(true)
        .schema(&arrow_schema)
        .delimiter(b'\t')
        .file_extension("zst")
        .file_compression_type(FileCompressionType::ZSTD);
    let df = ctx.read_csv(&file_path, opts).await?;

    // Skip only if the table already has committed data. A table can exist
    // but be empty if a prior run was killed mid-INSERT — Iceberg commits a
    // single snapshot only at the *end* of the INSERT, so a killed run leaves
    // an empty table. For those we fall through and populate them.
    let catalog = datalake.get_catalog().await?;
    let table_ident = TableIdent::new(namespace.clone(), table_name.clone());
    if catalog.table_exists(&table_ident).await? {
        let existing = catalog.load_table(&table_ident).await?;
        if existing.metadata().current_snapshot().is_some() {
            return Ok((table_name.clone(), 0.0));
        }
    }

    let started = std::time::Instant::now();

    // Create the table from the target schema.
    let iceberg_schema = arrow_schema_to_schema_auto_assign_ids(&arrow_schema)?;
    let creation = TableCreation::builder()
        .name(table_name.clone())
        .schema(iceberg_schema)
        .build();
    datalake
        .create_table_if_not_exist(&namespace, creation)
        .await?;

    // Refresh the provider so the planner sees the just-created table.
    let fresh_provider = datalake.get_provider().await?;
    ctx.register_catalog(
        "iceberg",
        Arc::new(fresh_provider) as Arc<dyn CatalogProvider>,
    );

    // Register the source as a temp view and INSERT.
    let src_name = format!("__sink_src_{:x}_{table_name}", std::process::id());
    ctx.register_table(&src_name, df.into_view())?;
    let fqn = format!("iceberg.{LD_NAMESPACE}.{table_name}");
    let sql = format!("INSERT INTO {fqn} SELECT * FROM {src_name}");
    ctx.sql(&sql).await?.collect().await?;
    ctx.deregister_table(&src_name)?;

    Ok((table_name, started.elapsed().as_secs_f64()))
}

/// Discover `1000G.<pop>.chr<N>.ld.vcor.zst` files and their table names.
fn discover_chromosomes(dir: &str, pop: &str) -> Vec<(String, String)> {
    let prefix = format!("1000G.{pop}.chr");
    let suffix = ".ld.vcor.zst";
    let mut out: Vec<(u32, String, String)> = std::fs::read_dir(dir)
        .expect("read LD data dir")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter_map(|name| {
            let n_str = name.strip_prefix(&prefix)?.strip_suffix(suffix)?;
            let n: u32 = n_str.parse().ok()?;
            Some((n, name, format!("{}_chr{n}", pop.to_lowercase())))
        })
        .collect();
    out.sort_by_key(|(n, _, _)| *n);
    out.into_iter()
        .map(|(_, name, table)| (format!("{dir}{name}"), table))
        .collect()
}

/// Sink all chromosomes for one population.
async fn sink_population(
    datalake: Arc<Datalake>,
    namespace: NamespaceIdent,
    pop: &str,
) -> Result<(), AnyError> {
    let data_dir = resolve_data_dir(pop);
    let chromosomes = discover_chromosomes(&data_dir, pop);

    if chromosomes.is_empty() {
        eprintln!("[WARN] no .zst files for {pop} in {data_dir}");
        return Ok(());
    }

    // Concurrency: default to one writer per core.
    let parallelism = std::env::var("SINK_CONCURRENCY")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
        });
    let sem = Arc::new(Semaphore::new(parallelism));

    println!(
        "[{pop}] sinking {} chromosomes from {data_dir} (concurrency={parallelism})",
        chromosomes.len()
    );

    let mut tasks: JoinSet<SinkResult> = JoinSet::new();
    for (file_path, table_name) in chromosomes {
        let datalake = datalake.clone();
        let namespace = namespace.clone();
        let sem = sem.clone();
        let pop_label = pop.to_string();
        tasks.spawn(async move {
            let _permit = match sem.acquire_owned().await {
                Ok(p) => p,
                Err(e) => {
                    return SinkResult::Failed(format!("{table_name}: semaphore closed: {e}"));
                }
            };
            println!("[start] {pop_label}/{table_name}");
            match sink_chromosome(datalake, namespace, file_path, table_name.clone()).await {
                Ok((table, 0.0)) => {
                    println!("[skip ] {pop_label}/{table} (exists)");
                    SinkResult::Skipped
                }
                Ok((table, secs)) => {
                    println!("[done ] {pop_label}/{table} in {secs:.1}s");
                    SinkResult::Written
                }
                Err(e) => {
                    let msg = format!("{e}");
                    eprintln!("[FAIL ] {pop_label}/{table_name}: {msg}");
                    SinkResult::Failed(format!("{pop_label}/{table_name}: {msg}"))
                }
            }
        });
    }

    let mut written = 0usize;
    let mut skipped = 0usize;
    let mut failed = Vec::new();
    while let Some(res) = tasks.join_next().await {
        match res.expect("task panicked") {
            SinkResult::Written => written += 1,
            SinkResult::Skipped => skipped += 1,
            SinkResult::Failed(msg) => failed.push(msg),
        }
    }

    println!(
        "[{pop}] summary: {written} written, {skipped} skipped, {} failed",
        failed.len()
    );
    for msg in &failed {
        println!("  - {msg}");
    }
    if !failed.is_empty() {
        return Err(format!("{pop}: {} chromosome(s) failed", failed.len()).into());
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), AnyError> {
    let pops: Vec<String> = std::env::args()
        .skip(1) // skip program name
        .map(|s| s.to_uppercase())
        .collect();
    let pops: Vec<&str> = if pops.is_empty() {
        ALL_POPS.to_vec()
    } else {
        pops.iter().map(|s| s.as_str()).collect()
    };

    let datalake = Arc::new(Datalake::new());
    let namespace = NamespaceIdent::from_vec(vec![LD_NAMESPACE.to_string()])?;

    let mut errors = Vec::new();
    for pop in &pops {
        if let Err(e) = sink_population(datalake.clone(), namespace.clone(), pop).await {
            errors.push(format!("{e}"));
        }
    }

    if !errors.is_empty() {
        eprintln!("\n{} population(s) had failures:", errors.len());
        for e in &errors {
            eprintln!("  - {e}");
        }
        return Err(format!("{} population(s) failed", errors.len()).into());
    }
    println!("\nAll done: {} population(s) processed", pops.len());
    Ok(())
}

// #[tokio::test]
// async fn test_iceberg() {
//     let ctx = datalake::Datalake::default().get_ctx().await.unwrap();
//     // let df = ctx
//     //     .sql("SELECT * FROM \"iceberg.eqtl.gene_filtered\".ACE")
//     //     .await
//     //     .unwrap();
//     // df.show_limit(10).await.unwrap();
//     let catalog = datalake::Datalake::default().get_catalog().await.unwrap();
//     let df = catalog
//         .load_table(&TableIdent::from_strs(vec!["eqtl", "gene_filtered", "ACE"]).unwrap())
//         .await
//         .unwrap();
// }
