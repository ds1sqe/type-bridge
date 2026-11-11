//! Model predicates, scalar evidence, projection and hydration authority.

use super::failure;
use crate::decimal::parse_decimal;
use crate::diagnostic::{Diagnostic, DiagnosticCategory};
use crate::id::{AttributeId, RoleId, TypeId};
use crate::limits::MAX_CANONICAL_STRING_BYTES;
use crate::migration_assertion::BindingId;
use crate::temporal::{CanonicalDateTime, CanonicalDateTimeTz, CanonicalDuration};
use crate::value::{CanonicalValue, Cardinality, ValueTypeTag};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::cmp::Ordering;

/// One descriptor-qualified field reference in the compatibility algebra.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelField {
    pub(super) attribute: AttributeId,
    #[serde(deserialize_with = "deserialize_binding")]
    pub(super) binding: BindingId,
    pub(super) descriptor: TypeId,
    pub(super) value_type: ValueTypeTag,
}

impl ModelField {
    /// Construct one binding- and descriptor-qualified field reference.
    #[must_use]
    pub const fn new(
        binding: BindingId,
        descriptor: TypeId,
        attribute: AttributeId,
        value_type: ValueTypeTag,
    ) -> Self {
        Self {
            attribute,
            binding,
            descriptor,
            value_type,
        }
    }

    /// Return the owning plan binding.
    #[must_use]
    pub const fn binding(&self) -> BindingId {
        self.binding
    }

    /// Return the descriptor under which the field was resolved.
    #[must_use]
    pub const fn descriptor(&self) -> &TypeId {
        &self.descriptor
    }

    /// Return the canonical provider attribute identity.
    #[must_use]
    pub const fn attribute(&self) -> &AttributeId {
        &self.attribute
    }

    /// Return the registry-resolved scalar domain.
    #[must_use]
    pub const fn value_type(&self) -> ValueTypeTag {
        self.value_type
    }
}

/// The closed comparison vocabulary accepted by the released match facade.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelComparator {
    /// Equality.
    Equal,
    /// Inequality.
    NotEqual,
    /// Strictly less than.
    Less,
    /// Less than or equal.
    LessOrEqual,
    /// Strictly greater than.
    Greater,
    /// Greater than or equal.
    GreaterOrEqual,
    /// String containment.
    Contains,
    /// String prefix.
    StartsWith,
    /// String suffix.
    EndsWith,
    /// TypeDB regular-expression matching.
    Regex,
}

/// A released-only lexical scalar domain carried by a model-request value.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ModelValueKind {
    /// A string above the canonical single-string byte ceiling.
    String,
    /// A timezone-free datetime spelling accepted by the released V1 validator.
    DateTime,
    /// A timezone-aware datetime spelling accepted by the released V1 validator.
    DateTimeTz,
    /// A duration spelling accepted by the released V1 validator.
    Duration,
    /// A decimal spelling exposed by the released V1 driver-backed hydrator.
    Decimal,
}

