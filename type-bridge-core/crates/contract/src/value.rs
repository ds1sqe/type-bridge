//! Domain-tagged canonical scalar, cardinality, and annotation values.

use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde::de::Error as _;

use crate::decimal::parse_decimal;
use crate::diagnostic::{Diagnostic, DiagnosticCategory};
use crate::limits::MAX_CANONICAL_STRING_BYTES;
use crate::temporal::{CanonicalDate, CanonicalDateTime, CanonicalDateTimeTz, CanonicalDuration};

/// Validated UTF-8 text bounded by the canonical per-string byte ceiling.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct CanonicalString(String);

impl CanonicalString {
    /// Construct canonical text, rejecting values above the byte ceiling.
    pub fn new(value: impl Into<String>) -> Result<Self, Diagnostic> {
        let value = value.into();
        if value.len() > MAX_CANONICAL_STRING_BYTES {
            return Err(Diagnostic::stable(
                DiagnosticCategory::ResourceLimit,
                "canonical_string_limit_exceeded",
                "canonical string exceeds the UTF-8 byte ceiling",
            )
            .with_detail("actual_bytes", i64::try_from(value.len()).unwrap_or(i64::MAX))
            .with_detail(
                "maximum_bytes",
                i64::try_from(MAX_CANONICAL_STRING_BYTES).expect("canonical string limit fits i64"),
            ));
        }
        Ok(Self(value))
    }

    /// Return the validated text.
    pub fn as_str(&self) -> &str { &self.0 }
}

impl fmt::Display for CanonicalString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for CanonicalString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

/// A finite IEEE-754 double represented by its exact bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CanonicalDouble(u64);

impl CanonicalDouble {
    /// Construct from a finite double, preserving signed zero and subnormals.
    pub fn new(value: f64) -> Result<Self, Diagnostic> {
        if value.is_finite() { Ok(Self(value.to_bits())) } else { Err(invalid_scalar("double")) }
    }
    /// Construct from finite IEEE bits.
    pub fn from_bits(bits: u64) -> Result<Self, Diagnostic> { Self::new(f64::from_bits(bits)) }
    /// Return exact IEEE bits.
    pub const fn bits(self) -> u64 { self.0 }
    /// Return the finite double.
    pub fn get(self) -> f64 { f64::from_bits(self.0) }
    /// Return fixed-width lowercase hexadecimal bits.
    pub fn bits_hex(self) -> String { format!("{:016x}", self.0) }
}

/// An owned validated canonical decimal spelling.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DecimalValue(String);

impl DecimalValue {
    /// Validate and normalize TypeQL or driver decimal text.
    pub fn new(value: impl AsRef<str>) -> Result<Self, Diagnostic> {
        parse_decimal(value.as_ref()).map(|value| Self(value.canonical_string())).ok_or_else(|| invalid_scalar("decimal"))
    }
    /// Return normalized decimal text without the driver suffix.
    pub fn as_str(&self) -> &str { &self.0 }
}
impl fmt::Display for DecimalValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(self.as_str()) }
}
impl Serialize for DecimalValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error> where S: Serializer { serializer.serialize_str(self.as_str()) }
}
impl<'de> Deserialize<'de> for DecimalValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error> where D: Deserializer<'de> {
        Self::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

fn invalid_scalar(value_type: &'static str) -> Diagnostic {
    Diagnostic::stable(DiagnosticCategory::InvalidContract, "invalid_canonical_scalar", "scalar value is outside its canonical domain")
        .with_detail("value_type", value_type)
}

/// The closed TypeDB scalar-domain vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueTypeTag {
    /// UTF-8 text.
    String,
    /// Signed 64-bit integer.
    Long,
    /// Finite IEEE-754 binary64.
    Double,
    /// Boolean.
    Boolean,
    /// Gregorian date.
    Date,
    /// Timezone-free date-time.
    #[serde(rename = "datetime")]
    DateTime,
    /// Timezone-aware date-time.
    #[serde(rename = "datetime_tz")]
    DateTimeTz,
    /// TypeDB decimal.
    Decimal,
    /// TypeDB duration.
    Duration,
}

