//! Safe Rust bindings for the GRF C++ core.
//!
//! This crate is a thin FFI layer over `wrapper/grf_shim.cpp` (which itself
//! wraps `reference/grf/core/`). The C++ core is built as `libgrf_core.a`
//! via `cc::Build` in `build.rs`; the C ABI shim is built as `libgrf_shim.a`.
//! Both staticlibs are linked into every crate that depends on `grf-sys`.
//!
//! Three layers of types:
//!
//! 1. **Raw FFI** (the `ffi` module): straight `extern "C"` declarations that
//!    mirror `wrapper/grf_shim.h` 1:1. These are `unsafe` to call and own
//!    opaque `*mut` handles.
//!
//! 2. **Safe wrappers** ([`Forest`], [`Predictions`], [`TrainOptions`]): RAII
//!    wrappers that `Drop` their C++ handle, expose `&[f64]` accessors, and
//!    enforce invariants (e.g. `Forest` is `!Send` because grf forests are
//!    not safe to share across threads concurrently).
//!
//! 3. **Reusable parameter struct** ([`TrainOptions`]): a builder-style spec
//!    that encodes every grf `ForestOptions` field, with defaults that
//!    match grf's own defaults.
//!
//! Higher-level Rust crates (`bio_crates/grf`, `crates/node-bundles/nodes-grf`)
//! should use the safe wrappers exclusively; the `ffi` module is `pub`
//! only so they can reach into edge cases not yet covered.

use std::ffi::{CStr, CString};
use std::marker::PhantomData;
use std::os::raw::{c_char, c_double, c_uint, c_ulonglong};
use std::ptr::NonNull;
use thiserror::Error;

pub mod ffi;

// ─────────────────────────── error type ───────────────────────────

#[derive(Debug, Error)]
pub enum GrfError {
    #[error("grf: null handle returned (no forest / no predictions)")]
    NullHandle,

    #[error("grf: invalid argument: {0}")]
    InvalidArgument(String),

    #[error("grf: {0}")]
    Cpp(String),

    #[error("grf: forest kind mismatch: expected {expected}, got {actual}")]
    KindMismatch { expected: String, actual: String },

    #[error("grf: deserialization failed: {0}")]
    Deserialize(String),
}

pub type Result<T> = std::result::Result<T, GrfError>;

// ─────────────────────────── common options ───────────────────────────

/// Common training options shared by every forest type.
///
/// Defaults mirror `grf::ForestOptions` defaults plus grf R wrapper defaults:
/// `num_trees=2000`, `sample_fraction=0.5`, `min_node_size=5`, `honesty=true`,
/// `honesty_fraction=0.5`, `honesty_prune_leaves=true`, `alpha=0.05`,
/// `imbalance_penalty=0`, `ci_group_size=2`.
///
/// `mtry=0` means "let grf pick `min(ceil(sqrt(p) + 20), p)`".
/// `num_threads=0` means "hardware concurrency".
#[derive(Debug, Clone)]
pub struct TrainOptions {
    pub num_trees: u32,
    pub ci_group_size: u32,
    pub sample_fraction: f64,
    pub mtry: u32,
    pub min_node_size: u32,
    pub honesty: bool,
    pub honesty_fraction: f64,
    pub honesty_prune_leaves: bool,
    pub alpha: f64,
    pub imbalance_penalty: f64,
    pub num_threads: u32,
    pub seed: u32,
    pub legacy_seed: bool,
    pub compute_oob_predictions: bool,
    pub verbose: bool,
    /// Cluster IDs (0-indexed) for cluster-robust subsampling. `None` disables.
    pub clusters: Option<Vec<usize>>,
}

impl Default for TrainOptions {
    fn default() -> Self {
        // IMPORTANT: must match grf::ForestOptions invariants. In particular
        // `ci_group_size >= 1` is required (ForestOptions.cpp divides by it).
        Self {
            num_trees: 2000,
            ci_group_size: 2,
            sample_fraction: 0.5,
            mtry: 0,
            min_node_size: 5,
            honesty: true,
            honesty_fraction: 0.5,
            honesty_prune_leaves: true,
            alpha: 0.05,
            imbalance_penalty: 0.0,
            num_threads: 0,
            seed: 42,
            legacy_seed: false,
            compute_oob_predictions: true,
            verbose: false,
            clusters: None,
        }
    }
}