impl ModelValueKind {
    /// Return the exact scalar domain represented by this spelling.
    #[must_use]
    pub const fn value_type(self) -> ValueTypeTag {
        match self {
            Self::String => ValueTypeTag::String,
            Self::DateTime => ValueTypeTag::DateTime,
            Self::DateTimeTz => ValueTypeTag::DateTimeTz,
            Self::Duration => ValueTypeTag::Duration,
            Self::Decimal => ValueTypeTag::Decimal,
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
enum ModelValueInner {
    Canonical(CanonicalValue),
    Released {
        kind: ModelValueKind,
        chunks: Vec<String>,
    },
}

/// One exact model-request scalar.
///
/// Ordinary values retain the released [`CanonicalValue`] wire. V1-only
/// strings and non-canonical temporal, duration, or decimal spellings use
/// deterministic bounded UTF-8 chunks so no individual JSON string exceeds
/// the canonical codec ceiling.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ModelValue {
    inner: ModelValueInner,
}

impl ModelValue {
    /// Wrap one ordinary canonical scalar without changing its wire shape.
    #[must_use]
    pub const fn canonical(value: CanonicalValue) -> Self {
        Self {
            inner: ModelValueInner::Canonical(value),
        }
    }

    /// Preserve one released string above the canonical per-string ceiling.
    pub fn released_string(value: impl Into<String>) -> Result<Self, Diagnostic> {
        Self::released(ModelValueKind::String, value.into())
    }

    /// Preserve one released timezone-free datetime spelling exactly.
    pub fn released_datetime(value: impl Into<String>) -> Result<Self, Diagnostic> {
        Self::released(ModelValueKind::DateTime, value.into())
    }

    /// Preserve one released timezone-aware datetime spelling exactly.
    pub fn released_datetime_tz(value: impl Into<String>) -> Result<Self, Diagnostic> {
        Self::released(ModelValueKind::DateTimeTz, value.into())
    }

    /// Preserve one released duration spelling exactly.
    pub fn released_duration(value: impl Into<String>) -> Result<Self, Diagnostic> {
        Self::released(ModelValueKind::Duration, value.into())
    }

    /// Preserve one released non-canonical decimal spelling exactly.
    pub fn released_decimal(value: impl Into<String>) -> Result<Self, Diagnostic> {
        Self::released(ModelValueKind::Decimal, value.into())
    }

    fn released(kind: ModelValueKind, value: String) -> Result<Self, Diagnostic> {
        let value = Self {
            inner: ModelValueInner::Released {
                kind,
                chunks: released_value_chunks(&value),
            },
        };
        value.validate()?;
        Ok(value)
    }

    /// Return the scalar domain claimed by this literal.
    #[must_use]
    pub const fn value_type(&self) -> ValueTypeTag {
        match &self.inner {
            ModelValueInner::Canonical(value) => value.value_type(),
            ModelValueInner::Released { kind, .. } => kind.value_type(),
        }
    }

    /// Return the ordinary canonical value, when this is not a V1-only spelling.
    #[must_use]
    pub const fn canonical_value(&self) -> Option<&CanonicalValue> {
        match &self.inner {
            ModelValueInner::Canonical(value) => Some(value),
            ModelValueInner::Released { .. } => None,
        }
    }

    /// Return the released-only lexical domain, when present.
    #[must_use]
    pub const fn released_kind(&self) -> Option<ModelValueKind> {
        match &self.inner {
            ModelValueInner::Canonical(_) => None,
            ModelValueInner::Released { kind, .. } => Some(*kind),
        }
    }

    /// Return canonical bounded chunks for a released-only spelling.
    #[must_use]
    pub fn released_chunks(&self) -> Option<&[String]> {
        match &self.inner {
            ModelValueInner::Canonical(_) => None,
            ModelValueInner::Released { chunks, .. } => Some(chunks),
        }
    }

    /// Reconstruct the exact released spelling.
    #[must_use]
    pub fn released_text(&self) -> Option<String> {
        self.released_chunks().map(<[String]>::concat)
    }

    /// Compare two same-domain values with the released V1 result semantics.
    ///
    /// Representation [`Ord`] remains the deterministic wire identity order.
    /// This method is for compatibility ordering and duplicate detection.
    #[must_use]
    pub fn semantic_cmp_same_domain(&self, other: &Self) -> Option<Ordering> {
        if self.value_type() != other.value_type() {
            return None;
        }
        if let (Some(left), Some(right)) = (self.canonical_value(), other.canonical_value())
            && self.value_type() != ValueTypeTag::Duration
        {
            return left.semantic_cmp_same_domain(right);
        }
        if self.value_type() == ValueTypeTag::String {
            return Some(compatibility_string_bytes(self)?.cmp(compatibility_string_bytes(other)?));
        }
        let left = compatibility_value_text(self)?;
        let right = compatibility_value_text(other)?;
        match self.value_type() {
            ValueTypeTag::DateTime => Some(
                parse_released_datetime(&left, false)?
                    .cmp(&parse_released_datetime(&right, false)?),
            ),
            ValueTypeTag::DateTimeTz => Some(
                parse_released_datetime(&left, true)?.cmp(&parse_released_datetime(&right, true)?),
            ),
            ValueTypeTag::Duration => {
                Some(parse_released_duration(&left)?.cmp(&parse_released_duration(&right)?))
            }
            ValueTypeTag::Decimal => Some(parse_decimal(&left)?.compare(&parse_decimal(&right)?)),
            ValueTypeTag::String
            | ValueTypeTag::Long
            | ValueTypeTag::Double
            | ValueTypeTag::Boolean
            | ValueTypeTag::Date => None,
        }
    }

    pub(super) fn validate(&self) -> Result<(), Diagnostic> {
        let ModelValueInner::Released { kind, chunks } = &self.inner else {
            return Ok(());
        };
        if chunks
            .iter()
            .try_fold(0usize, |total, chunk| total.checked_add(chunk.len()))
            .is_none()
        {
            return Err(failure(
                DiagnosticCategory::ResourceLimit,
                "query_plan_compatibility_value_limit",
                "compatibility value bytes overflow the bounded artifact domain",
            ));
        }
        let value = chunks.concat();
        if chunks != &released_value_chunks(&value) {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_compatibility_value_chunks",
                "compatibility value chunks are not in canonical UTF-8 boundaries",
            ));
        }
        let valid = match kind {
            ModelValueKind::String => value.len() > MAX_CANONICAL_STRING_BYTES,
            ModelValueKind::DateTime => {
                released_valid_datetime(&value, false)
                    && value.parse::<CanonicalDateTime>().is_err()
            }
            ModelValueKind::DateTimeTz => {
                released_valid_datetime(&value, true)
                    && value.parse::<CanonicalDateTimeTz>().is_err()
            }
            ModelValueKind::Duration => {
                released_valid_duration(&value) && value.parse::<CanonicalDuration>().is_err()
            }
            ModelValueKind::Decimal => {
                parse_decimal(&value).is_some_and(|parsed| parsed.canonical_string() != value)
            }
        };
        if valid {
            Ok(())
        } else {
            Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_compatibility_value_lexical",
                "compatibility value is invalid or has an ordinary canonical representation",
            ))
        }
    }
}

