//! Generated-code SPI for TypeBridge.
//!
//! Internal primitives and traits required by generated schema crates.
//! Hand-written trait implementations are not schema evidence.

use core::fmt;
use core::marker::PhantomData;

pub use crate::schema::{Schema, SchemaPackage, Unbound, sealed};

/// An owned, nestable validation path.
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct ValidationPath {
    segments: Vec<String>,
}

impl ValidationPath {
    #[must_use]
    pub fn root() -> Self {
        Self {
            segments: Vec::new(),
        }
    }

    #[must_use]
    pub fn join(&self, segment: impl Into<String>) -> Self {
        let mut s = self.segments.clone();
        s.push(segment.into());
        Self { segments: s }
    }

    #[must_use]
    pub fn join_index(&self, index: usize) -> Self {
        let mut s = self.segments.clone();
        if let Some(last) = s.last_mut() {
            last.push_str(&format!("[{index}]"));
        } else {
            s.push(format!("[{index}]"));
        }
        Self { segments: s }
    }

    #[must_use]
    pub fn path(&self) -> String {
        self.segments.join(".")
    }
}

/// A stable generated-input validation failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidationError {
    path: String,
    code: String,
}

impl ValidationError {
    #[must_use]
    pub fn new(path: impl Into<String>, code: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            code: code.into(),
        }
    }

    #[must_use]
    pub fn field(&self) -> &str {
        &self.path
    }

    #[must_use]
    pub fn code(&self) -> &str {
        &self.code
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.path.is_empty() {
            write!(formatter, "{}", self.code)
        } else {
            write!(formatter, "{}: {}", self.path, self.code)
        }
    }
}

impl std::error::Error for ValidationError {}

/// An exact resolved cardinality.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Cardinality {
    min: u64,
    max: Option<u64>,
}

impl Cardinality {
    #[must_use]
    pub const fn new(min: u64, max: Option<u64>) -> Self {
        Self { min, max }
    }

    #[must_use]
    pub const fn min(self) -> u64 {
        self.min
    }

    #[must_use]
    pub const fn max(self) -> Option<u64> {
        self.max
    }
}

/// A statically required scalar value.
#[derive(Clone, Debug, PartialEq)]
pub struct Required<T>(T);

impl<T> Required<T> {
    #[must_use]
    pub const fn new(value: T) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(&self) -> &T {
        &self.0
    }

    #[must_use]
    pub const fn value(&self) -> &T {
        &self.0
    }
}

impl<T> core::ops::Deref for Required<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// An optional scalar value.
#[derive(Clone, Debug, PartialEq)]
pub struct Optional<T>(Option<T>);

impl<T> Optional<T> {
    #[must_use]
    pub const fn new(value: Option<T>) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_ref(&self) -> Option<&T> {
        self.0.as_ref()
    }
}

/// A sequence checked against its exact resolved cardinality.
#[derive(Clone, Debug, PartialEq)]
pub struct Sequence<T> {
    values: Vec<T>,
    cardinality: Cardinality,
}

impl<T> Sequence<T> {
    pub fn try_new(
        values: Vec<T>,
        cardinality: Cardinality,
        path: &ValidationPath,
    ) -> Result<Self, ValidationError> {
        let length = u64::try_from(values.len())
            .map_err(|_| ValidationError::new(path.path(), "cardinality_overflow"))?;
        if length < cardinality.min() || cardinality.max().is_some_and(|maximum| length > maximum) {
            return Err(ValidationError::new(path.path(), "cardinality_violation"));
        }
        Ok(Self {
            values,
            cardinality,
        })
    }

    #[must_use]
    pub fn as_slice(&self) -> &[T] {
        &self.values
    }

    #[must_use]
    pub const fn cardinality(&self) -> Cardinality {
        self.cardinality
    }
}

/// A binary sum used to preserve exact heterogeneous model forms.
#[derive(Clone, Debug, PartialEq)]
pub enum Either<L, R> {
    Left(L),
    Right(R),
}

impl<L: sealed::Sealed, R: sealed::Sealed> sealed::Sealed for Either<L, R> {}

impl<L: Model<Schema = S>, R: Model<Schema = S>, S: Schema> Model for Either<L, R> {
    type Schema = S;
    const TYPE_ID_JSON: &'static str = "either";
}

/// The uninhabited projection of an empty accepted-player set.
#[derive(Clone, Debug, PartialEq)]
pub enum Never {}

/// A finite, normalized floating-point value stored as IEEE 754 bits.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct CanonicalDouble(u64);

impl CanonicalDouble {
    pub fn try_new(value: f64) -> Result<Self, ValidationError> {
        if value.is_nan() || value.is_infinite() {
            return Err(ValidationError::new("", "noncanonical_double"));
        }
        Ok(Self(value.to_bits()))
    }

