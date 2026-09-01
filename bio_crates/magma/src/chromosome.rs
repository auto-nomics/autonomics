//! Chromosome naming shared by MAGMA file readers.

/// Parse a chromosome string to an integer.
///
/// Handles `"1"` through `"22"`, `"X"` as 23, `"Y"` as 24, `"XY"` as 25, and
/// `"MT"`/`"M"` as 26. Unknown values return `None`.
pub(crate) fn parse_chr(s: &str) -> Option<i32> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(chromosome) = s.parse::<i32>() {
        return Some(chromosome);
    }
    match s.to_uppercase().as_str() {
        "X" => Some(23),
        "Y" => Some(24),
        "XY" => Some(25),
        "MT" | "M" => Some(26),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_autosomes_and_sex_chromosomes() {
        assert_eq!(parse_chr("1"), Some(1));
        assert_eq!(parse_chr("22"), Some(22));
        assert_eq!(parse_chr("X"), Some(23));
        assert_eq!(parse_chr("Y"), Some(24));
        assert_eq!(parse_chr("XY"), Some(25));
        assert_eq!(parse_chr("MT"), Some(26));
        assert_eq!(parse_chr("0"), Some(0));
        assert_eq!(parse_chr("abc"), None);
    }
}
