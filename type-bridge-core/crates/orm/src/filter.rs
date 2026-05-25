//! Query filters for entity lookups.

use serde::{Deserialize, Serialize};

use crate::value::AttributeValue;

/// An equality filter for querying entities by attribute value.
///
/// Phase 1 supports exact-match equality filters only.
/// Later phases will add comparison operators, ranges, etc.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Filter {
    /// The TypeDB attribute type name to filter on.
    pub attr_name: String,
    /// The comparison operator. Equality is represented as `"=="`.
    #[serde(default = "default_operator")]
    pub operator: String,
    /// The value to match.
    pub value: AttributeValue,
}

impl Filter {
    /// Create an equality filter with an [`AttributeValue`].
    pub fn eq(attr_name: impl Into<String>, value: AttributeValue) -> Self {
        Self {
            attr_name: attr_name.into(),
            operator: "==".into(),
            value,
        }
    }

    /// Create a comparison filter with an [`AttributeValue`].
    pub fn compare(
        attr_name: impl Into<String>,
        operator: impl Into<String>,
        value: AttributeValue,
    ) -> Self {
        Self {
            attr_name: attr_name.into(),
            operator: operator.into(),
            value,
        }
    }

    /// Create a string equality filter.
    pub fn string_eq(attr_name: impl Into<String>, value: impl Into<String>) -> Self {
        Self::eq(attr_name, AttributeValue::String(value.into()))
    }

    /// Create a long (integer) equality filter.
    pub fn long_eq(attr_name: impl Into<String>, value: i64) -> Self {
        Self::eq(attr_name, AttributeValue::Long(value))
    }

    /// Create a boolean equality filter.
    pub fn bool_eq(attr_name: impl Into<String>, value: bool) -> Self {
        Self::eq(attr_name, AttributeValue::Boolean(value))
    }

    /// Create a double equality filter.
    pub fn double_eq(attr_name: impl Into<String>, value: f64) -> Self {
        Self::eq(attr_name, AttributeValue::Double(value))
    }
}

fn default_operator() -> String {
    "==".into()
}
