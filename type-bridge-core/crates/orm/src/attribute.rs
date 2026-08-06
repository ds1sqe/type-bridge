//! TypeDB attribute type trait and convenience macro.

use serde::{Deserialize, Serialize};

use crate::value::AttributeValue;

/// TypeDB value type enum for type-safe metadata.
///
/// Mirrors the TypeDB value type system: `string`, `long`, `double`, `boolean`,
/// `date`, `datetime`, `datetime-tz`, `decimal`, `duration`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ValueType {
    /// UTF-8 string value.
    String,
    /// 64-bit signed integer.
    Long,
    /// 64-bit floating-point number.
    Double,
    /// Boolean true/false.
    Boolean,
    /// Calendar date (ISO 8601).
    Date,
    /// Date and time without timezone (ISO 8601).
    #[serde(rename = "datetime")]
    DateTime,
    /// Date and time with timezone (ISO 8601).
    #[serde(rename = "datetime-tz")]
    DateTimeTz,
    /// Arbitrary-precision decimal number.
    Decimal,
    /// ISO 8601 duration.
    Duration,
}

impl ValueType {
    /// Convert to the TypeDB value type string.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Long => "long",
            Self::Double => "double",
            Self::Boolean => "boolean",
            Self::Date => "date",
            Self::DateTime => "datetime",
            Self::DateTimeTz => "datetime-tz",
            Self::Decimal => "decimal",
            Self::Duration => "duration",
        }
    }

    /// Parse from a TypeDB value type string.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "string" => Some(Self::String),
            "long" => Some(Self::Long),
            "double" => Some(Self::Double),
            "boolean" => Some(Self::Boolean),
            "date" => Some(Self::Date),
            "datetime" => Some(Self::DateTime),
            "datetime-tz" => Some(Self::DateTimeTz),
            "decimal" => Some(Self::Decimal),
            "duration" => Some(Self::Duration),
            _ => None,
        }
    }
}

impl std::fmt::Display for ValueType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Trait for TypeDB attribute types.
///
/// Each attribute type in TypeDB (e.g., `name`, `age`, `email`) is represented
/// by a Rust type implementing this trait. The `define_attribute!` macro
/// provides a convenient way to define attribute types.
///
/// # Example
///
/// ```
/// use type_bridge_orm::define_attribute;
///
/// define_attribute!(Name, "name", "string");
/// define_attribute!(Age, "age", "long");
/// define_attribute!(Score, "score", "double");
/// define_attribute!(Active, "active", "boolean");
/// ```
pub trait TypeBridgeAttribute: Clone + Send + Sync + 'static {
    /// The TypeDB attribute type name (e.g. `"name"`, `"age"`, `"email"`).
    const ATTR_NAME: &'static str;

    /// The TypeDB value type as a string (e.g. `"string"`, `"long"`, `"double"`).
    const VALUE_TYPE: &'static str;

    /// The TypeDB value type as a [`ValueType`] enum variant.
    const VALUE_TYPE_ENUM: ValueType;

    /// Convert this attribute to a generic [`AttributeValue`].
    fn to_value(&self) -> AttributeValue;

    /// Parse an [`AttributeValue`] into this attribute type.
    /// Returns `None` if the value type doesn't match.
    fn from_value(value: &AttributeValue) -> Option<Self>;
}

