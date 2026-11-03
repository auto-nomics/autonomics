//! Provenance carried by a retained Arrow table itself, never a global cache
//! or a guess based on a plan's debug rendering. Only our retained-output
//! loader can construct this provider; arbitrary user ListingTables do not
//! qualify for remote forwarding.

use std::{any::Any, borrow::Cow, future::Future, ops::Range, sync::Arc};

use arrow::datatypes::SchemaRef;
use datafusion::{
    catalog::{ScanArgs, ScanResult, Session, TableProvider},
    common::{Constraints, Result as DataFusionResult, Statistics},
    datasource::{
        file_format::arrow::ArrowFormat,
        listing::{ListingTable, ListingTableUrl},
        source_as_provider,
    },
    execution::options::ArrowReadOptions,
    logical_expr::{Expr, LogicalPlan, TableProviderFilterPushDown, TableType},
    object_store::ObjectStoreExt,
    physical_plan::ExecutionPlan,
    prelude::DataFrame,
};

use crate::dag::DagError;
use crate::{NodeCtx, value::FileRef};

#[derive(Debug)]
pub(super) struct RetainedArrowProvider {
    inner: Arc<dyn TableProvider>,
    origin: FileRef,
}

/// Read only Arrow schema metadata now, preserving a reference for execution
/// on the cluster. Only the bounded IPC file footer is read: DataFusion's
/// stream schema inference assumes eight-byte file alignment, whereas Arrow's
/// writer defaults to 64. Local row access is deliberately closed:
/// it cannot safely reopen a mutable VFS path under a previously pinned hash.
pub(super) async fn load_retained_arrow(
    origin: &FileRef,
    ctx: &NodeCtx,
) -> Result<DataFrame, DagError> {
    let location = ListingTableUrl::parse(&origin.path)
        .map_err(|error| DagError::Schedule(error.to_string()))?;
    if location.scheme() != "vfs" || location.is_collection() || location.get_glob().is_some() {
        return Err(DagError::Schedule(
            "retained Arrow origin must be one exact VFS IPC file".into(),
        ));
    }
    let session = ctx.session();
    let store = session
        .runtime_env()
        .object_store(location.object_store())
        .map_err(|error| DagError::Schedule(error.to_string()))?;
    let object = location.prefix().clone();
    let metadata = store
        .head(&object)
        .await
        .map_err(|error| DagError::Schedule(error.to_string()))?;
    let schema = read_ipc_file_schema(metadata.size, |range| {
        let store = store.clone();
        let object = object.clone();
        async move {
            store
                .get_range(&object, range)
                .await
                .map(|bytes| bytes.to_vec())
                .map_err(|error| DagError::Schedule(error.to_string()))
        }
    })
    .await
    .map_err(|error| {
        DagError::Schedule(format!(
            "cannot open retained Arrow schema `{}`: {error}",
            origin.path
        ))
    })?;
    let frame = session
        .read_arrow(
            &origin.path,
            ArrowReadOptions {
                schema: Some(schema.as_ref()),
                file_extension: "",
                ..Default::default()
            },
        )
        .await
        .map_err(|error| {
            DagError::Schedule(format!(
                "cannot open retained Arrow schema `{}`: {error}",
                origin.path
            ))
        })?;
    let LogicalPlan::TableScan(scan) = frame.logical_plan() else {
        return Err(DagError::Schedule(
            "retained Arrow loader did not produce a table scan".into(),
        ));
    };
    let inner =
        source_as_provider(&scan.source).map_err(|error| DagError::Schedule(error.to_string()))?;
    // Keep this constructor tied to the known DataFusion Arrow reader. No
    // public constructor can attach a false origin to an arbitrary provider.
    let Some(listing) = inner.as_any().downcast_ref::<ListingTable>() else {
        return Err(DagError::Schedule(
            "retained Arrow reader did not produce a listing table".into(),
        ));
    };
    let paths = listing.table_paths();
    if paths.len() != 1
        || paths[0].get_url().scheme() != "vfs"
        || paths[0].is_collection()
        || paths[0].get_glob().is_some()
        || !listing.options().format.as_any().is::<ArrowFormat>()
        || !listing.options().table_partition_cols.is_empty()
    {
        return Err(DagError::Schedule(
            "retained Arrow origin must be one exact VFS IPC file".into(),
        ));
    }
    let provider = RetainedArrowProvider {
        inner,
        origin: origin.clone(),
    };
    session
        .read_table(Arc::new(provider))
        .map_err(|error| DagError::Schedule(error.to_string()))
}

// Cap untrusted footer allocation; record-batch bodies are never fetched by
// the coordinator. Both eight- and 64-byte writer alignment use this footer.
const MAX_IPC_FOOTER_BYTES: usize = 16 * 1024 * 1024;

