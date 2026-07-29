#![deny(missing_docs)]
//! Client-owned canonical query literals (Flight 3, F3-02).
//!
//! Query operands are deliberately distinct from generated attribute
//! wrappers: a prefix or range boundary is a valid operand even when it is
//! not itself storable, so no constructor here applies a field's storage
//! annotations. Invalid grammar, nonfinite doubles, over-limit text, and
//! invalid regex fail at literal construction.

use crate::__codegen::{
    CanonicalDouble, Date as CodegenDate, DateTime as CodegenDateTime,
    DateTimeTz as CodegenDateTimeTz, Decimal as CodegenDecimal, Duration as CodegenDuration,
    ValidationError,
};
use type_bridge_contract::value::CanonicalString;

fn literal_error(field: &'static str, code: &'static str) -> ValidationError {
    ValidationError::new(field, code)
}

/// Bounded canonical text for equality, ordering, and substring operators.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Text(String);

impl Text {
    /// Validate one bounded canonical text literal.
    pub fn new(value: impl Into<String>) -> Result<Self, ValidationError> {
        let value = value.into();
        CanonicalString::new(&value).map_err(|_| literal_error("text", "string_limit_exceeded"))?;
        Ok(Self(value))
    }
    /// Return the validated text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub(crate) fn into_string(self) -> String {
        self.0
    }
}

/// A client-owned validated regular expression operand.
#[derive(Clone, Debug)]
pub struct Regex(String);

impl Regex {
    /// Compile-validate one regular expression pattern.
    pub fn new(pattern: impl Into<String>) -> Result<Self, ValidationError> {
        let pattern = pattern.into();
        CanonicalString::new(&pattern)
            .map_err(|_| literal_error("regex", "string_limit_exceeded"))?;
        regex::Regex::new(&pattern).map_err(|_| literal_error("regex", "invalid_regex_pattern"))?;
        Ok(Self(pattern))
    }
    /// Return the validated pattern text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub(crate) fn into_string(self) -> String {
        self.0
    }
}

/// A finite exact-bit double literal; signed zero is preserved.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Double(f64);

impl Double {
    /// Validate one finite double literal.
    pub fn new(value: f64) -> Result<Self, ValidationError> {
        CanonicalDouble::try_new(value)?;
        Ok(Self(value))
    }
    /// Return the finite value.
    #[must_use]
    pub fn get(&self) -> f64 {
        self.0
    }
}

macro_rules! grammar_literal {
    ($(#[$doc:meta])* $name:ident, $inner:ident) => {
        $(#[$doc])*
        #[derive(Clone, Debug, PartialEq, Eq)]
        pub struct $name(String);

        impl $name {
            /// Validate one canonical literal of this domain.
            pub fn new(value: impl AsRef<str>) -> Result<Self, ValidationError> {
                let validated = $inner::try_new(value.as_ref())?;
                Ok(Self(validated.as_str().to_owned()))
            }
            /// Return the canonical literal text.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
            pub(crate) fn into_string(self) -> String {
                self.0
            }
        }
    };
}

grammar_literal!(
    /// A canonical decimal literal.
    Decimal,
    CodegenDecimal
);
grammar_literal!(
    /// A canonical date literal.
    Date,
    CodegenDate
);
grammar_literal!(
    /// A canonical datetime literal.
    DateTime,
    CodegenDateTime
);
grammar_literal!(
    /// A canonical datetime-tz literal.
    DateTimeTz,
    CodegenDateTimeTz
);
grammar_literal!(
    /// A canonical duration literal.
    Duration,
    CodegenDuration
);
