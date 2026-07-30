#![deny(missing_docs)]
//! Typed reduction terms and tuple decoding for the query facade
//! (Flight 3, F3-04b-3).
//!
//! Aggregates use the same query lineage and bound-field handles as row
//! terminals and lower onto the canonical typed reduce operation; they
//! never route through legacy string-keyed aggregate maps.

use std::marker::PhantomData;

use type_bridge_orm::match_request::{ReducedValue, Reduction};

use crate::__codegen::Model;
use crate::__codegen::NumericValued;
use crate::Result;
use crate::error::{Error, ModelValidationPhase};
use crate::query::{BindingKey, BoundField};
use crate::schema::Schema;

pub(crate) fn wrong_reduction_value() -> Error {
    Error::model_validation(
        ModelValidationPhase::Hydration,
        "wrong_result_shape",
        vec![],
        "provider reduction value does not fit its requested reducer",
        None,
    )
}

/// One typed reducer term over the query's distinct selected stream.
pub struct Agg<S: Schema, Out> {
    pub(crate) reduction: Reduction,
    pub(crate) input: Option<(BindingKey, &'static str)>,
    marker: PhantomData<fn() -> (S, Out)>,
}

impl<S: Schema, Out> Copy for Agg<S, Out> {}
impl<S: Schema, Out> Clone for Agg<S, Out> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<S: Schema, Out> Agg<S, Out> {
    pub(crate) fn new(reduction: Reduction, input: Option<(BindingKey, &'static str)>) -> Self {
        Self {
            reduction,
            input,
            marker: PhantomData,
        }
    }
}

/// Count distinct selected identities; zero on an empty ungrouped stream.
#[must_use]
pub fn count<S: Schema>() -> Agg<S, u64> {
    Agg::new(Reduction::Count, None)
}

/// Sealed decoding of one typed reduced scalar into its facade output.
pub trait ReducedOutput: reduced_sealed::Sealed + Sized {
    #[doc(hidden)]
    fn decode(value: &ReducedValue) -> Result<Self>;
}

mod reduced_sealed {
    pub trait Sealed {}
}

impl reduced_sealed::Sealed for u64 {}
impl ReducedOutput for u64 {
    fn decode(value: &ReducedValue) -> Result<Self> {
        match value {
            ReducedValue::Count(value) => Ok(*value),
            _ => Err(wrong_reduction_value()),
        }
    }
}

impl reduced_sealed::Sealed for i64 {}
impl ReducedOutput for i64 {
    fn decode(value: &ReducedValue) -> Result<Self> {
        match value {
            ReducedValue::Long(Some(value)) => Ok(*value),
            _ => Err(wrong_reduction_value()),
        }
    }
}

impl reduced_sealed::Sealed for f64 {}
impl ReducedOutput for f64 {
    fn decode(value: &ReducedValue) -> Result<Self> {
        match value {
            ReducedValue::Double(Some(value)) => Ok(*value),
            _ => Err(wrong_reduction_value()),
        }
    }
}

impl reduced_sealed::Sealed for Option<i64> {}
impl ReducedOutput for Option<i64> {
    fn decode(value: &ReducedValue) -> Result<Self> {
        match value {
            ReducedValue::Long(value) => Ok(*value),
            _ => Err(wrong_reduction_value()),
        }
    }
}

impl reduced_sealed::Sealed for Option<f64> {}
impl ReducedOutput for Option<f64> {
    fn decode(value: &ReducedValue) -> Result<Self> {
        match value {
            ReducedValue::Double(value) => Ok(*value),
            _ => Err(wrong_reduction_value()),
        }
    }
}

impl<S, Owner, V> BoundField<S, Owner, V>
where
    S: Schema,
    Owner: Model<Schema = S>,
    V: NumericValued,
{
    fn aggregate<Out>(self, reduction: Reduction) -> Agg<S, Out> {
        let (key, owns_id_json) = self.reduction_input();
        Agg::new(reduction, Some((key, owns_id_json)))
    }

    /// The total of this field's values; domain-preserving and total on an
    /// empty ungrouped stream (zero). A provider reporting an absent sum
    /// fails closed at decode.
    #[must_use]
    pub fn sum(self) -> Agg<S, V::Reduced> {
        self.aggregate(Reduction::Sum)
    }

    /// The smallest of this field's values; absent on an empty ungrouped
    /// stream.
    #[must_use]
    pub fn min(self) -> Agg<S, Option<V::Reduced>> {
        self.aggregate(Reduction::Min)
    }

    /// The largest of this field's values; absent on an empty ungrouped
    /// stream.
    #[must_use]
    pub fn max(self) -> Agg<S, Option<V::Reduced>> {
        self.aggregate(Reduction::Max)
    }

    /// The arithmetic mean of this field's values; absent on an empty
    /// ungrouped stream.
    #[must_use]
    pub fn mean(self) -> Agg<S, Option<f64>> {
        self.aggregate(Reduction::Mean)
    }

    /// The statistical median of this field's values; absent on an empty
    /// ungrouped stream.
    #[must_use]
    pub fn median(self) -> Agg<S, Option<f64>> {
        self.aggregate(Reduction::Median)
    }

    /// The sample standard deviation of this field's values; absent when
    /// fewer than two values are witnessed on an ungrouped stream.
    #[must_use]
    pub fn stddev(self) -> Agg<S, Option<f64>> {
        self.aggregate(Reduction::Std)
    }
}

/// Sealed tuple of typed reducer terms through the sixteen-slot ceiling.
pub trait AggregateTuple<S: Schema>: tuple_sealed::Sealed<S> {
    /// The typed tuple output decoded from one reduction row.
    type Output;
    #[doc(hidden)]
    fn terms(&self) -> Vec<(Reduction, Option<(BindingKey, &'static str)>)>;
    #[doc(hidden)]
    fn decode(values: &[ReducedValue]) -> Result<Self::Output>;
}

mod tuple_sealed {
    pub trait Sealed<S> {}
}

macro_rules! aggregate_tuple {
    ($(($name:ident, $out:ident, $index:tt)),+) => {
        impl<S: Schema, $($out: ReducedOutput),+> tuple_sealed::Sealed<S>
            for ($(Agg<S, $out>,)+)
        {
        }
        impl<S: Schema, $($out: ReducedOutput),+> AggregateTuple<S> for ($(Agg<S, $out>,)+) {
            type Output = ($($out,)+);
            fn terms(&self) -> Vec<(Reduction, Option<(BindingKey, &'static str)>)> {
                vec![$((self.$index.reduction, self.$index.input)),+]
            }
            fn decode(values: &[ReducedValue]) -> Result<Self::Output> {
                let expected = [$(stringify!($name)),+].len();
                if values.len() != expected {
                    return Err(wrong_reduction_value());
                }
                Ok(($($out::decode(&values[$index])?,)+))
            }
        }
    };
}

aggregate_tuple!((a, A, 0));
aggregate_tuple!((a, A, 0), (b, B, 1));
aggregate_tuple!((a, A, 0), (b, B, 1), (c, C, 2));
aggregate_tuple!((a, A, 0), (b, B, 1), (c, C, 2), (d, D, 3));
aggregate_tuple!((a, A, 0), (b, B, 1), (c, C, 2), (d, D, 3), (e, E, 4));
aggregate_tuple!(
    (a, A, 0),
    (b, B, 1),
    (c, C, 2),
    (d, D, 3),
    (e, E, 4),
    (f, F, 5)
);
aggregate_tuple!(
    (a, A, 0),
    (b, B, 1),
    (c, C, 2),
    (d, D, 3),
    (e, E, 4),
    (f, F, 5),
    (g, G, 6)
);
aggregate_tuple!(
    (a, A, 0),
    (b, B, 1),
    (c, C, 2),
    (d, D, 3),
    (e, E, 4),
    (f, F, 5),
    (g, G, 6),
    (h, H, 7)
);
