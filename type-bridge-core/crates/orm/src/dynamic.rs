//! Dynamic rows and inputs used by runtime descriptor managers.

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::error::{OrmError, Result};
use crate::expr::SortDir;
use crate::value::AttributeValue;

/// Runtime attribute values keyed by TypeDB attribute name or descriptor field name.
pub type DynamicAttributeMap = Vec<(String, AttributeValue)>;

/// Dynamic entity row hydrated from a fetch document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DynamicEntityRow {
    /// TypeDB internal identifier, when present in fetch output.
    pub iid: Option<String>,
    /// Concrete TypeDB type label from polymorphic fetch, when present.
    pub type_name: Option<String>,
    /// Declared attributes that were present in the document.
    pub attributes: DynamicAttributeMap,
}

/// Identity-only evidence for one concrete entity discovered under a root type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DynamicEntityIdentity {
    /// Canonical TypeDB internal identifier.
    pub iid: String,
    /// Concrete TypeDB entity type label.
    pub type_name: String,
}

/// Owned identity-only evidence for a concrete relation discovered under a root relation.
///
/// The IID is canonical TypeDB identity text and `type_name` is the concrete
/// relation label returned by the projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DynamicRelationIdentity {
    /// Canonical TypeDB internal identifier.
    pub iid: String,
    /// Concrete TypeDB relation type label.
    pub type_name: String,
}

pub(crate) fn is_blank_key_value(value: &AttributeValue) -> bool {
    match value {
        AttributeValue::String(s)
        | AttributeValue::Date(s)
        | AttributeValue::DateTime(s)
        | AttributeValue::DateTimeTZ(s)
        | AttributeValue::Decimal(s)
        | AttributeValue::Duration(s) => s.trim().is_empty(),
        _ => false,
    }
}

/// Dynamic role player hydrated from relation fetch output or provided for relation insert.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DynamicRolePlayer {
    /// Relation role name.
    pub role_name: String,
    /// Player entity IID, when known.
    pub player_iid: Option<String>,
    /// Player entity type name, when known.
    pub player_type_name: Option<String>,
    /// Player owned attributes, keyed by TypeDB attribute type name.
    pub attributes: Vec<(String, JsonValue)>,
}

/// Runtime relation row hydrated from a fetch document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DynamicRelationRow {
    /// TypeDB internal identifier, when present in fetch output.
    pub iid: Option<String>,
    /// Concrete TypeDB type label from polymorphic fetch, when present.
    pub type_name: Option<String>,
    /// Declared attributes that were present in the document.
    pub attributes: DynamicAttributeMap,
    /// Role players when the document shape exposes them.
    pub role_players: Vec<DynamicRolePlayer>,
}

/// Runtime role player reference used for dynamic relation inserts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DynamicRolePlayerInput {
    /// Relation role name.
    pub role_name: String,
    /// Entity type name for the player.
    pub player_type_name: String,
    /// IID-based identification, preferred when available.
    pub iid: Option<String>,
    /// Key-attribute fallback identification.
    pub key: Option<(String, AttributeValue)>,
}

/// Runtime aggregate requested by a language binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DynamicAggregate {
    /// Stable result key exposed to bindings.
    pub result_key: String,
    /// TypeDB reduce function name, such as `count`, `sum`, `mean`, `min`, or `max`.
    pub function: String,
    /// Attribute type name or field name for attribute-backed aggregates.
    pub attr_name: Option<String>,
}

/// Runtime comparison operator requested by a language binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DynamicComparisonOp {
    /// Equality.
    Eq,
    /// Not equal.
    Neq,
    /// Greater than.
    Gt,
    /// Greater than or equal.
    Gte,
    /// Less than.
    Lt,
    /// Less than or equal.
    Lte,
    /// String contains.
    Contains,
    /// String regex match.
    Like,
    /// Anchored prefix match: the caller passes the raw literal; Rust adds
    /// the `^` anchor, escapes regex metacharacters, and emits a `like` query.
    StartsWith,
    /// Anchored suffix match: the caller passes the raw literal; Rust adds
    /// the `$` anchor, escapes regex metacharacters, and emits a `like` query.
    EndsWith,
}

/// Runtime expression tree requested by a language binding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DynamicExpr {
    /// Attribute comparison or string operator.
    Compare {
        /// Attribute type name.
        attr_name: String,
        /// Typed comparison operator.
        operator: DynamicComparisonOp,
        /// Typed value to compare against.
        value: AttributeValue,
    },
    /// IID lookup for the current thing variable.
    Iid {
        /// TypeDB internal identifier.
        iid: String,
    },
    /// Presence/absence lookup for an owned attribute.
    IsNull {
        /// Attribute type name.
        attr_name: String,
        /// `true` means the owner must not have this attribute.
        is_null: bool,
    },
    /// All child expressions must match.
    And {
        /// Child expressions.
        exprs: Vec<DynamicExpr>,
    },
    /// At least one child expression must match.
    Or {
        /// Child expressions.
        exprs: Vec<DynamicExpr>,
    },
    /// The child expression must not match.
    Not {
        /// Child expression.
        expr: Box<DynamicExpr>,
    },
    /// Apply an expression to a relation role-player variable.
    RolePlayer {
        /// Relation role name.
        role_name: String,
        /// Expression applied to that role player.
        expr: Box<DynamicExpr>,
    },
}

/// Runtime sort requested by a language binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DynamicSort {
    /// Sort by an attribute owned by the queried thing.
    Attribute {
        /// Attribute type name.
        attr_name: String,
        /// Sort direction.
        direction: SortDir,
    },
    /// Sort a relation query by one role player's attribute.
    RolePlayerAttribute {
        /// Relation role name.
        role_name: String,
        /// Player attribute type name.
        attr_name: String,
        /// Sort direction.
        direction: SortDir,
    },
}

pub(crate) fn validate_relation_iid(type_name: &str, iid: &str) -> Result<()> {
    if type_bridge_contract::id::is_canonical_thing_iid(iid) {
        Ok(())
    } else {
        Err(OrmError::QueryExecution(format!(
            "Exact relation operation for {type_name} requires a canonical TypeDB IID"
        )))
    }
}

pub(crate) fn validate_exact_iid(type_name: &str, iid: &str) -> Result<()> {
    if type_bridge_contract::id::is_canonical_thing_iid(iid) {
        Ok(())
    } else {
        Err(OrmError::QueryExecution(format!(
            "Exact entity operation for {type_name} requires a canonical TypeDB IID"
        )))
    }
}
