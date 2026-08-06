//! Additive query-plan V2 compatibility contracts.
//!
//! The ordinary typed query vocabulary remains in [`super`]. This module owns
//! the extra closed algebra needed to losslessly adapt the released
//! model-oriented match request without accepting raw TypeQL or host-authored
//! JSON fragments.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::decimal::parse_decimal;
use crate::diagnostic::{Diagnostic, DiagnosticCategory};
use crate::id::{AttributeId, RoleId, TypeId, TypeKind, is_canonical_thing_iid};
use crate::limits::{MAX_CANONICAL_BYTES, MAX_CANONICAL_STRING_BYTES, StructuralLimits};
use crate::migration_assertion::BindingId;
use crate::temporal::{CanonicalDateTime, CanonicalDateTimeTz, CanonicalDuration};
use crate::value::{CanonicalValue, Cardinality, ValueTypeTag};

use super::{failure, insert_capability};
use crate::capability::CapabilitySet;

pub(crate) const CAP_PLAN_V2: &str = "query.plan.v2";
pub(crate) const CAP_DISJUNCTION: &str = "query.pattern.disjunction";
pub(crate) const CAP_STRING_OPERATORS: &str = "query.pattern.string-operators";
pub(crate) const CAP_LINKS_SUBTYPES: &str = "query.pattern.links-subtypes";
pub(crate) const CAP_IID: &str = "query.pattern.iid";
pub(crate) const CAP_CROSS_JOIN: &str = "query.topology.cross-join";
pub(crate) const CAP_OUTPUT_NAMED: &str = "query.output.named";
pub(crate) const CAP_OUTPUT_COLLECT: &str = "query.output.collect";
pub(crate) const CAP_OUTPUT_COLLECT_DISTINCT: &str = "query.output.collect-distinct";
pub(crate) const CAP_OUTPUT_HYDRATED: &str = "query.output.hydrated";
pub(crate) const CAP_EXACTLY_ONE: &str = "query.operation.exactly-one";
pub(crate) const CAP_PAGE: &str = "query.operation.page";
pub(crate) const CAP_DISTINCT_COUNT: &str = "query.operation.distinct-count";
pub(crate) const CAP_DISTINCT_EXISTS: &str = "query.operation.distinct-exists";
pub(crate) const CAP_STABLE_SELECTED: &str = "query.order.stable-selected";
pub(crate) const CAP_STABLE_ROOT: &str = "query.order.stable-root";
pub(crate) const CAP_STABLE_COLLECTION: &str = "query.order.stable-collection";
pub(crate) const CAP_SAME_SNAPSHOT_HYDRATION: &str = "query.execution.same-snapshot-hydration";
pub(crate) const CAP_BATCH_IDENTITY_REBIND: &str = "query.execution.batch-identity-rebind";

/// One descriptor-qualified field reference in the compatibility algebra.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryFieldV2 {
    attribute: AttributeId,
    #[serde(deserialize_with = "deserialize_binding")]
    binding: BindingId,
    descriptor: TypeId,
    value_type: ValueTypeTag,
}

impl QueryFieldV2 {
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
pub enum QueryComparatorV2 {
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

impl QueryComparatorV2 {
    const fn is_string_operator(self) -> bool {
        matches!(
            self,
            Self::Contains | Self::StartsWith | Self::EndsWith | Self::Regex
        )
    }
}

/// A released-only lexical scalar domain carried by a V2 compatibility value.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ReleasedValueKindV2 {
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

impl ReleasedValueKindV2 {
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
enum CompatibilityValueV2Inner {
    Canonical(CanonicalValue),
    Released {
        kind: ReleasedValueKindV2,
        chunks: Vec<String>,
    },
}

/// One exact V2 compatibility scalar.
///
/// Ordinary values retain the released [`CanonicalValue`] wire. V1-only
/// strings and non-canonical temporal, duration, or decimal spellings use
/// deterministic bounded UTF-8 chunks so no individual JSON string exceeds
/// the canonical codec ceiling.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CompatibilityValueV2 {
    inner: CompatibilityValueV2Inner,
}

impl CompatibilityValueV2 {
    /// Wrap one ordinary canonical scalar without changing its wire shape.
    #[must_use]
    pub const fn canonical(value: CanonicalValue) -> Self {
        Self {
            inner: CompatibilityValueV2Inner::Canonical(value),
        }
    }

    /// Preserve one released string above the canonical per-string ceiling.
    pub fn released_string(value: impl Into<String>) -> Result<Self, Diagnostic> {
        Self::released(ReleasedValueKindV2::String, value.into())
    }

    /// Preserve one released timezone-free datetime spelling exactly.
    pub fn released_datetime(value: impl Into<String>) -> Result<Self, Diagnostic> {
        Self::released(ReleasedValueKindV2::DateTime, value.into())
    }

    /// Preserve one released timezone-aware datetime spelling exactly.
    pub fn released_datetime_tz(value: impl Into<String>) -> Result<Self, Diagnostic> {
        Self::released(ReleasedValueKindV2::DateTimeTz, value.into())
    }

    /// Preserve one released duration spelling exactly.
    pub fn released_duration(value: impl Into<String>) -> Result<Self, Diagnostic> {
        Self::released(ReleasedValueKindV2::Duration, value.into())
    }

    /// Preserve one released non-canonical decimal spelling exactly.
    pub fn released_decimal(value: impl Into<String>) -> Result<Self, Diagnostic> {
        Self::released(ReleasedValueKindV2::Decimal, value.into())
    }

