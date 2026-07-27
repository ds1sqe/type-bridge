//! Validated typed identifiers that do not depend on schema envelopes.

use std::fmt;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use unicode_ident::{is_xid_continue, is_xid_start};

use crate::diagnostic::{Diagnostic, DiagnosticCategory};

/// Maximum UTF-8 byte length of one canonical label.
pub const MAX_LABEL_BYTES: usize = 255;

/// Maximum hexadecimal digit count accepted for one provider Thing IID.
pub const MAX_THING_IID_HEX_DIGITS: usize = 256;

/// Return whether `value` is one canonical provider Thing IID.
///
/// This preserves the released V1 evidence grammar: a lowercase `0x`
/// prefix followed by between one and 256 ASCII hexadecimal digits.
#[must_use]
pub fn is_canonical_thing_iid(value: &str) -> bool {
    value.strip_prefix("0x").is_some_and(|digits| {
        !digits.is_empty()
            && digits.len() <= MAX_THING_IID_HEX_DIGITS
            && digits.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

// TypeDB 3.12.1 pins the TypeQL 3.12.0 grammar. Keep this vocabulary
// explicit at the identifier boundary so every schema, plan, and local
// function projection rejects spellings the target semantic validator reserves.
const TYPEQL_3_12_1_RESERVED_LABELS: [&str; 42] = [
    "with",
    "given",
    "match",
    "fetch",
    "update",
    "define",
    "undefine",
    "redefine",
    "insert",
    "put",
    "delete",
    "end",
    "entity",
    "relation",
    "attribute",
    "role",
    "asc",
    "desc",
    "struct",
    "fun",
    "return",
    "alias",
    "sub",
    "owns",
    "as",
    "plays",
    "relates",
    "iid",
    "isa",
    "links",
    "has",
    "is",
    "or",
    "not",
    "try",
    "in",
    "true",
    "false",
    "of",
    "from",
    "first",
    "last",
];

// TypeQL parses these exact spellings as built-in expression functions before
// considering the ordinary user-function identifier alternative. Keep this
// separate from the reserved-label vocabulary: except for `iid`, these remain
// legitimate identifiers in contexts that do not emit a user-function call.
const TYPEQL_3_12_BUILTIN_FUNCTION_NAMES: [&str; 9] = [
    "abs", "ceil", "floor", "iid", "label", "len", "max", "min", "round",
];

fn is_typeql_3_12_1_reserved_label(value: &str) -> bool {
    TYPEQL_3_12_1_RESERVED_LABELS.contains(&value)
}

/// Return whether TypeQL 3.12 parses `value` as a built-in function call name.
///
/// Most of these spellings remain valid labels and function identities in the
/// binding-neutral contract. A semantic surface that emits a user-defined
/// function call or definition must reject them to avoid TypeQL resolving the
/// call as a built-in instead.
#[must_use]
pub fn is_typeql_3_12_builtin_function_name(value: &str) -> bool {
    TYPEQL_3_12_BUILTIN_FUNCTION_NAMES.contains(&value)
}

/// A validated TypeQL-facing label.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Label(String);

impl Label {
    /// Validate and construct a label.
    pub fn new(value: impl Into<String>) -> Result<Self, Diagnostic> {
        let value = value.into();
        let mut chars = value.chars();
        let valid = value.len() <= MAX_LABEL_BYTES
            && chars.next().is_some_and(|ch| ch == '_' || is_xid_start(ch))
            && chars.all(|ch| ch == '-' || is_xid_continue(ch))
            && !is_typeql_3_12_1_reserved_label(&value);
        if valid {
            Ok(Self(value))
        } else {
            Err(Diagnostic::stable(
                DiagnosticCategory::InvalidContract,
                "malformed_id",
                "identifier label is empty, oversized, reserved, or contains invalid characters",
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
        for value in ["", "9person", "person name", "person.", "a²"] {
            assert_eq!(
                Label::new(value).unwrap_err().code().as_str(),
                "malformed_id"
            );
        }
        assert!(serde_json::from_str::<Label>(r#""person name""#).is_err());
    }

    #[test]
    fn labels_follow_typeql_unicode_xid_grammar() {
        for value in ["_", "type-with-hyphens", "a·b", "a\u{301}", "℘x"] {
            assert_eq!(Label::new(value).unwrap().as_str(), value);
        }
    }

    #[test]
    fn labels_reject_the_typeql_3_12_1_reserved_vocabulary() {
        for value in TYPEQL_3_12_1_RESERVED_LABELS {
            assert_eq!(
                Label::new(value).unwrap_err().code().as_str(),
                "malformed_id",
                "reserved TypeQL word {value:?} must not cross the identifier boundary",
            );
        }
        assert!(Label::new("matching").is_ok());
        assert!(Label::new("entity-type").is_ok());
    }

    #[test]
    fn builtin_function_names_remain_contextual_identifiers() {
        for value in TYPEQL_3_12_BUILTIN_FUNCTION_NAMES {
            assert!(
                is_typeql_3_12_builtin_function_name(value),
                "missing TypeQL built-in function {value:?}",
            );
        }
        for value in ["absolute", "length", "person_name_length"] {
            assert!(!is_typeql_3_12_builtin_function_name(value));
        }

        assert!(FunctionId::new("abs").is_ok());
        assert!(FunctionId::new("label").is_ok());
        assert!(FunctionId::new("iid").is_err());
    }

    #[test]
    fn thing_iids_preserve_the_released_bounded_hexadecimal_grammar() {
        assert!(is_canonical_thing_iid("0x0"));
        assert!(is_canonical_thing_iid("0xAbCdEf"));
        assert!(is_canonical_thing_iid(&format!(
            "0x{}",
            "a".repeat(MAX_THING_IID_HEX_DIGITS)
        )));

        for malformed in ["", "0x", "0X1", "01", "0x1g", "0x1; delete $x;"] {
            assert!(!is_canonical_thing_iid(malformed), "{malformed:?}");
        }
        assert!(!is_canonical_thing_iid(&format!(
            "0x{}",
            "a".repeat(MAX_THING_IID_HEX_DIGITS + 1)
        )));
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