async fn read_ipc_file_schema<F, Fut>(size: u64, mut read: F) -> Result<SchemaRef, DagError>
where
    F: FnMut(Range<u64>) -> Fut,
    Fut: Future<Output = Result<Vec<u8>, DagError>>,
{
    let invalid =
        |reason: &str| DagError::Schedule(format!("invalid retained Arrow IPC file: {reason}"));
    if size < 18 {
        return Err(invalid("file is too short"));
    }
    let magic = read(0..6).await?;
    if magic.as_slice() != b"ARROW1" {
        return Err(invalid("missing file magic"));
    }
    let trailer = read(size - 10..size).await?;
    let trailer: [u8; 10] = trailer
        .try_into()
        .map_err(|_| invalid("truncated file trailer"))?;
    let footer_size = arrow::ipc::reader::read_footer_length(trailer)
        .map_err(|error| invalid(&error.to_string()))?;
    if footer_size == 0 || footer_size > MAX_IPC_FOOTER_BYTES {
        return Err(invalid("footer exceeds metadata budget or is empty"));
    }
    let footer_end = size - 10;
    let footer_start = footer_end
        .checked_sub(footer_size as u64)
        .filter(|start| *start >= 8)
        .ok_or_else(|| invalid("footer overlaps the file header"))?;
    let bytes = read(footer_start..footer_end).await?;
    if bytes.len() != footer_size {
        return Err(invalid("truncated file footer"));
    }
    let footer = arrow::ipc::root_as_footer(&bytes)
        .map_err(|error| invalid(&format!("malformed footer: {error}")))?;
    let schema = footer
        .schema()
        .ok_or_else(|| invalid("footer has no schema"))?;
    if !schema.endianness().equals_to_target_endianness() {
        return Err(invalid("schema endianness differs from this host"));
    }
    if footer.recordBatches().is_none() {
        return Err(invalid("footer has no record-batch index"));
    }
    Ok(Arc::new(arrow::ipc::convert::fb_to_schema(schema)))
}

/// Forward only an unchanged scan over a provider we constructed. Projection,
/// filtering, limits, joins, explicit schema overrides, directory scans and
/// arbitrary ListingTables cannot claim an unchanged retained origin. Trying
/// to materialize a transformed retained table locally fails explicitly.
pub fn pure_retained_arrow_origin(frame: &DataFrame) -> Option<FileRef> {
    let LogicalPlan::TableScan(scan) = frame.logical_plan() else {
        return None;
    };
    if scan.projection.is_some() || !scan.filters.is_empty() || scan.fetch.is_some() {
        return None;
    }
    let provider = source_as_provider(&scan.source).ok()?;
    let retained = provider.as_any().downcast_ref::<RetainedArrowProvider>()?;
    if !retained.origin.path.starts_with("vfs://")
        || scan.projected_schema.as_arrow() != retained.inner.schema().as_ref()
    {
        return None;
    }
    Some(retained.origin.clone())
}

#[async_trait::async_trait]
impl TableProvider for RetainedArrowProvider {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn schema(&self) -> SchemaRef {
        self.inner.schema()
    }
    fn constraints(&self) -> Option<&Constraints> {
        self.inner.constraints()
    }
    fn table_type(&self) -> TableType {
        self.inner.table_type()
    }
    fn get_table_definition(&self) -> Option<&str> {
        self.inner.get_table_definition()
    }
    fn get_logical_plan(&self) -> Option<Cow<'_, LogicalPlan>> {
        self.inner.get_logical_plan()
    }
    fn get_column_default(&self, column: &str) -> Option<&Expr> {
        self.inner.get_column_default(column)
    }
    fn statistics(&self) -> Option<Statistics> {
        self.inner.statistics()
    }
    fn supports_filters_pushdown(
        &self,
        filters: &[&Expr],
    ) -> DataFusionResult<Vec<TableProviderFilterPushDown>> {
        self.inner.supports_filters_pushdown(filters)
    }
    async fn scan(
        &self,
        _state: &dyn Session,
        _projection: Option<&Vec<usize>>,
        _filters: &[Expr],
        _limit: Option<usize>,
    ) -> DataFusionResult<Arc<dyn ExecutionPlan>> {
        Err(local_scan_rejected())
    }
    async fn scan_with_args<'a>(
        &self,
        _state: &dyn Session,
        _args: ScanArgs<'a>,
    ) -> DataFusionResult<ScanResult> {
        Err(local_scan_rejected())
    }
}

fn local_scan_rejected() -> datafusion::common::DataFusionError {
    datafusion::common::DataFusionError::Execution(
        "retained cluster Arrow rows require remote placement; submit computation or preview aggregation to the cluster worker".into(),
    )
}

#[cfg(test)]
#[path = "retained_arrow_tests.rs"]
mod tests;
