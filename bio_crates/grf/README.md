# grf — Rust port of R `grf` (Generalized Random Forests)

This crate is a Rust port of the R `grf` package (Wager, Athey, Tibshirani et al.,
v2.6.1). It exposes the same statistical API as R's `grf` but dispatches every
algorithm to the upstream C++ core via a thin `extern "C"` shim in
`crates/grf-sys/`.

## Why a port instead of a re-implementation?

`grf`'s C++ core (`reference/grf/core/`) is **independent of Rcpp** — it
was designed from the start to support multiple language bindings via
`RuntimeContext { verbose_stream, interrupt_handler }`. The full algorithm
library (~6000 lines of C++17 + Eigen) was authored by the grf-labs team
and is the gold-standard reference. Re-implementing it in Rust would risk
subtle numerical drift, whereas direct FFI gives us **bit-identical** outputs
to R `grf`.

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│ bio_crates/grf/                                             │
│   ├── src/forest.rs     — high-level Trainer API + ForestBlob│
│   ├── src/data.rs       — column-major ↔ Arrow helpers     │
│   ├── src/nodes/        — DAG node specs (regression, predict)│
│   └── tests/smoke.rs    — round-trip tests                  │
└─────────────────────────────────────────────────────────────┘
                           │
                           │ uses
                           ▼
┌─────────────────────────────────────────────────────────────┐
│ crates/grf-sys/                                             │
│   ├── build.rs          — `cc::Build` driver, no CMake      │
│   ├── wrapper/grf_shim.{h,cpp}  — extern "C" ABI shim       │
│   ├── src/lib.rs        — safe Rust wrappers                │
│   └── src/ffi.rs        — raw `extern "C"` declarations     │
└─────────────────────────────────────────────────────────────┘
                           │
                           │ links
                           ▼
                  libgrf_core.a + libgrf_shim.a
                  (built from reference/grf/core/src/*.cpp
                   + vendored Eigen 3.4.0)
```

## Building

`grf-sys` ships its own `build.rs` that drives the C++ compilation:

```
cargo build -p grf-sys   # builds the staticlibs (grf_core, grf_shim)
cargo build -p grf       # builds the Rust API on top
```

The build is ~6000 lines of C++ + Eigen; first build takes a couple of
minutes, subsequent rebuilds are fast.

## Smoke tests

```
cargo test -p grf --test smoke
```

Covers:
- `regression_train_predict_smoke`: train + predict round-trip on synthetic data.
- `regression_serialize_roundtrip`: forest blob survives serialize/deserialize.
- `oob_predictions_present`: OOB predictions are captured at training time.
- `arrow_round_trip_through_dag_node`: end-to-end through Arrow `RecordBatch`.

## Status

- **P0 ✅** `grf-sys` foundation (build, C ABI shim, Rust wrappers).
- **P0 ✅** `grf` crate skeleton (data, forest, ForestBlob).
- **P1 ✅** `regression_forest` + `predict_forest` (DAG node specs).
- **P2** baseline nodes (quantile / probability / survival / multi-regression).
- **P3** `causal_forest` + ATE / BLP / test_calibration.
- **P4** instrumental / lm / ll_regression / boosted_regression.
- **P5** causal_survival / multi_arm_causal / RATE / analysis tools.

See `docs/grf_analysis.md` for the full plan.

## License

`grf` is GPL-3. Static linking grf-sys therefore makes the resulting binary
GPL-3 unless alternative arrangements (dynamic loading, process isolation)
are used. See the project root LICENSE for the wrapper crate's own license.