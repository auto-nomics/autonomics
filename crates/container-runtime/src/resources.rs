//! Shared resource quantities for runtime flags and scheduler declarations.

use crate::ContainerRuntimeError;

/// Parse the memory dialect used by container nodes into exact whole bytes.
/// Decimal SI and binary IEC/Kubernetes suffixes retain distinct meanings.
/// Fractional bytes are truncated as by the runtime's existing CLI contract;
/// a result below one byte or outside `u64` is rejected. Decimal arithmetic
/// avoids rounding an explicit resource budget through a floating point value.
pub fn memory_quantity_bytes(value: &str) -> Result<u64, ContainerRuntimeError> {
    let invalid = || {
        ContainerRuntimeError::Invalid(format!(
            "`{value}` is not a supported container memory quantity"
        ))
    };
    let split = value
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(value.len());
    let (number, suffix) = value.split_at(split);
    let multiplier: u128 = match suffix.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "k" | "kb" => 1_000,
        "m" | "mb" => 1_000_000,
        "g" | "gb" => 1_000_000_000,
        "t" | "tb" => 1_000_000_000_000,
        "ki" | "kib" => 1 << 10,
        "mi" | "mib" => 1 << 20,
        "gi" | "gib" => 1 << 30,
        "ti" | "tib" => 1 << 40,
        _ => return Err(invalid()),
    };
    let (integer, fraction) = number.split_once('.').unwrap_or((number, ""));
    if (integer.is_empty() && fraction.is_empty())
        || !integer
            .bytes()
            .chain(fraction.bytes())
            .all(|c| c.is_ascii_digit())
    {
        return Err(invalid());
    }
    let denominator = 10_u128
        .checked_pow(u32::try_from(fraction.len()).map_err(|_| invalid())?)
        .ok_or_else(invalid)?;
    let integer: u128 = if integer.is_empty() {
        0
    } else {
        integer.parse().map_err(|_| invalid())?
    };
    let fraction: u128 = if fraction.is_empty() {
        0
    } else {
        fraction.parse().map_err(|_| invalid())?
    };
    let numerator = integer
        .checked_mul(denominator)
        .and_then(|n| n.checked_add(fraction))
        .ok_or_else(invalid)?;
    let bytes = numerator.checked_mul(multiplier).ok_or_else(invalid)? / denominator;
    let bytes = u64::try_from(bytes).map_err(|_| invalid())?;
    if bytes == 0 {
        return Err(invalid());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_quantities_preserve_exact_budget_boundaries() {
        for (quantity, expected) in [
            ("6Gi", 6 * (1 << 30)),
            ("6GB", 6_000_000_000),
            ("1.5GiB", 1_610_612_736),
            (".5Ki", 512),
            ("1.9B", 1),
            ("9007199254740993", 9_007_199_254_740_993),
            ("18446744073709551615", u64::MAX),
        ] {
            assert_eq!(
                memory_quantity_bytes(quantity).unwrap(),
                expected,
                "{quantity}"
            );
        }
        for invalid in [
            "",
            "0Gi",
            "0.1B",
            "-1Gi",
            "NaN",
            "1e3",
            "1..0Gi",
            "1PB",
            "18446744073709551616",
        ] {
            assert!(memory_quantity_bytes(invalid).is_err(), "{invalid}");
        }
    }
}