impl ValueTypeTag {
    /// Return the stable contract spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Long => "long",
            Self::Double => "double",
            Self::Boolean => "boolean",
            Self::Date => "date",
            Self::DateTime => "datetime",
            Self::DateTimeTz => "datetime_tz",
            Self::Decimal => "decimal",
            Self::Duration => "duration",
        }
    }
}

/// A canonical scalar value with an explicit domain tag.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CanonicalValue {
    /// UTF-8 text.
    String(CanonicalString),
    /// Signed 64-bit integer; JSON uses a decimal string.
    Long(i64),
    /// Finite binary64; JSON uses fixed-width IEEE bits.
    Double(CanonicalDouble),
    /// Boolean.
    Boolean(bool),
    /// Date.
    Date(CanonicalDate),
    /// Timezone-free date-time.
    DateTime(CanonicalDateTime),
    /// Timezone-aware date-time.
    DateTimeTz(CanonicalDateTimeTz),
    /// Decimal.
    Decimal(DecimalValue),
    /// Duration.
    Duration(CanonicalDuration),
}

impl CanonicalValue {
    /// Return the closed scalar-domain tag.
    pub const fn value_type(&self) -> ValueTypeTag {
        match self {
            Self::String(_) => ValueTypeTag::String, Self::Long(_) => ValueTypeTag::Long,
            Self::Double(_) => ValueTypeTag::Double, Self::Boolean(_) => ValueTypeTag::Boolean,
            Self::Date(_) => ValueTypeTag::Date, Self::DateTime(_) => ValueTypeTag::DateTime,
            Self::DateTimeTz(_) => ValueTypeTag::DateTimeTz, Self::Decimal(_) => ValueTypeTag::Decimal,
            Self::Duration(_) => ValueTypeTag::Duration,
        }
    }