fn compatibility_value_text(value: &ModelValue) -> Option<String> {
    match &value.inner {
        ModelValueInner::Released { chunks, .. } => Some(chunks.concat()),
        ModelValueInner::Canonical(value) => match value {
            CanonicalValue::String(_) => None,
            CanonicalValue::DateTime(value) => Some(value.to_string()),
            CanonicalValue::DateTimeTz(value) => Some(value.to_string()),
            CanonicalValue::Duration(value) => Some(value.to_string()),
            CanonicalValue::Decimal(value) => Some(value.to_string()),
            CanonicalValue::Long(_)
            | CanonicalValue::Double(_)
            | CanonicalValue::Boolean(_)
            | CanonicalValue::Date(_) => None,
        },
    }
}

fn compatibility_string_bytes(value: &ModelValue) -> Option<Box<dyn Iterator<Item = u8> + '_>> {
    match &value.inner {
        ModelValueInner::Canonical(CanonicalValue::String(value)) => {
            Some(Box::new(value.as_str().bytes()))
        }
        ModelValueInner::Released {
            kind: ModelValueKind::String,
            chunks,
        } => Some(Box::new(chunks.iter().flat_map(|chunk| chunk.bytes()))),
        ModelValueInner::Canonical(_) | ModelValueInner::Released { .. } => None,
    }
}

impl From<CanonicalValue> for ModelValue {
    fn from(value: CanonicalValue) -> Self {
        Self::canonical(value)
    }
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[expect(
    clippy::enum_variant_names,
    reason = "variant names are the frozen externally tagged wire discriminators"
)]
enum ReleasedValueWireRefV2<'value> {
    ReleasedString { chunks: &'value [String] },
    ReleasedDatetime { chunks: &'value [String] },
    ReleasedDatetimeTz { chunks: &'value [String] },
    ReleasedDuration { chunks: &'value [String] },
    ReleasedDecimal { chunks: &'value [String] },
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[expect(
    clippy::enum_variant_names,
    reason = "variant names are the frozen externally tagged wire discriminators"
)]
enum ReleasedValueWireV2 {
    ReleasedString { chunks: Vec<String> },
    ReleasedDatetime { chunks: Vec<String> },
    ReleasedDatetimeTz { chunks: Vec<String> },
    ReleasedDuration { chunks: Vec<String> },
    ReleasedDecimal { chunks: Vec<String> },
}

#[derive(Deserialize)]
#[serde(untagged)]
enum CompatibilityValueWireV2 {
    Canonical(CanonicalValue),
    Released(ReleasedValueWireV2),
}

impl Serialize for ModelValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match &self.inner {
            ModelValueInner::Canonical(value) => value.serialize(serializer),
            ModelValueInner::Released { kind, chunks } => match kind {
                ModelValueKind::String => {
                    ReleasedValueWireRefV2::ReleasedString { chunks }.serialize(serializer)
                }
                ModelValueKind::DateTime => {
                    ReleasedValueWireRefV2::ReleasedDatetime { chunks }.serialize(serializer)
                }
                ModelValueKind::DateTimeTz => {
                    ReleasedValueWireRefV2::ReleasedDatetimeTz { chunks }.serialize(serializer)
                }
                ModelValueKind::Duration => {
                    ReleasedValueWireRefV2::ReleasedDuration { chunks }.serialize(serializer)
                }
                ModelValueKind::Decimal => {
                    ReleasedValueWireRefV2::ReleasedDecimal { chunks }.serialize(serializer)
                }
            },
        }
    }
}

impl<'de> Deserialize<'de> for ModelValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let inner = match CompatibilityValueWireV2::deserialize(deserializer)? {
            CompatibilityValueWireV2::Canonical(value) => ModelValueInner::Canonical(value),
            CompatibilityValueWireV2::Released(wire) => match wire {
                ReleasedValueWireV2::ReleasedString { chunks } => ModelValueInner::Released {
                    kind: ModelValueKind::String,
                    chunks,
                },
                ReleasedValueWireV2::ReleasedDatetime { chunks } => ModelValueInner::Released {
                    kind: ModelValueKind::DateTime,
                    chunks,
                },
                ReleasedValueWireV2::ReleasedDatetimeTz { chunks } => ModelValueInner::Released {
                    kind: ModelValueKind::DateTimeTz,
                    chunks,
                },
                ReleasedValueWireV2::ReleasedDuration { chunks } => ModelValueInner::Released {
                    kind: ModelValueKind::Duration,
                    chunks,
                },
                ReleasedValueWireV2::ReleasedDecimal { chunks } => ModelValueInner::Released {
                    kind: ModelValueKind::Decimal,
                    chunks,
                },
            },
        };
        let value = Self { inner };
        value.validate().map_err(D::Error::custom)?;
        Ok(value)
    }
}

fn released_value_chunks(value: &str) -> Vec<String> {
    if value.is_empty() {
        return vec![String::new()];
    }
    let mut chunks = Vec::new();
    let mut start = 0usize;
    while start < value.len() {
        let mut end = start.saturating_add(MAX_CANONICAL_STRING_BYTES);
        if end >= value.len() {
            end = value.len();
        } else {
            while !value.is_char_boundary(end) {
                end -= 1;
            }
        }
        chunks.push(value[start..end].to_owned());
        start = end;
    }
    chunks
}

fn released_valid_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return false;
    }
    let Ok(year) = value[0..4].parse::<u32>() else {
        return false;
    };
    let Ok(month) = value[5..7].parse::<u32>() else {
        return false;
    };
    let Ok(day) = value[8..10].parse::<u32>() else {
        return false;
    };
    if year == 0 || !(1..=12).contains(&month) {
        return false;
    }
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    day >= 1 && day <= days[(month - 1) as usize]
}