    fn released(kind: ReleasedValueKindV2, value: String) -> Result<Self, Diagnostic> {
        let value = Self {
            inner: CompatibilityValueV2Inner::Released {
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
            CompatibilityValueV2Inner::Canonical(value) => value.value_type(),
            CompatibilityValueV2Inner::Released { kind, .. } => kind.value_type(),
        }
    }

    /// Return the ordinary canonical value, when this is not a V1-only spelling.
    #[must_use]
    pub const fn canonical_value(&self) -> Option<&CanonicalValue> {
        match &self.inner {
            CompatibilityValueV2Inner::Canonical(value) => Some(value),
            CompatibilityValueV2Inner::Released { .. } => None,
        }
    }

    /// Return the released-only lexical domain, when present.
    #[must_use]
    pub const fn released_kind(&self) -> Option<ReleasedValueKindV2> {
        match &self.inner {
            CompatibilityValueV2Inner::Canonical(_) => None,
            CompatibilityValueV2Inner::Released { kind, .. } => Some(*kind),
        }
    }

    /// Return canonical bounded chunks for a released-only spelling.
    #[must_use]
    pub fn released_chunks(&self) -> Option<&[String]> {
        match &self.inner {
            CompatibilityValueV2Inner::Canonical(_) => None,
            CompatibilityValueV2Inner::Released { chunks, .. } => Some(chunks),
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

    fn validate(&self) -> Result<(), Diagnostic> {
        let CompatibilityValueV2Inner::Released { kind, chunks } = &self.inner else {
            return Ok(());
        };
        let byte_len = chunks
            .iter()
            .try_fold(0usize, |total, chunk| total.checked_add(chunk.len()));
        let Some(byte_len) = byte_len else {
            return Err(failure(
                DiagnosticCategory::ResourceLimit,
                "query_plan_v2_compatibility_value_limit",
                "compatibility value bytes overflow the bounded artifact domain",
            ));
        };
        if byte_len > MAX_CANONICAL_BYTES {
            return Err(failure(
                DiagnosticCategory::ResourceLimit,
                "query_plan_v2_compatibility_value_limit",
                "compatibility value exceeds the canonical artifact byte ceiling",
            ));
        }
        let value = chunks.concat();
        if chunks != &released_value_chunks(&value) {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_v2_compatibility_value_chunks",
                "compatibility value chunks are not in canonical UTF-8 boundaries",
            ));
        }
        let valid = match kind {
            ReleasedValueKindV2::String => value.len() > MAX_CANONICAL_STRING_BYTES,
            ReleasedValueKindV2::DateTime => {
                released_valid_datetime(&value, false)
                    && value.parse::<CanonicalDateTime>().is_err()
            }
            ReleasedValueKindV2::DateTimeTz => {
                released_valid_datetime(&value, true)
                    && value.parse::<CanonicalDateTimeTz>().is_err()
            }
            ReleasedValueKindV2::Duration => {
                released_valid_duration(&value) && value.parse::<CanonicalDuration>().is_err()
            }
            ReleasedValueKindV2::Decimal => {
                parse_decimal(&value).is_some_and(|parsed| parsed.canonical_string() != value)
            }
        };
        if valid {
            Ok(())
        } else {
            Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_v2_compatibility_value_lexical",
                "compatibility value is invalid or has an ordinary canonical representation",
            ))
        }
    }
}

fn compatibility_value_text(value: &CompatibilityValueV2) -> Option<String> {
    match &value.inner {
        CompatibilityValueV2Inner::Released { chunks, .. } => Some(chunks.concat()),
        CompatibilityValueV2Inner::Canonical(value) => match value {
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

fn compatibility_string_bytes(
    value: &CompatibilityValueV2,
) -> Option<Box<dyn Iterator<Item = u8> + '_>> {
    match &value.inner {
        CompatibilityValueV2Inner::Canonical(CanonicalValue::String(value)) => {
            Some(Box::new(value.as_str().bytes()))
        }
        CompatibilityValueV2Inner::Released {
            kind: ReleasedValueKindV2::String,
            chunks,
        } => Some(Box::new(chunks.iter().flat_map(|chunk| chunk.bytes()))),
        CompatibilityValueV2Inner::Canonical(_) | CompatibilityValueV2Inner::Released { .. } => {
            None
        }
    }
}

impl From<CanonicalValue> for CompatibilityValueV2 {
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

impl Serialize for CompatibilityValueV2 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match &self.inner {
            CompatibilityValueV2Inner::Canonical(value) => value.serialize(serializer),
            CompatibilityValueV2Inner::Released { kind, chunks } => match kind {
                ReleasedValueKindV2::String => {
                    ReleasedValueWireRefV2::ReleasedString { chunks }.serialize(serializer)
                }
                ReleasedValueKindV2::DateTime => {
                    ReleasedValueWireRefV2::ReleasedDatetime { chunks }.serialize(serializer)
                }
                ReleasedValueKindV2::DateTimeTz => {
                    ReleasedValueWireRefV2::ReleasedDatetimeTz { chunks }.serialize(serializer)
                }
                ReleasedValueKindV2::Duration => {
                    ReleasedValueWireRefV2::ReleasedDuration { chunks }.serialize(serializer)
                }
                ReleasedValueKindV2::Decimal => {
                    ReleasedValueWireRefV2::ReleasedDecimal { chunks }.serialize(serializer)
                }
            },
        }
    }
}

impl<'de> Deserialize<'de> for CompatibilityValueV2 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let inner = match CompatibilityValueWireV2::deserialize(deserializer)? {
            CompatibilityValueWireV2::Canonical(value) => {
                CompatibilityValueV2Inner::Canonical(value)
            }
            CompatibilityValueWireV2::Released(wire) => match wire {
                ReleasedValueWireV2::ReleasedString { chunks } => {
                    CompatibilityValueV2Inner::Released {
                        kind: ReleasedValueKindV2::String,
                        chunks,
                    }
                }
                ReleasedValueWireV2::ReleasedDatetime { chunks } => {
                    CompatibilityValueV2Inner::Released {
                        kind: ReleasedValueKindV2::DateTime,
                        chunks,
                    }
                }
                ReleasedValueWireV2::ReleasedDatetimeTz { chunks } => {
                    CompatibilityValueV2Inner::Released {
                        kind: ReleasedValueKindV2::DateTimeTz,
                        chunks,
                    }
                }
                ReleasedValueWireV2::ReleasedDuration { chunks } => {
                    CompatibilityValueV2Inner::Released {
                        kind: ReleasedValueKindV2::Duration,
                        chunks,
                    }
                }
                ReleasedValueWireV2::ReleasedDecimal { chunks } => {
                    CompatibilityValueV2Inner::Released {
                        kind: ReleasedValueKindV2::Decimal,
                        chunks,
                    }
                }
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
                let (whole, fraction) = number
                    .split_once('.')
                    .map_or((number.as_str(), ""), |parts| parts);
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
pub enum QueryPatternV2 {
    /// Compare one bound field with one exact scalar literal.
    FieldValue {
        /// Descriptor-qualified field.
        field: QueryFieldV2,
        /// Closed comparison operator.
        comparator: QueryComparatorV2,
        /// Canonical or released-only typed literal.
        value: CompatibilityValueV2,
    },
    /// Compare two descriptor-qualified bound fields.
    FieldComparison {
        /// Left field.
        left: QueryFieldV2,
        /// Closed non-string comparison operator.
        comparator: QueryComparatorV2,
        /// Right field.
        right: QueryFieldV2,
    },
    /// Require one descriptor-qualified field to be present or absent.
    FieldPresence {
        /// Descriptor-qualified field.
        field: QueryFieldV2,
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
        patterns: Vec<QueryPatternV2>,
    },
    /// Require at least one branch in source order.
    Or {
        /// Non-empty branches.
        patterns: Vec<QueryPatternV2>,
    },
    /// Negate one closed child.
    Not {
        /// Negated child.
        pattern: Box<QueryPatternV2>,
    },
}

/// One canonical explicit cross-join permission.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryBindingPairV2 {
    #[serde(deserialize_with = "deserialize_binding")]
    left: BindingId,
    #[serde(deserialize_with = "deserialize_binding")]
    right: BindingId,
}

impl QueryBindingPairV2 {
    /// Construct a pair in ascending binding-ID order.
    #[must_use]
    pub const fn new(first: BindingId, second: BindingId) -> Self {
        if first.get() <= second.get() {
            Self {
                left: first,
                right: second,
            }
        } else {
            Self {
                left: second,
                right: first,
            }
        }
    }

    /// Return the lower binding ID.
    #[must_use]
    pub const fn left(self) -> BindingId {
        self.left
    }

    /// Return the higher binding ID.
    #[must_use]
    pub const fn right(self) -> BindingId {
        self.right
    }
}

/// Direction of one model-query order term.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryOrderDirectionV2 {
    /// Smallest first.
    Ascending,
    /// Largest first.
    Descending,
}

/// Placement of missing order values.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryMissingOrderV2 {
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
pub struct QueryOrderTermV2 {
    direction: QueryOrderDirectionV2,
    field: QueryFieldV2,
    missing: QueryMissingOrderV2,
}

impl QueryOrderTermV2 {
    /// Construct one order term.
    #[must_use]
    pub const fn new(
        field: QueryFieldV2,
        direction: QueryOrderDirectionV2,
        missing: QueryMissingOrderV2,
    ) -> Self {
        Self {
            direction,
            field,
            missing,
        }
    }

    /// Return the ordered field.
    #[must_use]
    pub const fn field(&self) -> &QueryFieldV2 {
        &self.field
    }

    /// Return the direction.
    #[must_use]
    pub const fn direction(&self) -> QueryOrderDirectionV2 {
        self.direction
    }

    /// Return missing-value placement.
    #[must_use]
    pub const fn missing(&self) -> QueryMissingOrderV2 {
        self.missing
    }
}

/// A validator-proven stable total order.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryStableOrderV2 {
    #[serde(deserialize_with = "deserialize_bindings")]
    identity_tiebreakers: Vec<BindingId>,
    terms: Vec<QueryOrderTermV2>,
}

impl QueryStableOrderV2 {
    /// Construct one stable order with its explicit identity tie breaker.
    #[must_use]
    pub const fn new(terms: Vec<QueryOrderTermV2>, identity_tiebreakers: Vec<BindingId>) -> Self {
        Self {
            identity_tiebreakers,
            terms,
        }
    }

