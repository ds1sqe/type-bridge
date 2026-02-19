//! TypeDB entity trait with default AST generation methods.

use type_bridge_core_lib::ast::{Clause, Constraint, FetchItem, Pattern, Statement};

use serde::{Deserialize, Serialize};

use crate::attribute::ValueType;
use crate::error::Result;
use crate::filter::Filter;
use crate::value::AttributeValue;

/// Ownership annotation on an attribute (mirrors TypeDB `@key`, `@unique`, `@card`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Annotation {
    /// `@key` — unique identifier attribute.
    Key,
    /// `@unique` — unique but not identifier.
    Unique,
    /// `@card(min, max)` — cardinality constraint. `None` max = unbounded.
    Card(u32, Option<u32>),
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
}

impl OwnedAttributeInfo {
    /// Whether this attribute has a `@key` annotation.
    pub fn is_key(&self) -> bool {
        self.annotations.iter().any(|a| matches!(a, Annotation::Key))
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
/// [`EntityManager`](crate::manager::EntityManager). Phase 1 uses manual
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
/// # Default methods
///
/// All query-building methods have default implementations that construct
/// AST nodes from the required methods above.
pub trait TypeBridgeEntity: Sized + Send + Sync + 'static {
    /// The TypeDB entity type name (e.g. `"person"`, `"company"`).
    const TYPE_NAME: &'static str;

    /// Whether this entity type is abstract (cannot be directly instantiated in TypeDB).
    const IS_ABSTRACT: bool = false;

    /// The parent type name if this entity extends another entity type (`sub` in TypeQL).
    const PARENT_TYPE: Option<&'static str> = None;

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

    // ------------------------------------------------------------------
    // Provided methods — default implementations using the above
    // ------------------------------------------------------------------

    /// Build AST insert clauses for this entity.
    ///
    /// Produces: `insert $var isa TYPE, has attr1 val1, has attr2 val2;`
    fn to_insert_clauses(&self, var: &str) -> Vec<Clause> {
        let mut statements = vec![Statement::Isa {
            variable: var.to_string(),
            type_name: Self::TYPE_NAME.to_string(),
        }];

        for (attr_name, value) in self.to_attribute_values() {
            statements.push(Statement::Has {
                subject_var: var.to_string(),
                attr_name: attr_name.to_string(),
                value: value.to_ast_value(),
            });
        }

        vec![Clause::Insert(statements)]
    }

    /// Build insert + fetch-IID clauses.
    ///
    /// Produces:
    /// ```text
    /// insert $var isa person, has name "Alice", has age 30;
    /// fetch { "iid": iid($var) };
    /// ```
    fn to_insert_with_iid_fetch(&self, var: &str) -> Vec<Clause> {
        let mut clauses = self.to_insert_clauses(var);
        clauses.push(Clause::Fetch(vec![FetchItem::Function {
            key: "iid".to_string(),
            func_name: "iid".to_string(),
            var: var.to_string(),
        }]));
        clauses
    }

    /// Build identification constraints for matching (IID preferred, then @key attrs).
    fn identification_constraints(&self) -> Vec<Constraint> {
        if let Some(iid) = self.iid() {
            return vec![Constraint::Iid(iid.to_string())];
        }

        let key_attrs: Vec<&'static str> = Self::owned_attributes()
            .iter()
            .filter(|a| a.is_key())
            .map(|a| a.attr_name)
            .collect();

        self.to_attribute_values()
            .into_iter()
            .filter(|(name, _)| key_attrs.contains(name))
            .map(|(attr_name, value)| Constraint::Has {
                attr_name: attr_name.to_string(),
                value: value.to_ast_value(),
            })
            .collect()
    }

    /// Build a match pattern for this entity (for delete, get_one, etc.).
    fn to_match_pattern(&self, var: &str) -> Pattern {
        Pattern::Entity {
            variable: var.to_string(),
            type_name: Self::TYPE_NAME.to_string(),
            constraints: self.identification_constraints(),
            is_strict: false,
        }
    }

    /// Build a polymorphic fetch query with `isa!` type variable resolution.
    ///
    /// Produces:
    /// ```text
    /// match $var isa! $t, has name "Alice"; $t sub person;
    /// fetch { "_iid": iid($var), "_type": label($t), "attributes": { $var.* } };
    /// ```
    fn build_polymorphic_fetch(
        var: &str,
        type_name: &str,
        filters: &[Filter],
    ) -> Vec<Clause> {
        let constraints: Vec<Constraint> = filters
            .iter()
            .map(|f| Constraint::Has {
                attr_name: f.attr_name.clone(),
                value: f.value.to_ast_value(),
            })
            .collect();

        let match_patterns = vec![
            Pattern::Entity {
                variable: var.to_string(),
                type_name: "$t".to_string(),
                constraints,
                is_strict: true,
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
