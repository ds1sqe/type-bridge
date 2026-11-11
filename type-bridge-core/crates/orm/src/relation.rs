//! TypeDB relation metadata and hydration.

use serde::Serialize;

use crate::_entity::OwnedAttributeInfo;
use crate::error::Result;
use crate::value::AttributeValue;

/// Static metadata about a role in a relation type.
#[derive(Debug, Clone, Serialize)]
pub struct RoleInfo {
    /// The role name within the relation (e.g. `"employee"`, `"employer"`).
    pub role_name: &'static str,
    /// The entity type that plays this role (e.g. `"person"`, `"company"`).
    pub player_type_name: &'static str,
    /// Optional `@doc("...")` documentation annotation on the role (TypeDB 3.12+).
    pub doc: Option<&'static str>,
    /// `@meta("key", "value")` annotations on the role (TypeDB 3.12+),
    /// as key/value pairs. TypeDB allows one value per key per subject.
    pub meta: &'static [(&'static str, &'static str)],
}

/// A reference to a role player for use in relation insert/match operations.
///
/// Identifies an entity that plays a specific role in a relation.
/// The entity can be identified by IID (preferred) or by a @key attribute.
#[derive(Debug, Clone, Serialize)]
pub struct RolePlayerRef {
    /// The role name this entity plays.
    pub role: &'static str,
    /// The entity type name of the player.
    pub entity_type_name: &'static str,
    /// IID-based identification (preferred when available).
    pub iid: Option<String>,
    /// Key-attribute-based identification (fallback).
    pub key: Option<(&'static str, AttributeValue)>,
}

/// Trait for TypeDB relation types.
///
/// Implement this for each relation struct to enable CRUD operations via
/// [`RelationManager`](crate::_manager::RelationManager).
///
/// # Required methods
///
/// - [`TYPE_NAME`](Self::TYPE_NAME): The TypeDB relation type name
/// - [`owned_attributes`](Self::owned_attributes): Static attribute metadata
/// - [`role_info`](Self::role_info): Static role metadata
/// - [`iid`](Self::iid) / [`set_iid`](Self::set_iid): Internal identifier access
/// - [`to_attribute_values`](Self::to_attribute_values): Serialize to attribute pairs
/// - [`to_role_player_refs`](Self::to_role_player_refs): Produce role player references
/// - [`from_document`](Self::from_document): Deserialize from JSON
///
pub trait TypeBridgeRelation: Sized + Send + Sync + 'static {
    /// The TypeDB relation type name (e.g. `"employment"`, `"friendship"`).
    const TYPE_NAME: &'static str;

    /// Whether this relation type is abstract.
    const IS_ABSTRACT: bool = false;

    /// The parent type name if this relation extends another relation type (`sub` in TypeQL).
    const PARENT_TYPE: Option<&'static str> = None;

    /// Optional `@doc("...")` documentation annotation on the type (TypeDB 3.12+).
    const DOC: Option<&'static str> = None;

    /// `@meta("key", "value")` annotations on the type (TypeDB 3.12+), as
    /// key/value pairs. TypeDB allows one value per key per subject.
    const META: &'static [(&'static str, &'static str)] = &[];

    /// Static metadata for all owned attributes in declaration order.
    fn owned_attributes() -> &'static [OwnedAttributeInfo];

    /// Static metadata for all roles in this relation.
    fn role_info() -> &'static [RoleInfo];

    /// Get the IID (internal identifier) assigned after insert/fetch.
    fn iid(&self) -> Option<&str>;

    /// Set the IID (called by the manager after insert or fetch).
    fn set_iid(&mut self, iid: String);

    /// Convert this relation's own attributes to `(attr_name, value)` pairs.
    fn to_attribute_values(&self) -> Vec<(&'static str, AttributeValue)>;

    /// Produce role player references for insertion.
    fn to_role_player_refs(&self) -> Vec<RolePlayerRef>;

    /// Hydrate from a flattened JSON attribute map.
    fn from_document(doc: &serde_json::Map<String, serde_json::Value>) -> Result<Self>;
}