impl TrainOptions {
    pub fn to_ffi(&self) -> ffi::grf_train_opts_t {
        let (clusters_ptr, n_clusters) = match &self.clusters {
            Some(v) => (v.as_ptr() as *const c_ulonglong, v.len()),
            None => (std::ptr::null(), 0),
        };
        ffi::grf_train_opts_t {
            num_trees: self.num_trees,
            ci_group_size: self.ci_group_size,
            sample_fraction: self.sample_fraction,
            mtry: self.mtry,
            min_node_size: self.min_node_size,
            honesty: self.honesty,
            honesty_fraction: self.honesty_fraction,
            honesty_prune_leaves: self.honesty_prune_leaves,
            alpha: self.alpha,
            imbalance_penalty: self.imbalance_penalty,
            num_threads: self.num_threads,
            seed: self.seed,
            legacy_seed: self.legacy_seed,
            compute_oob_predictions: self.compute_oob_predictions,
            verbose: self.verbose,
            clusters: clusters_ptr,
            n_clusters,
            samples_per_cluster: 0,
        }
    }
}

// ─────────────────────────── Forest handle ───────────────────────────

/// Owns a `*mut grf_forest_t` from the C++ shim.
///
/// Not `Send`: GRF forests mutate internal Eigen buffers during prediction;
/// concurrent calls to a single forest are not supported by the C++ side.
/// Multi-threaded use requires each thread to own a deep copy — use
/// [`Forest::deserialize`] after [`Forest::serialize`] on a copy, or
/// share [`Forest`] via `Arc<Forest>` with external synchronization.
pub struct Forest {
    ptr: NonNull<ffi::grf_forest_t>,
    _not_send: PhantomData<*mut ()>,
}

impl Forest {
    /// Take ownership of a raw FFI pointer. Returns `None` if `ptr` is null.
    pub unsafe fn from_raw(ptr: *mut ffi::grf_forest_t) -> Option<Self> {
        NonNull::new(ptr).map(|p| Self { ptr: p, _not_send: PhantomData })
    }

    /// Borrow the raw C handle. Used by grf to call predict / serialize /
    /// deserialize functions that need `*const grf_forest_t`.
    pub fn inner_handle(&self) -> *mut ffi::grf_forest_t {
        self.ptr.as_ptr()
    }

    pub fn kind(&self) -> &str {
        let s = unsafe { CStr::from_ptr(ffi::grf_forest_kind(self.ptr.as_ptr())) };
        s.to_str().unwrap_or("")
    }

    pub fn num_trees(&self) -> usize {
        unsafe { ffi::grf_forest_num_trees(self.ptr.as_ptr()) }
    }

    /// Pre-computed OOB predictions at training time. Returns
    /// `(buffer, pred_length)` if computed, or `None` if the forest was
    /// trained with `compute_oob_predictions=false`. The buffer is
    /// column-major with shape `(pred_length, n_samples)`.
    pub fn oob_predictions(&self) -> Option<(Vec<f64>, usize)> {
        let mut total_len = 0usize;
        let mut pred_length = 0usize;
        let p = unsafe {
            ffi::grf_forest_oob_predictions(
                self.ptr.as_ptr(),
                &mut total_len,
                &mut pred_length,
            )
        };
        if p.is_null() || total_len == 0 || pred_length == 0 {
            return None;
        }
        let slice = unsafe { std::slice::from_raw_parts(p, total_len) };
        Some((slice.to_vec(), pred_length))
    }

    pub fn serialize(&self) -> Result<Vec<u8>> {
        let mut len = 0usize;
        let p = unsafe { ffi::grf_forest_serialize(self.ptr.as_ptr(), &mut len) };
        if p.is_null() {
            return Err(check_error());
        }
        let slice = unsafe { std::slice::from_raw_parts(p, len) };
        let bytes = slice.to_vec();
        unsafe { libc::free(p as *mut libc::c_void); }
        Ok(bytes)
    }

    pub fn deserialize(bytes: &[u8]) -> Result<Self> {
        let ptr = unsafe { ffi::grf_forest_deserialize(bytes.as_ptr(), bytes.len()) };
        unsafe { Self::from_raw(ptr) }.ok_or_else(check_error)
    }
}

impl Drop for Forest {
    fn drop(&mut self) {
        unsafe { ffi::grf_forest_free(self.ptr.as_ptr()) }
    }
}

// ─────────────────────────── Predictions handle ───────────────────────────