fn released_valid_clock(value: &str) -> bool {
    parse_released_clock(value).is_some()
}

fn released_valid_datetime(value: &str, timezone_required: bool) -> bool {
    let Some((date, time)) = value.split_once('T') else {
        return false;
    };
    if !released_valid_date(date) {
        return false;
    }
    let (clock, has_timezone) = if let Some(clock) = time.strip_suffix('Z') {
        (clock, true)
    } else if let Some(index) = time
        .char_indices()
        .skip(1)
        .find_map(|(index, character)| matches!(character, '+' | '-').then_some(index))
    {
        let (clock, offset) = time.split_at(index);
        (clock, released_valid_clock(&offset[1..]))
    } else {
        (time, false)
    };
    released_valid_clock(clock) && has_timezone == timezone_required
}

fn released_valid_duration(value: &str) -> bool {
    value.starts_with('P')
        && value.len() > 1
        && value.bytes().skip(1).all(|byte| {
            byte.is_ascii_digit() || matches!(byte, b'Y' | b'M' | b'D' | b'T' | b'H' | b'S' | b'.')
        })
        && value.bytes().skip(1).any(|byte| byte.is_ascii_digit())
}

fn parse_released_date(value: &str) -> Option<i64> {
    if !released_valid_date(value) {
        return None;
    }
    let year = value[0..4].parse::<i32>().ok()?;
    let month = value[5..7].parse::<u32>().ok()?;
    let day = value[8..10].parse::<u32>().ok()?;
    let adjusted_year = year - i32::from(month <= 2);
    let era = if adjusted_year >= 0 {
        adjusted_year
    } else {
        adjusted_year - 399
    } / 400;
    let year_of_era = adjusted_year - era * 400;
    let shifted_month = month as i32 + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day as i32 - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Some(i64::from(era * 146_097 + day_of_era))
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ReleasedDateTimeKey {
    seconds: i64,
    fraction: String,
}

impl Ord for ReleasedDateTimeKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.seconds
            .cmp(&other.seconds)
            .then_with(|| compare_released_fraction(&self.fraction, &other.fraction))
    }
}

impl PartialOrd for ReleasedDateTimeKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn parse_released_datetime(value: &str, timezone_required: bool) -> Option<ReleasedDateTimeKey> {
    let (date, raw_time) = value.split_once('T')?;
    let days = parse_released_date(date)?;
    let (time, offset_seconds, has_timezone) = if let Some(time) = raw_time.strip_suffix('Z') {
        (time, 0_i64, true)
    } else if let Some(index) = raw_time
        .char_indices()
        .skip(1)
        .find_map(|(index, character)| matches!(character, '+' | '-').then_some(index))
    {
        let (time, offset) = raw_time.split_at(index);
        let sign = if offset.starts_with('-') {
            -1_i64
        } else {
            1_i64
        };
        let (hour, minute, second, fraction) = parse_released_clock(&offset[1..])?;
        if !fraction.is_empty() || hour > 23 {
            return None;
        }
        (
            time,
            sign * (i64::from(hour) * 3_600 + i64::from(minute) * 60 + i64::from(second)),
            true,
        )
    } else {
        (raw_time, 0_i64, false)
    };
    if has_timezone != timezone_required {
        return None;
    }
    let (hour, minute, second, fraction) = parse_released_clock(time)?;
    let seconds = days
        .checked_mul(86_400)?
        .checked_add(i64::from(hour) * 3_600 + i64::from(minute) * 60 + i64::from(second))?
        .checked_sub(offset_seconds)?;
    Some(ReleasedDateTimeKey { seconds, fraction })
}

fn parse_released_clock(value: &str) -> Option<(u32, u32, u32, String)> {
    let (main, fraction) = value
        .split_once('.')
        .map_or((value, ""), |(main, fraction)| (main, fraction));
    if (!fraction.is_empty() && !fraction.bytes().all(|byte| byte.is_ascii_digit()))
        || value.ends_with('.')
    {
        return None;
    }
    let mut parts = main.split(':');
    let hour = parts.next()?.parse::<u32>().ok()?;
    let minute = parts.next()?.parse::<u32>().ok()?;
    let second = parts.next().map_or(Some(0), |part| part.parse().ok())?;
    if parts.next().is_some() || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    Some((
        hour,
        minute,
        second,
        fraction.trim_end_matches('0').to_owned(),
    ))
}

