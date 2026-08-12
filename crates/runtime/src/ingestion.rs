//! Ingestion executor: reads `IngestionSpec` from the resource catalog and
//! executes the source-files → Iceberg-table pipeline.
//!
//! This is the execution counterpart to [`resource_catalog::IngestionSpec`].
//! The catalog declares *what* to ingest; this module does it, using
//! `Datalake` (Iceberg REST) + `DataFusion` (file reading + SQL INSERT).
//!
//! ## Lifecycle
//!
//! ```text
//! [archive] → restore_before → read source → create/append Iceberg → archive_after
//! ```

use std::path::PathBuf;
use std::sync::Arc;

use dag_core::resource_catalog::{
    IngestionOutcome, IngestionSpec, ResourceCatalog, ResourceKind, SourceFormat, WriteMode,
};
use datafusion::prelude::{CsvReadOptions, ParquetReadOptions, SessionContext};
use datalake::Datalake;
use futures::StreamExt;
use iceberg::arrow::arrow_schema_to_schema_auto_assign_ids;
use iceberg::spec::{
    DataFileFormat, Literal, PartitionKey, PartitionSpecBuilder, Struct,
    TableProperties, Transform,
};
use iceberg::transaction::{Transaction, ApplyTransactionAction};
use iceberg::writer::base_writer::data_file_writer::DataFileWriterBuilder;
use iceberg::writer::file_writer::{ParquetWriterBuilder, rolling_writer::RollingFileWriterBuilder};
use iceberg::writer::file_writer::location_generator::{DefaultLocationGenerator, DefaultFileNameGenerator};
use iceberg::writer::{IcebergWriter, IcebergWriterBuilder};
use iceberg::{Catalog, NamespaceIdent, TableCreation, TableIdent};

use datafusion::arrow::datatypes::{DataType, Field, Schema};

/// Executes ingestion jobs declared on `ResourceEntry`s.
pub struct IngestionExecutor {
    catalog: Arc<ResourceCatalog>,
    datalake: Arc<Datalake>,
}

/// Iceberg box errors → anyhow.
fn ice<T, E: std::fmt::Display>(r: std::result::Result<T, E>) -> anyhow::Result<T> {
    r.map_err(|e| anyhow::anyhow!("{e}"))
}

impl IngestionExecutor {
    pub fn new(catalog: Arc<ResourceCatalog>, datalake: Arc<Datalake>) -> Self {
        Self { catalog, datalake }
    }

