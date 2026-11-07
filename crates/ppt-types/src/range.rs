//! Slide-range strings: `"1,4-5"` → page numbers (Slidev `parseRangeString`).

use std::fmt;

use thiserror::Error;

/// Parse a 1-based, inclusive, comma-separated range into page indices
/// (0-based). Duplicates are removed; order follows the input order.
///
/// Errors when any page is out of `[1, total]` or a range is inverted.
pub fn parse_range_pages(raw: &str, total: usize) -> Result<Vec<usize>, ParseRangeError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(ParseRangeError::Empty);
    }
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for part in raw.split(',') {
        let part = part.trim();
        if part.is_empty() {
            return Err(ParseRangeError::EmptySegment);
        }
        if let Some((a, b)) = part.split_once('-') {
            let a: usize = a
                .trim()
                .parse()
                .map_err(|_| ParseRangeError::BadNumber(part.to_string()))?;
            let b: usize = b
                .trim()
                .parse()
                .map_err(|_| ParseRangeError::BadNumber(part.to_string()))?;
            if a == 0 || b == 0 || a > b {
                return Err(ParseRangeError::InvertedRange(part.to_string()));
            }
            if b > total {
                return Err(ParseRangeError::OutOfRange {
                    page: b,
                    total,
                });
            }
            for p in a..=b {
                if seen.insert(p) {
                    out.push(p - 1);
                }
            }
        } else {
            let p: usize = part
                .parse()
                .map_err(|_| ParseRangeError::BadNumber(part.to_string()))?;
            if p == 0 {
                return Err(ParseRangeError::InvertedRange(part.to_string()));
            }
            if p > total {
                return Err(ParseRangeError::OutOfRange { page: p, total });
            }
            if seen.insert(p) {
                out.push(p - 1);
            }
        }
    }
    Ok(out)
}

#[derive(Debug, Error, PartialEq)]
pub enum ParseRangeError {
    #[error("empty page range")]
    Empty,
    #[error("empty segment in page range")]
    EmptySegment,
    #[error("bad number in page range: {0}")]
    BadNumber(String),
    #[error("invalid or inverted page range segment: {0}")]
    InvertedRange(String),
    #[error("page {page} out of range (deck has {total} slides)")]
    OutOfRange { page: usize, total: usize },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_and_ranges() {
        assert_eq!(parse_range_pages("1", 5).unwrap(), vec![0]);
        assert_eq!(parse_range_pages("1,4-5", 5).unwrap(), vec![0, 3, 4]);
        assert_eq!(parse_range_pages("2-4", 5).unwrap(), vec![1, 2, 3]);
        // duplicates collapse
        assert_eq!(parse_range_pages("1,1,2", 5).unwrap(), vec![0, 1]);
        // whitespace tolerated
        assert_eq!(parse_range_pages(" 1 , 3 - 4 ", 5).unwrap(), vec![0, 2, 3]);
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(parse_range_pages("", 5), Err(ParseRangeError::Empty));
        assert_eq!(
            parse_range_pages("1,,2", 5),
            Err(ParseRangeError::EmptySegment)
        );
        assert_eq!(
            parse_range_pages("x", 5),
            Err(ParseRangeError::BadNumber("x".into()))
        );
        assert_eq!(
            parse_range_pages("5-2", 5),
            Err(ParseRangeError::InvertedRange("5-2".into()))
        );
        assert_eq!(
            parse_range_pages("0", 5),
            Err(ParseRangeError::InvertedRange("0".into()))
        );
        assert_eq!(
            parse_range_pages("6", 5),
            Err(ParseRangeError::OutOfRange { page: 6, total: 5 })
        );
    }
}