fn compare_released_fraction(left: &str, right: &str) -> Ordering {
    let width = left.len().max(right.len());
    left.bytes()
        .chain(std::iter::repeat_n(b'0', width - left.len()))
        .cmp(
            right
                .bytes()
                .chain(std::iter::repeat_n(b'0', width - right.len())),
        )
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ReleasedDurationKey {
    months: u64,
    days: u64,
    nanoseconds: u128,
}

fn parse_released_duration(value: &str) -> Option<ReleasedDurationKey> {
    let value = value.strip_prefix('P')?;
    if value.is_empty() {
        return None;
    }
    let mut months = 0_u64;
    let mut days = 0_u64;
    let mut nanoseconds = 0_u128;
    let mut number = String::new();
    let mut time = false;
    let mut saw_value = false;
    for character in value.chars() {
        if character == 'T' {
            if time || !number.is_empty() {
                return None;
            }
            time = true;
            continue;
        }
        if character.is_ascii_digit() || character == '.' {
            number.push(character);
            continue;
        }
        if number.is_empty() {
            return None;
        }
        saw_value = true;
        match (time, character) {
            (false, 'Y') => {
                months = months.checked_add(number.parse::<u64>().ok()?.checked_mul(12)?)?
            }
            (false, 'M') => months = months.checked_add(number.parse::<u64>().ok()?)?,
            (false, 'D') => days = days.checked_add(number.parse::<u64>().ok()?)?,
            (true, 'H') => {
                nanoseconds = nanoseconds.checked_add(
                    u128::from(number.parse::<u64>().ok()?).checked_mul(3_600_000_000_000)?,
                )?
            }
            (true, 'M') => {
                nanoseconds = nanoseconds.checked_add(
                    u128::from(number.parse::<u64>().ok()?).checked_mul(60_000_000_000)?,
                )?
            }
            (true, 'S') => {
                let (whole, fraction) = number.split_once('.').unwrap_or((number.as_str(), ""));
                if whole.is_empty()
                    || fraction.len() > 9
                    || !fraction.bytes().all(|byte| byte.is_ascii_digit())
                {
                    return None;
                }
                let seconds = whole.parse::<u64>().ok()?;
                let mut nanos = fraction.parse::<u64>().unwrap_or(0);
                for _ in fraction.len()..9 {
                    nanos *= 10;
                }
                nanoseconds = nanoseconds
                    .checked_add(u128::from(seconds).checked_mul(1_000_000_000)?)?
                    .checked_add(u128::from(nanos))?;
            }
            _ => return None,
        }
        number.clear();
    }
    if !number.is_empty() || !saw_value {
        return None;
    }
    Some(ReleasedDurationKey {
        months,
        days,
        nanoseconds,
    })
}

/// The closed boolean compatibility algebra used by the production adapter.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModelPredicate {
    /// Compare one bound field with one exact scalar literal.
    FieldValue {
        /// Descriptor-qualified field.
        field: ModelField,
        /// Closed comparison operator.
        comparator: ModelComparator,
        /// Canonical or released-only typed literal.
        value: ModelValue,
    },
    /// Compare two descriptor-qualified bound fields.
    FieldComparison {
        /// Left field.
        left: ModelField,
        /// Closed non-string comparison operator.
        comparator: ModelComparator,
        /// Right field.
        right: ModelField,
    },
    /// Require one descriptor-qualified field to be present or absent.
    FieldPresence {
        /// Descriptor-qualified field.
        field: ModelField,
        /// `true` requires at least one value; `false` requires none.
        present: bool,
    },
    /// Match one thing binding by its canonical provider IID.
    BindingIid {
        /// Thing binding.
        #[serde(deserialize_with = "deserialize_binding")]
        binding: BindingId,
        /// Canonical TypeDB thing IID.
        iid: String,
    },
    /// Require one descriptor-qualified role edge.
    RoleEdge {
        /// Whether relation subtypes remain admitted while matching the edge.
        include_relation_subtypes: bool,
        /// Player binding.
        #[serde(deserialize_with = "deserialize_binding")]
        player: BindingId,
        /// Relation binding.
        #[serde(deserialize_with = "deserialize_binding")]
        relation: BindingId,
        /// Declared relation type.
        relation_type: TypeId,
        /// Descriptor-qualified role.
        role: RoleId,
    },
    /// Require one finite directed walk.
    Reachable {
        /// Inclusive minimum hop count.
        min_depth: u8,
        /// Inclusive maximum hop count.
        max_depth: u8,
        /// Exact relation type used for every hop.
        relation: TypeId,
        /// Source role.
        role_from: RoleId,
        /// Target role.
        role_to: RoleId,
        /// Source binding.
        #[serde(deserialize_with = "deserialize_binding")]
        source: BindingId,
        /// Target binding.
        #[serde(deserialize_with = "deserialize_binding")]
        target: BindingId,
    },
    /// Require every child in source order.
    And {
        /// Non-empty children.
        patterns: Vec<ModelPredicate>,
    },
    /// Require at least one branch in source order.
    Or {
        /// Non-empty branches.
        patterns: Vec<ModelPredicate>,
    },
    /// Negate one closed child.
    Not {
        /// Negated child.
        pattern: Box<ModelPredicate>,
    },
}

/// Direction of one model-query order term.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelOrderDirection {
    /// Smallest first.
    Ascending,
    /// Largest first.
    Descending,
}

/// Placement of missing order values.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelMissingOrder {
    /// Missing evidence is invalid.
    Reject,
    /// Missing values precede present values.
    First,
    /// Missing values follow present values.
    Last,
}

/// One descriptor-qualified model-query order term.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelOrderTerm {
    pub(super) direction: ModelOrderDirection,
    pub(super) field: ModelField,
    pub(super) missing: ModelMissingOrder,
}

impl ModelOrderTerm {
    /// Construct one order term.
    #[must_use]
    pub const fn new(
        field: ModelField,
        direction: ModelOrderDirection,
        missing: ModelMissingOrder,
    ) -> Self {
        Self {
            direction,
            field,
            missing,
        }
    }

