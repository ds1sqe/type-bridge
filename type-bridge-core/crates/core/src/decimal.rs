//! Canonical TypeDB decimal parsing and semantic comparison.

use std::cmp::Ordering;

/// A validated decimal split into normalized, borrowed components.
///
/// The parser accepts TypeQL decimal text with or without the driver's `dec`
/// suffix. Leading whole-part zeroes, trailing fractional zeroes, and negative
/// zero are normalized so semantic comparison does not depend on spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CanonicalDecimal<'a> {
    negative: bool,
    whole: &'a str,
    fraction: &'a str,
}

impl CanonicalDecimal<'_> {
    /// Compare two validated decimals by numeric value.
    pub fn compare(&self, other: &Self) -> Ordering {
        if self.negative != other.negative {
            return if self.negative {
                Ordering::Less
            } else {
                Ordering::Greater
            };
        }

        let width = self.fraction.len().max(other.fraction.len());
        let magnitude = self
            .whole
            .len()
            .cmp(&other.whole.len())
            .then_with(|| self.whole.cmp(other.whole))
            .then_with(|| {
                self.fraction
                    .bytes()
                    .chain(std::iter::repeat_n(b'0', width - self.fraction.len()))
                    .cmp(
                        other
                            .fraction
                            .bytes()
                            .chain(std::iter::repeat_n(b'0', width - other.fraction.len())),
                    )
            });
        if self.negative {
            magnitude.reverse()
        } else {
            magnitude
        }
    }
}

/// Parse one TypeDB decimal using the canonical TypeQL/driver grammar.
///
/// Decimal magnitudes use a signed 64-bit whole part and at most 19
/// fractional digits. The lowercase driver suffix `dec` is accepted because
/// real concept-document hydration includes it for fractional values.
pub fn parse_decimal(value: &str) -> Option<CanonicalDecimal<'_>> {
    let value = value.strip_suffix("dec").unwrap_or(value);
    let (negative, unsigned) = if let Some(value) = value.strip_prefix('-') {
        (true, value)
    } else {
        (false, value.strip_prefix('+').unwrap_or(value))
    };
    let (raw_whole, raw_fraction) = match unsigned.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (unsigned, None),
    };
    if raw_whole.is_empty()
        || !raw_whole.bytes().all(|byte| byte.is_ascii_digit())
        || raw_fraction.is_some_and(|fraction| {
            fraction.is_empty()
                || fraction.len() > 19
                || !fraction.bytes().all(|byte| byte.is_ascii_digit())
        })
    {
        return None;
    }

    let whole = raw_whole.trim_start_matches('0');
    let whole = if whole.is_empty() { "0" } else { whole };
    let fraction = raw_fraction.unwrap_or_default().trim_end_matches('0');
    let limit = if negative {
        "9223372036854775808"
    } else {
        "9223372036854775807"
    };
    match whole.len().cmp(&limit.len()).then_with(|| whole.cmp(limit)) {
        Ordering::Greater => return None,
        Ordering::Equal if negative && !fraction.is_empty() => return None,
        Ordering::Less | Ordering::Equal => {}
    }

    Some(CanonicalDecimal {
        negative: negative && !(whole == "0" && fraction.is_empty()),
        whole,
        fraction,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_driver_suffix_and_compares_canonical_values() {
        let driver = parse_decimal("001234.5600dec").unwrap();
        let literal = parse_decimal("+1234.56").unwrap();
        assert_eq!(driver, literal);
        assert_eq!(driver.compare(&literal), Ordering::Equal);
        assert_eq!(parse_decimal("-0.000dec"), parse_decimal("0"));
    }

    #[test]
    fn enforces_fraction_width_and_decimal_range() {
        for valid in [
            "-9223372036854775808",
            "-9223372036854775808.0000000000000000000",
            "9223372036854775807.9999999999999999999dec",
        ] {
            assert!(
                parse_decimal(valid).is_some(),
                "expected {valid:?} to parse"
            );
        }
        for invalid in [
            "",
            "dec",
            "1.",
            ".1",
            "1.00000000000000000000",
            "1.0DEC",
            "1.0decdec",
            "9223372036854775808",
            "-9223372036854775808.0000000000000000001",
            "-9223372036854775809",
        ] {
            assert!(
                parse_decimal(invalid).is_none(),
                "expected {invalid:?} to fail"
            );
        }
    }
}
