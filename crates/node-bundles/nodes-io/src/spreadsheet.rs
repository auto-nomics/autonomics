//! Excel workbook-to-RecordBatch conversion for `file_to_dataframe`.

use std::collections::HashSet;
use std::io::Cursor;
use std::sync::Arc;

use arrow_array::{ArrayRef, BooleanArray, Float64Array, Int64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use calamine::{Data, DataType as CellDataType, Reader, open_workbook_auto_from_rs};
use datafusion::datasource::listing::ListingTableUrl;
use datafusion::error::DataFusionError;
use datafusion::object_store::ObjectStoreExt;
use datafusion::prelude::{DataFrame, SessionContext};

pub(crate) async fn read_spreadsheet(
    ctx: &SessionContext,
    path: &str,
    sheet_name: Option<&str>,
    has_header: bool,
) -> Result<DataFrame, DataFusionError> {
    let bytes = read_object_bytes(ctx, path).await?;
    let sheet_name = sheet_name.map(ToOwned::to_owned);
    let batch = tokio::task::spawn_blocking(move || {
        spreadsheet_to_batch(bytes, sheet_name.as_deref(), has_header)
    })
    .await
    .map_err(|error| DataFusionError::Execution(format!("Excel reader task failed: {error}")))?;

    ctx.read_batch(batch?)
}

async fn read_object_bytes(ctx: &SessionContext, path: &str) -> Result<Vec<u8>, DataFusionError> {
    let table_url = ListingTableUrl::parse(path).map_err(|source| {
        DataFusionError::Execution(format!("invalid Excel path `{path}`: {source}"))
    })?;
    let store = ctx.runtime_env().object_store(&table_url)?;
    let object = store
        .get(table_url.prefix())
        .await
        .map_err(|source| DataFusionError::ObjectStore(Box::new(source)))?;
    let bytes = object
        .bytes()
        .await
        .map_err(|source| DataFusionError::ObjectStore(Box::new(source)))?;
    Ok(bytes.to_vec())
}

fn spreadsheet_to_batch(
    bytes: Vec<u8>,
    sheet_name: Option<&str>,
    has_header: bool,
) -> Result<RecordBatch, DataFusionError> {
    let mut workbook = open_workbook_auto_from_rs(Cursor::new(bytes))
        .map_err(|error| DataFusionError::Execution(format!("invalid Excel workbook: {error}")))?;
    let sheet_names = workbook.sheet_names();
    let selected_sheet = match sheet_name {
        Some(name) => {
            if !sheet_names.iter().any(|candidate| candidate == name) {
                return Err(DataFusionError::Execution(format!(
                    "worksheet `{name}` not found; available worksheets: {}",
                    sheet_names.join(", ")
                )));
            }
            name.to_string()
        }
        None => sheet_names
            .first()
            .ok_or_else(|| {
                DataFusionError::Execution("Excel workbook contains no worksheets".to_string())
            })?
            .clone(),
    };
    let range = workbook.worksheet_range(&selected_sheet).map_err(|error| {
        DataFusionError::Execution(format!(
            "failed to read worksheet `{selected_sheet}`: {error}"
        ))
    })?;
    let mut rows: Vec<Vec<Data>> = range.rows().map(|row| row.to_vec()).collect();
    if rows.is_empty() {
        return Ok(RecordBatch::new_empty(Arc::new(Schema::empty())));
    }

    let width = rows.iter().map(Vec::len).max().unwrap_or(0);
    if width == 0 {
        return Ok(RecordBatch::new_empty(Arc::new(Schema::empty())));
    }
    let names = if has_header {
        let header = rows.remove(0);
        header_names(&header, width)
    } else {
        (0..width)
            .map(|index| format!("column_{}", index + 1))
            .collect::<Vec<_>>()
    };
    let types: Vec<DataType> = (0..width)
        .map(|column| column_data_type(&rows, column))
        .collect();

    let columns: Vec<ArrayRef> = types
        .iter()
        .enumerate()
        .map(|(column, data_type)| build_column(&rows, column, data_type))
        .collect();
    let fields: Vec<Field> = names
        .into_iter()
        .zip(types)
        .map(|(name, data_type)| Field::new(name, data_type, true))
        .collect();
    RecordBatch::try_new(Arc::new(Schema::new(fields)), columns).map_err(|error| {
        DataFusionError::Execution(format!("failed to build Excel RecordBatch: {error}"))
    })
}

fn build_column(rows: &[Vec<Data>], column: usize, data_type: &DataType) -> ArrayRef {
    match data_type {
        DataType::Boolean => {
            let values = rows
                .iter()
                .map(|row| match row.get(column) {
                    Some(Data::Bool(value)) => Some(*value),
                    _ => None,
                })
                .collect::<Vec<_>>();
            Arc::new(BooleanArray::from(values)) as ArrayRef
        }
        DataType::Int64 => {
            let values = rows
                .iter()
                .map(|row| match row.get(column) {
                    Some(Data::Int(value)) => Some(*value),
                    Some(Data::Float(value)) if value.is_finite() => Some(*value as i64),
                    _ => None,
                })
                .collect::<Vec<_>>();
            Arc::new(Int64Array::from(values)) as ArrayRef
        }
        DataType::Float64 => {
            let values = rows
                .iter()
                .map(|row| match row.get(column) {
                    Some(Data::Int(value)) => Some(*value as f64),
                    Some(Data::Float(value)) => Some(*value),
                    _ => None,
                })
                .collect::<Vec<_>>();
            Arc::new(Float64Array::from(values)) as ArrayRef
        }
        _ => {
            let values = rows
                .iter()
                .map(|row| row.get(column).and_then(data_to_text))
                .collect::<Vec<_>>();
            Arc::new(StringArray::from(values)) as ArrayRef
        }
    }
}

fn header_names(header: &[Data], width: usize) -> Vec<String> {
    let mut names = Vec::with_capacity(width);
    let mut seen = HashSet::new();
    for (index, cell) in header.iter().take(width).enumerate() {
        let name = data_to_text(cell)
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| format!("column_{}", index + 1));
        names.push(unique_name(name, &mut seen));
    }
    for index in header.len()..width {
        names.push(unique_name(format!("column_{}", index + 1), &mut seen));
    }
    names
}