pub struct Predictions {
    ptr: NonNull<ffi::grf_predictions_t>,
}

impl Predictions {
    /// Take ownership of a raw FFI pointer. Returns `None` if `ptr` is null.
    pub unsafe fn from_raw(ptr: *mut ffi::grf_predictions_t) -> Option<Self> {
        NonNull::new(ptr).map(|p| Self { ptr: p })
    }

    pub fn num_samples(&self) -> usize {
        unsafe { ffi::grf_predictions_num_samples(self.ptr.as_ptr()) }
    }

    pub fn pred_length(&self) -> usize {
        unsafe { ffi::grf_predictions_pred_length(self.ptr.as_ptr()) }
    }

    /// Predictions as a flat column-major buffer of shape (pred_length, n).
    pub fn values(&self) -> Vec<f64> {
        let n = self.num_samples();
        let pl = self.pred_length();
        if n == 0 || pl == 0 {
            return Vec::new();
        }
        let p = unsafe { ffi::grf_predictions_values(self.ptr.as_ptr()) };
        unsafe { std::slice::from_raw_parts(p, pl * n) }.to_vec()
    }

    pub fn variance(&self) -> Option<Vec<f64>> {
        let n = self.num_samples();
        let pl = self.pred_length();
        let p = unsafe { ffi::grf_predictions_variance(self.ptr.as_ptr()) };
        if p.is_null() {
            return None;
        }
        Some(unsafe { std::slice::from_raw_parts(p, pl * n) }.to_vec())
    }

    pub fn debiased_error(&self) -> Option<Vec<f64>> {
        let p = unsafe { ffi::grf_predictions_debiased_error(self.ptr.as_ptr()) };
        if p.is_null() { return None; }
        let n = self.num_samples();
        Some(unsafe { std::slice::from_raw_parts(p, n) }.to_vec())
    }

    pub fn excess_error(&self) -> Option<Vec<f64>> {
        let p = unsafe { ffi::grf_predictions_excess_error(self.ptr.as_ptr()) };
        if p.is_null() { return None; }
        let n = self.num_samples();
        Some(unsafe { std::slice::from_raw_parts(p, n) }.to_vec())
    }
}

impl Drop for Predictions {
    fn drop(&mut self) {
        unsafe { ffi::grf_predictions_free(self.ptr.as_ptr()) }
    }
}

// ─────────────────────────── helpers ───────────────────────────

/// Copy a flat `&[f64]` matrix of shape `(n_rows, n_cols)` from a Rust
/// slice-of-slices into a column-major buffer suitable for grf's
/// `Data(const double*, n_rows, n_cols)`.
pub fn column_major(rows: &[Vec<f64>]) -> Vec<f64> {
    let n_rows = rows.len();
    if n_rows == 0 {
        return Vec::new();
    }
    let n_cols = rows[0].len();
    let mut out = vec![0.0; n_rows * n_cols];
    for (i, row) in rows.iter().enumerate() {
        debug_assert_eq!(row.len(), n_cols);
        for (j, &v) in row.iter().enumerate() {
            out[j * n_rows + i] = v;
        }
    }
    out
}

/// Reinterpret a column-major `(rows, cols)` buffer back to a `Vec<Vec<f64>>`.
pub fn from_column_major(buf: &[f64], n_rows: usize, n_cols: usize) -> Vec<Vec<f64>> {
    debug_assert_eq!(buf.len(), n_rows * n_cols);
    let mut out = vec![vec![0.0; n_cols]; n_rows];
    for j in 0..n_cols {
        for i in 0..n_rows {
            out[i][j] = buf[j * n_rows + i];
        }
    }
    out
}

fn check_error() -> GrfError {
    let s = unsafe { CStr::from_ptr(ffi::grf_last_error()) };
    let s = s.to_string_lossy().into_owned();
    if s.is_empty() {
        GrfError::Cpp("unknown".into())
    } else {
        GrfError::Cpp(s)
    }
}

#[allow(dead_code)] // used by grf-sys callers (grf crate)
fn ptr_to_string(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

#[allow(dead_code)]
fn cstring(s: &str) -> Result<CString> {
    CString::new(s).map_err(|e| GrfError::InvalidArgument(format!("non-utf8: {e}")))
}

// Re-export of c_double/c_uint types for callers building buffers.
pub type F64 = c_double;
pub type U32 = c_uint;