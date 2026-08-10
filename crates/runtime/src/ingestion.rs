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

use std::sync::Arc;

use dag_core::resource_catalog::{
    IngestionOutcome, IngestionSpec, ResourceCatalog, ResourceKind, SourceFormat, WriteMode,
};
use datafusion::prelude::{CsvReadOptions, ParquetReadOptions, SessionContext};
use datalake::Datalake;
use iceberg::arrow::arrow_schema_to_schema_auto_assign_ids;
use iceberg::spec::{PartitionSpecBuilder, Transform};
use iceberg::{Catalog, NamespaceIdent, TableCreation, TableIdent};

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
        let (_catalog_name, schema, table) = ident.ident();
        let namespace = NamespaceIdent::from_vec(vec![schema])?;

        // ── Read source data via DataFusion ───────────────────────────
        let read_ctx = ice(self.datalake.get_ctx().await)?;
        let df = self.read_source(&read_ctx, spec).await?;
        let rows = df.clone().count().await?;
        let files_processed = count_source_files(&spec.source_path);

        // ── Get Arrow schema from the source DataFrame ────────────────
        let arrow_schema = df.schema().inner().clone();

        // ── Create/drop table based on write mode ─────────────────────
        let rest_catalog = ice(self.datalake.get_catalog().await)?;
        let table_ident = TableIdent::new(namespace.clone(), table.clone());

        match spec.mode {
            WriteMode::CreateOrReplace => {
                if ice(rest_catalog.table_exists(&table_ident).await)? {
                    ice(rest_catalog.drop_table(&table_ident).await)?;
                    tracing::info!("dropped existing table for replacement");
                }
                self.create_table(&namespace, &table, &arrow_schema, &spec.partition_by).await?;
            }
            WriteMode::CreateIfNotExists => {
                self.create_table(&namespace, &table, &arrow_schema, &spec.partition_by).await?;
            }
            WriteMode::Append => {
                if !ice(rest_catalog.table_exists(&table_ident).await)? {
                    anyhow::bail!("Append mode requires table to already exist");
                }
            }
        }

        // ── Get a FRESH context (the table was just created) ──────────
        let ctx = ice(self.datalake.get_ctx().await)?;

        // ── Check if already has data (for CreateIfNotExists) ─────────
        if spec.mode == WriteMode::CreateIfNotExists {
            let fqn = ident.sql();
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

        // ── INSERT INTO target SELECT * FROM source ───────────────────
        let fqn = ident.sql();
        let src_name = format!("__ingest_src_{table}");
        ctx.register_table(&src_name, df.into_view())?;
        let sql = format!("INSERT INTO {fqn} SELECT * FROM {src_name}");
        ctx.sql(&sql).await?.collect().await?;
        ctx.deregister_table(&src_name)?;

        tracing::info!(
            "ingested {rows} rows into {fqn} from {} ({files_processed} files, {}ms)",
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
            rows_written: rows as u64,
            files_processed,
            duration_ms: start.elapsed().as_millis(),
            skipped: false,
        })
    }

    /// Read source files into a DataFrame based on the format.
    async fn read_source(
        &self,
        ctx: &SessionContext,
        spec: &IngestionSpec,
    ) -> anyhow::Result<datafusion::dataframe::DataFrame> {
        let df = match spec.source_format {
            SourceFormat::Parquet => {
                ctx.read_parquet(&spec.source_path, ParquetReadOptions::default()).await?
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

        // Cast Utf8View columns to Utf8 — DataFusion ≥42 reads parquet string
        // columns as Utf8View, but iceberg-rust expects Utf8 for writes.
        // SQL CAST is optimized away by DataFusion's planner, so we must
        // physically materialize: collect batches, cast Arrow arrays, register
        // as a MemTable.
        let needs_cast = df
            .schema()
            .fields()
            .iter()
            .any(|f| f.data_type() == &datafusion::arrow::datatypes::DataType::Utf8View);

        if needs_cast {
            tracing::debug!("physically casting Utf8View → Utf8 for Iceberg compatibility");
            let orig_schema = df.schema().clone();
            let batches = df.collect().await?;
            let cast_schema = datafusion::arrow::datatypes::SchemaRef::new(
                datafusion::arrow::datatypes::Schema::new(
                    orig_schema
                        .fields()
                        .iter()
                        .map(|f| {
                            if f.data_type() == &datafusion::arrow::datatypes::DataType::Utf8View {
                                datafusion::arrow::datatypes::Field::new(
                                    f.name(),
                                    datafusion::arrow::datatypes::DataType::Utf8,
                                    f.is_nullable(),
                                )
                            } else {
                                f.as_ref().clone()
                            }
                        })
                        .collect::<Vec<_>>(),
                ),
            );

            let mut casted_batches = Vec::with_capacity(batches.len());
            for batch in &batches {
                let arrays: Vec<std::sync::Arc<dyn datafusion::arrow::array::Array>> = batch
                    .columns()
                    .iter()
                    .enumerate()
                    .map(|(i, col)| {
                        if batch.schema().field(i).data_type()
                            == &datafusion::arrow::datatypes::DataType::Utf8View
                        {
                            datafusion::arrow::compute::cast(col, &datafusion::arrow::datatypes::DataType::Utf8)
                                .unwrap_or_else(|_| col.clone())
                        } else {
                            col.clone()
                        }
                    })
                    .collect();
                casted_batches.push(
                    datafusion::arrow::record_batch::RecordBatch::try_new(
                        cast_schema.clone(),
                        arrays,
                    )?,
                );
            }

            let provider =
                datafusion::datasource::MemTable::try_new(cast_schema, vec![casted_batches])?;
            ctx.register_table("__cast_src", std::sync::Arc::new(provider))?;
            let result = ctx.table("__cast_src").await?;
            ctx.deregister_table("__cast_src")?;
            Ok(result)
        } else {
            Ok(df)
        }
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

/// Count files matching a path (best-effort).
fn count_source_files(path: &str) -> u64 {
    let p = std::path::Path::new(path);
    if p.is_file() {
        return 1;
    }
    if p.is_dir() {
        return std::fs::read_dir(p)
            .map(|entries| entries.filter(|e| e.as_ref().map(|e| e.path().is_file()).unwrap_or(false)).count() as u64)
            .unwrap_or(1);
    }
    1
}