    /// Return public field terms.
    #[must_use]
    pub fn terms(&self) -> &[QueryOrderTermV2] {
        &self.terms
    }

    /// Return the canonical identity tuple that makes the order total.
    #[must_use]
    pub fn identity_tiebreakers(&self) -> &[BindingId] {
        &self.identity_tiebreakers
    }
}

/// One bounded model-query result window.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryWindowV2 {
    limit: u64,
    offset: u64,
}

impl QueryWindowV2 {
    /// Construct an exact offset and positive limit.
    #[must_use]
    pub const fn new(offset: u64, limit: u64) -> Self {
        Self { limit, offset }
    }

    /// Return the skipped distinct row/root count.
    #[must_use]
    pub const fn offset(self) -> u64 {
        self.offset
    }

    /// Return the maximum returned distinct row/root count.
    #[must_use]
    pub const fn limit(self) -> u64 {
        self.limit
    }
}

/// One public model-query output slot.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum QueryModelOutputSlotV2 {
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
        order: QueryStableOrderV2,
    },
}

impl QueryModelOutputSlotV2 {
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
pub struct QueryNamedOutputSlotV2 {
    name: String,
    slot: QueryModelOutputSlotV2,
}

impl QueryNamedOutputSlotV2 {
    /// Construct one named output member.
    #[must_use]
    pub fn new(name: impl Into<String>, slot: QueryModelOutputSlotV2) -> Self {
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
    pub const fn slot(&self) -> &QueryModelOutputSlotV2 {
        &self.slot
    }
}

/// Positional or named model-query output shape.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum QueryModelOutputV2 {
    /// Tuple-like members in public order.
    Positional {
        /// Public slots.
        slots: Vec<QueryModelOutputSlotV2>,
    },
    /// Record-like members in public order.
    Named {
        /// Public named slots.
        slots: Vec<QueryNamedOutputSlotV2>,
    },
}

impl QueryModelOutputV2 {
    fn slot_count(&self) -> usize {
        match self {
            Self::Positional { slots } => slots.len(),
            Self::Named { slots } => slots.len(),
        }
    }

    fn visit_slots(&self, mut visit: impl FnMut(Option<&str>, &QueryModelOutputSlotV2)) {
        match self {
            Self::Positional { slots } => {
                for slot in slots {
                    visit(None, slot);
                }
            }
            Self::Named { slots } => {
                for slot in slots {
                    visit(Some(slot.name()), slot.slot());
                }
            }
        }
    }

    /// Return model output slots in public order.
    ///
    /// Named output metadata remains available on the enum; this allocation is
    /// a small bounded view used by response-shape validators.
    #[must_use]
    pub fn slots(&self) -> Vec<&QueryModelOutputSlotV2> {
        match self {
            Self::Positional { slots } => slots.iter().collect(),
            Self::Named { slots } => slots.iter().map(QueryNamedOutputSlotV2::slot).collect(),
        }
    }
}

/// Concise alias used by remote response-shape contracts.
pub type ModelOutputV2 = QueryModelOutputV2;

/// One descriptor-qualified hydrated model field.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HydrationFieldV2 {
    alias: String,
    attribute: AttributeId,
    cardinality: Cardinality,
    distinct: bool,
    ordered: bool,
    reference_owners: Vec<TypeId>,
    unique: bool,
    value_type: ValueTypeTag,
}

impl HydrationFieldV2 {
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
pub struct HydrationPlayerV2 {
    concrete_descriptors: Vec<TypeId>,
    declared_descriptor: TypeId,
}

impl HydrationPlayerV2 {
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
pub struct HydrationRoleV2 {
    cardinality: Cardinality,
    distinct: bool,
    ordered: bool,
    players: Vec<HydrationPlayerV2>,
    reference_roles: Vec<RoleId>,
    role: RoleId,
}

impl HydrationRoleV2 {
    /// Construct one complete role projection.
    #[must_use]
    pub const fn new(
        role: RoleId,
        reference_roles: Vec<RoleId>,
        players: Vec<HydrationPlayerV2>,
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
        }
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
    pub fn players(&self) -> &[HydrationPlayerV2] {
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
pub struct HydrationDescriptorV2 {
    descriptor: TypeId,
    fields: Vec<HydrationFieldV2>,
    roles: Vec<HydrationRoleV2>,
}

impl HydrationDescriptorV2 {
    /// Construct one concrete descriptor projection.
    #[must_use]
    pub const fn new(
        descriptor: TypeId,
        fields: Vec<HydrationFieldV2>,
        roles: Vec<HydrationRoleV2>,
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
    pub fn fields(&self) -> &[HydrationFieldV2] {
        &self.fields
    }

    /// Return complete selected-relation roles.
    #[must_use]
    pub fn roles(&self) -> &[HydrationRoleV2] {
        &self.roles
    }
}

/// Declared-to-concrete subtype authority for one plan binding.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HydrationBindingV2 {
    #[serde(deserialize_with = "deserialize_binding")]
    binding: BindingId,
    concrete_descriptors: Vec<TypeId>,
    declared_descriptor: TypeId,
}

impl HydrationBindingV2 {
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
pub struct HydrationProjectionV2 {
    bindings: Vec<HydrationBindingV2>,
    descriptors: Vec<HydrationDescriptorV2>,
}

impl HydrationProjectionV2 {
    /// Construct one complete hydration projection.
    #[must_use]
    pub const fn new(
        bindings: Vec<HydrationBindingV2>,
        descriptors: Vec<HydrationDescriptorV2>,
    ) -> Self {
        Self {
            bindings,
            descriptors,
        }
    }

    /// Return declared-to-concrete binding projections.
    #[must_use]
    pub fn bindings(&self) -> &[HydrationBindingV2] {
        &self.bindings
    }

    /// Return exact concrete descriptor projections.
    #[must_use]
    pub fn descriptors(&self) -> &[HydrationDescriptorV2] {
        &self.descriptors
    }
}

/// Cardinality required from a hydrated row query.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryRowCardinalityV2 {
    /// Require exactly one distinct selected-identity tuple.
    ExactlyOne,
    /// Return one bounded stable row window.
    BoundedMany,
}

impl QueryRowCardinalityV2 {
    /// Return whether exactly one distinct selected tuple is required.
    #[must_use]
    pub const fn is_exactly_one(self) -> bool {
        matches!(self, Self::ExactlyOne)
    }
}

/// The released model-query terminal contract carried by a V2 plan.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModelQueryV2 {
    /// Return hydrated selected rows.
    Rows {
        /// Required selected-row cardinality.
        cardinality: QueryRowCardinalityV2,
        /// Complete hydration authority.
        hydration: HydrationProjectionV2,
        /// Stable order; absent only for exactly-one.
        order: Option<QueryStableOrderV2>,
        /// Public output shape.
        output: QueryModelOutputV2,
        /// Exact row window.
        window: QueryWindowV2,
    },
    /// Page by distinct root identity and hydrate in the same snapshot.
    Page {
        /// Complete hydration authority.
        hydration: HydrationProjectionV2,
        /// Whether the same snapshot also returns a distinct-root total.
        include_total: bool,
        /// Stable distinct-root order.
        order: QueryStableOrderV2,
        /// Public output shape.
        output: QueryModelOutputV2,
        /// Root identity binding.
        #[serde(deserialize_with = "deserialize_binding")]
        root: BindingId,
        /// Exact root window.
        window: QueryWindowV2,
    },
    /// Count distinct root identities losslessly.
    DistinctCount {
        /// Complete plan-side descriptor and predicate authority.
        hydration: HydrationProjectionV2,
        /// Root identity binding.
        #[serde(deserialize_with = "deserialize_binding")]
        root: BindingId,
    },
    /// Test distinct-root existence.
    DistinctExists {
        /// Complete plan-side descriptor and predicate authority.
        hydration: HydrationProjectionV2,
        /// Root identity binding.
        #[serde(deserialize_with = "deserialize_binding")]
        root: BindingId,
    },
}

fn deserialize_binding<'de, D>(deserializer: D) -> Result<BindingId, D::Error>
where
    D: Deserializer<'de>,
{
    BindingId::new(u16::deserialize(deserializer)?).map_err(D::Error::custom)
}

fn deserialize_bindings<'de, D>(deserializer: D) -> Result<Vec<BindingId>, D::Error>
where
    D: Deserializer<'de>,
{
    Vec::<u16>::deserialize(deserializer)?
        .into_iter()
        .map(|value| BindingId::new(value).map_err(D::Error::custom))
        .collect()
}

/// The additive V2-only compatibility portion of one query plan.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryPlanV2Compatibility {
    allowed_cross_joins: Vec<QueryBindingPairV2>,
    model_query: Option<ModelQueryV2>,
    predicate: Option<QueryPatternV2>,
}

impl QueryPlanV2Compatibility {
    /// Construct the empty compatibility portion used by native low-level V2 plans.
    #[must_use]
    pub const fn native() -> Self {
        Self {
            allowed_cross_joins: Vec::new(),
            model_query: None,
            predicate: None,
        }
    }