    /// Return the ordered field.
    #[must_use]
    pub const fn field(&self) -> &ModelField {
        &self.field
    }

    /// Return the direction.
    #[must_use]
    pub const fn direction(&self) -> ModelOrderDirection {
        self.direction
    }

    /// Return missing-value placement.
    #[must_use]
    pub const fn missing(&self) -> ModelMissingOrder {
        self.missing
    }
}

/// A validator-proven stable total order.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelStableOrder {
    #[serde(deserialize_with = "deserialize_bindings")]
    pub(super) identity_tiebreakers: Vec<BindingId>,
    pub(super) terms: Vec<ModelOrderTerm>,
}

impl ModelStableOrder {
    /// Construct one stable order with its explicit identity tie breaker.
    #[must_use]
    pub const fn new(terms: Vec<ModelOrderTerm>, identity_tiebreakers: Vec<BindingId>) -> Self {
        Self {
            identity_tiebreakers,
            terms,
        }
    }

    /// Return public field terms.
    #[must_use]
    pub fn terms(&self) -> &[ModelOrderTerm] {
        &self.terms
    }

    /// Return the canonical identity tuple that makes the order total.
    #[must_use]
    pub fn identity_tiebreakers(&self) -> &[BindingId] {
        &self.identity_tiebreakers
    }
}

/// One public model-query output slot.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModelOutputSlot {
    /// Select one concept.
    One {
        /// Selected binding.
        #[serde(deserialize_with = "deserialize_binding")]
        binding: BindingId,
        /// Descriptor under which this occurrence was selected.
        declared: TypeId,
    },
    /// Collect concepts per selected row/root.
    Collect {
        /// Collected binding.
        #[serde(deserialize_with = "deserialize_binding")]
        binding: BindingId,
        /// Descriptor under which these occurrences were collected.
        declared: TypeId,
        /// Remove multiplicity by concept identity.
        distinct: bool,
        /// Stable collection-member order.
        order: ModelStableOrder,
    },
}

impl ModelOutputSlot {
    /// Return the selected binding.
    #[must_use]
    pub const fn binding(&self) -> BindingId {
        match self {
            Self::One { binding, .. } | Self::Collect { binding, .. } => *binding,
        }
    }

    /// Return the descriptor under which this output occurrence was selected.
    #[must_use]
    pub const fn declared(&self) -> &TypeId {
        match self {
            Self::One { declared, .. } | Self::Collect { declared, .. } => declared,
        }
    }

    /// Return whether this slot is a collection.
    #[must_use]
    pub const fn collection(&self) -> bool {
        matches!(self, Self::Collect { .. })
    }

    /// Return whether collection multiplicity is removed by concept identity.
    #[must_use]
    pub const fn distinct(&self) -> bool {
        matches!(self, Self::Collect { distinct: true, .. })
    }
}

/// One named public output member.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelNamedOutputSlot {
    pub(super) name: String,
    pub(super) slot: ModelOutputSlot,
}

impl ModelNamedOutputSlot {
    /// Construct one named output member.
    #[must_use]
    pub fn new(name: impl Into<String>, slot: ModelOutputSlot) -> Self {
        Self {
            name: name.into(),
            slot,
        }
    }

    /// Return the public member name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Return the selected slot.
    #[must_use]
    pub const fn slot(&self) -> &ModelOutputSlot {
        &self.slot
    }
}

/// Positional or named model-query output shape.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModelOutput {
    /// Tuple-like members in public order.
    Positional {
        /// Public slots.
        slots: Vec<ModelOutputSlot>,
    },
    /// Record-like members in public order.
    Named {
        /// Public named slots.
        slots: Vec<ModelNamedOutputSlot>,
    },
}

impl ModelOutput {
    /// Return model output slots in public order.
    ///
    /// Named output metadata remains available on the enum; this allocation is
    /// a small bounded view used by response-shape validators.
    #[must_use]
    pub fn slots(&self) -> Vec<&ModelOutputSlot> {
        match self {
            Self::Positional { slots } => slots.iter().collect(),
            Self::Named { slots } => slots.iter().map(ModelNamedOutputSlot::slot).collect(),
        }
    }
}

/// One descriptor-qualified hydrated model field.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HydrationField {
    pub(super) alias: String,
    pub(super) attribute: AttributeId,
    pub(super) cardinality: Cardinality,
    pub(super) distinct: bool,
    pub(super) ordered: bool,
    pub(super) reference_owners: Vec<TypeId>,
    pub(super) unique: bool,
    pub(super) value_type: ValueTypeTag,
}