    /// Compare values within one scalar domain by provider semantics.
    ///
    /// This is deliberately separate from representation `Ord`, which remains
    /// the deterministic identity order used by canonical sets and bytes.
    /// Compare values only when both belong to the same semantic scalar domain.
    ///
    /// Returns `None` for different domains or a domain without semantic ordering.
    pub fn semantic_cmp_same_domain(&self, other: &Self) -> Option<Ordering> {
        match (self, other) {
            (Self::String(left), Self::String(right)) => Some(left.cmp(right)),
            (Self::Long(left), Self::Long(right)) => Some(left.cmp(right)),
            (Self::Double(left), Self::Double(right)) => left.get().partial_cmp(&right.get()),
            (Self::Boolean(left), Self::Boolean(right)) => Some(left.cmp(right)),
            (Self::Date(left), Self::Date(right)) => Some(left.cmp(right)),
            (Self::DateTime(left), Self::DateTime(right)) => Some(left.cmp(right)),
            (Self::DateTimeTz(left), Self::DateTimeTz(right)) => {
                Some(
                    left.semantic_utc_nanoseconds()
                        .cmp(&right.semantic_utc_nanoseconds()),
                )
            }
            (Self::Decimal(left), Self::Decimal(right)) => {
                let left = parse_decimal(left.as_str()).expect("DecimalValue is always validated");
                let right = parse_decimal(right.as_str()).expect("DecimalValue is always validated");
                Some(left.compare(&right))
            }
            _ => None,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ValueWire {
    String { value: CanonicalString },
    Long { value: String },
    Double { bits: String },
    Boolean { value: bool },
    Date { value: CanonicalDate },
    #[serde(rename = "datetime")]
    DateTime { value: CanonicalDateTime },
    #[serde(rename = "datetime_tz")]
    DateTimeTz { value: CanonicalDateTimeTz },
    Decimal { value: DecimalValue },
    Duration { value: CanonicalDuration },
}

impl Serialize for CanonicalValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error> where S: Serializer {
        let wire = match self {
            Self::String(value) => ValueWire::String { value: value.clone() },
            Self::Long(value) => ValueWire::Long { value: value.to_string() },
            Self::Double(value) => ValueWire::Double { bits: value.bits_hex() },
            Self::Boolean(value) => ValueWire::Boolean { value: *value },
            Self::Date(value) => ValueWire::Date { value: *value },
            Self::DateTime(value) => ValueWire::DateTime { value: *value },
            Self::DateTimeTz(value) => ValueWire::DateTimeTz { value: value.clone() },
            Self::Decimal(value) => ValueWire::Decimal { value: value.clone() },
            Self::Duration(value) => ValueWire::Duration { value: *value },
        };
        wire.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for CanonicalValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error> where D: Deserializer<'de> {
        match ValueWire::deserialize(deserializer)? {
            ValueWire::String { value } => Ok(Self::String(value)),
            ValueWire::Boolean { value } => Ok(Self::Boolean(value)),
            ValueWire::Date { value } => Ok(Self::Date(value)),
            ValueWire::DateTime { value } => Ok(Self::DateTime(value)),
            ValueWire::DateTimeTz { value } => Ok(Self::DateTimeTz(value)),
            ValueWire::Decimal { value } => Ok(Self::Decimal(value)),
            ValueWire::Duration { value } => Ok(Self::Duration(value)),
            ValueWire::Long { value } => {
                let parsed = value.parse::<i64>().map_err(D::Error::custom)?;
                if parsed.to_string() != value { return Err(D::Error::custom("long value is not canonical")); }
                Ok(Self::Long(parsed))
            }
            ValueWire::Double { bits } => {
                if bits.len() != 16 || bits.bytes().any(|b| !b.is_ascii_digit() && !(b'a'..=b'f').contains(&b)) {
                    return Err(D::Error::custom("double bits are not canonical lowercase hex"));
                }
                let bits = u64::from_str_radix(&bits, 16).map_err(D::Error::custom)?;
                CanonicalDouble::from_bits(bits).map(Self::Double).map_err(D::Error::custom)
            }
        }
    }
}

/// A validated cardinality. `None` means an unbounded maximum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Cardinality { min: u64, max: Option<u64> }

impl Cardinality {
    /// Construct a cardinality, rejecting inverted and exact-zero ranges.
    pub fn new(min: u64, max: Option<u64>) -> Result<Self, Diagnostic> {
        if max.is_some_and(|max| max < min || max == 0) {
            Err(Diagnostic::stable(DiagnosticCategory::InvalidContract, "invalid_cardinality", "cardinality maximum is below its minimum or exactly zero"))
        } else { Ok(Self { min, max }) }
    }
    /// Return the minimum.
    pub const fn min(self) -> u64 { self.min }
    /// Return the optional finite maximum.
    pub const fn max(self) -> Option<u64> { self.max }
}

#[derive(Serialize, Deserialize)]
struct CardinalityWire { kind: CardinalityKind, min: String, max: String }
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CardinalityKind { Cardinality }

impl Serialize for Cardinality {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error> where S: Serializer {
        CardinalityWire { kind: CardinalityKind::Cardinality, min: self.min.to_string(), max: self.max.map_or_else(|| "unbounded".to_owned(), |max| max.to_string()) }.serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for Cardinality {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error> where D: Deserializer<'de> {
        let wire = CardinalityWire::deserialize(deserializer)?;
        let min = wire.min.parse::<u64>().map_err(D::Error::custom)?;
        if min.to_string() != wire.min { return Err(D::Error::custom("cardinality minimum is not canonical")); }
        let max = if wire.max == "unbounded" { None } else {
            let max = wire.max.parse::<u64>().map_err(D::Error::custom)?;
            if max.to_string() != wire.max { return Err(D::Error::custom("cardinality maximum is not canonical")); }
            Some(max)
        };
        Self::new(min, max).map_err(D::Error::custom)
    }
}

/// A generic canonical annotation value without freezing annotation fact DTOs.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum AnnotationValue {
    /// Marker annotation with no payload.
    Unit,
    /// One scalar payload.
    Scalar(CanonicalValue),
    /// A semantically ordered scalar sequence.
    Ordered(Vec<CanonicalValue>),
    /// A semantically unordered deterministically sorted scalar set.
    Unordered(BTreeSet<CanonicalValue>),
    /// A cardinality payload.
    Cardinality(Cardinality),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_decimal_double_and_cardinality_have_binding_safe_shapes() {
        assert_eq!(serde_json::to_string(&CanonicalValue::Long(9_007_199_254_740_993)).unwrap(), r#"{"kind":"long","value":"9007199254740993"}"#);
        let decimal = CanonicalValue::Decimal(DecimalValue::new("+001.2300dec").unwrap());
        assert_eq!(serde_json::to_string(&decimal).unwrap(), r#"{"kind":"decimal","value":"1.23"}"#);
        let negative_zero = CanonicalValue::Double(CanonicalDouble::new(-0.0).unwrap());
        assert_eq!(serde_json::to_string(&negative_zero).unwrap(), r#"{"kind":"double","bits":"8000000000000000"}"#);
        assert!(Cardinality::new(0, Some(0)).is_err());
    }

    #[test]
    fn double_policy_rejects_nonfinite_and_preserves_exact_finite_bits() {
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(CanonicalDouble::new(value).is_err());
        }

        let positive_zero = CanonicalDouble::new(0.0).unwrap();
        let negative_zero = CanonicalDouble::new(-0.0).unwrap();
        assert_ne!(positive_zero, negative_zero);

        for bits in [0, 0x8000_0000_0000_0000, 1] {
            let value =
                CanonicalValue::Double(CanonicalDouble::from_bits(bits).unwrap());
            let bytes = serde_json::to_vec(&value).unwrap();
            assert_eq!(
                serde_json::from_slice::<CanonicalValue>(&bytes).unwrap(),
                value,
            );
        }

        assert!(serde_json::from_str::<CanonicalValue>(
            r#"{"kind":"double","bits":"7ff0000000000000"}"#
        )
        .is_err());
    }

    #[test]
    fn canonical_strings_enforce_the_byte_limit_without_changing_wire_shape() {
        let boundary = CanonicalString::new("x".repeat(MAX_CANONICAL_STRING_BYTES)).unwrap();
        assert_eq!(boundary.as_str().len(), MAX_CANONICAL_STRING_BYTES);
        let error = CanonicalString::new("x".repeat(MAX_CANONICAL_STRING_BYTES + 1)).unwrap_err();
        assert_eq!(error.category(), DiagnosticCategory::ResourceLimit);
        assert_eq!(error.code().as_str(), "canonical_string_limit_exceeded");
        assert_eq!(
            serde_json::to_string(&CanonicalValue::String(CanonicalString::new("text").unwrap())).unwrap(),
            r#"{"kind":"string","value":"text"}"#,
        );
    }

    #[test]
    fn semantic_order_is_numeric_and_distinct_from_representation_order() {
        let decimal_two = CanonicalValue::Decimal(DecimalValue::new("2").unwrap());
        let decimal_ten = CanonicalValue::Decimal(DecimalValue::new("10").unwrap());
        assert_eq!(decimal_two.semantic_cmp_same_domain(&decimal_ten), Some(Ordering::Less));

        let negative_zero = CanonicalValue::Double(CanonicalDouble::new(-0.0).unwrap());
        let positive_zero = CanonicalValue::Double(CanonicalDouble::new(0.0).unwrap());
        assert_ne!(negative_zero, positive_zero);
        assert_eq!(negative_zero.semantic_cmp_same_domain(&positive_zero), Some(Ordering::Equal));

        assert_eq!(
            CanonicalValue::Long(-10).semantic_cmp_same_domain(&CanonicalValue::Long(-2)),
            Some(Ordering::Less),
        );
        assert_eq!(
            CanonicalValue::String(CanonicalString::new("alpha").unwrap()).semantic_cmp_same_domain(
                &CanonicalValue::String(CanonicalString::new("beta").unwrap()),
            ),
            Some(Ordering::Less),
        );
        assert_eq!(
            CanonicalValue::Long(1).semantic_cmp_same_domain(&CanonicalValue::Double(CanonicalDouble::new(1.0).unwrap())),
            None,
        );
    }
}