    /// Construct an adapter-authored compatibility contract.
    #[must_use]
    pub const fn new(
        predicate: Option<QueryPatternV2>,
        allowed_cross_joins: Vec<QueryBindingPairV2>,
        model_query: Option<ModelQueryV2>,
    ) -> Self {
        Self {
            allowed_cross_joins,
            model_query,
            predicate,
        }
    }

    /// Return the closed released boolean predicate, if present.
    #[must_use]
    pub const fn predicate(&self) -> Option<&QueryPatternV2> {
        self.predicate.as_ref()
    }

    /// Return canonical explicit cross-join permissions.
    #[must_use]
    pub fn allowed_cross_joins(&self) -> &[QueryBindingPairV2] {
        &self.allowed_cross_joins
    }

    /// Return the released model-query terminal contract, if present.
    #[must_use]
    pub const fn model_query(&self) -> Option<&ModelQueryV2> {
        self.model_query.as_ref()
    }

    pub(crate) fn validate(
        &self,
        binding_count: usize,
        limits: StructuralLimits,
    ) -> Result<(), Diagnostic> {
        if self.allowed_cross_joins.len() > limits.allowed_cross_joins {
            return Err(failure(
                DiagnosticCategory::ResourceLimit,
                "query_plan_v2_cross_join_limit",
                "explicit cross joins exceed the structural ceiling",
            ));
        }
        let mut previous = None;
        for pair in &self.allowed_cross_joins {
            if pair.left >= pair.right {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_v2_cross_join_pair",
                    "cross-join pairs must contain two distinct ascending binding IDs",
                ));
            }
            if previous.is_some_and(|previous| previous >= *pair) {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_v2_cross_joins_not_canonical",
                    "cross-join pairs must be strictly sorted and unique",
                ));
            }
            previous = Some(*pair);
            check_binding(pair.left, binding_count)?;
            check_binding(pair.right, binding_count)?;
        }

        if let Some(predicate) = &self.predicate {
            let mut nodes = 0usize;
            validate_pattern(predicate, 1, true, binding_count, limits, &mut nodes)?;
        }
        if let Some(model_query) = &self.model_query {
            validate_model_query(model_query, binding_count, limits)?;
        }
        if let Some(predicate) = &self.predicate {
            validate_pattern_authority(
                predicate,
                self.model_query.as_ref().and_then(model_hydration),
            )?;
        }
        Ok(())
    }

    pub(crate) fn add_capabilities(
        &self,
        capabilities: &mut CapabilitySet,
    ) -> Result<(), Diagnostic> {
        if let Some(predicate) = &self.predicate {
            add_pattern_capabilities(predicate, capabilities)?;
        }
        if !self.allowed_cross_joins.is_empty() {
            insert_capability(capabilities, CAP_CROSS_JOIN)?;
        }
        if let Some(model_query) = &self.model_query {
            add_model_query_capabilities(model_query, capabilities)?;
        }
        Ok(())
    }
}

const fn model_hydration(query: &ModelQueryV2) -> Option<&HydrationProjectionV2> {
    match query {
        ModelQueryV2::Rows { hydration, .. }
        | ModelQueryV2::Page { hydration, .. }
        | ModelQueryV2::DistinctCount { hydration, .. }
        | ModelQueryV2::DistinctExists { hydration, .. } => Some(hydration),
    }
}

fn validate_pattern_authority(
    pattern: &QueryPatternV2,
    hydration: Option<&HydrationProjectionV2>,
) -> Result<(), Diagnostic> {
    match pattern {
        QueryPatternV2::RoleEdge {
            include_relation_subtypes,
            player,
            relation,
            relation_type,
            role,
        } => validate_role_edge_authority(
            *include_relation_subtypes,
            *player,
            *relation,
            relation_type,
            role,
            hydration,
        ),
        QueryPatternV2::And { patterns } | QueryPatternV2::Or { patterns } => {
            for child in patterns {
                validate_pattern_authority(child, hydration)?;
            }
            Ok(())
        }
        QueryPatternV2::Not { pattern } => validate_pattern_authority(pattern, hydration),
        QueryPatternV2::FieldValue { field, .. } => {
            validate_predicate_field_authority(field, hydration)
        }
        QueryPatternV2::FieldComparison { left, right, .. } => {
            validate_predicate_field_authority(left, hydration)?;
            validate_predicate_field_authority(right, hydration)
        }
        QueryPatternV2::FieldPresence { field, .. } => {
            validate_predicate_field_authority(field, hydration)
        }
        QueryPatternV2::BindingIid { .. } => Ok(()),
        QueryPatternV2::Reachable { .. } => Ok(()),
    }
}

fn validate_predicate_field_authority(
    field: &QueryFieldV2,
    hydration: Option<&HydrationProjectionV2>,
) -> Result<(), Diagnostic> {
    let hydration = hydration.ok_or_else(|| {
        failure(
            DiagnosticCategory::InvalidContract,
            "query_plan_v2_field_authority",
            "descriptor-qualified predicates require closed hydration authority",
        )
    })?;
    let binding = hydration
        .bindings()
        .iter()
        .find(|binding| binding.binding() == field.binding())
        .ok_or_else(|| {
            failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_v2_field_authority",
                "predicate field binding lacks hydration authority",
            )
        })?;
    if binding.concrete_descriptors().is_empty() {
        return Ok(());
    }
    let admitted = binding.concrete_descriptors().iter().any(|concrete| {
        hydration
            .descriptors()
            .iter()
            .find(|descriptor| descriptor.descriptor() == concrete)
            .and_then(|descriptor| {
                descriptor.fields().iter().find(|projected| {
                    projected.reference_owners().contains(field.descriptor())
                        && projected.attribute() == field.attribute()
                        && projected.value_type() == field.value_type()
                })
            })
            .is_some()
    });
    if !admitted {
        return Err(failure(
            DiagnosticCategory::InvalidContract,
            "query_plan_v2_field_authority",
            "predicate field owner, attribute, or type has no applicable concrete authority",
        ));
    }
    Ok(())
}

