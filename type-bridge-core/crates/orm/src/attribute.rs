//! TypeDB attribute type trait and convenience macro.

use crate::value::AttributeValue;

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

    /// The TypeDB value type (e.g. `"string"`, `"long"`, `"double"`).
    const VALUE_TYPE: &'static str;

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
#[macro_export]
macro_rules! define_attribute {
    ($name:ident, $attr_name:expr, "string") => {
        #[derive(Debug, Clone, PartialEq)]
        pub struct $name(pub String);

        impl $crate::TypeBridgeAttribute for $name {
            const ATTR_NAME: &'static str = $attr_name;
            const VALUE_TYPE: &'static str = "string";

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
        #[derive(Debug, Clone, PartialEq)]
        pub struct $name(pub i64);

        impl $crate::TypeBridgeAttribute for $name {
            const ATTR_NAME: &'static str = $attr_name;
            const VALUE_TYPE: &'static str = "long";

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
    ($name:ident, $attr_name:expr, "double") => {
        #[derive(Debug, Clone, PartialEq)]
        pub struct $name(pub f64);

        impl $crate::TypeBridgeAttribute for $name {
            const ATTR_NAME: &'static str = $attr_name;
            const VALUE_TYPE: &'static str = "double";

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
        #[derive(Debug, Clone, PartialEq)]
        pub struct $name(pub bool);

        impl $crate::TypeBridgeAttribute for $name {
            const ATTR_NAME: &'static str = $attr_name;
            const VALUE_TYPE: &'static str = "boolean";

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
    ($name:ident, $attr_name:expr, "date") => {
        #[derive(Debug, Clone, PartialEq)]
        pub struct $name(pub String);

        impl $crate::TypeBridgeAttribute for $name {
            const ATTR_NAME: &'static str = $attr_name;
            const VALUE_TYPE: &'static str = "date";

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
        #[derive(Debug, Clone, PartialEq)]
        pub struct $name(pub String);

        impl $crate::TypeBridgeAttribute for $name {
            const ATTR_NAME: &'static str = $attr_name;
            const VALUE_TYPE: &'static str = "datetime";

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
        #[derive(Debug, Clone, PartialEq)]
        pub struct $name(pub String);

        impl $crate::TypeBridgeAttribute for $name {
            const ATTR_NAME: &'static str = $attr_name;
            const VALUE_TYPE: &'static str = "datetime-tz";

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
        #[derive(Debug, Clone, PartialEq)]
        pub struct $name(pub String);

        impl $crate::TypeBridgeAttribute for $name {
            const ATTR_NAME: &'static str = $attr_name;
            const VALUE_TYPE: &'static str = "decimal";

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
        #[derive(Debug, Clone, PartialEq)]
        pub struct $name(pub String);

        impl $crate::TypeBridgeAttribute for $name {
            const ATTR_NAME: &'static str = $attr_name;
            const VALUE_TYPE: &'static str = "duration";

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
