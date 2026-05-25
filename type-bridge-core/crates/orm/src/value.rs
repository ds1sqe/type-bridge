//! Typed attribute values corresponding to TypeDB value types.

use serde::{Deserialize, Serialize};
use type_bridge_core_lib::ast::{LiteralValue, Value};

/// A typed TypeDB attribute value.
///
/// Each variant corresponds to a TypeDB value type. The ORM converts
/// these to [`Value::Literal`] for query compilation and parses
/// TypeDB JSON results back into these variants during hydration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
        let json = unwrap_value(json);
        match value_type {
            "string" => json.as_str().map(|s| Self::String(s.to_string())),
            "long" | "integer" => json_to_i64(json).map(Self::Long),
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

fn unwrap_value<'a>(value: &'a serde_json::Value) -> &'a serde_json::Value {
    let Some(obj) = value.as_object() else {
        return value;
    };
    if let Some(inner) = obj.get("value") {
        return unwrap_value(inner);
    }
    for key in [
        "string",
        "long",
        "integer",
        "double",
        "boolean",
        "date",
        "datetime",
        "datetime-tz",
        "decimal",
        "duration",
    ] {
        if let Some(inner) = obj.get(key) {
            return unwrap_value(inner);
        }
    }
    value
}

fn json_to_i64(value: &serde_json::Value) -> Option<i64> {
    if let Some(n) = value.as_i64() {
        return Some(n);
    }
    if let Some(n) = value.as_f64()
        && n.fract() == 0.0
        && n >= i64::MIN as f64
        && n <= i64::MAX as f64
    {
        return Some(n as i64);
    }
    value.as_str().and_then(|s| s.parse::<i64>().ok())
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
    fn long_from_document_wrappers() {
        assert_eq!(
            AttributeValue::from_json(&serde_json::json!({"value": 42}), "long"),
            Some(AttributeValue::Long(42))
        );
        assert_eq!(
            AttributeValue::from_json(&serde_json::json!({"value": {"integer": 42}}), "long"),
            Some(AttributeValue::Long(42))
        );
        assert_eq!(
            AttributeValue::from_json(&serde_json::json!("42"), "long"),
            Some(AttributeValue::Long(42))
        );
        assert_eq!(
            AttributeValue::from_json(&serde_json::json!(42.0), "long"),
            Some(AttributeValue::Long(42))
        );
        assert_eq!(
            AttributeValue::from_json(&serde_json::json!(42.5), "long"),
            None
        );
    }

    #[test]
    fn from_json_type_mismatch() {
        let json = serde_json::json!("not a number");
        assert_eq!(AttributeValue::from_json(&json, "long"), None);
    }

    #[test]
    fn serde_string_roundtrip() {
        let val = AttributeValue::String("hello".into());
        let json = serde_json::to_string(&val).unwrap();
        let parsed: AttributeValue = serde_json::from_str(&json).unwrap();
        assert_eq!(val, parsed);
    }

    #[test]
    fn serde_long_roundtrip() {
        let val = AttributeValue::Long(42);
        let json = serde_json::to_string(&val).unwrap();
        let parsed: AttributeValue = serde_json::from_str(&json).unwrap();
        assert_eq!(val, parsed);
    }

    #[test]
    fn serde_double_roundtrip() {
        let val = AttributeValue::Double(2.78);
        let json = serde_json::to_string(&val).unwrap();
        let parsed: AttributeValue = serde_json::from_str(&json).unwrap();
        assert_eq!(val, parsed);
    }

    #[test]
    fn serde_all_variants() {
        let variants = vec![
            AttributeValue::String("test".into()),
            AttributeValue::Long(99),
            AttributeValue::Double(1.5),
            AttributeValue::Boolean(true),
            AttributeValue::Date("2024-01-15".into()),
            AttributeValue::DateTime("2024-01-15T10:30:00".into()),
            AttributeValue::DateTimeTZ("2024-01-15T10:30:00+05:00".into()),
            AttributeValue::Decimal("123.456".into()),
            AttributeValue::Duration("P1Y2M3D".into()),
        ];
        for val in variants {
            let json = serde_json::to_string(&val).unwrap();
            let parsed: AttributeValue = serde_json::from_str(&json).unwrap();
            assert_eq!(val, parsed);
        }
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