fn validate_role_edge_authority(
    include_relation_subtypes: bool,
    player: BindingId,
    relation: BindingId,
    relation_type: &TypeId,
    role: &RoleId,
    hydration: Option<&HydrationProjectionV2>,
) -> Result<(), Diagnostic> {
    let exact_reference = role.declaring_relation() == relation_type.label();
    let Some(hydration) = hydration else {
        return if exact_reference && !include_relation_subtypes {
            Ok(())
        } else {
            Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_v2_role_edge_authority",
                "inherited or subtype role edges require closed hydration authority",
            ))
        };
    };
    let Some(relation_binding) = hydration
        .bindings()
        .iter()
        .find(|binding| binding.binding() == relation)
    else {
        return if exact_reference && !include_relation_subtypes {
            Ok(())
        } else {
            Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_v2_role_edge_authority",
                "role-edge relation binding lacks closed hydration authority",
            ))
        };
    };
    let Some(player_binding) = hydration
        .bindings()
        .iter()
        .find(|binding| binding.binding() == player)
    else {
        return Err(failure(
            DiagnosticCategory::InvalidContract,
            "query_plan_v2_role_player_authority",
            "role-edge player binding lacks closed hydration authority",
        ));
    };
    if relation_binding.declared_descriptor() != relation_type {
        return Err(failure(
            DiagnosticCategory::InvalidContract,
            "query_plan_v2_role_relation_authority",
            "role-edge relation descriptor contradicts its binding authority",
        ));
    }
    if relation_binding.concrete_descriptors().is_empty()
        || player_binding.concrete_descriptors().is_empty()
    {
        return Ok(());
    }
    let mut role_applicable = false;
    for concrete in relation_binding.concrete_descriptors() {
        let Some(descriptor) = hydration
            .descriptors()
            .iter()
            .find(|descriptor| descriptor.descriptor() == concrete)
        else {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_v2_role_edge_authority",
                "role-edge concrete relation lacks descriptor authority",
            ));
        };
        for projected_role in descriptor
            .roles()
            .iter()
            .filter(|projected| projected.reference_roles().contains(role))
        {
            role_applicable = true;
            // An exact subtype binding legitimately differs from a role's base
            // declaration; schema claims independently prove both closures.
            let player_admitted = projected_role.players().iter().any(|authority| {
                player_binding
                    .concrete_descriptors()
                    .iter()
                    .any(|concrete| authority.concrete_descriptors().contains(concrete))
            });
            if player_admitted {
                return Ok(());
            }
        }
    }
    if role_applicable {
        Err(failure(
            DiagnosticCategory::InvalidContract,
            "query_plan_v2_role_player_authority",
            "role player binding has no applicable declared-to-concrete role authority",
        ))
    } else {
        Err(failure(
            DiagnosticCategory::InvalidContract,
            "query_plan_v2_role_edge_authority",
            "role reference has no applicable concrete relation authority",
        ))
    }
}

fn check_binding(binding: BindingId, binding_count: usize) -> Result<(), Diagnostic> {
    if usize::from(binding.get()) < binding_count {
        Ok(())
    } else {
        Err(failure(
            DiagnosticCategory::InvalidContract,
            "query_plan_v2_unknown_binding",
            "V2 compatibility metadata references an undeclared binding",
        ))
    }
}

fn check_field(field: &QueryFieldV2, binding_count: usize) -> Result<(), Diagnostic> {
    check_binding(field.binding(), binding_count)?;
    if matches!(
        field.descriptor().kind(),
        TypeKind::Entity | TypeKind::Relation
    ) {
        Ok(())
    } else {
        Err(failure(
            DiagnosticCategory::InvalidContract,
            "query_plan_v2_invalid_field_owner",
            "a model field owner must be an entity or relation descriptor",
        ))
    }
}

fn validate_pattern(
    pattern: &QueryPatternV2,
    depth: usize,
    positive_root: bool,
    binding_count: usize,
    limits: StructuralLimits,
    nodes: &mut usize,
) -> Result<(), Diagnostic> {
    *nodes = nodes.saturating_add(1);
    if !limits.allows_predicate_nodes(*nodes) {
        return Err(failure(
            DiagnosticCategory::ResourceLimit,
            "query_plan_v2_pattern_node_limit",
            "V2 compatibility predicate nodes exceed the structural ceiling",
        ));
    }
    if !limits.allows_predicate_depth(depth) {
        return Err(failure(
            DiagnosticCategory::ResourceLimit,
            "query_plan_v2_pattern_depth_limit",
            "V2 compatibility predicate depth exceeds the structural ceiling",
        ));
    }
    match pattern {
        QueryPatternV2::FieldValue {
            field,
            comparator,
            value,
        } => {
            check_field(field, binding_count)?;
            value.validate()?;
            if field.value_type() != value.value_type() {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_v2_field_literal_type",
                    "field and literal scalar domains must match exactly",
                ));
            }
            if comparator.is_string_operator() && field.value_type() != ValueTypeTag::String {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_v2_string_operator_type",
                    "string operators require a canonical string literal",
                ));
            }
            if field.value_type() == ValueTypeTag::Boolean
                && !matches!(
                    comparator,
                    QueryComparatorV2::Equal | QueryComparatorV2::NotEqual
                )
            {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_v2_boolean_operator",
                    "boolean fields admit only equality and inequality",
                ));
            }
            Ok(())
        }
        QueryPatternV2::FieldComparison {
            left,
            comparator,
            right,
        } => {
            check_field(left, binding_count)?;
            check_field(right, binding_count)?;
            if left.value_type() != right.value_type() {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_v2_field_comparison_type",
                    "field-to-field comparisons require one exact scalar domain",
                ));
            }
            if comparator.is_string_operator() {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_v2_field_string_operator",
                    "released field-to-field comparisons do not admit string operators",
                ));
            }
            if left.value_type() == ValueTypeTag::Boolean
                && !matches!(
                    comparator,
                    QueryComparatorV2::Equal | QueryComparatorV2::NotEqual
                )
            {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_v2_boolean_operator",
                    "boolean fields admit only equality and inequality",
                ));
            }
            Ok(())
        }
        QueryPatternV2::FieldPresence { field, .. } => check_field(field, binding_count),
        QueryPatternV2::BindingIid { binding, iid } => {
            check_binding(*binding, binding_count)?;
            if !is_canonical_thing_iid(iid) {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_v2_invalid_iid",
                    "IID predicates require a canonical TypeDB thing IID",
                ));
            }
            Ok(())
        }
        QueryPatternV2::RoleEdge {
            player,
            relation,
            relation_type,
            ..
        } => {
            check_binding(*relation, binding_count)?;
            check_binding(*player, binding_count)?;
            if relation_type.kind() != TypeKind::Relation {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_v2_role_edge_kind",
                    "role edges require a relation descriptor",
                ));
            }
            Ok(())
        }
        QueryPatternV2::Reachable {
            min_depth,
            max_depth,
            relation,
            role_from,
            role_to,
            source,
            target,
        } => {
            if !positive_root {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_v2_reachable_not_root_positive",
                    "bounded reachability is admitted only in the positive root conjunction",
                ));
            }
            if relation.kind() != TypeKind::Relation
                || role_from.declaring_relation() != relation.label()
                || role_to.declaring_relation() != relation.label()
                || min_depth > max_depth
            {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_v2_reachable_contract",
                    "reachability requires relation-owned roles and canonical inclusive bounds",
                ));
            }
            if !limits.allows_predicate_depth(usize::from(*max_depth)) {
                return Err(failure(
                    DiagnosticCategory::ResourceLimit,
                    "query_plan_v2_reachable_depth",
                    "reachability exceeds the finite depth ceiling",
                ));
            }
            let first_positive = usize::from((*min_depth).max(1));
            let bound = usize::from(*max_depth);
            let expanded_hops = if first_positive <= bound {
                (first_positive..=bound).fold(0usize, usize::saturating_add)
            } else {
                0
            };
            let expanded_clauses = expanded_hops.saturating_add(usize::from(*min_depth == 0));
            *nodes = nodes.saturating_add(expanded_clauses.saturating_sub(1));
            if !limits.allows_predicate_nodes(*nodes) {
                return Err(failure(
                    DiagnosticCategory::ResourceLimit,
                    "query_plan_v2_reachable_expansion_limit",
                    "reachability expansion exceeds the predicate-node ceiling",
                ));
            }
            check_binding(*source, binding_count)?;
            check_binding(*target, binding_count)
        }
        QueryPatternV2::And { patterns } => {
            validate_boolean_children(patterns, depth, positive_root, binding_count, limits, nodes)
        }
        QueryPatternV2::Or { patterns } => {
            validate_boolean_children(patterns, depth, false, binding_count, limits, nodes)
        }
        QueryPatternV2::Not { pattern } => {
            validate_pattern(pattern, depth + 1, false, binding_count, limits, nodes)
        }
    }
}