    pub fn try_from_bits(bits: u64) -> Result<Self, ValidationError> {
        let val = f64::from_bits(bits);
        if val.is_nan() || val.is_infinite() {
            return Err(ValidationError::new("", "noncanonical_double"));
        }
        Ok(Self(bits))
    }

    #[must_use]
    pub fn get(self) -> f64 {
        f64::from_bits(self.0)
    }

    #[must_use]
    pub const fn to_bits(self) -> u64 {
        self.0
    }
}

macro_rules! canonical_scalar {
    ($name:ident, $code:expr, $parse_expr:expr) => {
        #[derive(Clone, Debug, Eq, PartialEq, Hash)]
        pub struct $name(String);

        impl $name {
            pub fn try_new(value: impl Into<String>) -> Result<Self, ValidationError> {
                let s = value.into();
                if type_bridge_contract::value::CanonicalString::new(&s).is_err() {
                    return Err(ValidationError::new("", "string_limit_exceeded"));
                }
                let check_fn: fn(&str) -> bool = $parse_expr;
                if !check_fn(&s) {
                    return Err(ValidationError::new("", $code));
                }
                Ok(Self(s))
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

canonical_scalar!(Decimal, "noncanonical_decimal", |s| {
    if let Some(dec) = type_bridge_contract::decimal::parse_decimal(s) {
        dec.canonical_string() == s
    } else {
        false
    }
});

canonical_scalar!(Date, "noncanonical_date", |s| {
    if let Ok(d) = s.parse::<type_bridge_contract::temporal::CanonicalDate>() {
        d.to_string() == s
    } else {
        false
    }
});

canonical_scalar!(DateTime, "noncanonical_datetime", |s| {
    if let Ok(dt) = s.parse::<type_bridge_contract::temporal::CanonicalDateTime>() {
        dt.to_string() == s
    } else {
        false
    }
});

canonical_scalar!(DateTimeTz, "noncanonical_datetime_tz", |s| {
    if let Ok(dtz) = s.parse::<type_bridge_contract::temporal::CanonicalDateTimeTz>() {
        dtz.to_string() == s
    } else {
        false
    }
});

canonical_scalar!(Duration, "noncanonical_duration", |s| {
    if let Ok(dur) = s.parse::<type_bridge_contract::temporal::CanonicalDuration>() {
        dur.to_string() == s
    } else {
        false
    }
});

/// An opaque capability token required for model materialization.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HydrationCapability {
    _private: (),
}

impl HydrationCapability {
    pub(crate) const fn new() -> Self {
        Self { _private: () }
    }
}

/// Test-support materializer entry point available under test-harness feature.
#[cfg(feature = "test-harness")]
#[doc(hidden)]
pub fn materialize_model_for_test<M: MaterializeModel>(
    row: &HydratedRow,
) -> Result<M, ValidationError> {
    let cap = HydrationCapability::new();
    M::materialize(row, &cap)
}

/// Conversion trait into an `EncodedScalar`.
#[doc(hidden)]
pub trait IntoEncodedScalar {
    #[allow(clippy::wrong_self_convention)]
    fn into_encoded_scalar(&self) -> EncodedScalar;
}

impl IntoEncodedScalar for String {
    fn into_encoded_scalar(&self) -> EncodedScalar {
        EncodedScalar::String(self.clone())
    }
}

impl IntoEncodedScalar for i64 {
    fn into_encoded_scalar(&self) -> EncodedScalar {
        EncodedScalar::Long(*self)
    }
}

impl IntoEncodedScalar for CanonicalDouble {
    fn into_encoded_scalar(&self) -> EncodedScalar {
        EncodedScalar::Double(*self)
    }
}

impl IntoEncodedScalar for bool {
    fn into_encoded_scalar(&self) -> EncodedScalar {
        EncodedScalar::Boolean(*self)
    }
}

impl IntoEncodedScalar for Decimal {
    fn into_encoded_scalar(&self) -> EncodedScalar {
        EncodedScalar::Decimal(self.clone())
    }
}

impl IntoEncodedScalar for Date {
    fn into_encoded_scalar(&self) -> EncodedScalar {
        EncodedScalar::Date(self.clone())
    }
}

impl IntoEncodedScalar for DateTime {
    fn into_encoded_scalar(&self) -> EncodedScalar {
        EncodedScalar::DateTime(self.clone())
    }
}

impl IntoEncodedScalar for DateTimeTz {
    fn into_encoded_scalar(&self) -> EncodedScalar {
        EncodedScalar::DateTimeTz(self.clone())
    }
}

impl IntoEncodedScalar for Duration {
    fn into_encoded_scalar(&self) -> EncodedScalar {
        EncodedScalar::Duration(self.clone())
    }
}

impl IntoEncodedScalar for EncodedScalar {
    fn into_encoded_scalar(&self) -> EncodedScalar {
        self.clone()
    }
}

impl<T: IntoEncodedScalar> IntoEncodedScalar for &T {
    fn into_encoded_scalar(&self) -> EncodedScalar {
        (*self).into_encoded_scalar()
    }
}

/// A generated value wrapper's canonical scalar query domain.
///
/// Generated schema crates implement this marker for attribute wrappers so
/// equality operands can preserve their scalar domain without exposing engine
/// value DTOs.
#[doc(hidden)]
pub trait QueryValued: IntoEncodedScalar {
    /// The canonical scalar type represented by this value.
    type Domain;
}

/// A generated attribute wrapper that can materialize a validated grouped
/// query key from its canonical scalar evidence.
#[doc(hidden)]
pub trait GroupedQueryValue: QueryValued + Sized {
    fn from_group_scalar(value: EncodedScalar) -> Result<Self, ValidationError>;
}

impl QueryValued for String {
    type Domain = String;
}
impl QueryValued for i64 {
    type Domain = i64;
}
impl QueryValued for CanonicalDouble {
    type Domain = CanonicalDouble;
}
impl QueryValued for bool {
    type Domain = bool;
}
impl QueryValued for Decimal {
    type Domain = Decimal;
}
impl QueryValued for Date {
    type Domain = Date;
}
impl QueryValued for DateTime {
    type Domain = DateTime;
}
impl QueryValued for DateTimeTz {
    type Domain = DateTimeTz;
}
impl QueryValued for Duration {
    type Domain = Duration;
}
impl<T: QueryValued> QueryValued for &T {
    type Domain = T::Domain;
}

/// Target thing category: Entity or Relation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThingKind {
    Entity,
    Relation,
}

/// A sealed generated schema model associated with its schema marker `S`.
pub trait Model: sealed::Sealed {
    type Schema: Schema;
    const TYPE_ID_JSON: &'static str;
}

/// An entity or relation schema model.
pub trait ThingModel: Model {
    fn thing_kind() -> ThingKind;
}

/// An entity schema model.
pub trait EntityModel: ThingModel {}

/// A relation schema model.
pub trait RelationModel: ThingModel {}

/// A complete materialized generated model with a mandatory canonical IID.
pub trait CompleteModel: ThingModel + MaterializeModel {
    type Create: IntoEncodedCreate + Clone;
    fn iid(&self) -> &str;
}

#[doc(hidden)]
pub trait SubtypeRootModel: ThingModel {
    type Subtypes;
    fn __tb_dispatch_subtype(
        row: &HydratedRow,
        cap: &HydrationCapability,
    ) -> Result<Self::Subtypes, ValidationError>;
}

/// An abstract generated model marker.
pub trait AbstractModel: ThingModel {}

/// Marker for generated attribute values whose canonical domain is text.
pub trait TextValued {}
impl TextValued for String {}

/// Marker for generated attribute values whose canonical domain admits
/// canonical ordering for range comparisons.
pub trait OrderedValued {}

/// Marker for generated attribute values whose canonical domain admits
/// numeric reduction, carrying the canonical reduced scalar domain.
pub trait NumericValued {
    /// The canonical scalar domain of domain-preserving reductions.
    type Reduced;
}
impl NumericValued for i64 {
    type Reduced = i64;
}
impl NumericValued for CanonicalDouble {
    type Reduced = f64;
}
impl OrderedValued for i64 {}
impl OrderedValued for CanonicalDouble {}
impl OrderedValued for Date {}
impl OrderedValued for DateTime {}
impl OrderedValued for DateTimeTz {}
impl OrderedValued for Decimal {}
impl OrderedValued for Duration {}

/// A nonrecursive generated reference model. Key-based references may not have an IID initially.
pub trait ReferenceModel: ThingModel {
    fn iid(&self) -> Option<&str>;
}

/// A closed client scalar value over all nine canonical domains.
#[doc(hidden)]
#[derive(Clone, Debug, PartialEq)]
pub enum EncodedScalar {
    String(String),
    Long(i64),
    Double(CanonicalDouble),
    Decimal(Decimal),
    Boolean(bool),
    Date(Date),
    DateTime(DateTime),
    DateTimeTz(DateTimeTz),
    Duration(Duration),
}

impl EncodedScalar {
    pub fn as_string(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn as_long(&self) -> Option<i64> {
        match self {
            Self::Long(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_double(&self) -> Option<CanonicalDouble> {
        match self {
            Self::Double(d) => Some(*d),
            _ => None,
        }
    }

    pub fn as_boolean(&self) -> Option<bool> {
        match self {
            Self::Boolean(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_decimal(&self) -> Option<&Decimal> {
        match self {
            Self::Decimal(d) => Some(d),
            _ => None,
        }
    }

    pub fn as_date(&self) -> Option<&Date> {
        match self {
            Self::Date(d) => Some(d),
            _ => None,
        }
    }

    pub fn as_datetime(&self) -> Option<&DateTime> {
        match self {
            Self::DateTime(d) => Some(d),
            _ => None,
        }
    }

    pub fn as_datetime_tz(&self) -> Option<&DateTimeTz> {
        match self {
            Self::DateTimeTz(d) => Some(d),
            _ => None,
        }
    }

    pub fn as_duration(&self) -> Option<&Duration> {
        match self {
            Self::Duration(d) => Some(d),
            _ => None,
        }
    }

    fn to_canonical_value(
        &self,
        path: &ValidationPath,
    ) -> Result<type_bridge_contract::value::CanonicalValue, ValidationError> {
        use type_bridge_contract::temporal::{
            CanonicalDate, CanonicalDateTime, CanonicalDateTimeTz, CanonicalDuration,
        };
        use type_bridge_contract::value::{CanonicalDouble, CanonicalString, CanonicalValue};
        match self {
            Self::String(s) => CanonicalString::new(s)
                .map(CanonicalValue::String)
                .map_err(|_| ValidationError::new(path.path(), "string_limit_exceeded")),
            Self::Long(n) => Ok(CanonicalValue::Long(*n)),
            Self::Double(d) => CanonicalDouble::new(d.get())
                .map(CanonicalValue::Double)
                .map_err(|_| ValidationError::new(path.path(), "noncanonical_double")),
            Self::Boolean(b) => Ok(CanonicalValue::Boolean(*b)),
            Self::Decimal(d) => {
                if type_bridge_contract::decimal::parse_decimal(d.as_str()).is_some() {
                    type_bridge_contract::value::DecimalValue::new(d.as_str())
                        .map(CanonicalValue::Decimal)
                        .map_err(|_| ValidationError::new(path.path(), "noncanonical_decimal"))
                } else {
                    Err(ValidationError::new(path.path(), "noncanonical_decimal"))
                }
            }
            Self::Date(d) => d
                .as_str()
                .parse::<CanonicalDate>()
                .map(CanonicalValue::Date)
                .map_err(|_| ValidationError::new(path.path(), "noncanonical_date")),
            Self::DateTime(dt) => dt
                .as_str()
                .parse::<CanonicalDateTime>()
                .map(CanonicalValue::DateTime)
                .map_err(|_| ValidationError::new(path.path(), "noncanonical_datetime")),
            Self::DateTimeTz(dtz) => dtz
                .as_str()
                .parse::<CanonicalDateTimeTz>()
                .map(CanonicalValue::DateTimeTz)
                .map_err(|_| ValidationError::new(path.path(), "noncanonical_datetime_tz")),
            Self::Duration(dur) => dur
                .as_str()
                .parse::<CanonicalDuration>()
                .map(CanonicalValue::Duration)
                .map_err(|_| ValidationError::new(path.path(), "noncanonical_duration")),
        }
    }
}

pub fn validate_canonical_string(
    value: &str,
    path: &ValidationPath,
) -> Result<(), ValidationError> {
    if type_bridge_contract::value::CanonicalString::new(value).is_err() {
        return Err(ValidationError::new(path.path(), "string_limit_exceeded"));
    }
    Ok(())
}

pub fn prefix_validation_path(
    err: ValidationError,
    parent_path: &ValidationPath,
) -> ValidationError {
    let sub_path = err.field();
    let full_path = if sub_path.is_empty() || sub_path == "value" {
        parent_path.path()
    } else {
        format!("{}.{}", parent_path.path(), sub_path)
    };
    ValidationError::new(full_path, err.code())
}

/// A document-hidden closed constraint descriptor for attribute value and owns-edge validation.
#[doc(hidden)]
#[derive(Clone, Debug, PartialEq)]
pub struct ConstraintDescriptor {
    range_min: Option<EncodedScalar>,
    range_max: Option<EncodedScalar>,
    regex: Option<&'static str>,
    values: Option<Vec<EncodedScalar>>,
}

impl ConstraintDescriptor {
    #[must_use]
    pub fn new(
        range_min: Option<EncodedScalar>,
        range_max: Option<EncodedScalar>,
        regex: Option<&'static str>,
        values: Option<Vec<EncodedScalar>>,
    ) -> Self {
        Self {
            range_min,
            range_max,
            regex,
            values,
        }
    }

    pub fn validate(
        &self,
        value: &EncodedScalar,
        path: &ValidationPath,
    ) -> Result<(), ValidationError> {
        if let Some(pattern) = self.regex {
            let Some(s) = value.as_string() else {
                return Err(ValidationError::new(path.path(), "wrong_scalar_domain"));
            };
            let re = regex::Regex::new(pattern)
                .map_err(|_| ValidationError::new(path.path(), "invalid_regex_pattern"))?;
            if !re.is_match(s) {
                return Err(ValidationError::new(path.path(), "regex_violation"));
            }
        }

        let canonical_val = value.to_canonical_value(path)?;

        if let Some(allowed) = &self.values {
            let mut found = false;
            for item in allowed {
                let allowed_canon = item.to_canonical_value(path)?;
                if canonical_val.value_type() != allowed_canon.value_type() {
                    return Err(ValidationError::new(path.path(), "wrong_scalar_domain"));
                }
                let equal = match canonical_val.semantic_cmp_same_domain(&allowed_canon) {
                    Some(std::cmp::Ordering::Equal) => true,
                    Some(_) => false,
                    None => canonical_val == allowed_canon,
                };
                if equal {
                    found = true;
                    break;
                }
            }
            if !found {
                return Err(ValidationError::new(path.path(), "values_violation"));
            }
        }
        if let Some(min) = &self.range_min {
            let min_canon = min.to_canonical_value(path)?;
            let cmp = canonical_val
                .semantic_cmp_same_domain(&min_canon)
                .ok_or_else(|| ValidationError::new(path.path(), "wrong_scalar_domain"))?;
            if cmp == std::cmp::Ordering::Less {
                return Err(ValidationError::new(path.path(), "range_violation"));
            }
        }
        if let Some(max) = &self.range_max {
            let max_canon = max.to_canonical_value(path)?;
            let cmp = canonical_val
                .semantic_cmp_same_domain(&max_canon)
                .ok_or_else(|| ValidationError::new(path.path(), "wrong_scalar_domain"))?;
            if cmp == std::cmp::Ordering::Greater {
                return Err(ValidationError::new(path.path(), "range_violation"));
            }
        }
        Ok(())
    }
}

/// A transport-neutral encoded IID-or-typed-key reference.
#[doc(hidden)]
#[derive(Clone, Debug, PartialEq)]
pub struct EncodedReference {
    type_id_json: &'static str,
    iid: Option<String>,
    keys: Vec<(&'static str, EncodedScalar)>,
}

impl EncodedReference {
    pub fn try_new(
        type_id_json: &'static str,
        iid: Option<String>,
        keys: Vec<(&'static str, EncodedScalar)>,
        path: &ValidationPath,
    ) -> Result<Self, ValidationError> {
        if iid.as_deref().is_some_and(|value| value.trim().is_empty()) {
            return Err(ValidationError::new(path.join("iid").path(), "empty_iid"));
        }
        let mut seen = std::collections::BTreeSet::new();
        for (index, (token, _)) in keys.iter().enumerate() {
            if !seen.insert(*token) {
                return Err(ValidationError::new(
                    path.join("keys").join_index(index).path(),
                    "duplicate_reference_key",
                ));
            }
        }
        if iid.is_none() && keys.is_empty() {
            return Err(ValidationError::new(
                path.path(),
                "missing_reference_identity",
            ));
        }
        if iid.is_none() && keys.len() > 1 {
            return Err(ValidationError::new(
                path.path(),
                "multiple_reference_keys_without_iid",
            ));
        }
        Ok(Self {
            type_id_json,
            iid,
            keys,
        })
    }

    #[must_use]
    pub const fn type_id_json(&self) -> &'static str {
        self.type_id_json
    }

    #[must_use]
    pub fn iid(&self) -> Option<&str> {
        self.iid.as_deref()
    }

    #[must_use]
    pub fn keys(&self) -> &[(&'static str, EncodedScalar)] {
        &self.keys
    }
}

/// Ordered encoded owned fields and active roles for a thing creation payload.
#[doc(hidden)]
#[derive(Clone, Debug, PartialEq)]
pub struct EncodedCreate {
    type_id_json: &'static str,
    fields: Vec<(&'static str, Vec<EncodedScalar>)>,
    roles: Vec<(&'static str, Vec<EncodedReference>)>,
}

impl EncodedCreate {
    #[must_use]
    pub const fn new(
        type_id_json: &'static str,
        fields: Vec<(&'static str, Vec<EncodedScalar>)>,
        roles: Vec<(&'static str, Vec<EncodedReference>)>,
    ) -> Self {
        Self {
            type_id_json,
            fields,
            roles,
        }
    }

    #[must_use]
    pub const fn type_id_json(&self) -> &'static str {
        self.type_id_json
    }

    #[must_use]
    pub fn fields(&self) -> &[(&'static str, Vec<EncodedScalar>)] {
        &self.fields
    }

    #[must_use]
    pub fn roles(&self) -> &[(&'static str, Vec<EncodedReference>)] {
        &self.roles
    }
}

/// Nonrecursive player reference evidence carried by a hydrated role.
#[doc(hidden)]
#[derive(Clone, Debug, PartialEq)]
pub struct HydratedPlayer {
    type_id_json: String,
    iid: Option<String>,
    keys: Vec<(String, EncodedScalar)>,
}

impl HydratedPlayer {
    #[must_use]
    pub fn new(
        type_id_json: &'static str,
        iid: Option<String>,
        keys: Vec<(&'static str, EncodedScalar)>,
    ) -> Self {
        Self {
            type_id_json: type_id_json.to_owned(),
            iid,
            keys: keys
                .into_iter()
                .map(|(identity, value)| (identity.to_owned(), value))
                .collect(),
        }
    }

    #[must_use]
    #[allow(dead_code)]
    pub(crate) fn from_owned(
        type_id_json: String,
        iid: Option<String>,
        keys: Vec<(String, EncodedScalar)>,
    ) -> Self {
        Self {
            type_id_json,
            iid,
            keys,
        }
    }

    #[must_use]
    pub fn type_id_json(&self) -> &str {
        &self.type_id_json
    }

    #[must_use]
    pub fn iid(&self) -> Option<&str> {
        self.iid.as_deref()
    }

    #[must_use]
    pub fn keys(&self) -> &[(String, EncodedScalar)] {
        &self.keys
    }
}

/// A transport-neutral hydrated thing with exact concrete type identity, mandatory IID, owned-value evidence, and role-player evidence.
#[doc(hidden)]
#[derive(Clone, Debug, PartialEq)]
pub struct HydratedRow {
    type_id_json: String,
    iid: String,
    fields: Vec<(String, Vec<EncodedScalar>)>,
    roles: Vec<(String, Vec<HydratedPlayer>)>,
}

impl HydratedRow {
    #[must_use]
    pub fn new(
        type_id_json: &'static str,
        iid: String,
        fields: Vec<(&'static str, Vec<EncodedScalar>)>,
        roles: Vec<(&'static str, Vec<HydratedPlayer>)>,
    ) -> Self {
        Self {
            type_id_json: type_id_json.to_owned(),
            iid,
            fields: fields
                .into_iter()
                .map(|(identity, values)| (identity.to_owned(), values))
                .collect(),
            roles: roles
                .into_iter()
                .map(|(identity, players)| (identity.to_owned(), players))
                .collect(),
        }
    }

    #[must_use]
    #[allow(dead_code)]
    pub(crate) fn from_owned(
        type_id_json: String,
        iid: String,
        fields: Vec<(String, Vec<EncodedScalar>)>,
        roles: Vec<(String, Vec<HydratedPlayer>)>,
    ) -> Self {
        Self {
            type_id_json,
            iid,
            fields,
            roles,
        }
    }

    #[must_use]
    pub fn type_id_json(&self) -> &str {
        &self.type_id_json
    }

    #[must_use]
    pub fn iid(&self) -> &str {
        &self.iid
    }

    #[must_use]
    pub fn fields(&self) -> &[(String, Vec<EncodedScalar>)] {
        &self.fields
    }

    #[must_use]
    pub fn roles(&self) -> &[(String, Vec<HydratedPlayer>)] {
        &self.roles
    }

    pub fn validate_shape(
        &self,
        expected_type_id: &'static str,
        expected_fields: &[&'static str],
        expected_roles: &[&'static str],
        path: &ValidationPath,
    ) -> Result<(), ValidationError> {
        if self.type_id_json != expected_type_id {
            return Err(ValidationError::new(
                path.path(),
                "wrong_concrete_model_type",
            ));
        }
        let mut seen_fields = std::collections::BTreeSet::new();
        for (k, _) in &self.fields {
            if !seen_fields.insert(k.as_str()) {
                return Err(ValidationError::new(
                    path.path(),
                    "duplicate_scalar_evidence",
                ));
            }
            if !expected_fields.contains(&k.as_str()) {
                return Err(ValidationError::new(
                    path.path(),
                    "unexpected_field_evidence",
                ));
            }
        }
        let mut seen_roles = std::collections::BTreeSet::new();
        for (r, _) in &self.roles {
            if !seen_roles.insert(r.as_str()) {
                return Err(ValidationError::new(path.path(), "duplicate_role_evidence"));
            }
            if !expected_roles.contains(&r.as_str()) {
                return Err(ValidationError::new(
                    path.path(),
                    "unexpected_role_evidence",
                ));
            }
        }
        Ok(())
    }
}

/// Lowering trait from a generated create payload into an `EncodedCreate`.
#[doc(hidden)]
pub trait IntoEncodedCreate: sealed::Sealed {
    fn into_encoded_create(self) -> Result<EncodedCreate, ValidationError>;
}

/// Uninhabited create payload for a concrete read model whose schema shape
/// cannot be instantiated at its own scope.
#[doc(hidden)]
#[derive(Clone, Debug, PartialEq)]
pub enum UnconstructibleCreate {}

impl sealed::Sealed for UnconstructibleCreate {}

impl IntoEncodedCreate for UnconstructibleCreate {
    fn into_encoded_create(self) -> Result<EncodedCreate, ValidationError> {
        match self {}
    }
}

/// Lowering trait from a generated reference into an `EncodedReference`.
#[doc(hidden)]
pub trait IntoEncodedReference: sealed::Sealed {
    fn into_encoded_reference(self) -> Result<EncodedReference, ValidationError>;
}

/// Materializing trait for generated complete read models or families from a `HydratedRow`.
#[doc(hidden)]
pub trait MaterializeModel: Model + Sized {
    fn materialize(row: &HydratedRow, cap: &HydrationCapability) -> Result<Self, ValidationError>;
}

/// A closed subtype family enum representing a concrete descendant closure for a root `Root`.
pub trait ModelFamily: sealed::Sealed {
    type Root: ThingModel;
    type Schema: Schema;
    fn iid(&self) -> &str;
}

/// A sealed generated struct value associated with its schema marker `S`.
pub trait StructValue: sealed::Sealed {
    type Schema: Schema;
    const STRUCT_ID_JSON: &'static str;
}

/// A resolver-proven nominal upcast relation.
pub trait NominalUpcast<Target: Model>: Model {}

/// A resolver-proven specialized-role upcast relation.
pub trait RoleUpcast<ActiveRole, AncestorRole>: Model {}

/// Positive generated evidence that one role token is active on a relation.
///
/// Unlike nominal inheritance, this relation is intentionally subtractive:
/// generated specializing relations do not implement compatibility for the
/// specialized-away ancestor role.
pub trait RoleTokenCompatible<Owner: RelationModel, Players>: RelationModel {}

/// Positive generated evidence that a role's exact player union admits one
/// bound player model.
pub trait RolePlayer<Player: ThingModel> {}

/// Positive generated evidence that a role admits one binding mode and model.
pub trait RolePlayerBinding<Player: ThingModel, Mode> {}

impl<Players, Player> RolePlayerBinding<Player, crate::query::Exact> for Players
where
    Player: ThingModel,
    Players: RolePlayer<Player>,
{
}

/// A schema type/query token branded by its exact owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TypeToken<Owner: Model> {
    type_id_json: &'static str,
    metadata_json: &'static str,
    marker: PhantomData<fn() -> Owner>,
}

impl<Owner: Model> TypeToken<Owner> {
    #[must_use]
    pub const fn new(type_id_json: &'static str, metadata_json: &'static str) -> Self {
        Self {
            type_id_json,
            metadata_json,
            marker: PhantomData,
        }
    }

    #[must_use]
    pub const fn type_id_json(self) -> &'static str {
        self.type_id_json
    }

    #[must_use]
    pub const fn metadata_json(self) -> &'static str {
        self.metadata_json
    }
}

/// An owned-field token branded by owner and value model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FieldToken<Owner: Model, Value> {
    owns_id_json: &'static str,
    metadata_json: &'static str,
    marker: PhantomData<fn() -> (Owner, Value)>,
}

impl<Owner: Model, Value> FieldToken<Owner, Value> {
    #[must_use]
    pub const fn new(owns_id_json: &'static str, metadata_json: &'static str) -> Self {
        Self {
            owns_id_json,
            metadata_json,
            marker: PhantomData,
        }
    }

    #[must_use]
    pub const fn owns_id_json(self) -> &'static str {
        self.owns_id_json
    }

    #[must_use]
    pub const fn metadata_json(self) -> &'static str {
        self.metadata_json
    }
}

/// A related-role token branded by owner and exact accepted-player enum.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RoleToken<Owner: Model, Players> {
    role_id_json: &'static str,
    metadata_json: &'static str,
    marker: PhantomData<fn() -> (Owner, Players)>,
}

impl<Owner: Model, Players> RoleToken<Owner, Players> {
    #[must_use]
    pub const fn new(role_id_json: &'static str, metadata_json: &'static str) -> Self {
        Self {
            role_id_json,
            metadata_json,
            marker: PhantomData,
        }
    }

    #[must_use]
    pub const fn role_id_json(self) -> &'static str {
        self.role_id_json
    }

    #[must_use]
    pub const fn metadata_json(self) -> &'static str {
        self.metadata_json
    }
}

/// A playing-fact token branded by player, role owner, and accepted-player enum.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlaysToken<Player: Model, Owner: Model, Players> {
    plays_id_json: &'static str,
    metadata_json: &'static str,
    #[allow(clippy::type_complexity)]
    marker: PhantomData<fn() -> (Player, Owner, Players)>,
}

impl<Player: Model, Owner: Model, Players> PlaysToken<Player, Owner, Players> {
    #[must_use]
    pub const fn new(plays_id_json: &'static str, metadata_json: &'static str) -> Self {
        Self {
            plays_id_json,
            metadata_json,
            marker: PhantomData,
        }
    }

    #[must_use]
    pub const fn plays_id_json(self) -> &'static str {
        self.plays_id_json
    }

    #[must_use]
    pub const fn metadata_json(self) -> &'static str {
        self.metadata_json
    }
}

/// A typed schema-function token branded by schema `S`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FunctionToken<S: Schema, Arguments, Output> {
    function_id: &'static str,
    metadata_json: &'static str,
    marker: PhantomData<fn(Arguments) -> (S, Output)>,
}

impl<S: Schema, Arguments, Output> FunctionToken<S, Arguments, Output> {
    #[must_use]
    pub const fn new(function_id: &'static str, metadata_json: &'static str) -> Self {
        Self {
            function_id,
            metadata_json,
            marker: PhantomData,
        }
    }

    #[must_use]
    pub const fn function_id(self) -> &'static str {
        self.function_id
    }

    #[must_use]
    pub const fn metadata_json(self) -> &'static str {
        self.metadata_json
    }
}

/// A typed asynchronous stream result marker.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Stream<T>(PhantomData<fn() -> T>);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_scalar_wrappers_do_not_expose_lexical_ordering() {
        let source = include_str!("__codegen.rs");
        let (_, macro_and_invocations) = source
            .split_once("macro_rules! canonical_scalar")
            .expect("canonical scalar macro remains present");
        let (macro_body, _) = macro_and_invocations
            .split_once("canonical_scalar!(Decimal")
            .expect("canonical scalar invocations remain present");
        assert!(macro_body.contains("#[derive(Clone, Debug, Eq, PartialEq, Hash)]"));
        assert!(!macro_body.contains("Ord"));
        assert!(!macro_body.contains("PartialOrd"));
    }

    #[test]
    fn scalar_domain_wrappers_and_validation() {
        let path = ValidationPath::root().join("test");

        let double_ok = CanonicalDouble::try_new(3.125).unwrap();
        assert_eq!(double_ok.get(), 3.125);
        assert_eq!(
            CanonicalDouble::try_new(f64::NAN).unwrap_err().code(),
            "noncanonical_double"
        );
        assert_eq!(
            CanonicalDouble::try_new(f64::INFINITY).unwrap_err().code(),
            "noncanonical_double"
        );
        let neg_zero = CanonicalDouble::try_new(-0.0).unwrap();
        let pos_zero = CanonicalDouble::try_new(0.0).unwrap();
        assert_ne!(neg_zero.to_bits(), pos_zero.to_bits());

        let dec = Decimal::try_new("123.45").unwrap();
        assert_eq!(dec.as_str(), "123.45");
        assert_eq!(
            Decimal::try_new("123.4500").unwrap_err().code(),
            "noncanonical_decimal"
        );

        let date = Date::try_new("2026-07-28").unwrap();
        assert_eq!(date.as_str(), "2026-07-28");
        assert_eq!(
            Date::try_new("2026-7-28").unwrap_err().code(),
            "noncanonical_date"
        );

        let dt = DateTime::try_new("2026-07-28T03:55:00").unwrap();
        assert_eq!(dt.as_str(), "2026-07-28T03:55:00");

        let dtz = DateTimeTz::try_new("2026-07-28T03:55:00Z").unwrap();
        assert_eq!(dtz.as_str(), "2026-07-28T03:55:00Z");

        let dur = Duration::try_new("P1D").unwrap();
        assert_eq!(dur.as_str(), "P1D");

        let seq = Sequence::try_new(vec![1, 2], Cardinality::new(1, Some(3)), &path).unwrap();
        assert_eq!(seq.as_slice(), &[1, 2]);

        let seq_err = Sequence::try_new(vec![1, 2, 3, 4], Cardinality::new(1, Some(3)), &path);
        assert_eq!(seq_err.unwrap_err().code(), "cardinality_violation");

        let constraint = ConstraintDescriptor::new(
            Some(EncodedScalar::String("a".to_owned())),
            Some(EncodedScalar::String("z".to_owned())),
            Some("^a.*z$"),
            None,
        );
        let s_val = EncodedScalar::String("abcz".to_owned());
        assert_eq!(constraint.validate(&s_val, &path), Ok(()));
    }
}
