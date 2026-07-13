//! Strongly typed identities used by the typed match-request protocol.
//!
//! Registry-backed identities are serialized as stable strings. Plan-local
//! identities are compact ordinals. Invocation and session tokens deliberately
//! do not implement Serde traits so serialized diagnostics can never stand in
//! for validated, live state.

use std::fmt;

use serde::{Deserialize, Serialize};

macro_rules! string_id {
    ($name:ident, $description:literal) => {
        #[doc = $description]
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Construct an unvalidated identity from its canonical spelling.
            ///
            /// Syntax and registry membership are checked by request validation,
            /// not by the wire-algebra type.
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            /// Return the canonical string spelling.
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Consume the identity and return its canonical spelling.
            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }
    };
}

macro_rules! ordinal_id {
    ($name:ident, $description:literal) => {
        #[doc = $description]
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(u16);

        impl $name {
            /// Construct an unvalidated plan-local ordinal.
            pub const fn new(value: u16) -> Self {
                Self(value)
            }

            /// Return the plan-local ordinal.
            pub const fn get(self) -> u16 {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

string_id!(
    DescriptorId,
    "A deterministic registry descriptor identity, including its descriptor kind."
);
string_id!(
    ResultShapeId,
    "A deterministic fingerprint of a request's validated result shape."
);
string_id!(
    SchemaFingerprint,
    "A deterministic fingerprint of the schema facts relevant to one request."
);

ordinal_id!(
    BindingId,
    "A deterministic plan-local binding ordinal assigned during canonical lowering."
);
ordinal_id!(
    RoleEdgeId,
    "A deterministic plan-local identity for a role-edge predicate."
);

/// A field identity qualified by its declaring descriptor.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct FieldId {
    /// Descriptor that declares the field.
    pub owner: DescriptorId,
    /// Canonical schema member name.
    pub name: String,
}

impl FieldId {
    /// Construct an unvalidated descriptor-qualified field identity.
    pub fn new(owner: DescriptorId, name: impl Into<String>) -> Self {
        Self {
            owner,
            name: name.into(),
        }
    }
}

/// A role identity qualified by its declaring relation descriptor.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RoleId {
    /// Relation descriptor that declares the role.
    pub owner: DescriptorId,
    /// Canonical schema role name.
    pub name: String,
}

impl RoleId {
    /// Construct an unvalidated descriptor-qualified role identity.
    pub fn new(owner: DescriptorId, name: impl Into<String>) -> Self {
        Self {
            owner,
            name: name.into(),
        }
    }
}

/// A field reference resolved to one plan binding.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct BoundFieldId {
    /// Binding whose matched concept owns the field.
    pub binding: BindingId,
    /// Descriptor-qualified field identity.
    pub field: FieldId,
}

impl BoundFieldId {
    /// Construct an unvalidated bound-field identity.
    pub fn new(binding: BindingId, field: FieldId) -> Self {
        Self { binding, field }
    }
}

macro_rules! live_token {
    ($name:ident, $description:literal) => {
        #[doc = $description]
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name([u8; 16]);

        impl $name {
            /// Construct a live token inside the crate's owning lifecycle.
            pub(crate) const fn new(value: [u8; 16]) -> Self {
                Self(value)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(concat!(stringify!($name), "(..)"))
            }
        }
    };
}

live_token!(
    SessionId,
    "An opaque process-local identity for one handle-construction session."
);
live_token!(
    SessionBindingToken,
    "An opaque process-local identity for one persistent binding handle."
);

/// An opaque invocation-local token issued only after request validation succeeds.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RequestToken([u8; 16]);

impl RequestToken {
    /// Issue a live token while holding the validator-owned construction seal.
    pub(super) const fn issue(
        value: [u8; 16],
        _seal: super::validation::RequestTokenIssuanceSeal,
    ) -> Self {
        Self(value)
    }
}

impl fmt::Debug for RequestToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RequestToken(..)")
    }
}

#[cfg(test)]
impl RequestToken {
    /// Return token bytes for crate-internal non-serialization assertions.
    pub(crate) const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_ids_keep_owner_qualification() {
        let person = DescriptorId::new("entity:person");
        let company = DescriptorId::new("entity:company");

        assert_ne!(FieldId::new(person, "name"), FieldId::new(company, "name"));
    }

    #[test]
    fn serializable_ids_have_deterministic_structural_shapes() {
        let field = BoundFieldId::new(
            BindingId::new(2),
            FieldId::new(DescriptorId::new("entity:person"), "name"),
        );

        assert_eq!(
            serde_json::to_string(&field).unwrap(),
            r#"{"binding":2,"field":{"owner":"entity:person","name":"name"}}"#
        );
    }

    #[test]
    fn live_tokens_are_opaque_and_redacted_in_debug_output() {
        let token = super::super::validation::request_token_for_test([7; 16]);

        assert_eq!(format!("{token:?}"), "RequestToken(..)");
        assert_eq!(token.as_bytes(), &[7; 16]);
    }
}
