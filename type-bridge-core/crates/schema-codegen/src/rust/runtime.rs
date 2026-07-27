//! Fixed, dependency-free runtime primitives for generated schema crates.

use core::fmt;
use core::marker::PhantomData;

/// A stable generated-input validation failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidationError {
    field: &'static str,
    code: &'static str,
}

impl ValidationError {
    #[must_use]
    pub const fn new(field: &'static str, code: &'static str) -> Self {
        Self { field, code }
    }

    #[must_use]
    pub const fn field(&self) -> &'static str { self.field }

    #[must_use]
    pub const fn code(&self) -> &'static str { self.code }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.field, self.code)
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
    pub const fn new(min: u64, max: Option<u64>) -> Self { Self { min, max } }

    #[must_use]
    pub const fn min(self) -> u64 { self.min }

    #[must_use]
    pub const fn max(self) -> Option<u64> { self.max }
}

/// A statically required scalar value.
#[derive(Clone, Debug, PartialEq)]
pub struct Required<T>(T);

impl<T> Required<T> {
    #[must_use]
    pub const fn new(value: T) -> Self { Self(value) }

    #[must_use]
    pub const fn get(&self) -> &T { &self.0 }
}

/// An optional scalar value.
#[derive(Clone, Debug, PartialEq)]
pub struct Optional<T>(Option<T>);

impl<T> Optional<T> {
    #[must_use]
    pub const fn new(value: Option<T>) -> Self { Self(value) }

    #[must_use]
    pub const fn as_ref(&self) -> Option<&T> { self.0.as_ref() }
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
        field: &'static str,
    ) -> Result<Self, ValidationError> {
        let length = u64::try_from(values.len())
            .map_err(|_| ValidationError::new(field, "cardinality_overflow"))?;
        if length < cardinality.min()
            || cardinality.max().is_some_and(|maximum| length > maximum)
        {
            return Err(ValidationError::new(field, "cardinality_violation"));
        }
        Ok(Self { values, cardinality })
    }

    #[must_use]
    pub fn as_slice(&self) -> &[T] { &self.values }

    #[must_use]
    pub const fn cardinality(&self) -> Cardinality { self.cardinality }
}

/// A dependency-free binary sum used to preserve exact heterogeneous model forms.
#[derive(Clone, Debug, PartialEq)]
pub enum Either<L, R> {
    Left(L),
    Right(R),
}

/// The uninhabited projection of an empty accepted-player set.
#[derive(Clone, Debug, PartialEq)]
pub enum Never {}

/// A finite, normalized floating-point value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CanonicalDouble(f64);

impl CanonicalDouble {
    pub fn try_new(value: f64) -> Result<Self, ValidationError> {
        if !value.is_finite() {
            return Err(ValidationError::new("value", "noncanonical_double"));
        }
        Ok(Self(if value == 0.0 { 0.0 } else { value }))
    }

    #[must_use]
    pub const fn get(self) -> f64 { self.0 }
}

macro_rules! string_value {
    ($name:ident) => {
        #[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
        pub struct $name(String);

        impl $name {
            #[must_use]
            pub fn new(value: impl Into<String>) -> Self { Self(value.into()) }

            #[must_use]
            pub fn as_str(&self) -> &str { &self.0 }
        }
    };
}

string_value!(Decimal);
string_value!(Date);
string_value!(DateTime);
string_value!(DateTimeTz);
string_value!(Duration);

pub(crate) mod sealed {
    pub trait Sealed {}
}

/// A sealed generated schema model.
pub trait Model: sealed::Sealed {
    const TYPE_ID_JSON: &'static str;
}

/// A complete materialized generated model.
pub trait CompleteModel: Model {}

/// A nonrecursive generated reference model.
pub trait ReferenceModel: Model {}

/// A sealed generated struct value.
pub trait StructValue: sealed::Sealed {
    const STRUCT_ID_JSON: &'static str;
}

/// A resolver-proven nominal upcast relation.
pub trait NominalUpcast<Target: Model>: Model {}

/// A resolver-proven specialized-role upcast relation.
pub trait RoleUpcast<ActiveRole, AncestorRole>: Model {}

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
        Self { type_id_json, metadata_json, marker: PhantomData }
    }

    #[must_use]
    pub const fn type_id_json(self) -> &'static str { self.type_id_json }

    #[must_use]
    pub const fn metadata_json(self) -> &'static str { self.metadata_json }
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
        Self { owns_id_json, metadata_json, marker: PhantomData }
    }

    #[must_use]
    pub const fn owns_id_json(self) -> &'static str { self.owns_id_json }

    #[must_use]
    pub const fn metadata_json(self) -> &'static str { self.metadata_json }
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
        Self { role_id_json, metadata_json, marker: PhantomData }
    }

    #[must_use]
    pub const fn role_id_json(self) -> &'static str { self.role_id_json }

    #[must_use]
    pub const fn metadata_json(self) -> &'static str { self.metadata_json }
}

/// A playing-fact token branded by player, role owner, and accepted-player enum.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlaysToken<Player: Model, Owner: Model, Players> {
    plays_id_json: &'static str,
    metadata_json: &'static str,
    marker: PhantomData<fn() -> (Player, Owner, Players)>,
}

impl<Player: Model, Owner: Model, Players> PlaysToken<Player, Owner, Players> {
    #[must_use]
    pub const fn new(plays_id_json: &'static str, metadata_json: &'static str) -> Self {
        Self { plays_id_json, metadata_json, marker: PhantomData }
    }

    #[must_use]
    pub const fn plays_id_json(self) -> &'static str { self.plays_id_json }

    #[must_use]
    pub const fn metadata_json(self) -> &'static str { self.metadata_json }
}

/// A typed schema-function token.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FunctionToken<Arguments, Output> {
    function_id: &'static str,
    metadata_json: &'static str,
    marker: PhantomData<fn(Arguments) -> Output>,
}

impl<Arguments, Output> FunctionToken<Arguments, Output> {
    #[must_use]
    pub const fn new(function_id: &'static str, metadata_json: &'static str) -> Self {
        Self { function_id, metadata_json, marker: PhantomData }
    }

    #[must_use]
    pub const fn function_id(self) -> &'static str { self.function_id }

    #[must_use]
    pub const fn metadata_json(self) -> &'static str { self.metadata_json }
}

/// A typed asynchronous stream result marker.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Stream<T>(PhantomData<fn() -> T>);
