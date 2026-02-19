//! Typed attribute values corresponding to TypeDB value types.

use type_bridge_core_lib::ast::{LiteralValue, Value};

/// A typed TypeDB attribute value.
///
/// Each variant corresponds to a TypeDB value type. The ORM converts
/// these to [`Value::Literal`] for query compilation and parses
/// TypeDB JSON results back into these variants during hydration.
#[derive(Debug, Clone, PartialEq)]
pub enum AttributeValue {
    /// TypeDB `string` value.
    String(String),
    /// TypeDB `long` (64-bit integer) value.
    Long(i64),
    /// TypeDB `double` (64-bit float) value.
    Double(f64),
    /// TypeDB `boolean` value.
    Boolean(bool),
    /// TypeDB `date` value (ISO 8601 date string, e.g. `"2024-01-15"`).
    Date(String),
    /// TypeDB `datetime` value (ISO 8601 datetime, e.g. `"2024-01-15T10:30:00"`).
    DateTime(String),
    /// TypeDB `datetime-tz` value (datetime with timezone offset).
    DateTimeTZ(String),
    /// TypeDB `decimal` value (arbitrary-precision, stored as string).
    Decimal(String),
    /// TypeDB `duration` value (ISO 8601 duration, e.g. `"P1Y2M3D"`).
    Duration(String),
}

impl AttributeValue {
    /// Convert to a core AST [`Value::Literal`].
    pub fn to_ast_value(&self) -> Value {
        let (json_val, type_name) = match self {
            Self::String(s) => (serde_json::Value::String(s.clone()), "string"),
            Self::Long(n) => (serde_json::json!(*n), "long"),
            Self::Double(n) => (serde_json::json!(*n), "double"),
            Self::Boolean(b) => (serde_json::Value::Bool(*b), "boolean"),
            Self::Date(s) => (serde_json::Value::String(s.clone()), "date"),
            Self::DateTime(s) => (serde_json::Value::String(s.clone()), "datetime"),
            Self::DateTimeTZ(s) => (serde_json::Value::String(s.clone()), "datetime-tz"),
            Self::Decimal(s) => (serde_json::Value::String(s.clone()), "decimal"),
            Self::Duration(s) => (serde_json::Value::String(s.clone()), "duration"),
        };
        Value::Literal(LiteralValue {
            value: json_val,
            value_type: type_name.to_string(),
        })
    }

    /// The TypeQL value type name (e.g. `"string"`, `"long"`, `"double"`).
    pub fn value_type_name(&self) -> &'static str {
        match self {
            Self::String(_) => "string",
            Self::Long(_) => "long",
            Self::Double(_) => "double",
            Self::Boolean(_) => "boolean",
            Self::Date(_) => "date",
            Self::DateTime(_) => "datetime",
            Self::DateTimeTZ(_) => "datetime-tz",
            Self::Decimal(_) => "decimal",
            Self::Duration(_) => "duration",
        }
    }

    /// Parse from a [`serde_json::Value`] given a known TypeDB value type.
    ///
    /// Used during hydration to convert TypeDB fetch results back to
    /// typed attribute values. Returns `None` if the JSON value doesn't
    /// match the expected type.
    pub fn from_json(json: &serde_json::Value, value_type: &str) -> Option<Self> {
        match value_type {
            "string" => json.as_str().map(|s| Self::String(s.to_string())),
            "long" => json.as_i64().map(Self::Long),
            "double" => json.as_f64().map(Self::Double),
            "boolean" => json.as_bool().map(Self::Boolean),
            "date" => json.as_str().map(|s| Self::Date(s.to_string())),
            "datetime" => json.as_str().map(|s| Self::DateTime(s.to_string())),
            "datetime-tz" => json.as_str().map(|s| Self::DateTimeTZ(s.to_string())),
            "decimal" => json.as_str().map(|s| Self::Decimal(s.to_string())),
            "duration" => json.as_str().map(|s| Self::Duration(s.to_string())),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_roundtrip() {
        let val = AttributeValue::String("hello".into());
        assert_eq!(val.value_type_name(), "string");
        let ast = val.to_ast_value();
        if let Value::Literal(lit) = ast {
            assert_eq!(lit.value_type, "string");
            assert_eq!(lit.value, serde_json::json!("hello"));
        } else {
            panic!("expected Literal");
        }
    }

    #[test]
    fn long_roundtrip() {
        let val = AttributeValue::Long(42);
        assert_eq!(val.value_type_name(), "long");
        let json = serde_json::json!(42);
        let parsed = AttributeValue::from_json(&json, "long");
        assert_eq!(parsed, Some(AttributeValue::Long(42)));
    }

    #[test]
    fn from_json_type_mismatch() {
        let json = serde_json::json!("not a number");
        assert_eq!(AttributeValue::from_json(&json, "long"), None);
    }

    #[test]
    fn boolean_value() {
        let val = AttributeValue::Boolean(true);
        let ast = val.to_ast_value();
        if let Value::Literal(lit) = ast {
            assert_eq!(lit.value, serde_json::Value::Bool(true));
        } else {
            panic!("expected Literal");
        }
    }
}
