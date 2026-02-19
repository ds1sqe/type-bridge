//! Derive macros for the `type-bridge-orm` crate.
//!
//! Provides `#[derive(TypeBridgeAttribute)]`, `#[derive(TypeBridgeEntity)]`,
//! and `#[derive(TypeBridgeRelation)]` to eliminate manual trait implementations.

use proc_macro::TokenStream;

mod attribute;
mod entity;
mod relation;

/// Derive `TypeBridgeAttribute` for a newtype struct.
///
/// # Attributes
///
/// - `#[attribute(name = "attr-name", value_type = "string")]` (required)
///
/// # Supported value types
///
/// `"string"`, `"long"`, `"double"`, `"boolean"`, `"date"`,
/// `"datetime"`, `"datetime-tz"`, `"decimal"`, `"duration"`
///
/// # Example
///
/// ```ignore
/// #[derive(TypeBridgeAttribute, Debug, Clone, PartialEq)]
/// #[attribute(name = "name", value_type = "string")]
/// struct Name(pub String);
/// ```
#[proc_macro_derive(TypeBridgeAttribute, attributes(attribute))]
pub fn derive_attribute(input: TokenStream) -> TokenStream {
    attribute::derive(input.into())
        .unwrap_or_else(|e| e.to_compile_error())
        .into()
}

/// Derive `TypeBridgeEntity` for a struct with TypeDB attribute fields.
///
/// # Requirements
///
/// - Must have a field `iid: Option<String>`
/// - All other fields must implement `TypeBridgeAttribute` (or be `Option<T>`
///   where `T: TypeBridgeAttribute`)
///
/// # Attributes
///
/// - `#[entity(name = "person")]` on the struct (required)
/// - `#[field(key)]` on a field to mark it as `@key`
/// - `#[field(name = "custom-attr")]` to override the attribute name
///
/// # Example
///
/// ```ignore
/// #[derive(TypeBridgeEntity)]
/// #[entity(name = "person")]
/// struct Person {
///     iid: Option<String>,
///     #[field(key)]
///     name: Name,
///     age: Age,
///     email: Option<Email>,
/// }
/// ```
#[proc_macro_derive(TypeBridgeEntity, attributes(entity, field))]
pub fn derive_entity(input: TokenStream) -> TokenStream {
    entity::derive(input.into())
        .unwrap_or_else(|e| e.to_compile_error())
        .into()
}

/// Derive `TypeBridgeRelation` for a struct with role player fields.
///
/// # Requirements
///
/// - Must have a field `iid: Option<String>`
/// - Role player fields must be `RolePlayerRef` with `#[role(...)]`
/// - Other fields are treated as owned attributes
///
/// # Attributes
///
/// - `#[relation(name = "employment")]` on the struct (required)
/// - `#[role(name = "employee", player_type = "person")]` on `RolePlayerRef` fields
/// - `#[field(key)]` / `#[field(name = "...")]` on attribute fields
///
/// # Example
///
/// ```ignore
/// #[derive(TypeBridgeRelation)]
/// #[relation(name = "employment")]
/// struct Employment {
///     iid: Option<String>,
///     #[role(name = "employee", player_type = "person")]
///     employee: RolePlayerRef,
///     #[role(name = "employer", player_type = "company")]
///     employer: RolePlayerRef,
///     position: Option<Position>,
/// }
/// ```
#[proc_macro_derive(TypeBridgeRelation, attributes(relation, role, field))]
pub fn derive_relation(input: TokenStream) -> TokenStream {
    relation::derive(input.into())
        .unwrap_or_else(|e| e.to_compile_error())
        .into()
}