    /// Ingest data into the Iceberg table identified by `resource_name`.
    ///
    /// Uses iceberg-rust's **direct writer API** (DataFileWriter + RollingFileWriter)
    /// instead of DataFusion's INSERT INTO.  DataFusion reads the source parquet
    /// streaming; each batch is handed to the iceberg writer which accumulates
    /// 512 MB data files before flushing to S3.  This avoids the OOM caused by
    /// iceberg-datafusion's TableSink buffering all data in memory.
    pub async fn ingest(&self, resource_name: &str) -> anyhow::Result<IngestionOutcome> {
        let entry = self.catalog.get(resource_name)
            .ok_or_else(|| anyhow::anyhow!("unknown resource '{resource_name}'"))?;

        if entry.kind != ResourceKind::IcebergTable {
            anyhow::bail!("resource '{resource_name}' is {:?}, not IcebergTable", entry.kind);
        }

        let spec = entry.ingestion_spec.as_ref()
            .ok_or_else(|| anyhow::anyhow!("resource '{resource_name}' has no ingestion_spec"))?;

        let start = std::time::Instant::now();

        // ── Optional pre-ingest restore ───────────────────────────────
        if spec.restore_before
            && let Some(src_name) = &spec.source_resource
        {
            tracing::info!("restoring source '{src_name}' from archive before ingest");
            self.catalog.restore(src_name).await
                .map_err(|e| anyhow::anyhow!("restore failed: {e}"))?;
        }

        // ── Resolve target table ident ────────────────────────────────
        let ident = self.catalog.resolve_iceberg(resource_name)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let (_catalog_name, schema_name, table_name) = ident.ident();
        let namespace = NamespaceIdent::from_vec(vec![schema_name])?;

        // ── Expand source files ───────────────────────────────────────
        let source_files = expand_source_files(&spec.source_path);
        let files_processed = source_files.len() as u64;

        if source_files.is_empty() {
            anyhow::bail!("no source files found at '{}'", spec.source_path);
        }

        tracing::info!(
            "ingesting {} file(s) from {} via direct writer",
            source_files.len(),
            spec.source_path
        );

        // ── Read first file to get schema ─────────────────────────────
        let read_ctx = ice(self.datalake.get_ctx().await)?;
        let first_spec = IngestionSpec {
            source_path: source_files[0].clone(),
            ..spec.clone()
        };
        let (first_df, _) = self.read_source(&read_ctx, &first_spec).await?;
        let arrow_schema = first_df.schema().inner().clone();
        drop(first_df);
        drop(read_ctx);

        // ── Create / drop table ───────────────────────────────────────
        let rest_catalog = ice(self.datalake.get_catalog().await)?;
        let table_ident = TableIdent::new(namespace.clone(), table_name.clone());

        match spec.mode {
            WriteMode::CreateOrReplace => {
                if ice(rest_catalog.table_exists(&table_ident).await)? {
                    ice(rest_catalog.drop_table(&table_ident).await)?;
                    tracing::info!("dropped existing table for replacement");
                }
                self.create_table(&namespace, &table_name, &arrow_schema, &spec.partition_by).await?;
            }
            WriteMode::CreateIfNotExists => {
                self.create_table(&namespace, &table_name, &arrow_schema, &spec.partition_by).await?;
            }
            WriteMode::Append => {
                if !ice(rest_catalog.table_exists(&table_ident).await)? {
                    anyhow::bail!("Append mode requires table to already exist");
                }
            }
        }

        // ── Check if already has data (CreateIfNotExists) ─────────────
        if spec.mode == WriteMode::CreateIfNotExists {
            let fqn = ident.sql();
            let ctx = ice(self.datalake.get_ctx_with_partitions(1).await)?;
            if let Ok(df) = ctx.sql(&format!("SELECT COUNT(*) AS n FROM {fqn}")).await {
                if let Ok(batches) = df.collect().await {
                    if let Some(count) = batches.first()
                        .and_then(|b| b.column_by_name("n"))
                        .and_then(|c| c.as_any().downcast_ref::<datafusion::arrow::array::Int64Array>())
                        .map(|a| a.value(0))
                        .filter(|&n| n > 0)
                    {
                        tracing::info!("table {fqn} already has {count} rows — skipping");
                        return Ok(IngestionOutcome {
                            rows_written: 0, files_processed: 0,
                            duration_ms: start.elapsed().as_millis(), skipped: true,
                        });
                    }
                }
            }
        }

        // ── Load table for direct writing ─────────────────────────────
        let table = ice(rest_catalog.load_table(&table_ident).await)?;
        let file_io = table.file_io().clone();
        let table_props = ice(table.metadata().table_properties())?;
        let iceberg_schema = table.metadata().current_schema().clone();
        let target_file_size = table_props.write_target_file_size_bytes;

        tracing::info!(
            "table loaded, target_file_size = {} MB",
            target_file_size / 1024 / 1024
        );

        let mut total_rows: u64 = 0;

        for (i, file_path) in source_files.iter().enumerate() {
            let file_start = std::time::Instant::now();

            // ── Build writer chain ────────────────────────────────────
            let parquet_builder = ParquetWriterBuilder::from_table_properties(
                &table_props,
                iceberg_schema.clone(),
            )
            .with_match_mode(iceberg::arrow::FieldMatchMode::Name);
            let location_gen = ice(DefaultLocationGenerator::new(table.metadata()))?;
            let file_name_gen = DefaultFileNameGenerator::new(
                uuid::Uuid::new_v4().to_string(),
                None,
                DataFileFormat::Parquet,
            );
            let rolling_builder = RollingFileWriterBuilder::new(
                parquet_builder,
                target_file_size,
                file_io.clone(),
                location_gen,
                file_name_gen,
            );
            let data_file_builder = DataFileWriterBuilder::new(rolling_builder);

            // ── Read source to get partition value ────────────────────
            let ctx = ice(self.datalake.get_ctx_with_partitions(1).await)?;
            let file_spec = IngestionSpec {
                source_path: file_path.clone(),
                ..spec.clone()
            };
            let (df, _) = self.read_source(&ctx, &file_spec).await?;
            let mut stream = df.execute_stream().await?;

            let first_batch = stream.next().await
                .ok_or_else(|| anyhow::anyhow!("empty source: {file_path}"))??;

            // Extract partition key from first batch
            let p_spec = table.metadata().default_partition_spec().clone();
            let partition_key = if p_spec.is_unpartitioned() {
                None
            } else {
                let mut vals: Vec<Option<Literal>> = Vec::new();
                for pf in p_spec.fields() {
                    // Look up field name by source_id from iceberg schema
                    let pf_schema = iceberg_schema.as_ref();
                    let src_field = pf_schema.field_by_id(pf.source_id)
                        .ok_or_else(|| anyhow::anyhow!("partition source_id {} not in schema", pf.source_id))?;
                    let idx = arrow_schema.fields().iter()
                        .position(|f| f.name() == &src_field.name)
                        .ok_or_else(|| anyhow::anyhow!("partition col '{}' missing in arrow", src_field.name))?;
                    let col = first_batch.column(idx);
                    use datafusion::arrow::array::*;
                    let lit = match col.data_type() {
                        datafusion::arrow::datatypes::DataType::Int64 =>
                            Literal::long(col.as_any().downcast_ref::<Int64Array>().unwrap().value(0)),
                        datafusion::arrow::datatypes::DataType::Int32 =>
                            Literal::int(col.as_any().downcast_ref::<Int32Array>().unwrap().value(0)),
                        datafusion::arrow::datatypes::DataType::Utf8 =>
                            Literal::string(col.as_any().downcast_ref::<StringArray>().unwrap().value(0)),
                        dt => anyhow::bail!("unsupported partition type: {dt:?}"),
                    };
                    vals.push(Some(lit));
                }
                Some(PartitionKey::new(
                    p_spec.as_ref().clone(),
                    iceberg_schema.clone(),
                    Struct::from_iter(vals),
                ))
            };

            // Build writer with partition key
            let mut writer = ice(data_file_builder.build(partition_key).await)?;

            // Write first batch + remaining
            let mut file_rows: u64 = first_batch.num_rows() as u64;
            writer.write(first_batch).await
                .map_err(|e| anyhow::anyhow!("writer error: {e}"))?;
            let mut batch_count = 1u32;
            while let Some(result) = stream.next().await {
                let batch = result?;
                file_rows += batch.num_rows() as u64;
                writer.write(batch).await
                    .map_err(|e| anyhow::anyhow!("writer error: {e}"))?;
                batch_count += 1;
            }
            drop(stream);
            drop(ctx);


            // ── Close writer → collect data files ─────────────────────
            let data_files = writer.close().await
                .map_err(|e| anyhow::anyhow!("writer close error: {e}"))?;

            total_rows += file_rows;

            tracing::info!(
                "[{}/{}] {} — {file_rows} rows, {} data files, {} batches ({}ms, total {total_rows})",
                i + 1, source_files.len(),
                std::path::Path::new(file_path).file_name().unwrap_or_default().to_string_lossy(),
                data_files.len(),
                batch_count,
                file_start.elapsed().as_millis(),
            );

            // ── Commit via Transaction::fast_append ───────────────────
            let tx = Transaction::new(&table);
            let action = tx.fast_append().add_data_files(data_files);
            action
                .apply(tx)
                .map_err(|e| anyhow::anyhow!("apply error: {e}"))?
                .commit(rest_catalog.as_ref())
                .await
                .map_err(|e| anyhow::anyhow!("commit error: {e}"))?;
        }

        let fqn = ident.sql();
        tracing::info!(
            "ingested {total_rows} rows into {fqn} from {} ({files_processed} files, {}ms)",
            spec.source_path, start.elapsed().as_millis()
        );

        // ── Optional post-ingest archive ──────────────────────────────
        if spec.archive_after
            && let Some(src_name) = &spec.source_resource
        {
            tracing::info!("archiving source '{src_name}' after successful ingest");
            self.catalog.archive(src_name).await
                .map_err(|e| anyhow::anyhow!("archive failed: {e}"))?;
        }

        Ok(IngestionOutcome {
            rows_written: total_rows,
            files_processed,
            duration_ms: start.elapsed().as_millis(),
            skipped: false,
        })
    }

