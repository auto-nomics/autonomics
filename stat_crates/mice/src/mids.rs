//! `Mids` object — the output of [`crate::orchestrator::mice`].
//!
//! Faithful port of R's `mids` class structure (see `R/mids.R` and
//! `R/initialize.imp.R`). Stores the original incomplete data, the per-column
//! imputation matrices (`imp[j]` is `n_mis_j × m`), the number of imputations
//! `m`, the method vector, and the predictor matrix.

use std::collections::HashMap;

/// Per-column imputation storage: `imp[j]` is the `n_mis_j × m` matrix of
/// imputed values for column `j`. Empty (no missing entries) columns have
/// `imp[j].is_empty() == true`.
#[derive(Debug, Clone, Default)]
pub struct ImpList {
    /// `imp[j]` — `n_mis_j × m` flat row-major matrix.
    pub by_column: HashMap<String, Vec<f64>>,
    /// `m` — number of imputations (column count of the matrices).
    pub m: usize,
}

impl ImpList {
    /// Number of rows for column `j` (i.e. `nmis[j]`).
    pub fn nrow(&self, col: &str) -> usize {
        if let Some(v) = self.by_column.get(col) {
            v.len() / self.m.max(1)
        } else {
            0
        }
    }

    /// Imputation `i` (0-indexed) for column `j`, row `r`.
    pub fn get(&self, col: &str, imputation: usize, row: usize) -> Option<f64> {
        self.by_column.get(col).and_then(|v| {
            let ncol = self.m.max(1);
            let idx = row * ncol + imputation;
            v.get(idx).copied()
        })
    }

    /// Set imputation `i` (0-indexed) for column `j`, row `r`.
    ///
    /// Silently ignores if `col` was never initialised.
    pub fn set(&mut self, col: &str, imputation: usize, row: usize, value: f64) {
        let ncol = self.m.max(1);
        let idx = row * ncol + imputation;
        if let Some(v) = self.by_column.get_mut(col) {
            if idx < v.len() {
                v[idx] = value;
            }
        }
    }

    /// Initialise the storage for a column with `n_mis` rows × `m` cols.
    pub fn init_column(&mut self, col: &str, n_mis: usize, m: usize) {
        self.m = m;
        self.by_column.insert(col.to_string(), vec![0.0; n_mis * m]);
    }
}

/// Per-block method specification.
#[derive(Debug, Clone)]
pub struct MethodSpec {
    /// Block name (typically the column name; equals `colnames(data)[j]`).
    pub block: String,
    /// R-style method string (e.g. `"pmm"`, `"norm"`, `""` for empty).
    pub method: String,
}

/// The `mids` object: original data, imputed values, and configuration.
#[derive(Debug, Clone)]
pub struct Mids {
    /// Original (incomplete) data, column-major: `data[col][i]` is the i-th
    /// observation of column `col`.
    pub data: HashMap<String, Vec<f64>>,
    /// Per-column imputation storage (`nmis[j] × m`).
    pub imp: ImpList,
    /// Number of imputations.
    pub m: usize,
    /// Number of Gibbs iterations actually run.
    pub iteration: usize,
    /// Per-column number of missing values in the original data.
    pub nmis: HashMap<String, usize>,
    /// Method per column (empty string == no imputation).
    pub method: HashMap<String, String>,
    /// Predictor matrix: `pred[col][pred_col] == 1` iff `pred_col` is a
    /// predictor of `col` in the univariate model.
    pub predictor_matrix: HashMap<String, HashMap<String, i32>>,
    /// Visit sequence (column names in iteration order).
    pub visit_sequence: Vec<String>,
    /// Per-column observed indicator from the original data.
    pub where_missing: HashMap<String, Vec<bool>>,
    /// Column names in their original order.
    pub column_names: Vec<String>,
}
