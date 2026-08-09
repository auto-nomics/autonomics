//! `complete` — extract completed datasets from a [`Mids`] object.
//!
//! Faithful port of `R/complete.R` for the actions we expose:
//!
//! * `"first"` (alias `"1"`) — return the data with the 1st imputation filled
//!   in (single data frame).
//! * `"all"` — return an `m` × `n` stack of imputed datasets in long form:
//!   columns `(.imp, .id, original cols...)`.
//! * `"broad"` — return a wide form where each column is suffixed with
//!   `.imp{i}`, one set per imputation.

use crate::mids::Mids;

/// Output format selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompleteFormat {
    /// `action = "first"` — single completed dataset, original column order.
    First,
    /// `action = "all"` — long format with `.imp` and `.id` columns prepended.
    All,
    /// `action = "broad"` — wide format with columns suffixed `.imp{i}`.
    Broad,
}

impl CompleteFormat {
    /// Parse R-style action argument.
    pub fn from_action(action: &str) -> Option<Self> {
        match action.to_ascii_lowercase().as_str() {
            "1" | "first" => Some(Self::First),
            "all" => Some(Self::All),
            "broad" => Some(Self::Broad),
            _ => None,
        }
    }
}

/// A completed dataset — either a single frame or multiple stacked frames.
///
/// When [`CompleteFormat::First`], `columns` and `rows` describe a single
/// data frame. When [`All`] or [`Broad`], they describe a long/broad table
/// where `rows` are organised by imputation index first.
#[derive(Debug, Clone)]
pub struct CompletedData {
    pub columns: Vec<String>,
    /// Row-major flat: `rows[r * columns.len() + c]` is column `c` of row `r`.
    pub rows: Vec<f64>,
}

/// Reproduce R's `complete(mids, action)` for the formats listed above.
///
/// `imputation` is 1-indexed for `CompleteFormat::First`; ignored for the
/// other formats.
pub fn complete(mids: &Mids, format: CompleteFormat, imputation: usize) -> CompletedData {
    match format {
        CompleteFormat::First => {
            // For each row r:
            //   if original observed, use data[col][r];
            //   else use imp[col][r, imputation - 1].
            let cols = mids.column_names.clone();
            let mut rows = Vec::with_capacity(mids.data[&cols[0]].len() * cols.len());
            for r_idx in 0..mids.data[&cols[0]].len() {
                for col in &cols {
                    let observed = mids.where_missing[col][r_idx];
                    let val = if observed {
                        mids.data[col][r_idx]
                    } else {
                        // Find the position of r_idx within the imp matrix.
                        let wy_positions: Vec<usize> = mids.where_missing[col]
                            .iter()
                            .enumerate()
                            .filter(|(_, ro)| !**ro)
                            .map(|(i, _)| i)
                            .collect();
                        let row_in_imp = wy_positions.iter().position(|&x| x == r_idx).unwrap_or(0);
                        mids.imp
                            .get(col, imputation.saturating_sub(1), row_in_imp)
                            .unwrap_or(0.0)
                    };
                    rows.push(val);
                }
            }
            CompletedData {
                columns: cols,
                rows,
            }
        }
        CompleteFormat::All => {
            // Long format: for each imputation i (1..=m), for each row r,
            // emit (imp = i, id = r+1, col values...).
            let cols = mids.column_names.clone();
            let n = mids.data[&cols[0]].len();
            let mut columns = vec![".imp".to_string(), ".id".to_string()];
            columns.extend(cols.iter().cloned());
            let mut rows = Vec::with_capacity(mids.m * n * columns.len());
            for i in 0..mids.m {
                for r_idx in 0..n {
                    rows.push((i + 1) as f64);
                    rows.push((r_idx + 1) as f64);
                    for col in &cols {
                        let observed = mids.where_missing[col][r_idx];
                        let val = if observed {
                            mids.data[col][r_idx]
                        } else {
                            let wy_positions: Vec<usize> = mids.where_missing[col]
                                .iter()
                                .enumerate()
                                .filter(|(_, ro)| !**ro)
                                .map(|(i, _)| i)
                                .collect();
                            let row_in_imp =
                                wy_positions.iter().position(|&x| x == r_idx).unwrap_or(0);
                            mids.imp.get(col, i, row_in_imp).unwrap_or(0.0)
                        };
                        rows.push(val);
                    }
                }
            }
            CompletedData { columns, rows }
        }
        CompleteFormat::Broad => {
            // Wide format: original columns repeated `m` times, suffixed
            // `.imp{i}`. Order: col1.1, col1.2, ..., col1.m, col2.1, ...
            let cols = mids.column_names.clone();
            let n = mids.data[&cols[0]].len();
            let mut columns = Vec::new();
            for col in &cols {
                for i in 0..mids.m {
                    columns.push(format!("{}.imp{}", col, i + 1));
                }
            }
            let mut rows = Vec::with_capacity(n * columns.len());
            for r_idx in 0..n {
                for col in &cols {
                    let observed = mids.where_missing[col][r_idx];
                    for i in 0..mids.m {
                        let val = if observed {
                            mids.data[col][r_idx]
                        } else {
                            let wy_positions: Vec<usize> = mids.where_missing[col]
                                .iter()
                                .enumerate()
                                .filter(|(_, ro)| !**ro)
                                .map(|(i, _)| i)
                                .collect();
                            let row_in_imp =
                                wy_positions.iter().position(|&x| x == r_idx).unwrap_or(0);
                            mids.imp.get(col, i, row_in_imp).unwrap_or(0.0)
                        };
                        rows.push(val);
                    }
                }
            }
            CompletedData { columns, rows }
        }
    }
}
