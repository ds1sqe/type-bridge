//! TypeDB relation trait with default AST generation methods.

use type_bridge_core_lib::ast::{Clause, Constraint, FetchItem, Pattern, RolePlayer, Statement};

use serde::Serialize;

use crate::entity::OwnedAttributeInfo;
use crate::error::Result;
use crate::filter::Filter;
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

impl RolePlayerRef {
    /// Build a match pattern for this role player.
    ///
    /// Produces `$var isa entity_type, iid 0x...` or
    /// `$var isa entity_type, has key_attr value`.
    fn to_match_pattern(&self, var: &str) -> Pattern {
        let mut constraints = Vec::new();

        if let Some(ref iid) = self.iid {
            constraints.push(Constraint::Iid(iid.clone()));
        } else if let Some((attr_name, ref value)) = self.key {
            constraints.push(Constraint::Has {
                attr_name: attr_name.to_string(),
                value: value.to_ast_value(),
            });
        }

        Pattern::Entity {
            variable: var.to_string(),
            type_name: self.entity_type_name.to_string(),
            constraints,
            is_strict: false,
        }
    }
}

/// Trait for TypeDB relation types.
///
/// Implement this for each relation struct to enable CRUD operations via
/// [`RelationManager`](crate::manager::RelationManager).
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
/// # Default methods
///
/// All query-building methods have default implementations that construct
/// AST nodes from the required methods above.
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

    // ------------------------------------------------------------------
    // Provided methods — default implementations using the above
    // ------------------------------------------------------------------

    /// Build AST clauses for inserting this relation.
    ///
    /// Produces match clauses for each role player, then an insert clause
    /// with the relation type, links, and attributes:
    /// ```text
    /// match $rp0 isa person, has name "Alice"; $rp1 isa company, has name "Acme";
    /// insert $r isa employment, links (employee: $rp0, employer: $rp1), has position "Engineer";
    /// ```
    fn to_insert_clauses(&self, var: &str) -> Vec<Clause> {
        let role_refs = self.to_role_player_refs();

        // Build match patterns for role players
        let match_patterns: Vec<Pattern> = role_refs
            .iter()
            .enumerate()
            .map(|(i, rp)| rp.to_match_pattern(&format!("$rp{i}")))
            .collect();

        // Build role player bindings
        let role_players: Vec<RolePlayer> = role_refs
            .iter()
            .enumerate()
            .map(|(i, rp)| RolePlayer {
                role: rp.role.to_string(),
                player_var: format!("$rp{i}"),
            })
            .collect();

        // Build inline attribute statements
        let attributes: Vec<Statement> = self
            .to_attribute_values()
            .into_iter()
            .map(|(attr_name, value)| Statement::Has {
                subject_var: var.to_string(),
                attr_name: attr_name.to_string(),
                value: value.to_ast_value(),
            })
            .collect();

        let mut clauses = Vec::new();
        if !match_patterns.is_empty() {
            clauses.push(Clause::Match(match_patterns));
        }

        clauses.push(Clause::Insert(vec![Statement::Relation {
            variable: var.to_string(),
            type_name: Self::TYPE_NAME.to_string(),
            role_players,
            include_variable: true,
            attributes,
        }]));

        clauses
    }

    /// Build insert + fetch-IID clauses.
    fn to_insert_with_iid_fetch(&self, var: &str) -> Vec<Clause> {
        let mut clauses = self.to_insert_clauses(var);
        clauses.push(Clause::Fetch(vec![FetchItem::Function {
            key: "iid".to_string(),
            func_name: "iid".to_string(),
            var: var.to_string(),
        }]));
        clauses
    }

    /// Build identification constraints for matching.
    fn identification_constraints(&self) -> Vec<Constraint> {
        if let Some(iid) = self.iid() {
            return vec![Constraint::Iid(iid.to_string())];
        }

        // For relations, use role players for identification if no IID.
        // This is less common — relations are typically identified by IID.
        vec![]
    }

    /// Build a match pattern for this relation.
    ///
    /// If the relation has an IID, matches by IID. Otherwise matches
    /// by type with role player constraints.
    fn to_match_pattern(&self, var: &str) -> Vec<Pattern> {
        if let Some(iid) = self.iid() {
            return vec![Pattern::Relation {
                variable: var.to_string(),
                type_name: Self::TYPE_NAME.to_string(),
                role_players: vec![],
                constraints: vec![Constraint::Iid(iid.to_string())],
            }];
        }

        // Match via role players
        let role_refs = self.to_role_player_refs();
        let mut patterns = Vec::new();

        // Add match patterns for each role player
        for (i, rp) in role_refs.iter().enumerate() {
            patterns.push(rp.to_match_pattern(&format!("$rp{i}")));
        }

        // Build relation pattern with role players
        let rp_bindings: Vec<RolePlayer> = role_refs
            .iter()
            .enumerate()
            .map(|(i, rp)| RolePlayer {
                role: rp.role.to_string(),
                player_var: format!("$rp{i}"),
            })
            .collect();

        let attr_constraints: Vec<Constraint> = self
            .to_attribute_values()
            .into_iter()
            .map(|(attr_name, value)| Constraint::Has {
                attr_name: attr_name.to_string(),
                value: value.to_ast_value(),
            })
            .collect();

        patterns.push(Pattern::Relation {
            variable: var.to_string(),
            type_name: Self::TYPE_NAME.to_string(),
            role_players: rp_bindings,
            constraints: attr_constraints,
        });

        patterns
    }

    /// Build a polymorphic fetch query for relations.
    fn build_polymorphic_fetch(var: &str, type_name: &str, filters: &[Filter]) -> Vec<Clause> {
        let constraints: Vec<Constraint> = filters
            .iter()
            .map(|f| Constraint::Has {
                attr_name: f.attr_name.clone(),
                value: f.value.to_ast_value(),
            })
            .collect();

        let match_patterns = vec![
            Pattern::Relation {
                variable: var.to_string(),
                type_name: "$t".to_string(),
                role_players: vec![],
                constraints,
            },
            Pattern::SubType {
                variable: "$t".to_string(),
                parent_type: type_name.to_string(),
            },
        ];

        let fetch_items = vec![
            FetchItem::Function {
                key: "_iid".to_string(),
                func_name: "iid".to_string(),
                var: var.to_string(),
            },
            FetchItem::Function {
                key: "_type".to_string(),
                func_name: "label".to_string(),
                var: "$t".to_string(),
            },
            FetchItem::NestedWildcard {
                key: "attributes".to_string(),
                var: var.to_string(),
            },
        ];

        vec![Clause::Match(match_patterns), Clause::Fetch(fetch_items)]
    }
}
