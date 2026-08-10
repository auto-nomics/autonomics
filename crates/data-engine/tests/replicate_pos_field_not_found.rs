//! Regression: the Iceberg column name `pos` collides with iceberg-rust's
//! reserved metadata column name `RESERVED_COL_NAME_DELETE_FILE_POS = "pos"`.
//!
//! ## Root cause
//!
//! In `iceberg/src/metadata_columns.rs`, iceberg-rust reserves the bare name
//! `pos` (no leading underscore) for position-delete files:
//!
//! ```text
//! pub const RESERVED_COL_NAME_DELETE_FILE_POS: &str = "pos";
//! ```
//!
//! During scan (`iceberg/src/scan/mod.rs`), every projected column name is
//! passed through `is_metadata_column_name()`. For a column literally named
//! `pos` this returns `true`, so the scanner substitutes the reserved field id
//! `RESERVED_FIELD_ID_DELETE_FILE_POS` (≈ `i32::MAX`) instead of the data
//! column's real field id. The Parquet file obviously has no column with that
//! reserved id, so the read fails with `External(Unexpected => field not found)`.
//!
//! The symptom is exactly what the user hit with VCF data: oxbow names the
//! VCF `POS` fixed field `pos`, so after a VCF → Iceberg round-trip the column
//! exists in the catalog schema but can never be scanned back.
//!
//! Note this is *not* a DataFusion reserved word — `pos` parses fine as a
//! column reference. It is purely an iceberg-rust metadata-column name clash.
//!
//! ## Workaround (built into `IcebergSinkNode`)
//!
//! `IcebergSinkNode`'s execute now calls `rename_iceberg_reserved_columns`, which
//! renames top-level `pos`/`file_path` to `pos_col`/`file_path_col` before
//! writing (see `sink_iceberg.rs`, tagged `WORKAROUND(iceberg-rust)`). So a DataFrame
//! with a `pos` column round-trips as `pos_col`.
//!
//! ## Tests
//!
//! * [`upstream_iceberg_rust_misclassifies_pos_as_metadata`] — logic-level
//!   repro via the public `iceberg` API (no catalog needed). Pins the upstream
//!   bug; will fail (as a regression alarm) once upstream fixes it.
//! * [`sink_auto_renames_reserved_columns`] — end-to-end: writing a `pos`
//!   column through `IcebergSinkNode` yields a readable `pos_col` (needs a catalog).

use std::sync::Arc;