impl HydrationField {
    /// Construct one field projection.
    #[must_use]
    #[expect(
        clippy::too_many_arguments,
        reason = "the constructor mirrors the closed hydration-field wire contract"
    )]
    pub fn new(
        alias: impl Into<String>,
        reference_owners: Vec<TypeId>,
        attribute: AttributeId,
        value_type: ValueTypeTag,
        cardinality: Cardinality,
        ordered: bool,
        distinct: bool,
        unique: bool,
    ) -> Self {
        Self {
            alias: alias.into(),
            attribute,
            cardinality,
            distinct,
            ordered,
            reference_owners,
            unique,
            value_type,
        }
    }

    /// Return the generated-model field alias.
    #[must_use]
    pub fn alias(&self) -> &str {
        &self.alias
    }

    /// Return the provider attribute identity.
    #[must_use]
    pub const fn attribute(&self) -> &AttributeId {
        &self.attribute
    }

    /// Return every owner descriptor under which this field can be referenced.
    #[must_use]
    pub fn reference_owners(&self) -> &[TypeId] {
        &self.reference_owners
    }

    /// Return the scalar domain.
    #[must_use]
    pub const fn value_type(&self) -> ValueTypeTag {
        self.value_type
    }

    /// Return the effective ownership cardinality.
    #[must_use]
    pub const fn cardinality(&self) -> Cardinality {
        self.cardinality
    }

    /// Return whether the registry proves this scalar field unique.
    #[must_use]
    pub const fn unique(&self) -> bool {
        self.unique
    }

    /// Return whether this field preserves list order.
    #[must_use]
    pub const fn ordered(&self) -> bool {
        self.ordered
    }

    /// Return whether an ordered list forbids repeated attribute values.
    #[must_use]
    pub const fn distinct(&self) -> bool {
        self.distinct
    }
}

/// Declared-to-concrete authority for one hydrated role-player occurrence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HydrationPlayer {
    pub(super) concrete_descriptors: Vec<TypeId>,
    pub(super) declared_descriptor: TypeId,
}

impl HydrationPlayer {
    /// Construct one closed player compatibility projection.
    #[must_use]
    pub const fn new(declared_descriptor: TypeId, concrete_descriptors: Vec<TypeId>) -> Self {
        Self {
            concrete_descriptors,
            declared_descriptor,
        }
    }

    /// Return the descriptor carried by the graph reference.
    #[must_use]
    pub const fn declared_descriptor(&self) -> &TypeId {
        &self.declared_descriptor
    }

    /// Return every concrete descriptor admitted under the declaration.
    #[must_use]
    pub fn concrete_descriptors(&self) -> &[TypeId] {
        &self.concrete_descriptors
    }
}

/// One descriptor-qualified hydrated relation role.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HydrationRole {
    pub(super) cardinality: Cardinality,
    pub(super) distinct: bool,
    pub(super) ordered: bool,
    pub(super) players: Vec<HydrationPlayer>,
    pub(super) reference_roles: Vec<RoleId>,
    pub(super) role: RoleId,
    /// Parent roles this role replaces by specialization (`relates worker as
    /// member`). Absent for roles that specialize nothing, so existing
    /// encodings are unchanged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) specialized_roles: Vec<RoleId>,
}

impl HydrationRole {
    /// Construct one complete role projection.
    #[must_use]
    pub const fn new(
        role: RoleId,
        reference_roles: Vec<RoleId>,
        players: Vec<HydrationPlayer>,
        cardinality: Cardinality,
        ordered: bool,
        distinct: bool,
    ) -> Self {
        Self {
            cardinality,
            distinct,
            ordered,
            players,
            reference_roles,
            role,
            specialized_roles: Vec::new(),
        }
    }

    /// Record the parent roles this role replaces by specialization.
    #[must_use]
    pub fn with_specialized_roles(mut self, mut roles: Vec<RoleId>) -> Self {
        roles.sort();
        roles.dedup();
        self.specialized_roles = roles;
        self
    }

    /// Return the parent roles this role replaces by specialization. TypeDB
    /// answers a replaced parent role on this relation with this role's players.
    #[must_use]
    pub fn specialized_roles(&self) -> &[RoleId] {
        &self.specialized_roles
    }

    /// Return the descriptor-qualified role.
    #[must_use]
    pub const fn role(&self) -> &RoleId {
        &self.role
    }

    /// Return every inherited/effective role reference admitted for this role.
    #[must_use]
    pub fn reference_roles(&self) -> &[RoleId] {
        &self.reference_roles
    }

    /// Return closed declared-to-concrete player authorities.
    #[must_use]
    pub fn players(&self) -> &[HydrationPlayer] {
        &self.players
    }

    /// Return the effective role cardinality.
    #[must_use]
    pub const fn cardinality(&self) -> Cardinality {
        self.cardinality
    }

    /// Return whether provider role-player order is semantic.
    #[must_use]
    pub const fn ordered(&self) -> bool {
        self.ordered
    }

    /// Return whether an ordered role forbids repeated players.
    #[must_use]
    pub const fn distinct(&self) -> bool {
        self.distinct
    }
}

/// Complete field/role authority for one concrete model descriptor.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HydrationDescriptor {
    pub(super) descriptor: TypeId,
    pub(super) fields: Vec<HydrationField>,
    pub(super) roles: Vec<HydrationRole>,
}

impl HydrationDescriptor {
    /// Construct one concrete descriptor projection.
    #[must_use]
    pub const fn new(
        descriptor: TypeId,
        fields: Vec<HydrationField>,
        roles: Vec<HydrationRole>,
    ) -> Self {
        Self {
            descriptor,
            fields,
            roles,
        }
    }

    /// Return the exact concrete descriptor.
    #[must_use]
    pub const fn descriptor(&self) -> &TypeId {
        &self.descriptor
    }

    /// Return complete concrete-descriptor fields.
    #[must_use]
    pub fn fields(&self) -> &[HydrationField] {
        &self.fields
    }

