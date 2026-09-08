//! TOML schema DSL transpiler for type-bridge.
//!
//! Converts a TOML schema document into a canonical TypeQL `define` block.
//! This crate has no PyO3, no `type-bridge-*` dependencies, and no runtime
//! dependency beyond `toml`, `serde`, `indexmap`, and `thiserror`.

#![deny(missing_docs)]

#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub mod readme_doctests {}

mod emit;
mod model;
mod validate;

use std::fmt;

use thiserror::Error;

/// Kind of schema type involved in a dangling-sub or missing-role diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeKind {
    /// An attribute type.
    Attribute,
    /// An entity type.
    Entity,
    /// A relation type.
    Relation,
}

impl fmt::Display for TypeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TypeKind::Attribute => write!(f, "attribute"),
            TypeKind::Entity => write!(f, "entity"),
            TypeKind::Relation => write!(f, "relation"),
        }
    }
}

/// Errors that can occur during TOML-to-TypeQL transpilation.
#[derive(Debug, Error)]
pub enum TranspileError {
    /// The TOML document could not be parsed or the schema structure did not
    /// match the expected shape (e.g. unknown key, missing required field).
    #[error("TOML parse or deserialisation error: {0}")]
    Toml(#[from] toml::de::Error),

    /// An attribute declares a `value` type that is not one of the 12 known
    /// TypeDB value type keywords.
    #[error(
        "attribute `{type_name}` has unknown value type `{value}` \
         (expected one of: datetime-tz, datetime, boolean, decimal, duration, \
         integer, string, double, date, long, bool, int)"
    )]
    UnknownValueType {
        /// The attribute type containing the invalid value declaration.
        type_name: String,
        /// The unsupported value-type spelling.
        value: String,
    },

    /// A struct field declares a `type` that is not one of the 12 known
    /// TypeDB value type keywords.
    #[error(
        "struct `{struct_name}` field `{field}` has unknown value type `{value}` \
         (expected one of: datetime-tz, datetime, boolean, decimal, duration, \
         integer, string, double, date, long, bool, int)"
    )]
    UnknownStructFieldType {
        /// The struct containing the invalid field declaration.
        struct_name: String,
        /// The field whose value type is invalid.
        field: String,
        /// The unsupported value-type spelling.
        value: String,
    },

    /// An attribute sets both `value` and `sub`; exactly one must be present.
    #[error("attribute `{attr}` sets both `value` and `sub`; specify exactly one")]
    AttributeValueSubConflict {
        /// The attribute that declares both `value` and `sub`.
        attr: String,
    },

    /// An attribute sets neither `value` nor `sub`; exactly one must be present.
    #[error("attribute `{attr}` sets neither `value` nor `sub`; specify exactly one")]
    AttributeMissingValueSub {
        /// The attribute missing both `value` and `sub`.
        attr: String,
    },

    /// A type declares `sub = "<parent>"` but the parent is not defined in the
    /// same section of the schema.
    #[error(
        "{kind} `{type_name}` declares `sub = \"{parent}\"` but `{parent}` \
         is not defined in the schema"
    )]
    DanglingSubParent {
        /// The kind of schema type containing the invalid `sub` declaration.
        kind: TypeKind,
        /// The type whose parent cannot be resolved.
        type_name: String,
        /// The unresolved parent type name.
        parent: String,
    },

    /// An entity or relation `plays` entry references a relation that is not
    /// defined in the schema.
    #[error(
        "`{player}` plays `{relation}:{role}` but relation `{relation}` \
         is not defined in the schema"
    )]
    MissingRoleRelation {
        /// The entity or relation that declares the `plays` capability.
        player: String,
        /// The unresolved relation type name.
        relation: String,
        /// The role name qualified by the missing relation.
        role: String,
    },

    /// An entity or relation `plays` entry references a role name that is not
    /// declared on the specified relation.
    #[error(
        "`{player}` plays `{relation}:{role}` but relation `{relation}` \
         has no role named `{role}`"
    )]
    MissingRole {
        /// The entity or relation that declares the `plays` capability.
        player: String,
        /// The resolved relation type name.
        relation: String,
        /// The role not declared by the relation.
        role: String,
    },

    /// A struct declares no fields; structs require at least one field.
    #[error("struct `{struct_name}` declares no fields; structs require at least one")]
    EmptyStruct {
        /// The struct that declares no fields.
        struct_name: String,
    },

    /// A function body has no `return` clause; a function body must reach
    /// `return ...;`.
    #[error(
        "function `{function}` body has no `return` clause; \
         a function body must reach `return ...;`"
    )]
    MalformedFunctionBody {
        /// The function whose body has no return clause.
        function: String,
    },

    /// A role or owns clause has `distinct = true` but `ordered = false`.
    /// `@distinct` is only valid on a list form (`relates name[]` / `owns attr[]`).
    #[error(
        "{kind} `{type_name}` has `distinct = true` on `{item}` without `ordered = true`; \
         @distinct requires the list form"
    )]
    DistinctWithoutOrdered {
        /// The kind of schema type containing the capability.
        kind: TypeKind,
        /// The type that owns or relates the invalid item.
        type_name: String,
        /// The role or owned attribute marked distinct without list ordering.
        item: String,
    },
}

/// Parse `toml_text` as a type-bridge schema document and emit a canonical
/// TypeQL `define` block.
///
/// # Errors
///
/// Returns [`TranspileError::Toml`] when the input is not valid TOML or does
/// not conform to the schema model (unknown keys, missing required fields).
/// Returns a semantic [`TranspileError`] variant when the schema is structurally
/// valid TOML but contains a semantic error (unknown value type, dangling sub
/// parent, missing role, empty struct, malformed function body, etc.).
pub fn toml_to_typeql(toml_text: &str) -> Result<String, TranspileError> {
    let schema: model::TomlSchema = toml::from_str(toml_text)?;
    validate::validate(&schema)?;
    Ok(emit::emit(&schema))
}
