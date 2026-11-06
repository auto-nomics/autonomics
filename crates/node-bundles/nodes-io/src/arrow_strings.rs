//! Shared helpers for reading Arrow string columns regardless of which
//! string representation a column uses.

use arrow_array::{Array, LargeStringArray, StringArray, StringViewArray};
use arrow_schema::DataType;

/// Read one array as UTF-8 strings, accepting every string layout Arrow
/// uses for text columns (`Utf8`, `LargeUtf8`, `Utf8View`).
///
/// DataFusion's string functions (e.g. `concat`) return `Utf8View`, so
/// nodes reading an ID column from an upstream SQL node must not assume
/// `StringArray`. Returns `None` when the array is not a string type.
pub(crate) fn utf8_strings(array: &dyn Array) -> Option<Vec<Option<&str>>> {
    macro_rules! collect {
        ($ty:ty) => {{
            let values = array.as_any().downcast_ref::<$ty>()?;
            Some(
                (0..values.len())
                    .map(|i| (!values.is_null(i)).then(|| values.value(i)))
                    .collect(),
            )
        }};
    }
    match array.data_type() {
        DataType::Utf8 => collect!(StringArray),
        DataType::LargeUtf8 => collect!(LargeStringArray),
        DataType::Utf8View => collect!(StringViewArray),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_all_string_layouts_identically() {
        let rows = vec![Some("1_1_A_T"), None, Some("longer-than-12-bytes")];
        let expected: Vec<Option<&str>> = rows.clone();
        let utf8_arr = StringArray::from(rows.clone());
        let large_arr = LargeStringArray::from(rows.clone());
        let view_arr = StringViewArray::from(rows);
        let utf8 = utf8_strings(&utf8_arr).unwrap();
        let large = utf8_strings(&large_arr).unwrap();
        let view = utf8_strings(&view_arr).unwrap();
        assert_eq!(utf8, expected);
        assert_eq!(large, expected);
        assert_eq!(view, expected);
    }

    #[test]
    fn non_string_array_is_rejected() {
        let array = arrow_array::Int32Array::from(vec![1, 2]);
        assert!(utf8_strings(&array).is_none());
    }
}
