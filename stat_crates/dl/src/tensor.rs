//! Tensor — a dense 2-D matrix of `f64` values backed by a flat `Vec<f64>`.
//!
//! This is the **public API tensor** used at the boundary between the `dl`
//! crate and the DAG node layer (`nodes-dl`). Internally, model training and
//! inference use Burn tensors (`Tensor<BurnBackend, D>`); conversions happen
//! in [`crate::data`].

// ─── Tensor ────────────────────────────────────────────────────────────

/// A row-major 2-D tensor of `f64` values.
#[derive(Debug, Clone)]
pub struct Tensor {
    pub(crate) data: Vec<f64>,
    pub(crate) nrows: usize,
    pub(crate) ncols: usize,
}

impl Tensor {
    /// Create from row-major flat data.
    pub fn from_rows(nrows: usize, ncols: usize, data: &[f64]) -> Self {
        assert_eq!(data.len(), nrows * ncols, "data length mismatch");
        Self { data: data.to_vec(), nrows, ncols }
    }

    /// All-zeros tensor.
    pub fn zeros(nrows: usize, ncols: usize) -> Self {
        Self { data: vec![0.0; nrows * ncols], nrows, ncols }
    }

    /// Shape `(nrows, ncols)`.
    pub fn shape(&self) -> (usize, usize) {
        (self.nrows, self.ncols)
    }

    /// Number of rows.
    pub fn nrows(&self) -> usize { self.nrows }

    /// Number of columns.
    pub fn ncols(&self) -> usize { self.ncols }

    /// Element access.
    pub fn at(&self, row: usize, col: usize) -> f64 {
        self.data[row * self.ncols + col]
    }

    /// Mutable element access.
    pub fn set(&mut self, row: usize, col: usize, val: f64) {
        self.data[row * self.ncols + col] = val;
    }

    /// Row `i` as `Vec<f64>`.
    pub fn row(&self, i: usize) -> Vec<f64> {
        (0..self.ncols).map(|j| self.at(i, j)).collect()
    }

    /// Column `j` as `Vec<f64>`.
    pub fn col(&self, j: usize) -> Vec<f64> {
        (0..self.nrows).map(|i| self.at(i, j)).collect()
    }

    /// Flat row-major data.
    pub fn as_flat(&self) -> &[f64] {
        &self.data
    }
}

// ─── Free helpers ──────────────────────────────────────────────────────

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

/// Convert a `Tensor` to row-major `Vec<Vec<f64>>`.
pub fn to_row_vecs(t: &Tensor) -> Vec<Vec<f64>> {
    (0..t.nrows()).map(|i| t.row(i)).collect()
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
        assert_eq!(to_row_vecs(&t), rows);
    }

    #[test]
    fn test_zeros() {
        let t = Tensor::zeros(3, 4);
        assert_eq!(t.shape(), (3, 4));
        for i in 0..3 {
            for j in 0..4 {
                assert_eq!(t.at(i, j), 0.0);
            }
        }
    }
}
