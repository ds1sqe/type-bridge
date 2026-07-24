//! Bounded canonical JSON encoding and fail-closed decoding.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::diagnostic::{Diagnostic, DiagnosticCategory};
use crate::limits::{CANONICAL_CODEC_LIMITS, CodecLimits};

/// A format version owned by a later schema/query/migration envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FormatVersion(u16);

impl FormatVersion {
    /// Initial version value for owning formats.
    pub const V1: Self = Self(1);
    /// Preserve an unvalidated raw version.
    pub const fn from_raw(value: u16) -> Self {
        Self(value)
    }
    /// Return the raw number.
    pub const fn get(self) -> u16 {
        self.0
    }
}

/// Version of the canonical JSON codec itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CodecVersion(u16);

impl CodecVersion {
    /// The Phase 1 canonical JSON codec.
    pub const V1: Self = Self(1);
    /// Preserve an unvalidated raw version.
    pub const fn from_raw(value: u16) -> Self {
        Self(value)
    }
    /// Return the raw number.
    pub const fn get(self) -> u16 {
        self.0
    }
}

/// Require an exact owning-format version before payload construction.
pub fn ensure_format_version(
    actual: FormatVersion,
    supported: FormatVersion,
) -> Result<(), Diagnostic> {
    if actual == supported {
        Ok(())
    } else {
        Err(Diagnostic::stable(
            DiagnosticCategory::InvalidContract,
            "unsupported_format_version",
            "contract format version is not supported",
        )
        .with_detail("actual", i64::from(actual.get()))
        .with_detail("supported", i64::from(supported.get())))
    }
}

/// Require an exact codec version before payload construction.
pub fn ensure_codec_version(
    actual: CodecVersion,
    supported: CodecVersion,
) -> Result<(), Diagnostic> {
    if actual == supported {
        Ok(())
    } else {
        Err(Diagnostic::stable(
            DiagnosticCategory::InvalidContract,
            "unsupported_codec_version",
            "canonical codec version is not supported",
        )
        .with_detail("actual", i64::from(actual.get()))
        .with_detail("supported", i64::from(supported.get())))
    }
}

/// Encode one value to compact, key-sorted canonical JSON bytes.
pub fn to_canonical_json<T: Serialize>(value: &T) -> Result<Vec<u8>, Diagnostic> {
    to_canonical_json_with_limits(value, CANONICAL_CODEC_LIMITS)
}

/// Encode one value under explicit structural limits.
pub fn to_canonical_json_with_limits<T: Serialize>(
    value: &T,
    limits: CodecLimits,
) -> Result<Vec<u8>, Diagnostic> {
    let mut value = serde_json::to_value(value).map_err(|_| {
        Diagnostic::stable(
            DiagnosticCategory::InvalidContract,
            "canonical_json_encode_failed",
            "value cannot be represented as canonical JSON",
        )
    })?;
    normalize_numbers(&mut value).map_err(|()| {
        Diagnostic::stable(
            DiagnosticCategory::InvalidContract,
            "canonical_json_encode_failed",
            "value contains a number outside the canonical JSON domain",
        )
    })?;
    sort_object_keys(&mut value);
    inspect(&value, 1, limits)?;
    let bytes = serde_json::to_vec(&value).map_err(|_| {
        Diagnostic::stable(
            DiagnosticCategory::InvalidContract,
            "canonical_json_encode_failed",
            "value cannot be encoded as canonical JSON",
        )
    })?;
    ensure_bytes(bytes.len(), limits)?;
    Ok(bytes)
}

/// Decode only exact canonical bytes, checking limits before constructing `T`.
pub fn from_canonical_json<T>(bytes: &[u8]) -> Result<T, Diagnostic>
where
    T: DeserializeOwned + Serialize,
{
    from_canonical_json_with_limits(bytes, CANONICAL_CODEC_LIMITS)
}

