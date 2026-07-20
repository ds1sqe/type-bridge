//! Validated typed identifiers that do not depend on schema envelopes.

use std::fmt;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::diagnostic::{Diagnostic, DiagnosticCategory};

/// Maximum UTF-8 byte length of one canonical label.
pub const MAX_LABEL_BYTES: usize = 255;

/// A validated TypeQL-facing label.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Label(String);

impl Label {
    /// Validate and construct a label.
    pub fn new(value: impl Into<String>) -> Result<Self, Diagnostic> {
        let value = value.into();
        let mut chars = value.chars();
        let valid = value.len() <= MAX_LABEL_BYTES
            && chars
                .next()
                .is_some_and(|ch| ch == '_' || ch.is_alphabetic())
            && chars.all(|ch| ch == '_' || ch == '-' || ch.is_alphanumeric());
        if valid {
            Ok(Self(value))
        } else {
            Err(Diagnostic::stable(
                DiagnosticCategory::InvalidContract,
                "malformed_id",
                "identifier label is empty, oversized, or contains invalid characters",
            )
            .with_detail(
                "maximum_bytes",
                i64::try_from(MAX_LABEL_BYTES).unwrap_or(i64::MAX),
            ))
        }
    }
    /// Return the canonical label spelling.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Label {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}
impl Serialize for Label {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}
impl<'de> Deserialize<'de> for Label {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

/// The closed kind component of a schema type identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TypeKind {
    /// Entity type.
    Entity,
    /// Relation type.
    Relation,
    /// Attribute type.
    Attribute,
    /// Struct type.
    Struct,
}

/// A type identity containing both kind and label.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TypeId {
    kind: TypeKind,
    label: Label,
}

impl TypeId {
    /// Construct a typed identity from a validated label spelling.
    pub fn new(kind: TypeKind, label: impl Into<String>) -> Result<Self, Diagnostic> {
        Ok(Self {
            kind,
            label: Label::new(label)?,
        })
    }
    /// Return the type kind.
    pub const fn kind(&self) -> TypeKind {
        self.kind
    }
    /// Return the type label.
    pub fn label(&self) -> &Label {
        &self.label
    }
}

/// A relation-qualified role identity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RoleId {
    declaring_relation: Label,
    label: Label,
}

impl RoleId {
    /// Construct a role identity. Equal role labels under different relations remain unequal.
    pub fn new(
        declaring_relation: impl Into<String>,
        label: impl Into<String>,
    ) -> Result<Self, Diagnostic> {
        Ok(Self {
            declaring_relation: Label::new(declaring_relation)?,
            label: Label::new(label)?,
        })
    }
    /// Return the declaring relation label.
    pub fn declaring_relation(&self) -> &Label {
        &self.declaring_relation
    }
    /// Return the role label.
    pub fn label(&self) -> &Label {
        &self.label
    }
}

macro_rules! label_id {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Label);
        impl $name {
            /// Validate and construct this identity.
            pub fn new(label: impl Into<String>) -> Result<Self, Diagnostic> {
                Ok(Self(Label::new(label)?))
            }
            /// Return the validated label.
            pub fn label(&self) -> &Label {
                &self.0
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

label_id!(AttributeId, "A typed attribute identity.");
label_id!(FunctionId, "A typed function identity.");
label_id!(StructId, "A typed struct identity.");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_reject_malformed_input_during_deserialization() {
        for value in ["", "9person", "person name", "person."] {
            assert_eq!(
                Label::new(value).unwrap_err().code().as_str(),
                "malformed_id"
            );
        }
        assert!(serde_json::from_str::<Label>(r#""person name""#).is_err());
    }

    #[test]
    fn malformed_typed_id_wires_fail_closed() {
        assert!(serde_json::from_str::<TypeId>(r#"{"kind":"entity","label":"9person"}"#).is_err());
        assert!(serde_json::from_str::<TypeId>(r#"{"kind":"future","label":"person"}"#).is_err());
        assert!(
            serde_json::from_str::<RoleId>(
                r#"{"declaring_relation":"9employment","label":"employee"}"#
            )
            .is_err()
        );
    }

    #[test]
    fn role_identity_includes_the_declaring_relation() {
        let employee = RoleId::new("employment", "employee").unwrap();
        let membership = RoleId::new("membership", "employee").unwrap();
        assert_ne!(employee, membership);
    }
}