    /// Read source files into a DataFrame based on the format.
    ///
    /// For parquet sources with Utf8View columns (DataFusion ≥42 default), we
    /// re-read with an explicit Utf8 schema so the Arrow decoder produces Utf8
    /// arrays directly — no temp file, no `collect()`, fully streaming.
    async fn read_source(
        &self,
        ctx: &SessionContext,
        spec: &IngestionSpec,
    ) -> anyhow::Result<(datafusion::dataframe::DataFrame, Option<PathBuf>)> {
        let df = match spec.source_format {
            SourceFormat::Parquet => {
                // First read infers the schema (reads parquet footer only, no data).
                let df = ctx
                    .read_parquet(&spec.source_path, ParquetReadOptions::default())
                    .await?;

                // If any string columns came back as Utf8View, re-read with an
                // explicit Utf8 schema so iceberg-rust can consume the batches
                // without any cast or materialisation.
                let has_utf8view = df
                    .schema()
                    .fields()
                    .iter()
                    .any(|f| f.data_type() == &DataType::Utf8View);

                if has_utf8view {
                    let orig = df.schema().inner();
                    let utf8_schema = Schema::new(
                        orig.fields()
                            .iter()
                            .map(|f| {
                                if f.data_type() == &DataType::Utf8View {
                                    Field::new(f.name(), DataType::Utf8, f.is_nullable())
                                } else {
                                    f.as_ref().clone()
                                }
                            })
                            .collect::<Vec<_>>(),
                    );
                    tracing::debug!("re-reading parquet with Utf8 schema (avoiding Utf8View)");
                    ctx.read_parquet(
                        &spec.source_path,
                        ParquetReadOptions {
                            schema: Some(&utf8_schema),
                            ..Default::default()
                        },
                    )
                    .await?
                } else {
                    df
                }
            }
            SourceFormat::Csv | SourceFormat::Tsv => {
                let csv_opts = spec.csv_options.clone().unwrap_or_default();
                let delimiter = match spec.source_format {
                    SourceFormat::Tsv => b'\t',
                    _ => csv_opts.delimiter as u8,
                };
                let mut opts = CsvReadOptions::new()
                    .has_header(csv_opts.has_header)
                    .delimiter(delimiter);
                if let Some(ext) = &csv_opts.file_extension {
                    opts = opts.file_extension(ext);
                }
                ctx.read_csv(&spec.source_path, opts).await?
            }
        };

        Ok((df, None))
    }

