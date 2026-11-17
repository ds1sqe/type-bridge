//! TypeDB entity metadata and hydration.

use serde::{Deserialize, Serialize};

use crate::_attribute::ValueType;
use crate::error::Result;
use crate::value::AttributeValue;

/// Ownership annotation on an attribute (mirrors TypeDB `@key`, `@unique`, `@card`, `@distinct`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Annotation {
    /// `@key` — unique identifier attribute.
    Key,
    /// `@unique` — unique but not identifier.
    Unique,
    /// `@card(min, max)` — cardinality constraint. `None` max = unbounded.
    Card(u32, Option<u32>),
    /// `@distinct` — unique values within a list attribute ownership.
    ///
    /// Only valid on ordered ownerships (`owns name[]`). Projected create and
    /// hydration paths enforce canonical member uniqueness before provider I/O
    /// or before returning hydrated output, respectively.
    Distinct,
}

/// Metadata about one owned attribute on an entity type.
#[derive(Debug, Clone, Serialize)]
pub struct OwnedAttributeInfo {
    /// TypeDB attribute type name (e.g. `"name"`, `"age"`).
    pub attr_name: &'static str,
    /// TypeDB value type.
    pub value_type: ValueType,
    /// Ownership annotations (e.g. `@key`, `@unique`, `@card`).
    pub annotations: &'static [Annotation],
    /// Optional `@doc("...")` documentation annotation on the ownership (TypeDB 3.12+).
    pub doc: Option<&'static str>,
    /// `@meta("key", "value")` annotations on the ownership (TypeDB 3.12+),
    /// as key/value pairs. TypeDB allows one value per key per subject.
    pub meta: &'static [(&'static str, &'static str)],
}

impl OwnedAttributeInfo {
    /// Whether this attribute has a `@key` annotation.
    pub fn is_key(&self) -> bool {
        self.annotations
            .iter()
            .any(|a| matches!(a, Annotation::Key))
    }

    /// Whether this attribute has a `@unique` annotation.
    pub fn is_unique(&self) -> bool {
        self.annotations
            .iter()
            .any(|a| matches!(a, Annotation::Unique))
    }

    /// Get the cardinality constraint, if any.
    pub fn cardinality(&self) -> Option<(u32, Option<u32>)> {
        self.annotations.iter().find_map(|a| match a {
            Annotation::Card(min, max) => Some((*min, *max)),
            _ => None,
        })
    }
}

/// Trait for TypeDB entity types.
///
/// Implement this for each entity struct to enable CRUD operations via
/// [`EntityManager`](crate::_manager::EntityManager). Phase 1 uses manual
/// implementations; derive macros will be added in a later phase.
///
/// # Required methods
///
/// - [`TYPE_NAME`](Self::TYPE_NAME): The TypeDB type name
/// - [`owned_attributes`](Self::owned_attributes): Static attribute metadata
/// - [`iid`](Self::iid) / [`set_iid`](Self::set_iid): Internal identifier access
/// - [`to_attribute_values`](Self::to_attribute_values): Serialize to attribute pairs
/// - [`from_document`](Self::from_document): Deserialize from JSON
///
pub trait TypeBridgeEntity: Sized + Send + Sync + 'static {
    /// The TypeDB entity type name (e.g. `"person"`, `"company"`).
    const TYPE_NAME: &'static str;

    /// Whether this entity type is abstract (cannot be directly instantiated in TypeDB).
    const IS_ABSTRACT: bool = false;

    /// The parent type name if this entity extends another entity type (`sub` in TypeQL).
    const PARENT_TYPE: Option<&'static str> = None;

    /// Optional `@doc("...")` documentation annotation on the type (TypeDB 3.12+).
    const DOC: Option<&'static str> = None;

    /// `@meta("key", "value")` annotations on the type (TypeDB 3.12+), as
    /// key/value pairs. TypeDB allows one value per key per subject.
    const META: &'static [(&'static str, &'static str)] = &[];

    /// Static metadata for all owned attributes in declaration order.
    fn owned_attributes() -> &'static [OwnedAttributeInfo];

    /// Get the IID (internal identifier) assigned after insert/fetch.
    fn iid(&self) -> Option<&str>;

    /// Set the IID (called by the manager after insert or fetch).
    fn set_iid(&mut self, iid: String);

    /// Convert this entity to a list of `(attr_name, value)` pairs.
    ///
    /// Used to build INSERT statements. Optional attributes should be
    /// omitted from the returned list when `None`.
    fn to_attribute_values(&self) -> Vec<(&'static str, AttributeValue)>;

    /// Hydrate from a flattened JSON attribute map.
    ///
    /// The map contains `attr_name -> json_value` pairs extracted from
    /// a TypeDB fetch result. Used by the hydration layer to construct
    /// typed Rust structs from query results.
    fn from_document(doc: &serde_json::Map<String, serde_json::Value>) -> Result<Self>;
}
