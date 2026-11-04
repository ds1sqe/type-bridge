//! Canonical scalar conversion and host-text admission shared by plan bindings.

use crate::plan_lowering::failure;
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory};
use type_bridge_contract::temporal::{
    CanonicalDate, CanonicalDateTime, CanonicalDateTimeTz, CanonicalDuration,
};
use type_bridge_contract::value::{
    CanonicalDouble, CanonicalString, CanonicalValue, DecimalValue, ValueTypeTag,
};

/// Primitive host value accepted by the binding-neutral scalar converter.
///
/// Python and Node inspect only the host primitive kind, then delegate all
/// canonical-domain parsing and validation to this shared conversion.
#[doc(hidden)]
pub enum PlanScalarInput {
    /// A host string, including canonical temporal/decimal/duration text.
    Text(String),
    /// An exact signed 64-bit integer.
    Long(i64),
    /// A host binary64 number.
    Double(f64),
    /// A host boolean.
    Boolean(bool),
}

/// Parse the exact low-level scalar-domain spelling shared by both bindings.
#[doc(hidden)]
pub fn plan_value_type(value: &str) -> Result<ValueTypeTag, Diagnostic> {
    value.parse().map_err(Diagnostic::from)
}

/// Convert one checked host primitive into its exact canonical scalar.
#[doc(hidden)]
pub fn plan_scalar(
    value_type: ValueTypeTag,
    value: PlanScalarInput,
) -> Result<CanonicalValue, Diagnostic> {
    match (value_type, value) {
        (ValueTypeTag::String, PlanScalarInput::Text(value)) => {
            CanonicalString::new(value).map(CanonicalValue::String)
        }
        (ValueTypeTag::Long, PlanScalarInput::Long(value)) => Ok(CanonicalValue::Long(value)),
        (ValueTypeTag::Double, PlanScalarInput::Double(value)) => {
            CanonicalDouble::new(value).map(CanonicalValue::Double)
        }
        (ValueTypeTag::Boolean, PlanScalarInput::Boolean(value)) => {
            Ok(CanonicalValue::Boolean(value))
        }
        (ValueTypeTag::Date, PlanScalarInput::Text(value)) => {
            value.parse::<CanonicalDate>().map(CanonicalValue::Date)
        }
        (ValueTypeTag::DateTime, PlanScalarInput::Text(value)) => value
            .parse::<CanonicalDateTime>()
            .map(CanonicalValue::DateTime),
        (ValueTypeTag::DateTimeTz, PlanScalarInput::Text(value)) => value
            .parse::<CanonicalDateTimeTz>()
            .map(CanonicalValue::DateTimeTz),
        (ValueTypeTag::Decimal, PlanScalarInput::Text(value)) => {
            DecimalValue::new(value).map(CanonicalValue::Decimal)
        }
        (ValueTypeTag::Duration, PlanScalarInput::Text(value)) => value
            .parse::<CanonicalDuration>()
            .map(CanonicalValue::Duration),
        _ => Err(failure(
            DiagnosticCategory::InvalidContract,
            "query_builder_scalar_host_type",
            "host scalar kind does not match the declared canonical value type",
        )),
    }
}

/// Return the stable diagnostic for a host primitive of the wrong kind.
#[doc(hidden)]
#[must_use]
pub fn plan_scalar_host_type_error() -> Diagnostic {
    failure(
        DiagnosticCategory::InvalidContract,
        "query_builder_scalar_host_type",
        "host scalar kind does not match the declared canonical value type",
    )
}

/// Return the stable diagnostic for an integer outside signed 64-bit range.
#[doc(hidden)]
#[must_use]
pub fn plan_scalar_integer_range_error() -> Diagnostic {
    failure(
        DiagnosticCategory::InvalidContract,
        "query_builder_scalar_integer_range",
        "long scalar must fit the exact signed 64-bit domain",
    )
}

/// Return the stable diagnostic for host text that cannot be represented as
/// one unchanged Unicode scalar sequence.
#[doc(hidden)]
#[must_use]
pub fn plan_scalar_unicode_error() -> Diagnostic {
    failure(
        DiagnosticCategory::InvalidContract,
        "query_builder_scalar_unicode",
        "string scalar must be valid Unicode without surrogate code points",
    )
}

/// Return the binding-neutral diagnostic for a host string that cannot be
/// represented as a Rust/JSON Unicode scalar sequence.
#[doc(hidden)]
pub fn host_string_unicode_error() -> Diagnostic {
    failure(
        DiagnosticCategory::InvalidContract,
        "query_v2_host_string_unicode",
        "host string input must be valid Unicode without surrogate code points",
    )
}

/// Return the stable diagnostic for a non-string value at a host text boundary.
#[doc(hidden)]
#[must_use]
pub fn host_string_type_error() -> Diagnostic {
    failure(
        DiagnosticCategory::InvalidContract,
        "query_v2_host_string_type",
        "host string input must be a string value",
    )
}
