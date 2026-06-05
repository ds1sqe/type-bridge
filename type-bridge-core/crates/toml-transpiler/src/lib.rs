//! TOML schema DSL transpiler for type-bridge.
//!
//! Converts a TOML schema document into a canonical TypeQL `define` block.
//! This crate has no PyO3, no `type-bridge-*` dependencies, and no runtime
//! dependency beyond `toml`, `serde`, `indexmap`, and `thiserror`.

mod emit;
mod model;
mod validate;

use std::fmt;

use thiserror::Error;

/// Kind of schema type involved in a dangling-sub or missing-role diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeKind {
    Attribute,
    Entity,
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
    UnknownValueType { type_name: String, value: String },

    /// A struct field declares a `type` that is not one of the 12 known
    /// TypeDB value type keywords.
    #[error(
        "struct `{struct_name}` field `{field}` has unknown value type `{value}` \
         (expected one of: datetime-tz, datetime, boolean, decimal, duration, \
         integer, string, double, date, long, bool, int)"
    )]
    UnknownStructFieldType {
        struct_name: String,
        field: String,
        value: String,
    },

    /// An attribute sets both `value` and `sub`; exactly one must be present.
    #[error("attribute `{attr}` sets both `value` and `sub`; specify exactly one")]
    AttributeValueSubConflict { attr: String },

    /// An attribute sets neither `value` nor `sub`; exactly one must be present.
    #[error("attribute `{attr}` sets neither `value` nor `sub`; specify exactly one")]
    AttributeMissingValueSub { attr: String },

    /// A type declares `sub = "<parent>"` but the parent is not defined in the
    /// same section of the schema.
    #[error(
        "{kind} `{type_name}` declares `sub = \"{parent}\"` but `{parent}` \
         is not defined in the schema"
    )]
    DanglingSubParent {
        kind: TypeKind,
        type_name: String,
        parent: String,
    },

    /// An entity or relation `plays` entry references a relation that is not
    /// defined in the schema.
    #[error(
        "`{player}` plays `{relation}:{role}` but relation `{relation}` \
         is not defined in the schema"
    )]
    MissingRoleRelation {
        player: String,
        relation: String,
        role: String,
    },

    /// An entity or relation `plays` entry references a role name that is not
    /// declared on the specified relation.
    #[error(
        "`{player}` plays `{relation}:{role}` but relation `{relation}` \
         has no role named `{role}`"
    )]
    MissingRole {
        player: String,
        relation: String,
        role: String,
    },

    /// A struct declares no fields; structs require at least one field.
    #[error("struct `{struct_name}` declares no fields; structs require at least one")]
    EmptyStruct { struct_name: String },

    /// A function body has no `return` clause; a function body must reach
    /// `return ...;`.
    #[error(
        "function `{function}` body has no `return` clause; \
         a function body must reach `return ...;`"
    )]
    MalformedFunctionBody { function: String },
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
