//! Column-major ↔ row-major matrix conversion utilities for grf input.
//!
//! GRF's C++ core reads matrices in column-major (Fortran) order:
//! `data[col * n_rows + row]` is element `(row, col)`. This is the same as
//! Eigen's default column-major layout.
//!
//! Most callers in this repo work with Arrow `RecordBatch`es that are
//! row-major. These helpers provide cheap conversion paths so the per-cell
//! cost is just an `O(n*p)` memcpy with no intermediate `Vec<Vec<f64>>`
//! (which would force heap-per-column and defeat cache locality).

/// A flat column-major buffer plus its dimensions. Cheap to clone (just
/// bumps the `Vec<f64>` refcount) and `Send`.
#[derive(Debug, Clone)]
pub struct Matrix {
    /// Length must equal `n_rows * n_cols`.
    pub data: Vec<f64>,
    pub n_rows: usize,
    pub n_cols: usize,
}

impl Matrix {
    pub fn empty() -> Self {
        Self {
            data: Vec::new(),
            n_rows: 0,
            n_cols: 0,
        }
    }

    /// Build a column-major matrix from a row-major row-of-vectors input.
    pub fn from_rows(rows: &[Vec<f64>]) -> Self {
        let n_rows = rows.len();
        let n_cols = if n_rows == 0 { 0 } else { rows[0].len() };
        let mut data = vec![0.0; n_rows * n_cols];
        for (i, row) in rows.iter().enumerate() {
            debug_assert_eq!(row.len(), n_cols);
            for (j, &v) in row.iter().enumerate() {
                data[j * n_rows + i] = v;
            }
        }
        Self {
            data,
            n_rows,
            n_cols,
        }
    }

    /// Flatten a flat column-major `&[f64]` into the proper Matrix shape.
    pub fn from_column_major(data: Vec<f64>, n_rows: usize, n_cols: usize) -> Self {
        debug_assert_eq!(data.len(), n_rows * n_cols);
        Self {
            data,
            n_rows,
            n_cols,
        }
    }

    /// View a column-major matrix as row-of-vectors. Useful for emitting
    /// Arrow RecordBatches.
    pub fn to_rows(&self) -> Vec<Vec<f64>> {
        let mut out = vec![vec![0.0; self.n_cols]; self.n_rows];
        for j in 0..self.n_cols {
            for i in 0..self.n_rows {
                out[i][j] = self.data[j * self.n_rows + i];
            }
        }
        out
    }

    /// Append a new column (length == n_rows) to the matrix, returning a
    /// new matrix. Useful for the R-learner "add Y/W centered columns to X".
    pub fn with_column(&self, col: &[f64]) -> Self {
        assert_eq!(col.len(), self.n_rows, "new column must be n_rows long");
        let mut data = self.data.clone();
        data.extend_from_slice(col);
        Self {
            data,
            n_rows: self.n_rows,
            n_cols: self.n_cols + 1,
        }
    }

    /// View a single column as a slice (no copy).
    pub fn column(&self, j: usize) -> &[f64] {
        let start = j * self.n_rows;
        &self.data[start..start + self.n_rows]
    }

    /// View a single row as a slice (no copy).
    pub fn row(&self, i: usize) -> Vec<f64> {
        (0..self.n_cols)
            .map(|j| self.data[j * self.n_rows + i])
            .collect()
    }
}

/// Free-function form of `Matrix::from_rows`.
pub fn column_major(rows: &[Vec<f64>]) -> Vec<f64> {
    Matrix::from_rows(rows).data
}

/// Free-function form of `Matrix::to_rows`-equivalent.
pub fn from_column_major(buf: &[f64], n_rows: usize, n_cols: usize) -> Vec<Vec<f64>> {
    Matrix::from_column_major(buf.to_vec(), n_rows, n_cols).to_rows()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let rows = vec![vec![1.0, 2.0, 3.0], vec![4.0, 5.0, 6.0]];
        let m = Matrix::from_rows(&rows);
        // Column-major: [1, 4, 2, 5, 3, 6]
        assert_eq!(m.data, vec![1.0, 4.0, 2.0, 5.0, 3.0, 6.0]);
        assert_eq!(m.n_rows, 2);
        assert_eq!(m.n_cols, 3);
        assert_eq!(m.to_rows(), rows);
        assert_eq!(m.column(1), &[2.0, 5.0]);
        assert_eq!(m.row(0), vec![1.0, 2.0, 3.0]);
    }

    #[test]
    fn with_column() {
        let rows = vec![vec![1.0, 2.0], vec![3.0, 4.0]];
        let m = Matrix::from_rows(&rows);
        let m2 = m.with_column(&[10.0, 20.0]);
        assert_eq!(m2.n_cols, 3);
        assert_eq!(m2.column(2), &[10.0, 20.0]);
    }
}