fn validate_boolean_children(
    patterns: &[QueryPatternV2],
    depth: usize,
    positive_root: bool,
    binding_count: usize,
    limits: StructuralLimits,
    nodes: &mut usize,
) -> Result<(), Diagnostic> {
    if patterns.is_empty() || patterns.len() > limits.boolean_terms {
        return Err(failure(
            DiagnosticCategory::ResourceLimit,
            "query_plan_v2_boolean_term_limit",
            "boolean branches are empty or exceed the term ceiling",
        ));
    }
    for child in patterns {
        validate_pattern(
            child,
            depth + 1,
            positive_root,
            binding_count,
            limits,
            nodes,
        )?;
    }
    Ok(())
}

fn validate_model_query(
    query: &ModelQueryV2,
    binding_count: usize,
    limits: StructuralLimits,
) -> Result<(), Diagnostic> {
    match query {
        ModelQueryV2::Rows {
            cardinality,
            hydration,
            order,
            output,
            window,
        } => {
            validate_window(*window)?;
            validate_model_output(output, hydration, binding_count, limits)?;
            let selected_identity = output_identity_tuple(output);
            if selected_identity.is_empty() {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_v2_missing_selected_identity",
                    "row queries require at least one singular public identity slot",
                ));
            }
            match cardinality {
                QueryRowCardinalityV2::ExactlyOne => {
                    if window.offset() != 0 || window.limit() != 1 || order.is_some() {
                        return Err(failure(
                            DiagnosticCategory::InvalidContract,
                            "query_plan_v2_exactly_one_contract",
                            "exactly-one requires offset zero, limit one, and no row order",
                        ));
                    }
                }
                QueryRowCardinalityV2::BoundedMany => {
                    let order = order.as_ref().ok_or_else(|| {
                        failure(
                            DiagnosticCategory::InvalidContract,
                            "query_plan_v2_rows_missing_order",
                            "bounded rows require a validator-proven stable total order",
                        )
                    })?;
                    validate_stable_order(order, binding_count, limits.order_terms)?;
                    let exposed = selected_identity.iter().copied().collect();
                    validate_order_against_hydration(order, hydration, &exposed)?;
                    if order.identity_tiebreakers() != selected_identity {
                        return Err(failure(
                            DiagnosticCategory::InvalidContract,
                            "query_plan_v2_selected_order_tiebreaker",
                            "bounded row ordering must use the selected identity tuple",
                        ));
                    }
                }
            }
        }
        ModelQueryV2::Page {
            hydration,
            order,
            output,
            root,
            window,
            ..
        } => {
            check_binding(*root, binding_count)?;
            validate_window(*window)?;
            validate_model_output(output, hydration, binding_count, limits)?;
            let mut root_is_singular = false;
            output.visit_slots(|_, slot| {
                if matches!(slot, QueryModelOutputSlotV2::One { binding, .. } if binding == root) {
                    root_is_singular = true;
                }
            });
            if !root_is_singular {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_v2_page_root_not_singular",
                    "the page root must be one singular public output slot",
                ));
            }
            validate_stable_order(order, binding_count, limits.order_terms)?;
            validate_order_against_hydration(order, hydration, &BTreeSet::from([*root]))?;
            if order.identity_tiebreakers() != [*root] {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_v2_root_order_tiebreaker",
                    "page ordering must use the exact root identity tie breaker",
                ));
            }
        }
        ModelQueryV2::DistinctCount { hydration, root }
        | ModelQueryV2::DistinctExists { hydration, root } => {
            check_binding(*root, binding_count)?;
            validate_hydration(hydration, binding_count, limits)?;
            if !hydration
                .bindings()
                .iter()
                .any(|binding| binding.binding() == *root)
            {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_v2_root_not_hydrated",
                    "distinct root operations require root descriptor authority",
                ));
            }
        }
    }
    Ok(())
}

fn validate_window(window: QueryWindowV2) -> Result<(), Diagnostic> {
    if window.limit() == 0 || window.offset().checked_add(window.limit()).is_none() {
        Err(failure(
            DiagnosticCategory::InvalidContract,
            "query_plan_v2_window",
            "model-query windows require a positive non-overflowing limit",
        ))
    } else {
        Ok(())
    }
}

fn validate_stable_order(
    order: &QueryStableOrderV2,
    binding_count: usize,
    term_limit: usize,
) -> Result<(), Diagnostic> {
    if order.terms().len() > term_limit || order.identity_tiebreakers().is_empty() {
        return Err(failure(
            DiagnosticCategory::ResourceLimit,
            "query_plan_v2_order_limit",
            "stable order terms are unbounded or lack an identity tie breaker",
        ));
    }
    let mut fields = BTreeSet::new();
    for term in order.terms() {
        check_field(term.field(), binding_count)?;
        if !fields.insert((
            term.field().binding(),
            term.field().descriptor(),
            term.field().attribute(),
        )) {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_v2_duplicate_order_field",
                "stable order fields must be unique",
            ));
        }
    }
    let mut previous = None;
    for binding in order.identity_tiebreakers() {
        check_binding(*binding, binding_count)?;
        if previous.is_some_and(|previous| previous >= *binding) {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_v2_order_tiebreakers_not_canonical",
                "identity tie breakers must be strictly ascending and unique",
            ));
        }
        previous = Some(*binding);
    }
    Ok(())
}

