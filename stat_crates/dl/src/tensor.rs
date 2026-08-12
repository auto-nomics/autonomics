//! Tensor — a dense matrix wrapper around `faer::Mat<f64>`.
//!
//! Unlike a full autodiff framework, this crate uses **explicit forward/backward
//! per layer** (micrograd style).  The `Tensor` type is simply a shaped matrix
//! with convenience methods — no computation graph is recorded.

use faer::Mat;

/// A 2-D tensor of `f64` values stored in row-major logical layout.
///
/// Internally backed by `faer::Mat<f64>` (column-major), but all public APIs
/// work with `(row, col)` indexing and row-major slices so that callers never
/// need to think about faer's memory layout.
#[derive(Debug, Clone)]
pub struct Tensor {
    pub data: Mat<f64>,
}

impl Tensor {
    /// Create from row-major flat data.
    pub fn from_rows(nrows: usize, ncols: usize, data: &[f64]) -> Self {
        assert_eq!(data.len(), nrows * ncols, "data length mismatch");
        Self {
            data: Mat::from_fn(nrows, ncols, |i, j| data[i * ncols + j]),
        }
    }

    /// Create an all-zeros tensor.
    pub fn zeros(nrows: usize, ncols: usize) -> Self {
        Self {
            data: Mat::zeros(nrows, ncols),
        }
    }

    /// Shape as `(nrows, ncols)`.
    pub fn shape(&self) -> (usize, usize) {
        self.data.shape()
    }

    /// Number of rows.
    pub fn nrows(&self) -> usize {
        self.data.nrows()
    }

    /// Number of columns.
    pub fn ncols(&self) -> usize {
        self.data.ncols()
    }

    /// Element-wise access.
    pub fn at(&self, row: usize, col: usize) -> f64 {
        self.data[(row, col)]
    }

    /// Mutable element-wise access.
    pub fn set(&mut self, row: usize, col: usize, val: f64) {
        self.data[(row, col)] = val;
    }

    /// Extract row `i` as a `Vec<f64>`.
    pub fn row(&self, i: usize) -> Vec<f64> {
        (0..self.ncols()).map(|j| self.data[(i, j)]).collect()
    }

    /// Extract column `j` as a `Vec<f64>`.
    pub fn col(&self, j: usize) -> Vec<f64> {
        (0..self.nrows()).map(|i| self.data[(i, j)]).collect()
    }

    /// Create from a `Mat<f64>`.
    pub fn from_mat(mat: Mat<f64>) -> Self {
        Self { data: mat }
    }

    /// Borrow the underlying `Mat<f64>`.
    pub fn as_mat(&self) -> &Mat<f64> {
        &self.data
    }

    /// Consume into the underlying `Mat<f64>`.
    pub fn into_mat(self) -> Mat<f64> {
        self.data
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Tensor conversion helpers
// ═══════════════════════════════════════════════════════════════════════

/// Build a `Tensor` from a slice of row vectors.
pub fn from_row_vecs(rows: &[Vec<f64>]) -> Tensor {
    let nrows = rows.len();
    let ncols = rows.first().map(|r| r.len()).unwrap_or(0);
    let mut flat = Vec::with_capacity(nrows * ncols);
    for r in rows {
        flat.extend_from_slice(r);
    }
    Tensor::from_rows(nrows, ncols, &flat)
}

/// Convert a `Tensor` back to row-major `Vec<Vec<f64>>`.
pub fn to_row_vecs(t: &Tensor) -> Vec<Vec<f64>> {
    (0..t.nrows())
        .map(|i| t.row(i))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_rows() {
        let t = Tensor::from_rows(2, 3, &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        assert_eq!(t.shape(), (2, 3));
        assert_eq!(t.at(0, 0), 1.0);
        assert_eq!(t.at(1, 2), 6.0);
    }

    #[test]
    fn test_row_col() {
        let t = Tensor::from_rows(3, 2, &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        assert_eq!(t.row(1), vec![3.0, 4.0]);
        assert_eq!(t.col(0), vec![1.0, 3.0, 5.0]);
    }

    #[test]
    fn test_from_row_vecs_roundtrip() {
        let rows = vec![vec![1.0, 2.0], vec![3.0, 4.0]];
        let t = from_row_vecs(&rows);
        assert_eq!(t.shape(), (2, 2));
        let back = to_row_vecs(&t);
        assert_eq!(back, rows);
    }
}
