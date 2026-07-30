# Visualization (`visualization`)

[English](visualization.md) | [中文](visualization_zh.md)

A DAG node that renders a DataFusion `DataFrame` to PNG via R/ggplot2.

## R/ggplot2 rendering

Data crosses the Rust→R boundary as an **Arrow IPC stream** (`arrow::ipc::writer::StreamWriter` → `arrow::read_ipc_stream`), so column types are preserved exactly — no CSV re-inference, no row-wise JSON.

```text
DataFrame → collect() → Arrow IPC stream bytes → tempdir → Rscript
   (arrow::read_ipc_stream → df → <r_code> → ggsave) → PNG bytes
   → opendal op.write(virtual_path) → NodeReport.artifact_path
```

## Subprocess, not in-process R

The renderer shells out to `Rscript` rather than linking `libR` in-process. R is therefore a _runtime-only, optional_ dependency: the workspace compiles and the core pipeline (LDSC, MR, …) runs without R installed. A missing or misconfigured R surfaces as a typed `VizError::RscriptNotFound` / `RscriptFailed` at render time, never as a build failure.

## DAG node

Exposed as the `visualization` node kind (`data-engine/nodes/viz.rs`), reached by the agent through the generic `add_node` / `run_dag` tools — no dedicated tool. It mirrors `SinkNode`: one untyped input port, no output ports. The rendered path is reported back via `NodeReport.artifact_path`.

## opendal output

The PNG is written into the engine's **opendal-virtualized filesystem** (the same isolated space as source/sink data), not the host filesystem. `output_path` is a virtual path (e.g. `/plots/scatter.png`); the opendal handle is threaded from the builder through `NodeCtx.opendal`.

## Plot spec

The `r_code` field is ggplot2 R code that runs with a `data.frame` named `df` already bound to the input; it must build a plot and assign it to a variable named `p`.

Example:

```r
p <- ggplot(df, aes(x = bp, y = pval)) + geom_point()
```

Dimensions (`width`/`height`/`dpi`) are optional.

## R requirement (optional)

Rendering needs `Rscript` on `PATH` with the `arrow` and `ggplot2` packages installed. Override the binary with the `VISUALIZATION_RSCRIPT` env var. The engine does **not** detect or pin an R version — whichever `Rscript` the launching process resolves wins. A conda env is the cleanest way to provision a known-good R (see the crate's tests for the expected packages).