fn validate_model_output(
    output: &QueryModelOutputV2,
    hydration: &HydrationProjectionV2,
    binding_count: usize,
    limits: StructuralLimits,
) -> Result<(), Diagnostic> {
    if output.slot_count() == 0 || !limits.allows_selected_slots(output.slot_count()) {
        return Err(failure(
            DiagnosticCategory::ResourceLimit,
            "query_plan_v2_output_limit",
            "model output is empty or exceeds the selected-slot ceiling",
        ));
    }
    validate_hydration(hydration, binding_count, limits)?;
    let hydrated = hydration
        .bindings()
        .iter()
        .map(|binding| (binding.binding(), binding.declared_descriptor()))
        .collect::<BTreeMap<_, _>>();
    let mut selected = BTreeSet::new();
    let mut names = BTreeSet::new();
    let mut error = None;
    output.visit_slots(|name, slot| {
        if error.is_some() {
            return;
        }
        if let Some(name) = name
            && (name.is_empty()
                || name.len() > limits.output_name_bytes
                || name.chars().any(char::is_control)
                || !names.insert(name.to_owned()))
        {
            error = Some(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_v2_output_name",
                "named output members must be bounded, non-control, and unique",
            ));
            return;
        }
        let binding = slot.binding();
        if check_binding(binding, binding_count).is_err()
            || !selected.insert(binding)
            || hydrated.get(&binding).copied() != Some(slot.declared())
        {
            error = Some(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_v2_output_binding",
                "model output bindings must be declared, unique, and hydrated",
            ));
            return;
        }
        if let QueryModelOutputSlotV2::Collect { binding, order, .. } = slot {
            if let Err(order_error) =
                validate_stable_order(order, binding_count, limits.collection_order_terms)
            {
                error = Some(order_error);
            } else if let Err(order_error) =
                validate_order_against_hydration(order, hydration, &BTreeSet::from([*binding]))
            {
                error = Some(order_error);
            } else if order.identity_tiebreakers() != [*binding] {
                error = Some(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_v2_collection_order_tiebreaker",
                    "collection order must use the collected identity tie breaker",
                ));
            }
        }
    });
    if let Some(error) = error {
        return Err(error);
    }
    Ok(())
}

fn output_identity_tuple(output: &QueryModelOutputV2) -> Vec<BindingId> {
    let mut bindings = output
        .slots()
        .into_iter()
        .filter_map(|slot| match slot {
            QueryModelOutputSlotV2::One { binding, .. } => Some(*binding),
            QueryModelOutputSlotV2::Collect { .. } => None,
        })
        .collect::<Vec<_>>();
    bindings.sort_unstable();
    bindings
}

fn validate_order_against_hydration(
    order: &QueryStableOrderV2,
    hydration: &HydrationProjectionV2,
    exposed_bindings: &BTreeSet<BindingId>,
) -> Result<(), Diagnostic> {
    let mut stable_unique_bindings = BTreeSet::new();
    for term in order.terms() {
        let field = term.field();
        if !exposed_bindings.contains(&field.binding()) {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_v2_order_term_not_exposed",
                "stable order terms must refer to the response slot they order",
            ));
        }
        let Some(binding) = hydration
            .bindings()
            .iter()
            .find(|binding| binding.binding() == field.binding())
        else {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_v2_order_field_not_hydrated",
                "stable order fields require hydration authority for their binding",
            ));
        };
        let mut unique_on_every_concrete = true;
        for concrete in binding.concrete_descriptors() {
            let Some(descriptor) = hydration
                .descriptors()
                .iter()
                .find(|projection| projection.descriptor() == concrete)
            else {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_v2_order_field_not_hydrated",
                    "stable order field concrete descriptor lacks hydration authority",
                ));
            };
            let Some(projected) = descriptor.fields().iter().find(|projected| {
                projected.reference_owners().contains(field.descriptor())
                    && projected.attribute() == field.attribute()
                    && projected.value_type() == field.value_type()
            }) else {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_v2_order_field_claim",
                    "stable order field is absent or differently typed on an admitted concrete descriptor",
                ));
            };
            if projected.ordered() || projected.cardinality().max() != Some(1) {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_v2_non_scalar_order_field",
                    "stable order fields must be scalar descriptor ownerships",
                ));
            }
            unique_on_every_concrete &= projected.unique()
                && projected.cardinality().min() >= 1
                && projected.cardinality().max() == Some(1)
                && !projected.ordered();
        }
        if unique_on_every_concrete {
            stable_unique_bindings.insert(field.binding());
        }
    }
    for binding in order.identity_tiebreakers() {
        if !stable_unique_bindings.contains(binding) {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_v2_unproven_identity_tiebreaker",
                "each identity tie breaker requires one present unique scalar order field",
            ));
        }
    }
    Ok(())
}

fn validate_hydration(
    hydration: &HydrationProjectionV2,
    binding_count: usize,
    limits: StructuralLimits,
) -> Result<(), Diagnostic> {
    if hydration.bindings().is_empty() {
        return Err(failure(
            DiagnosticCategory::InvalidContract,
            "query_plan_v2_empty_hydration",
            "hydrated model output requires binding authority",
        ));
    }
    let mut previous_binding = None;
    let mut admitted = BTreeSet::new();
    for binding in hydration.bindings() {
        check_binding(binding.binding(), binding_count)?;
        if previous_binding.is_some_and(|previous| previous >= binding.binding()) {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_v2_hydration_bindings_not_canonical",
                "hydration bindings must be strictly ascending and unique",
            ));
        }
        previous_binding = Some(binding.binding());
        if !matches!(
            binding.declared_descriptor().kind(),
            TypeKind::Entity | TypeKind::Relation
        ) {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_v2_hydration_binding",
                "hydration bindings require a thing descriptor",
            ));
        }
        let mut previous = None;
        for descriptor in binding.concrete_descriptors() {
            if descriptor.kind() != binding.declared_descriptor().kind()
                || previous.is_some_and(|previous: &TypeId| previous >= descriptor)
            {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_v2_concrete_descriptors_not_canonical",
                    "concrete descriptors must match kind and be strictly sorted",
                ));
            }
            previous = Some(descriptor);
            admitted.insert(descriptor.clone());
        }
    }

    let mut projections = BTreeMap::new();
    let mut previous_descriptor = None;
    for descriptor in hydration.descriptors() {
        if previous_descriptor
            .as_ref()
            .is_some_and(|previous| previous >= descriptor.descriptor())
        {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_v2_hydration_descriptors_not_canonical",
                "hydration descriptors must be strictly sorted and unique",
            ));
        }
        previous_descriptor = Some(descriptor.descriptor().clone());
        validate_hydration_descriptor(descriptor, limits)?;
        projections.insert(descriptor.descriptor().clone(), descriptor);
    }

    let full = admitted.clone();
    for concrete in &full {
        let Some(descriptor) = projections.get(concrete) else {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_v2_missing_hydration_descriptor",
                "every admitted concrete descriptor requires complete hydration authority",
            ));
        };
        for role in descriptor.roles() {
            for player in role.players() {
                for concrete in player.concrete_descriptors() {
                    if !projections.contains_key(concrete) {
                        return Err(failure(
                            DiagnosticCategory::InvalidContract,
                            "query_plan_v2_missing_role_player_descriptor",
                            "every admitted concrete role player requires hydration authority",
                        ));
                    }
                    admitted.insert(concrete.clone());
                }
            }
        }
    }
    if projections
        .keys()
        .any(|descriptor| !admitted.contains(descriptor))
    {
        return Err(failure(
            DiagnosticCategory::InvalidContract,
            "query_plan_v2_unadmitted_hydration_descriptor",
            "hydration descriptor is absent from binding or role-player closure",
        ));
    }
    Ok(())
}