fn unique_name(mut name: String, seen: &mut HashSet<String>) -> String {
    let base = name.clone();
    let mut suffix = 1;
    while !seen.insert(name.clone()) {
        name = format!("{base}_{suffix}");
        suffix += 1;
    }
    name
}

fn column_data_type(rows: &[Vec<Data>], column: usize) -> DataType {
    let mut inferred: Option<CellKind> = None;
    for row in rows {
        let Some(cell) = row.get(column) else {
            continue;
        };
        if cell.is_empty() {
            continue;
        }
        let kind = match cell {
            Data::Bool(_) => CellKind::Bool,
            Data::Int(_) => CellKind::Int,
            Data::Float(value) => {
                let integer = value.is_finite()
                    && value.fract() == 0.0
                    && *value >= i64::MIN as f64
                    && *value <= i64::MAX as f64;
                if integer {
                    CellKind::Int
                } else {
                    CellKind::Float
                }
            }
            Data::String(_)
            | Data::DateTime(_)
            | Data::DateTimeIso(_)
            | Data::DurationIso(_)
            | Data::Error(_) => CellKind::Text,
            Data::Empty => continue,
        };
        inferred = Some(match inferred {
            Some(existing) => existing.merge(kind),
            None => kind,
        });
    }

    match inferred {
        Some(CellKind::Bool) => DataType::Boolean,
        Some(CellKind::Int) => DataType::Int64,
        Some(CellKind::Float) => DataType::Float64,
        Some(CellKind::Text) | None => DataType::Utf8,
    }
}

fn data_to_text(cell: &Data) -> Option<String> {
    match cell {
        Data::String(value) => Some(value.clone()),
        Data::Int(value) => Some(value.to_string()),
        Data::Float(value) => Some(value.to_string()),
        Data::Bool(value) => Some(value.to_string()),
        Data::DateTimeIso(value) | Data::DurationIso(value) => Some(value.clone()),
        Data::Error(error) => Some(error.to_string()),
        Data::DateTime(value) => {
            if value.is_duration() {
                let duration = value.as_duration()?;
                let milliseconds = duration.num_milliseconds();
                Some(format!(
                    "PT{}.{:03}S",
                    milliseconds / 1000,
                    milliseconds % 1000
                ))
            } else {
                value.as_datetime().map(|datetime| datetime.to_string())
            }
        }
        _ => None,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CellKind {
    Bool,
    Int,
    Float,
    Text,
}

impl CellKind {
    fn merge(self, other: Self) -> Self {
        match (self, other) {
            (Self::Bool, Self::Bool) => Self::Bool,
            (Self::Int, Self::Int) => Self::Int,
            (Self::Int, Self::Float) | (Self::Float, Self::Int | Self::Float) => Self::Float,
            _ => Self::Text,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excel_headers_are_filled_and_deduplicated() {
        let header = vec![
            Data::String("id".into()),
            Data::Empty,
            Data::String("id".into()),
            Data::Int(9),
        ];

        assert_eq!(header_names(&header, 4), ["id", "column_2", "id_1", "9"]);
    }

    #[test]
    fn excel_numeric_columns_promote_consistently() {
        let int_rows = vec![vec![Data::Int(1)], vec![Data::Int(2)]];
        let float_rows = vec![vec![Data::Int(1)], vec![Data::Float(2.5)]];
        let text_rows = vec![vec![Data::Int(1)], vec![Data::String("two".into())]];

        assert_eq!(column_data_type(&int_rows, 0), DataType::Int64);
        assert_eq!(column_data_type(&float_rows, 0), DataType::Float64);
        assert_eq!(column_data_type(&text_rows, 0), DataType::Utf8);
    }
}