    /// Return complete selected-relation roles.
    #[must_use]
    pub fn roles(&self) -> &[HydrationRole] {
        &self.roles
    }
}

/// Declared-to-concrete subtype authority for one plan binding.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HydrationBinding {
    #[serde(deserialize_with = "deserialize_binding")]
    pub(super) binding: BindingId,
    pub(super) concrete_descriptors: Vec<TypeId>,
    pub(super) declared_descriptor: TypeId,
}

impl HydrationBinding {
    /// Construct one binding hydration projection.
    #[must_use]
    pub const fn new(
        binding: BindingId,
        declared_descriptor: TypeId,
        concrete_descriptors: Vec<TypeId>,
    ) -> Self {
        Self {
            binding,
            concrete_descriptors,
            declared_descriptor,
        }
    }

    /// Return the plan binding.
    #[must_use]
    pub const fn binding(&self) -> BindingId {
        self.binding
    }

    /// Return the declared descriptor selected by the model surface.
    #[must_use]
    pub const fn declared_descriptor(&self) -> &TypeId {
        &self.declared_descriptor
    }

    /// Return every admitted registered concrete descriptor.
    #[must_use]
    pub fn concrete_descriptors(&self) -> &[TypeId] {
        &self.concrete_descriptors
    }
}

/// Complete same-snapshot hydration authority for one model query.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HydrationProjection {
    pub(super) bindings: Vec<HydrationBinding>,
    pub(super) descriptors: Vec<HydrationDescriptor>,
}

impl HydrationProjection {
    /// Construct one complete hydration projection.
    #[must_use]
    pub const fn new(
        bindings: Vec<HydrationBinding>,
        descriptors: Vec<HydrationDescriptor>,
    ) -> Self {
        Self {
            bindings,
            descriptors,
        }
    }

    /// Return declared-to-concrete binding projections.
    #[must_use]
    pub fn bindings(&self) -> &[HydrationBinding] {
        &self.bindings
    }

    /// Return exact concrete descriptor projections.
    #[must_use]
    pub fn descriptors(&self) -> &[HydrationDescriptor] {
        &self.descriptors
    }
}

/// The closed reducer vocabulary used by model-request terminals.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelReductionKind {
    /// Count distinct reduced roots.
    Count,
    /// Sum a numeric scalar field.
    Sum,
    /// Select the smallest numeric scalar field value.
    Min,
    /// Select the largest numeric scalar field value.
    Max,
    /// Compute the arithmetic mean.
    Mean,
    /// Compute the statistical median.
    Median,
    /// Compute the sample standard deviation.
    Std,
}

impl ModelReductionKind {
    /// Return whether this reducer consumes a numeric scalar field.
    #[must_use]
    pub const fn requires_input(self) -> bool {
        !matches!(self, Self::Count)
    }

    /// Return whether this reducer always returns the double domain.
    #[must_use]
    pub const fn widens_to_double(self) -> bool {
        matches!(self, Self::Mean | Self::Median | Self::Std)
    }
}

/// One ordered typed reducer term.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelReductionTerm {
    pub(super) input: Option<ModelField>,
    pub(super) reduction: ModelReductionKind,
}

impl ModelReductionTerm {
    /// Construct one typed reducer term.
    #[must_use]
    pub const fn new(reduction: ModelReductionKind, input: Option<ModelField>) -> Self {
        Self { input, reduction }
    }

    /// Return the reducer operation.
    #[must_use]
    pub const fn reduction(&self) -> ModelReductionKind {
        self.reduction
    }

    /// Return the optional numeric input field.
    #[must_use]
    pub const fn input(&self) -> Option<&ModelField> {
        self.input.as_ref()
    }
}

/// Optional grouping contract for one model-compatible reduction terminal.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModelReductionKey {
    /// Group by one attached thing identity.
    Binding {
        /// Attached grouping binding.
        #[serde(deserialize_with = "deserialize_binding")]
        binding: BindingId,
    },
    /// Group by each witnessed value of one owned field.
    Field {
        /// Descriptor-qualified grouping field.
        field: ModelField,
    },
    /// Group by the Cartesian product of multiple owned field values.
    Fields {
        /// Ordered descriptor-qualified grouping fields.
        fields: Vec<ModelField>,
    },
}

impl ModelReductionKey {
    /// Construct a thing-identity grouping contract.
    #[must_use]
    pub const fn binding(binding: BindingId) -> Self {
        Self::Binding { binding }
    }

    /// Construct a single-field grouping contract.
    #[must_use]
    pub const fn field(field: ModelField) -> Self {
        Self::Field { field }
    }

    /// Construct a tuple-field grouping contract.
    #[must_use]
    pub const fn fields(fields: Vec<ModelField>) -> Self {
        Self::Fields { fields }
    }
}

pub(super) fn deserialize_binding<'de, D>(deserializer: D) -> Result<BindingId, D::Error>
where
    D: Deserializer<'de>,
{
    BindingId::new(u16::deserialize(deserializer)?).map_err(D::Error::custom)
}

pub(super) fn deserialize_bindings<'de, D>(deserializer: D) -> Result<Vec<BindingId>, D::Error>
where
    D: Deserializer<'de>,
{
    Vec::<u16>::deserialize(deserializer)?
        .into_iter()
        .map(|value| BindingId::new(value).map_err(D::Error::custom))
        .collect()
}
