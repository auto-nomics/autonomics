//! Arrow helpers shared by node implementations.

/// Extract string values from an Arrow column, accepting both physical layouts:
/// `Utf8` (`StringArray`) and `Utf8View` (`StringViewArray`).
///
/// DataFusion can emit `Utf8View` for SQL output and Parquet reads, while many
/// older code paths produce `Utf8`. Accepting either layout prevents a
/// physical-layout change from surfacing as a misleading missing-column error.
/// Returns `None` only when the column is not a string type.
pub fn string_opt_values(arr: &dyn arrow_array::Array) -> Option<Vec<Option<String>>> {
    use arrow_array::{Array as _, StringArray, StringViewArray};

    if let Some(values) = arr.as_any().downcast_ref::<StringArray>() {
        Some(
            (0..values.len())
                .map(|index| {
                    values
                        .is_null(index)
                        .then(|| values.value(index).to_string())
                })
                .collect(),
        )
    } else {
        arr.as_any()
            .downcast_ref::<StringViewArray>()
            .map(|values| {
                (0..values.len())
                    .map(|index| {
                        values
                            .is_null(index)
                            .then(|| values.value(index).to_string())
                    })
                    .collect()
            })
    }
}
