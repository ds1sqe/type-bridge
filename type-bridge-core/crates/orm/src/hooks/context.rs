//! Hook context types passed to lifecycle hooks.

use std::collections::HashMap;
use std::time::SystemTime;

use crate::value::AttributeValue;

/// The CRUD operation being performed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrudOperation {
    /// Create a new stored instance.
    Insert,
    /// Replace the mutable state of an existing instance.
    Update,
    /// Remove an existing instance.
    Delete,
    /// Insert a new instance or replace the matching keyed instance.
    Put,
}

/// Whether the target is an entity or relation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeKind {
    /// The hook target is an entity instance.
    Entity,
    /// The hook target is a relation instance.
    Relation,
}

/// Context passed to lifecycle hooks before and after CRUD operations.
///
/// Pre-hooks receive `&mut HookContext` and may inspect attributes or
/// set metadata.  Post-hooks receive `&HookContext` (read-only).
pub struct HookContext {
    /// The TypeDB type name (e.g. `"person"`, `"employment"`).
    pub type_name: &'static str,
    /// Whether this is an entity or relation.
    pub type_kind: TypeKind,
    /// Which CRUD operation is being performed.
    pub operation: CrudOperation,
    /// Attribute name–value pairs of the instance.
    pub attributes: Vec<(&'static str, AttributeValue)>,
    /// IID if available (set after insert, or present for update/delete).
    pub iid: Option<String>,
    /// Arbitrary user metadata for passing data between hooks.
    pub metadata: HashMap<String, serde_json::Value>,
    /// When the operation started.
    pub timestamp: SystemTime,
}