fn validate_hydration_descriptor(
    descriptor: &HydrationDescriptorV2,
    limits: StructuralLimits,
) -> Result<(), Diagnostic> {
    if !matches!(
        descriptor.descriptor().kind(),
        TypeKind::Entity | TypeKind::Relation
    ) {
        return Err(failure(
            DiagnosticCategory::InvalidContract,
            "query_plan_v2_hydration_descriptor_kind",
            "hydration descriptors must be entities or relations",
        ));
    }
    let mut aliases = BTreeSet::new();
    let mut attributes = BTreeSet::new();
    let mut previous_alias: Option<&str> = None;
    for field in descriptor.fields() {
        if field.reference_owners().is_empty()
            || field
                .reference_owners()
                .iter()
                .any(|owner| owner.kind() != descriptor.descriptor().kind())
            || field
                .reference_owners()
                .windows(2)
                .any(|owners| owners[0] >= owners[1])
        {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_v2_hydration_field_owners_not_canonical",
                "hydration field reference owners must be nonempty sorted unique thing descriptors",
            ));
        }
        if field.distinct() && !field.ordered() {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_v2_field_distinct_requires_ordered",
                "distinct ownership requires ordered-list authority",
            ));
        }
        if field.alias().is_empty()
            || field.alias().len() > limits.output_name_bytes
            || field.alias().chars().any(char::is_control)
            || previous_alias.is_some_and(|previous| previous >= field.alias())
            || !aliases.insert(field.alias())
            || !attributes.insert(field.attribute())
        {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_v2_hydration_fields_not_canonical",
                "hydration fields require unique sorted aliases and provider attributes",
            ));
        }
        previous_alias = Some(field.alias());
    }
    if descriptor.descriptor().kind() == TypeKind::Entity && !descriptor.roles().is_empty() {
        return Err(failure(
            DiagnosticCategory::InvalidContract,
            "query_plan_v2_entity_roles",
            "entity hydration projections cannot declare roles",
        ));
    }
    let mut previous_role = None;
    let mut admitted_role_references = BTreeSet::new();
    for role in descriptor.roles() {
        if role.distinct() && !role.ordered() {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_v2_role_distinct_requires_ordered",
                "distinct role players require ordered-list authority",
            ));
        }
        if previous_role
            .as_ref()
            .is_some_and(|previous| previous >= role.role())
            || role.role().declaring_relation() != descriptor.descriptor().label()
        {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_v2_hydration_roles_not_canonical",
                "hydration roles must be owned, sorted, and unique",
            ));
        }
        previous_role = Some(role.role().clone());
        if role.reference_roles().is_empty()
            || !role.reference_roles().contains(role.role())
            || role
                .reference_roles()
                .iter()
                .any(|reference| reference.label() != role.role().label())
            || role
                .reference_roles()
                .windows(2)
                .any(|roles| roles[0] >= roles[1])
            || role
                .reference_roles()
                .iter()
                .any(|reference| !admitted_role_references.insert(reference.clone()))
        {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_v2_role_references_not_canonical",
                "role references must include the provider role and be sorted, unique, same-label, and unambiguous",
            ));
        }
        let mut previous_declared = None;
        for player in role.players() {
            if !matches!(
                player.declared_descriptor().kind(),
                TypeKind::Entity | TypeKind::Relation
            ) || previous_declared
                .as_ref()
                .is_some_and(|previous| previous >= player.declared_descriptor())
            {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "query_plan_v2_role_players_not_canonical",
                    "role-player declarations must be sorted unique thing descriptors",
                ));
            }
            previous_declared = Some(player.declared_descriptor().clone());
            let mut previous_concrete = None;
            for concrete in player.concrete_descriptors() {
                if concrete.kind() != player.declared_descriptor().kind()
                    || previous_concrete
                        .as_ref()
                        .is_some_and(|previous| previous >= concrete)
                {
                    return Err(failure(
                        DiagnosticCategory::InvalidContract,
                        "query_plan_v2_role_player_concretes_not_canonical",
                        "role-player concrete descriptors must be sorted unique and match the declared thing kind",
                    ));
                }
                previous_concrete = Some(concrete.clone());
            }
        }
    }
    Ok(())
}

fn add_pattern_capabilities(
    pattern: &QueryPatternV2,
    capabilities: &mut CapabilitySet,
) -> Result<(), Diagnostic> {
    match pattern {
        QueryPatternV2::FieldValue { comparator, .. } => {
            insert_capability(capabilities, super::CAP_VALUE)?;
            if comparator.is_string_operator() {
                insert_capability(capabilities, CAP_STRING_OPERATORS)?;
            }
        }
        QueryPatternV2::FieldComparison { .. } => {
            insert_capability(capabilities, super::CAP_VALUE)?;
        }
        QueryPatternV2::FieldPresence { .. } => {
            insert_capability(capabilities, super::CAP_HAS)?;
            // Both absence and existential presence lower through closed
            // negation patterns so cardinality-many values cannot multiply an
            // owner row.
            insert_capability(capabilities, super::CAP_NEGATION)?;
        }
        QueryPatternV2::BindingIid { .. } => {
            insert_capability(capabilities, CAP_IID)?;
        }
        QueryPatternV2::RoleEdge {
            include_relation_subtypes,
            ..
        } => {
            insert_capability(capabilities, super::CAP_LINKS)?;
            if *include_relation_subtypes {
                insert_capability(capabilities, CAP_LINKS_SUBTYPES)?;
            }
        }
        QueryPatternV2::Reachable { .. } => {
            insert_capability(capabilities, super::CAP_REACHABLE)?;
        }
        QueryPatternV2::And { patterns } => {
            for child in patterns {
                add_pattern_capabilities(child, capabilities)?;
            }
        }
        QueryPatternV2::Or { patterns } => {
            insert_capability(capabilities, CAP_DISJUNCTION)?;
            for child in patterns {
                add_pattern_capabilities(child, capabilities)?;
            }
        }
        QueryPatternV2::Not { pattern } => {
            insert_capability(capabilities, super::CAP_NEGATION)?;
            add_pattern_capabilities(pattern, capabilities)?;
        }
    }
    Ok(())
}

fn add_model_query_capabilities(
    query: &ModelQueryV2,
    capabilities: &mut CapabilitySet,
) -> Result<(), Diagnostic> {
    match query {
        ModelQueryV2::Rows {
            cardinality,
            output,
            ..
        } => {
            add_hydrated_output_capabilities(output, capabilities)?;
            match cardinality {
                QueryRowCardinalityV2::ExactlyOne => {
                    insert_capability(capabilities, CAP_EXACTLY_ONE)?;
                }
                QueryRowCardinalityV2::BoundedMany => {
                    insert_capability(capabilities, CAP_STABLE_SELECTED)?;
                }
            }
        }
        ModelQueryV2::Page {
            include_total,
            output,
            ..
        } => {
            add_hydrated_output_capabilities(output, capabilities)?;
            insert_capability(capabilities, CAP_PAGE)?;
            insert_capability(capabilities, CAP_STABLE_ROOT)?;
            if *include_total {
                insert_capability(capabilities, CAP_DISTINCT_COUNT)?;
            }
        }
        ModelQueryV2::DistinctCount { .. } => {
            insert_capability(capabilities, CAP_DISTINCT_COUNT)?;
        }
        ModelQueryV2::DistinctExists { .. } => {
            insert_capability(capabilities, CAP_DISTINCT_EXISTS)?;
        }
    }
    Ok(())
}

fn add_hydrated_output_capabilities(
    output: &QueryModelOutputV2,
    capabilities: &mut CapabilitySet,
) -> Result<(), Diagnostic> {
    insert_capability(capabilities, CAP_OUTPUT_HYDRATED)?;
    insert_capability(capabilities, CAP_SAME_SNAPSHOT_HYDRATION)?;
    insert_capability(capabilities, CAP_BATCH_IDENTITY_REBIND)?;
    if matches!(output, QueryModelOutputV2::Named { .. }) {
        insert_capability(capabilities, CAP_OUTPUT_NAMED)?;
    }
    output.visit_slots(|_, slot| {
        if let QueryModelOutputSlotV2::Collect { distinct, .. } = slot {
            let capability = if *distinct {
                CAP_OUTPUT_COLLECT_DISTINCT
            } else {
                CAP_OUTPUT_COLLECT
            };
            // All IDs above are fixed valid constants.
            capabilities.insert(
                crate::capability::CapabilityId::new(capability)
                    .expect("static V2 query capability is canonical"),
            );
            capabilities.insert(
                crate::capability::CapabilityId::new(CAP_STABLE_COLLECTION)
                    .expect("static V2 query capability is canonical"),
            );
        }
    });
    Ok(())
}