use arrow_array::{ArrayRef, Int32Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use dag_core::{NodeInput, SinkMode};
use data_engine::dag::DagNode;
use datafusion::prelude::SessionContext;
use datalake::Datalake;
use iceberg::metadata_columns::{
    RESERVED_COL_NAME_DELETE_FILE_POS, RESERVED_FIELD_ID_DELETE_FILE_POS, get_metadata_field_id,
    is_metadata_column_name,
};
use nodes_io::sink_iceberg::IcebergSinkNode;

/// Zero-dependency logic-level repro of the upstream iceberg-rust bug: the bare
/// column name `pos` (no leading underscore) is reserved as a metadata column
/// (`RESERVED_COL_NAME_DELETE_FILE_POS`), so `is_metadata_column_name("pos")`
/// returns `true`. `scan/mod.rs` consults this for every projected column of a
/// data-table scan, so `SELECT pos` resolves to the reserved delete-file field
/// id instead of the data column's real field id → "field not found".
///
/// This test uses only the public `iceberg` crate API — no catalog, no storage,
/// no Parquet — so it reproduces the root cause directly and runs in CI without
/// infrastructure. See the end-to-end confirmation in [`replicate_pos_field_not_found`].
#[test]
fn upstream_iceberg_rust_misclassifies_pos_as_metadata() {
    // The reserved name is the bare `pos` (not the spec's `_pos`).
    assert_eq!(RESERVED_COL_NAME_DELETE_FILE_POS, "pos");

    // A user data column named `pos` is misclassified as a metadata column.
    // Per the Iceberg spec only `_`-prefixed names are metadata columns, so
    // this should be `false`.
    assert!(
        is_metadata_column_name("pos"),
        "iceberg-rust misclassifies `pos` as a metadata column (the bug)"
    );
    assert!(
        is_metadata_column_name("file_path"),
        "`file_path` (RESERVED_COL_NAME_DELETE_FILE_PATH) has the same bug"
    );

    // Consequently a data column `pos` resolves to the reserved delete-file
    // field id (near i32::MAX) rather than its real schema field id.
    assert_eq!(
        get_metadata_field_id("pos").unwrap(),
        RESERVED_FIELD_ID_DELETE_FILE_POS,
    );
}

/// Build a 3-row DataFrame with `chrom: Utf8` (nullable) + an Int32 column of
/// the given name (nullable). Using plain Utf8 (not the Dictionary oxbow emits)
/// isolates the failure to the column *name*, independent of Arrow encoding.
fn build_df(int32_col_name: &str) -> datafusion::prelude::DataFrame {
    let ctx = SessionContext::new();
    let schema = Arc::new(Schema::new(vec![
        Field::new("chrom", DataType::Utf8, true),
        Field::new(int32_col_name, DataType::Int32, true),
    ]));
    let chrom: ArrayRef = Arc::new(StringArray::from(vec![Some("1"), Some("1"), Some("2")]));
    let value: ArrayRef = Arc::new(Int32Array::from(vec![Some(100), Some(200), Some(300)]));
    let batch = RecordBatch::try_new(schema, vec![chrom, value]).unwrap();
    ctx.read_batch(batch).unwrap()
}

/// Write `df` to `ident` (overwrite) and re-register the catalog so the new
/// table is visible. Returns nothing; callers query the table afterwards.
async fn write_to_iceberg(
    ctx: &datafusion::prelude::SessionContext,
    datalake: &Arc<Datalake>,
    ident: &str,
    df: datafusion::prelude::DataFrame,
) {
    let provider = datalake.get_provider().await.unwrap();
    let node_ctx = data_engine::node_registry::registry::NodeCtx {
        runtime_env: ctx.runtime_env(),
        iceberg_catalog: Some(Arc::new(provider)),
        datalake: datalake.clone(),
        opendal: None,
        resources: std::sync::Arc::new(dag_core::resource_catalog::ResourceCatalog::new(std::path::PathBuf::from("."))),
    };
    let mut sink_node =
        IcebergSinkNode::new(ident.to_string(), SinkMode::Overwrite, datalake.clone());
    sink_node
        .execute(
            &node_ctx,
            &[NodeInput { port: 0, data: df }],
            &data_engine::dag::node_event::NodeReporter::noop(),
        )
        .await
        .unwrap();

    let fresh_provider = datalake.get_provider().await.unwrap();
    ctx.register_catalog("iceberg", Arc::new(fresh_provider));
}

/// `SELECT col FROM iceberg.<ident> LIMIT 3` succeeds iff it collects rows.
async fn can_select(
    ctx: &datafusion::prelude::SessionContext,
    ident: &str,
    col: &str,
) -> Result<usize, String> {
    let sql = format!("SELECT {col} FROM iceberg.{ident} LIMIT 3");
    match ctx.sql(&sql).await {
        Ok(r) => r
            .collect()
            .await
            .map(|b| b.iter().map(|x| x.num_rows()).sum())
            .map_err(|e| format!("collect: {e}")),
        Err(e) => Err(format!("plan: {e}")),
    }
}

/// End-to-end: `IcebergSinkNode` auto-renames the reserved `pos` column to `pos_col`
/// on the Iceberg write path, so the data survives the round-trip readable.
/// Without the workaround (in `sink_iceberg.rs`, tagged `WORKAROUND(iceberg-rust)`),
/// `SELECT pos` would fail with "field not found" and no `pos_col` would exist.
#[tokio::test]
// #[ignore] // requires a running Iceberg REST catalog
async fn sink_auto_renames_reserved_columns() {
    let ctx = Datalake::default().get_ctx().await.unwrap();
    let datalake = Arc::new(Datalake::default());

    write_to_iceberg(&ctx, &datalake, "gwas.test_pos_autorename", build_df("pos")).await;

    // chrom passes through untouched.
    assert_eq!(
        can_select(&ctx, "gwas.test_pos_autorename", "chrom").await,
        Ok(3)
    );

    // The reserved name is gone (renamed away by the sink), so SELECT pos
    // fails — but for a *different* reason than the upstream bug: the column
    // simply no longer exists in the table.
    assert!(
        can_select(&ctx, "gwas.test_pos_autorename", "pos")
            .await
            .is_err(),
        "`pos` should have been renamed away by the sink"
    );

    // The renamed column reads back fine — this is the whole point of the
    // workaround.
    assert_eq!(
        can_select(&ctx, "gwas.test_pos_autorename", "pos_col").await,
        Ok(3)
    );
}