    /// Create an Iceberg table with optional partition spec.
    async fn create_table(
        &self,
        namespace: &NamespaceIdent,
        table_name: &str,
        arrow_schema: &datafusion::arrow::datatypes::SchemaRef,
        partition_by: &[String],
    ) -> anyhow::Result<()> {
        let iceberg_schema = ice(arrow_schema_to_schema_auto_assign_ids(arrow_schema))?;
        let mut builder = PartitionSpecBuilder::new(iceberg_schema.clone()).with_spec_id(0);
        for col in partition_by {
            builder = builder.add_partition_field(col, col, Transform::Identity)?;
        }
        let partition_spec = builder.build()?.into_unbound();

        let creation = TableCreation::builder()
            .name(table_name.to_string())
            .schema(iceberg_schema)
            .partition_spec(partition_spec)
            .build();

        ice(self.datalake.create_table_if_not_exist(namespace, creation).await)?;
        Ok(())
    }
}

/// Expand a source path (which may contain glob wildcards) into individual
/// file paths.  Handles:
/// - Literal file path → single-element vec
/// - Glob like `/dir/*.parquet` → all matching files, sorted
/// - Directory → all files within, sorted
fn expand_source_files(path: &str) -> Vec<String> {
    let p = std::path::Path::new(path);

    // Literal file
    if p.is_file() {
        return vec![path.to_string()];
    }

    // Check if path contains glob characters
    if path.contains('*') || path.contains('?') {
        // Split into directory + pattern
        let parent = p.parent().unwrap_or(std::path::Path::new("."));
        let pattern = p.file_name().and_then(|n| n.to_str()).unwrap_or("");

        if let Ok(entries) = std::fs::read_dir(parent) {
            let mut files: Vec<String> = entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_file())
                .filter(|e| {
                    let name = e.file_name();
                    let name = name.to_string_lossy();
                    glob_match(pattern, &name)
                })
                .map(|e| e.path().to_string_lossy().to_string())
                .collect();
            files.sort();
            return files;
        }
    }

    // Directory: list all files
    if p.is_dir() {
        if let Ok(entries) = std::fs::read_dir(p) {
            let mut files: Vec<String> = entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_file())
                .map(|e| e.path().to_string_lossy().to_string())
                .collect();
            files.sort();
            return files;
        }
    }

    Vec::new()
}

/// Simple glob matcher supporting `*` (any chars) and `?` (single char).
fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    glob_match_inner(&p, &t)
}

fn glob_match_inner(pattern: &[char], text: &[char]) -> bool {
    match (pattern.first(), text.first()) {
        (None, None) => true,
        (Some('?'), Some(_)) => glob_match_inner(&pattern[1..], &text[1..]),
        (Some('*'), _) => {
            // Try matching zero or more characters
            glob_match_inner(&pattern[1..], text)
                || text.first().map_or(false, |_| glob_match_inner(pattern, &text[1..]))
        }
        (Some(&pc), Some(&tc)) if pc == tc => glob_match_inner(&pattern[1..], &text[1..]),
        _ => false,
    }
}