/// Decode exact canonical bytes under explicit structural limits.
pub fn from_canonical_json_with_limits<T>(
    bytes: &[u8],
    limits: CodecLimits,
) -> Result<T, Diagnostic>
where
    T: DeserializeOwned + Serialize,
{
    ensure_bytes(bytes.len(), limits)?;
    let mut value: Value = serde_json::from_slice(bytes).map_err(|_| {
        Diagnostic::stable(
            DiagnosticCategory::InvalidContract,
            "malformed_canonical_json",
            "input is not valid canonical JSON",
        )
    })?;
    inspect(&value, 1, limits)?;
    normalize_numbers(&mut value).map_err(|()| {
        Diagnostic::stable(
            DiagnosticCategory::InvalidContract,
            "malformed_canonical_json",
            "input is not valid canonical JSON",
        )
    })?;
    sort_object_keys(&mut value);
    let canonical = serde_json::to_vec(&value).map_err(|_| {
        Diagnostic::stable(
            DiagnosticCategory::InvalidContract,
            "canonical_json_encode_failed",
            "decoded JSON cannot be re-encoded",
        )
    })?;
    if canonical != bytes {
        return Err(Diagnostic::stable(
            DiagnosticCategory::InvalidContract,
            "non_canonical_json",
            "input is valid JSON but not the canonical encoding",
        )
        .with_detail("actual_bytes", count(bytes.len()))
        .with_detail("canonical_bytes", count(canonical.len())));
    }
    serde_json::from_value(value).map_err(|_| {
        Diagnostic::stable(
            DiagnosticCategory::InvalidContract,
            "invalid_canonical_value",
            "canonical JSON does not satisfy the requested contract type",
        )
    })
}

