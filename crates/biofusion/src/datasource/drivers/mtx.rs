//! MatrixMarket coordinate driver.
//!
//! Sparse `.mtx` files are exposed as a long table rather than materializing a
//! dense gene-by-cell matrix. Coordinates retain the Matrix Market convention
//! and are therefore 1-based.

use std::io::BufRead;
use std::sync::Arc;

use arrow::array::{Float64Array, Int64Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use datafusion::error::{DataFusionError, Result};

use super::super::core::{BioBatchStream, BioDriver, BioInput, buf_reader, map_ext, sync_stream};

pub struct MtxDriver;

fn mtx_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("row", DataType::Int64, false),
        Field::new("column", DataType::Int64, false),
        Field::new("value", DataType::Float64, false),
    ]))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MtxField {
    Integer,
    Real,
    Pattern,
}

fn parse_header(line: &str) -> Result<MtxField> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() != 5
        || !parts[0].eq_ignore_ascii_case("%%MatrixMarket")
        || !parts[1].eq_ignore_ascii_case("matrix")
        || !parts[2].eq_ignore_ascii_case("coordinate")
    {
        return Err(DataFusionError::Plan(
            "expected a MatrixMarket coordinate header: \
             %%MatrixMarket matrix coordinate <field> <symmetry>"
                .to_string(),
        ));
    }

    let field = match parts[3].to_ascii_lowercase().as_str() {
        "integer" => MtxField::Integer,
        "real" => MtxField::Real,
        "pattern" => MtxField::Pattern,
        other => {
            return Err(DataFusionError::NotImplemented(format!(
                "unsupported MatrixMarket field type '{other}'; \
                 supported fields are integer, real, and pattern"
            )));
        }
    };

    if !parts[4].eq_ignore_ascii_case("general") {
        return Err(DataFusionError::NotImplemented(format!(
            "unsupported MatrixMarket symmetry '{}'; \
             only general sparse matrices are supported",
            parts[4]
        )));
    }
    Ok(field)
}

fn parse_dimensions(line: &str) -> Result<(usize, usize, usize)> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() != 3 {
        return Err(DataFusionError::Plan(format!(
            "invalid MatrixMarket dimensions line '{line}'; expected \
             '<rows> <columns> <entries>'"
        )));
    }
    let parse = |value: &str, name: &str| {
        value
            .parse::<usize>()
            .map_err(|_| DataFusionError::Plan(format!("invalid MatrixMarket {name} '{value}'")))
    };
    Ok((
        parse(parts[0], "row count")?,
        parse(parts[1], "column count")?,
        parse(parts[2], "entry count")?,
    ))
}

fn parse_entry(line: &str, field: MtxField) -> Result<(i64, i64, f64)> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    let expected = if field == MtxField::Pattern { 2 } else { 3 };
    if parts.len() != expected {
        return Err(DataFusionError::Plan(format!(
            "invalid MatrixMarket {field:?} entry '{line}'; expected {expected} fields"
        )));
    }

    let row = parts[0].parse::<i64>().map_err(|_| {
        DataFusionError::Plan(format!("invalid MatrixMarket row index '{}'", parts[0]))
    })?;
    let column = parts[1].parse::<i64>().map_err(|_| {
        DataFusionError::Plan(format!("invalid MatrixMarket column index '{}'", parts[1]))
    })?;
    if row < 1 || column < 1 {
        return Err(DataFusionError::Plan(
            "MatrixMarket coordinate indices are 1-based and must be positive".to_string(),
        ));
    }

    let value = match field {
        MtxField::Pattern => 1.0,
        MtxField::Integer => parts[2]
            .parse::<i64>()
            .map(|value| value as f64)
            .map_err(|_| {
                DataFusionError::Plan(format!("invalid MatrixMarket integer '{}'", parts[2]))
            })?,
        MtxField::Real => parts[2].parse::<f64>().map_err(|_| {
            DataFusionError::Plan(format!("invalid MatrixMarket real value '{}'", parts[2]))
        })?,
    };
    Ok((row, column, value))
}

fn next_content_line<R: BufRead>(reader: &mut R) -> std::io::Result<Option<String>> {
    loop {
        let mut line = String::new();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 {
            return Ok(None);
        }
        let trimmed = line.trim();
        if !trimmed.is_empty() && (!trimmed.starts_with('%') || trimmed.starts_with("%%")) {
            return Ok(Some(trimmed.to_string()));
        }
    }
}

#[async_trait]
impl BioDriver for MtxDriver {
    const FILE_TYPE: &'static str = "mtx";

    async fn infer_schema(_input: &BioInput) -> Result<SchemaRef> {
        Ok(mtx_schema())
    }