/// Define a TypeDB attribute type with a single line.
///
/// Generates a newtype struct implementing [`TypeBridgeAttribute`].
///
/// # Supported value types
///
/// - `"string"` — inner type `String`
/// - `"long"` — inner type `i64`
/// - `"double"` — inner type `f64`
/// - `"boolean"` — inner type `bool`
/// - `"date"` — inner type `String` (ISO 8601 date)
/// - `"datetime"` — inner type `String` (ISO 8601 datetime)
/// - `"datetime-tz"` — inner type `String` (ISO 8601 datetime with timezone)
/// - `"decimal"` — inner type `String` (decimal representation)
/// - `"duration"` — inner type `String` (ISO 8601 duration)
///
/// # Examples
///
/// ```
/// use type_bridge_orm::define_attribute;
///
/// define_attribute!(Name, "name", "string");
/// define_attribute!(Age, "age", "long");
///
/// let name = Name("Alice".to_string());
/// let age = Age(30);
/// ```
#[doc(hidden)]
#[macro_export]
macro_rules! _define_attribute {
    ($name:ident, $attr_name:expr, "string") => {
        #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
        pub struct $name(pub String);

        impl $crate::_attribute::TypeBridgeAttribute for $name {
            const ATTR_NAME: &'static str = $attr_name;
            const VALUE_TYPE: &'static str = "string";
            const VALUE_TYPE_ENUM: $crate::ValueType = $crate::ValueType::String;

            fn to_value(&self) -> $crate::AttributeValue {
                $crate::AttributeValue::String(self.0.clone())
            }

            fn from_value(value: &$crate::AttributeValue) -> Option<Self> {
                match value {
                    $crate::AttributeValue::String(s) => Some($name(s.clone())),
                    _ => None,
                }
            }
        }
    };
    ($name:ident, $attr_name:expr, "long") => {
        #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
        pub struct $name(pub i64);

        impl $crate::_attribute::TypeBridgeAttribute for $name {
            const ATTR_NAME: &'static str = $attr_name;
            const VALUE_TYPE: &'static str = "long";
            const VALUE_TYPE_ENUM: $crate::ValueType = $crate::ValueType::Long;

            fn to_value(&self) -> $crate::AttributeValue {
                $crate::AttributeValue::Long(self.0)
            }

            fn from_value(value: &$crate::AttributeValue) -> Option<Self> {
                match value {
                    $crate::AttributeValue::Long(n) => Some($name(*n)),
                    _ => None,
                }
            }
        }
    };
    ($name:ident, $attr_name:expr, "integer") => {
        $crate::_define_attribute!($name, $attr_name, "long");
    };
    ($name:ident, $attr_name:expr, "int") => {
        $crate::_define_attribute!($name, $attr_name, "long");
    };
    ($name:ident, $attr_name:expr, "double") => {
        #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
        pub struct $name(pub f64);

        impl $crate::_attribute::TypeBridgeAttribute for $name {
            const ATTR_NAME: &'static str = $attr_name;
            const VALUE_TYPE: &'static str = "double";
            const VALUE_TYPE_ENUM: $crate::ValueType = $crate::ValueType::Double;

            fn to_value(&self) -> $crate::AttributeValue {
                $crate::AttributeValue::Double(self.0)
            }

            fn from_value(value: &$crate::AttributeValue) -> Option<Self> {
                match value {
                    $crate::AttributeValue::Double(n) => Some($name(*n)),
                    _ => None,
                }
            }
        }
    };
    ($name:ident, $attr_name:expr, "boolean") => {
        #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
        pub struct $name(pub bool);

        impl $crate::_attribute::TypeBridgeAttribute for $name {
            const ATTR_NAME: &'static str = $attr_name;
            const VALUE_TYPE: &'static str = "boolean";
            const VALUE_TYPE_ENUM: $crate::ValueType = $crate::ValueType::Boolean;

            fn to_value(&self) -> $crate::AttributeValue {
                $crate::AttributeValue::Boolean(self.0)
            }

            fn from_value(value: &$crate::AttributeValue) -> Option<Self> {
                match value {
                    $crate::AttributeValue::Boolean(b) => Some($name(*b)),
                    _ => None,
                }
            }
        }
    };
    ($name:ident, $attr_name:expr, "bool") => {
        $crate::_define_attribute!($name, $attr_name, "boolean");
    };
    ($name:ident, $attr_name:expr, "date") => {
        #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
        pub struct $name(pub String);

        impl $crate::_attribute::TypeBridgeAttribute for $name {
            const ATTR_NAME: &'static str = $attr_name;
            const VALUE_TYPE: &'static str = "date";
            const VALUE_TYPE_ENUM: $crate::ValueType = $crate::ValueType::Date;

            fn to_value(&self) -> $crate::AttributeValue {
                $crate::AttributeValue::Date(self.0.clone())
            }

            fn from_value(value: &$crate::AttributeValue) -> Option<Self> {
                match value {
                    $crate::AttributeValue::Date(s) => Some($name(s.clone())),
                    _ => None,
                }
            }
        }
    };
    ($name:ident, $attr_name:expr, "datetime") => {
        #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
        pub struct $name(pub String);

        impl $crate::_attribute::TypeBridgeAttribute for $name {
            const ATTR_NAME: &'static str = $attr_name;
            const VALUE_TYPE: &'static str = "datetime";
            const VALUE_TYPE_ENUM: $crate::ValueType = $crate::ValueType::DateTime;

            fn to_value(&self) -> $crate::AttributeValue {
                $crate::AttributeValue::DateTime(self.0.clone())
            }

            fn from_value(value: &$crate::AttributeValue) -> Option<Self> {
                match value {
                    $crate::AttributeValue::DateTime(s) => Some($name(s.clone())),
                    _ => None,
                }
            }
        }
    };
    ($name:ident, $attr_name:expr, "datetime-tz") => {
        #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
        pub struct $name(pub String);

        impl $crate::_attribute::TypeBridgeAttribute for $name {
            const ATTR_NAME: &'static str = $attr_name;
            const VALUE_TYPE: &'static str = "datetime-tz";
            const VALUE_TYPE_ENUM: $crate::ValueType = $crate::ValueType::DateTimeTz;

            fn to_value(&self) -> $crate::AttributeValue {
                $crate::AttributeValue::DateTimeTZ(self.0.clone())
            }

            fn from_value(value: &$crate::AttributeValue) -> Option<Self> {
                match value {
                    $crate::AttributeValue::DateTimeTZ(s) => Some($name(s.clone())),
                    _ => None,
                }
            }
        }
    };
    ($name:ident, $attr_name:expr, "decimal") => {
        #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
        pub struct $name(pub String);

        impl $crate::_attribute::TypeBridgeAttribute for $name {
            const ATTR_NAME: &'static str = $attr_name;
            const VALUE_TYPE: &'static str = "decimal";
            const VALUE_TYPE_ENUM: $crate::ValueType = $crate::ValueType::Decimal;

            fn to_value(&self) -> $crate::AttributeValue {
                $crate::AttributeValue::Decimal(self.0.clone())
            }

            fn from_value(value: &$crate::AttributeValue) -> Option<Self> {
                match value {
                    $crate::AttributeValue::Decimal(s) => Some($name(s.clone())),
                    _ => None,
                }
            }
        }
    };
    ($name:ident, $attr_name:expr, "duration") => {
        #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
        pub struct $name(pub String);

        impl $crate::_attribute::TypeBridgeAttribute for $name {
            const ATTR_NAME: &'static str = $attr_name;
            const VALUE_TYPE: &'static str = "duration";
            const VALUE_TYPE_ENUM: $crate::ValueType = $crate::ValueType::Duration;

            fn to_value(&self) -> $crate::AttributeValue {
                $crate::AttributeValue::Duration(self.0.clone())
            }

            fn from_value(value: &$crate::AttributeValue) -> Option<Self> {
                match value {
                    $crate::AttributeValue::Duration(s) => Some($name(s.clone())),
                    _ => None,
                }
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_type_serde_roundtrip() {
        for vt in [
            ValueType::String,
            ValueType::Long,
            ValueType::Double,
            ValueType::Boolean,
            ValueType::Date,
            ValueType::DateTime,
            ValueType::DateTimeTz,
            ValueType::Decimal,
            ValueType::Duration,
        ] {
            let json = serde_json::to_string(&vt).unwrap();
            let parsed: ValueType = serde_json::from_str(&json).unwrap();
            assert_eq!(vt, parsed);
        }
    }

    #[test]
    fn value_type_serde_names() {
        assert_eq!(
            serde_json::to_string(&ValueType::String).unwrap(),
            "\"string\""
        );
        assert_eq!(serde_json::to_string(&ValueType::Long).unwrap(), "\"long\"");
        assert_eq!(
            serde_json::to_string(&ValueType::DateTime).unwrap(),
            "\"datetime\""
        );
        assert_eq!(
            serde_json::to_string(&ValueType::DateTimeTz).unwrap(),
            "\"datetime-tz\""
        );
    }
}
