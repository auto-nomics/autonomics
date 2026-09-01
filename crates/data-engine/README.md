# data-engine

`data-engine` is the DataFusion-based workflow engine used by autonomics. It models a data-analysis pipeline as a typed, directed acyclic graph (DAG), validates port connections, schedules ready nodes asynchronously, and retains each node's output for downstream consumers or inspection.

It is intentionally independent of the Agent loop. `data-engine-tools` adapts its channel-based runtime into Agentik `ToolFunction`s.

## Data flow

```text
FileToDataFrameNode ── DataFrame ──> SqlNode / LinearRegressionNode ──> DataFrameToFileNode
       │                           │
       └──────────── fan-out ──────┴──> more transformations
```

Every edge connects one named output port to one named input port. The public convenience APIs use port `0` for the common single-input/single-output case. A graph must be acyclic and have compatible ports before it runs.

## Built-in nodes

| Node | Inputs → outputs | Purpose |
| --- | --- | --- |
| `FileToDataFrameNode` | 0/1 → 1 | Reads an external path or upstream file as CSV/TSV/Parquet, JSON/NDJSON, or a biological file (VCF, BAM, BED, ...) and emits a DataFusion `DataFrame`. |
| `SqlNode` | 1+ → 1 | Runs a DataFusion SQL query. Inputs are registered in an isolated context as `port_0`, `port_1`, and so on. |
| `LinearRegressionNode` | 1 → 1 | Fits an OLS regression with configurable predictor columns and optional intercept. |
| `ldsc_h2_container` | 1 → 1 | Runs the official LDSC h² CLI with cataloged EUR reference panels. |
| `ldsc_rg_container` | 2 → 1 | Runs the official LDSC rg CLI with the same image and panel contract. |
| `LdscSldscNode` | 1 → 1 | Stratified LD Score Regression (S-LDSC). Reads multi-annotation baselineLD from files (`ref_ld_chr` / `w_ld_chr` config prefixes). Outputs a per-annotation result table. |
| `TwasFusionNode` | 1 → 3 | Runs official FUSION TWAS association testing with GTEx v8 weights and 1000G EUR LDREF. |
| `DataFrameToFileNode` | 1 → 1 | Writes CSV, TSV, or Parquet to a local path or configured VFS mount and emits a file reference. |

`biofusion` supplies the JSON and biological readers used by `FileToDataFrameNode`: JSON arrays, NDJSON, VCF, BCF, FASTA, FASTQ, BED, GTF, GFF, SAM, BAM, CRAM, BigWig, and BigBed. Formats are normally inferred from the file suffix, including compressed suffixes such as `.json.gz` and `.vcf.gz`.

## Build and run a pipeline

Nodes are created through the engine registry by `kind` and a JSON `spec`:

```rust,no_run
use data_engine::DataEngine;
use serde_json::json;

# async fn example() -> Result<(), Box<dyn std::error::Error>> {
let mut engine = DataEngine::builder().build();

engine.add_node_from_registry(
    "variants",
    "file_to_dataframe",
    json!({ "path": "input.vcf.gz", "format": null }),
)?;
engine.add_node_from_registry("filtered", "sql", json!({ "sql_query": "SELECT * FROM port_0" }))?;
engine.add_node_from_registry(
    "write",
    "dataframe_to_file",
    json!({ "path": "output.parquet", "format": "parquet", "mode": "overwrite" }),
)?;
engine.add_edge("variants", "filtered", 0, 0)?;
engine.add_edge("filtered", "write", 0, 0)?;

let report = engine.run().await?;
assert!(report.ok, "pipeline errors: {:?}", report.errors);
# Ok(())
# }
```

Use `engine.view_dag()` to obtain a Graphviz DOT representation. `engine.get_output(node_id).await` returns the in-memory port outputs of a completed node.

## Partitioned Parquet

`file_to_dataframe` and `dataframe_to_file` support Hive-style Parquet partitioning with the
same `partition_by` field:

```rust,no_run
# async fn example() -> Result<(), Box<dyn std::error::Error>> {
# let mut engine = data_engine::DataEngine::builder().build();
engine.add_node_from_registry(
    "read_partitions",
    "file_to_dataframe",
    serde_json::json!({
        "path": "/data/variants",
        "format": "parquet",
        "partition_by": ["chrom"]
    }),
)?;
engine.add_node_from_registry(
    "write_partitions",
    "dataframe_to_file",
    serde_json::json!({
        "path": "/output/variants",
        "format": "parquet",
        "partition_by": ["chrom"],
        "mode": "overwrite"
    }),
)?;
# Ok(())
# }
```

Partition values are restored as Utf8 strings. In overwrite mode the sink first
clears the destination; in append mode it adds files to the existing partition
set and invalidates DataFusion's directory listing cache.

## Object storage

`DataEngine::builder()` creates a standalone DataFusion session. Add integrations only when the pipeline needs them:

```rust,no_run
use std::sync::Arc;
use data_engine::DataEngine;
use vfs::OpendalFileStorage;

# async fn example() -> Result<(), Box<dyn std::error::Error>> {
let files = Arc::new(OpendalFileStorage::new("/data"));
let engine = DataEngine::builder()
    .register_opendal_fs(files)?
    .build();
# Ok(())
# }
```

The engine routes mounted virtual paths through the registered VFS object store.

## Agent-facing runtime

`runtime::spawn_with_engine` hosts a `DataEngine` in a Tokio task and returns a cloneable `DataEngineClient`. Requests use an unbounded command channel plus one-shot replies. `data-engine-tools` exposes the following operations to an Agent:

- add source, SQL, sink, and linear-regression nodes;
- connect nodes with default or explicit ports;
- run, view, clear, and remove DAG nodes; and
- retrieve paginated output.

This boundary serializes graph mutations while allowing the Agent runtime and UI to remain independent of the engine implementation.

## Tests

Run the crate tests with a writable target directory:

```bash
CARGO_TARGET_DIR=/tmp/autonomics-target cargo test -p data-engine
```

The crate includes fixture-driven pipeline tests, graph validation tests, scheduler/concurrency tests, and VFS-backed integration tests.