    async fn scan(
        input: BioInput,
        batch_size: usize,
        limit: Option<usize>,
    ) -> Result<BioBatchStream> {
        let bytes = input.fetch_all().await?;
        let mut reader = buf_reader(bytes, input.gz);

        let header_line = next_content_line(&mut reader)
            .map_err(map_ext)?
            .ok_or_else(|| DataFusionError::Plan("empty MatrixMarket file".to_string()))?;
        let field = parse_header(&header_line)?;
        let dimensions = next_content_line(&mut reader)
            .map_err(map_ext)?
            .ok_or_else(|| DataFusionError::Plan("missing MatrixMarket dimensions".to_string()))?;
        let (rows, columns, entries) = parse_dimensions(&dimensions)?;

        let max_entries = limit.unwrap_or(entries).min(entries);
        let batch_size = batch_size.max(1);
        let mut produced = 0usize;
        let mut finished = false;

        let batches = std::iter::from_fn(move || {
            if finished || produced >= max_entries {
                return None;
            }

            let mut batch_rows = Vec::with_capacity(batch_size.min(max_entries - produced));
            let mut batch_columns = Vec::with_capacity(batch_rows.capacity());
            let mut batch_values = Vec::with_capacity(batch_rows.capacity());

            while batch_rows.len() < batch_size && produced < max_entries {
                let line = match next_content_line(&mut reader) {
                    Ok(Some(line)) => line,
                    Ok(None) => {
                        finished = true;
                        return Some(Err(DataFusionError::Plan(format!(
                            "MatrixMarket header declares {entries} entries, but only \
                             {produced} were found"
                        ))));
                    }
                    Err(e) => return Some(Err(map_ext(e))),
                };
                produced += 1;
                let (row, column, value) = match parse_entry(&line, field) {
                    Ok(entry) => entry,
                    Err(e) => {
                        finished = true;
                        return Some(Err(e));
                    }
                };
                if row > rows as i64 {
                    finished = true;
                    return Some(Err(DataFusionError::Plan(format!(
                        "MatrixMarket row index {row} exceeds declared row count {rows}"
                    ))));
                }
                if column > columns as i64 {
                    finished = true;
                    return Some(Err(DataFusionError::Plan(format!(
                        "MatrixMarket column index {column} exceeds declared column count \
                         {columns}"
                    ))));
                }
                batch_rows.push(row);
                batch_columns.push(column);
                batch_values.push(value);
            }

            let batch = RecordBatch::try_new(
                mtx_schema(),
                vec![
                    Arc::new(Int64Array::from(batch_rows)),
                    Arc::new(Int64Array::from(batch_columns)),
                    Arc::new(Float64Array::from(batch_values)),
                ],
            );
            match batch {
                Ok(batch) => Some(Ok(batch)),
                Err(e) => {
                    finished = true;
                    Some(Err(DataFusionError::from(e)))
                }
            }
        })
        .map(|result| {
            result
                .map_err(|e: DataFusionError| arrow::error::ArrowError::ExternalError(Box::new(e)))
        });

        Ok(sync_stream(batches))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ext::DataFusionReadExt;
    use arrow::array::AsArray;
    use datafusion::prelude::SessionContext;

    const SAMPLE: &str = "%%MatrixMarket matrix coordinate real general\n\
% a small sparse matrix\n2 3 4\n1 1 1.5\n1 3 2\n2 1 3\n2 2 -4\n";

    #[tokio::test]
    async fn reads_coordinate_matrix_as_long_table() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.mtx");
        std::fs::write(&path, SAMPLE).unwrap();

        let ctx = SessionContext::new();
        let df = ctx
            .read_mtx(
                path.to_str().unwrap(),
                crate::datasource::BioReadOptions::default(),
            )
            .await
            .unwrap();
        let batches = df.collect().await.unwrap();
        let values = batches[0]
            .column_by_name("value")
            .unwrap()
            .as_primitive::<arrow::datatypes::Float64Type>();
        assert_eq!(
            batches.iter().map(|batch| batch.num_rows()).sum::<usize>(),
            4
        );
        assert_eq!(values.value(0), 1.5);
        assert_eq!(values.value(3), -4.0);
    }

    #[tokio::test]
    async fn rejects_symmetric_coordinate_matrix() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("symmetric.mtx");
        std::fs::write(
            &path,
            "%%MatrixMarket matrix coordinate real symmetric\n2 2 2\n1 1 1\n2 2 2\n",
        )
        .unwrap();

        let ctx = SessionContext::new();
        let err = ctx
            .read_mtx(
                path.to_str().unwrap(),
                crate::datasource::BioReadOptions::default(),
            )
            .await
            .unwrap()
            .collect()
            .await
            .unwrap_err();
        assert!(err.to_string().contains("symmetry"));
    }
}