/// Sort every JSON object lexicographically without relying on
/// `serde_json::Map`'s backing representation.
///
/// Cargo features are additive, so a downstream crate can enable
/// `serde_json/preserve_order` for the shared dependency even though this
/// crate does not request it. Re-inserting sorted entries keeps canonical
/// bytes independent of that feature-unified map backend.
fn sort_object_keys(value: &mut Value) {
    match value {
        Value::Array(values) => {
            for value in values {
                sort_object_keys(value);
            }
        }
        Value::Object(values) => {
            for value in values.values_mut() {
                sort_object_keys(value);
            }
            let mut entries = std::mem::take(values).into_iter().collect::<Vec<_>>();
            entries.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
            values.extend(entries);
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

/// Rebuild numbers through the semantic representation used by serde_json's
/// ordinary backend. With `arbitrary_precision` feature-unified downstream,
/// parsed numbers otherwise retain raw spellings such as `1e0` and integers
/// beyond `u64`, making canonical acceptance depend on the Cargo feature graph.
fn normalize_numbers(value: &mut Value) -> Result<(), ()> {
    match value {
        Value::Array(values) => {
            for value in values {
                normalize_numbers(value)?;
            }
        }
        Value::Object(values) => {
            for value in values.values_mut() {
                normalize_numbers(value)?;
            }
        }
        Value::Number(number) => {
            let normalized = if let Some(value) = number.as_i64() {
                value.into()
            } else if let Some(value) = number.as_u64() {
                value.into()
            } else if let Some(value) = number.as_f64() {
                serde_json::Number::from_f64(value).ok_or(())?
            } else {
                return Err(());
            };
            *number = normalized;
        }
        Value::Null | Value::Bool(_) | Value::String(_) => {}
    }
    Ok(())
}

fn inspect(value: &Value, depth: usize, limits: CodecLimits) -> Result<(), Diagnostic> {
    if depth > limits.max_depth {
        return Err(Diagnostic::stable(
            DiagnosticCategory::ResourceLimit,
            "canonical_json_too_deep",
            "canonical JSON exceeds the nesting-depth ceiling",
        )
        .with_detail("maximum_depth", count(limits.max_depth)));
    }
    match value {
        Value::String(value) => ensure_string(value.len(), limits),
        Value::Array(values) => {
            ensure_collection(values.len(), limits)?;
            for value in values {
                inspect(value, depth + 1, limits)?;
            }
            Ok(())
        }
        Value::Object(values) => {
            ensure_collection(values.len(), limits)?;
            for (key, value) in values {
                ensure_string(key.len(), limits)?;
                inspect(value, depth + 1, limits)?;
            }
            Ok(())
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => Ok(()),
    }
}

fn ensure_bytes(actual: usize, limits: CodecLimits) -> Result<(), Diagnostic> {
    if actual <= limits.max_bytes {
        Ok(())
    } else {
        Err(Diagnostic::stable(
            DiagnosticCategory::ResourceLimit,
            "canonical_json_too_large",
            "canonical JSON exceeds the byte ceiling",
        )
        .with_detail("actual_bytes", count(actual))
        .with_detail("maximum_bytes", count(limits.max_bytes)))
    }
}
fn ensure_collection(actual: usize, limits: CodecLimits) -> Result<(), Diagnostic> {
    if actual <= limits.max_collection_len {
        Ok(())
    } else {
        Err(Diagnostic::stable(
            DiagnosticCategory::ResourceLimit,
            "canonical_collection_too_large",
            "canonical JSON collection exceeds its member ceiling",
        )
        .with_detail("actual_items", count(actual))
        .with_detail("maximum_items", count(limits.max_collection_len)))
    }
}
fn ensure_string(actual: usize, limits: CodecLimits) -> Result<(), Diagnostic> {
    if actual <= limits.max_string_bytes {
        Ok(())
    } else {
        Err(Diagnostic::stable(
            DiagnosticCategory::ResourceLimit,
            "canonical_string_too_large",
            "canonical JSON string exceeds its byte ceiling",
        )
        .with_detail("actual_bytes", count(actual))
        .with_detail("maximum_bytes", count(limits.max_string_bytes)))
    }
}
fn count(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::CanonicalValue;
    use serde::{Deserialize, Serialize};
    use serde_json::Value;

    #[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
    struct OutOfOrderObject {
        zeta: u8,
        alpha: OutOfOrderNested,
    }

    #[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
    struct OutOfOrderNested {
        zeta: u8,
        alpha: u8,
    }

    #[test]
    fn canonical_object_order_is_independent_of_the_serde_json_map_backend() {
        let value = OutOfOrderObject {
            zeta: 3,
            alpha: OutOfOrderNested { zeta: 2, alpha: 1 },
        };
        let canonical = br#"{"alpha":{"alpha":1,"zeta":2},"zeta":3}"#;
        assert_eq!(to_canonical_json(&value).unwrap(), canonical);
        assert_eq!(
            from_canonical_json::<OutOfOrderObject>(canonical).unwrap(),
            value
        );

        let insertion_order = br#"{"zeta":3,"alpha":{"zeta":2,"alpha":1}}"#;
        assert_eq!(
            from_canonical_json::<OutOfOrderObject>(insertion_order)
                .unwrap_err()
                .code()
                .as_str(),
            "non_canonical_json"
        );
    }

    #[test]
    fn canonical_decoder_distinguishes_malformed_and_noncanonical_input() {
        assert_eq!(
            from_canonical_json::<CanonicalValue>(b"{")
                .unwrap_err()
                .code()
                .as_str(),
            "malformed_canonical_json"
        );
        let spaced = br#"{ "kind":"long","value":"1"}"#;
        assert_eq!(
            from_canonical_json::<CanonicalValue>(spaced)
                .unwrap_err()
                .code()
                .as_str(),
            "non_canonical_json"
        );

        for noncanonical in [b"1e0" as &[u8], b"1E+0", b"-0"] {
            for error in [
                from_canonical_json::<FormatVersion>(noncanonical).unwrap_err(),
                from_canonical_json::<Value>(noncanonical).unwrap_err(),
            ] {
                assert_eq!(error.code().as_str(), "non_canonical_json");
            }
        }
        assert_eq!(
            from_canonical_json::<Value>(b"01")
                .unwrap_err()
                .code()
                .as_str(),
            "malformed_canonical_json"
        );
    }

    #[test]
    fn canonical_numbers_are_independent_of_the_serde_json_number_backend() {
        for canonical in [
            b"0" as &[u8],
            b"-1",
            b"-9223372036854775808",
            b"18446744073709551615",
            b"1.0",
            b"0.0",
            b"-0.0",
            b"5e-324",
        ] {
            let value = from_canonical_json::<Value>(canonical).unwrap();
            assert_eq!(to_canonical_json(&value).unwrap(), canonical);
        }

        for noncanonical in [
            b"-9223372036854775809" as &[u8],
            b"18446744073709551616",
            b"100000000000000000000000000000000000000000000000000",
            b"4.9406564584124654e-324",
        ] {
            assert_eq!(
                from_canonical_json::<Value>(noncanonical)
                    .unwrap_err()
                    .code()
                    .as_str(),
                "non_canonical_json"
            );
        }

        assert_eq!(to_canonical_json(&1.0_f64).unwrap(), b"1.0");
        assert_eq!(to_canonical_json(&f64::from_bits(1)).unwrap(), b"5e-324");
    }

    #[test]
    fn exact_limits_accept_boundary_and_reject_next_value() {
        let value = CanonicalValue::String(crate::value::CanonicalString::new("abc").unwrap());
        let bytes = to_canonical_json(&value).unwrap();
        let mut limits = CodecLimits::CANONICAL;
        limits.max_bytes = bytes.len();
        assert!(from_canonical_json_with_limits::<CanonicalValue>(&bytes, limits).is_ok());
        limits.max_bytes -= 1;
        assert_eq!(
            from_canonical_json_with_limits::<CanonicalValue>(&bytes, limits)
                .unwrap_err()
                .code()
                .as_str(),
            "canonical_json_too_large"
        );
    }

    #[test]
    fn required_versions_fail_closed() {
        assert!(serde_json::from_str::<FormatVersion>(r#""1""#).is_err());
        assert!(serde_json::from_str::<CodecVersion>(r#""1""#).is_err());
        assert!(ensure_format_version(FormatVersion::V1, FormatVersion::V1).is_ok());
        assert!(ensure_codec_version(CodecVersion::V1, CodecVersion::V1).is_ok());

        assert_eq!(
            ensure_format_version(FormatVersion::from_raw(2), FormatVersion::V1)
                .unwrap_err()
                .code()
                .as_str(),
            "unsupported_format_version",
        );
        assert_eq!(
            ensure_codec_version(CodecVersion::from_raw(2), CodecVersion::V1)
                .unwrap_err()
                .code()
                .as_str(),
            "unsupported_codec_version",
        );
    }

    #[test]
    fn structural_limits_reject_depth_members_strings_and_keys() {
        let base = CodecLimits {
            max_bytes: 128,
            max_depth: 8,
            max_collection_len: 8,
            max_string_bytes: 8,
        };

        let depth = CodecLimits {
            max_depth: 2,
            ..base
        };
        assert_eq!(
            from_canonical_json_with_limits::<Value>(b"[[0]]", depth)
                .unwrap_err()
                .code()
                .as_str(),
            "canonical_json_too_deep",
        );

        let members = CodecLimits {
            max_collection_len: 1,
            ..base
        };
        assert_eq!(
            from_canonical_json_with_limits::<Value>(b"[0,1]", members)
                .unwrap_err()
                .code()
                .as_str(),
            "canonical_collection_too_large",
        );

        let strings = CodecLimits {
            max_string_bytes: 3,
            ..base
        };
        for bytes in [br#""abcd""# as &[u8], br#"{"abcd":0}"# as &[u8]] {
            assert_eq!(
                from_canonical_json_with_limits::<Value>(bytes, strings)
                    .unwrap_err()
                    .code()
                    .as_str(),
                "canonical_string_too_large",
            );
        }
    }
}
