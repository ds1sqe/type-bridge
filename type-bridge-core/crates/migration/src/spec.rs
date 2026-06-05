//! Serde migration specification types.

use serde::{Deserialize, Serialize};
use type_bridge_orm::schema::info::{
    AttributeSchemaEntry, EntitySchemaEntry, OwnedAttributeEntry, RelationSchemaEntry, RoleEntry,
    SchemaInfo,
};

/// Reference to a migration that must be ordered before another migration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MigrationDependencySpec {
    /// Application or migration package label.
    pub app_label: String,
    /// Migration file stem, such as `0001_initial`.
    pub migration_name: String,
}

/// One migration lowered from the Python authoring API.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MigrationSpec {
    /// Application or migration package label.
    pub app_label: String,
    /// Migration file stem, such as `0001_initial`.
    pub name: String,
    /// Ordered dependency references.
    #[serde(default)]
    pub dependencies: Vec<MigrationDependencySpec>,
    /// Ordered operations in this migration.
    #[serde(default)]
    pub operations: Vec<OperationSpec>,
    /// Optional loader checksum for later drift detection.
    #[serde(default)]
    pub checksum: Option<String>,
    /// Whether the migration is declared reversible by Python.
    pub reversible: bool,
}

/// Ordered migration container.
///
/// Full graph validation is deliberately left to sub-plan 04.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MigrationGraph {
    /// Migrations in discovery order.
    pub migrations: Vec<MigrationSpec>,
}

/// Operation variants covering the frozen Python `ops.*` authoring surface.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OperationSpec {
    /// Define the complete schema for a model-based initial migration.
    DefineSchema {
        /// Complete model schema.
        schema: SchemaInfo,
    },
    /// Add a standalone attribute type.
    AddAttribute {
        /// Attribute schema payload.
        attribute: AttributeSchemaEntry,
    },
    /// Remove a standalone attribute type.
    RemoveAttribute {
        /// Attribute type name.
        attr_name: String,
    },
    /// Add an entity type.
    AddEntity {
        /// Entity schema payload.
        entity: EntitySchemaEntry,
    },
    /// Remove an entity type.
    RemoveEntity {
        /// Entity type name.
        type_name: String,
    },
    /// Add a relation type.
    AddRelation {
        /// Relation schema payload.
        relation: RelationSchemaEntry,
    },
    /// Remove a relation type.
    RemoveRelation {
        /// Relation type name.
        type_name: String,
    },
    /// Add attribute ownership to an entity or relation.
    AddOwnership {
        /// Owning entity or relation type name.
        owner_type: String,
        /// Owned attribute payload.
        attribute: OwnedAttributeEntry,
    },
    /// Remove attribute ownership from an entity or relation.
    RemoveOwnership {
        /// Owning entity or relation type name.
        owner_type: String,
        /// Attribute type name.
        attr_name: String,
    },
    /// Modify ownership annotations.
    ModifyOwnership {
        /// Owning entity or relation type name.
        owner_type: String,
        /// Attribute type name.
        attr_name: String,
        /// Previous annotations as authored by Python.
        old_annotations: String,
        /// New annotations as authored by Python.
        new_annotations: String,
    },
    /// Add a role to a relation.
    AddRole {
        /// Relation type name.
        relation_type: String,
        /// Role payload.
        role: RoleEntry,
    },
    /// Remove a role from a relation.
    RemoveRole {
        /// Relation type name.
        relation_type: String,
        /// Role name.
        role_name: String,
    },
    /// Add a player type to a role.
    AddRolePlayer {
        /// Relation type name.
        relation_type: String,
        /// Role name.
        role_name: String,
        /// Player type name.
        player_type_name: String,
    },
    /// Remove a player type from a role.
    RemoveRolePlayer {
        /// Relation type name.
        relation_type: String,
        /// Role name.
        role_name: String,
        /// Player type name.
        player_type_name: String,
    },
    /// Execute authored TypeQL without interpreting it in this sub-plan.
    RunTypeql {
        /// Forward TypeQL text.
        forward: String,
        /// Optional rollback TypeQL text.
        #[serde(default)]
        reverse: Option<String>,
    },
    /// Rename an attribute type.
    RenameAttribute {
        /// Previous attribute type name.
        old_name: String,
        /// New attribute type name.
        new_name: String,
        /// TypeDB value type.
        value_type: String,
    },
    /// Copy an attribute from source to dest on every instance of the owner type.
    ///
    /// This is a DML (write-typed) backfill operation — it inserts attribute values, not
    /// schema.  The forward TypeQL is an insert-if-absent backfill; the reverse deletes the
    /// destination attribute.
    CopyAttribute {
        /// Forward backfill TypeQL, carried verbatim from the frozen
        /// `CopyAttribute.to_typeql()`. The executor runs this string and derives
        /// counts from its match clause; it is never re-synthesized in Rust
        /// (invariant 2: a single TypeQL source).
        forward: String,
        /// Reverse (rollback) TypeQL from `to_rollback_typeql()`, or `None` when
        /// the migration is irreversible.
        #[serde(default)]
        reverse: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use type_bridge_orm::Annotation;
    use type_bridge_orm::ValueType;

    fn attribute(name: &str) -> AttributeSchemaEntry {
        AttributeSchemaEntry {
            attr_name: name.to_string(),
            value_type: ValueType::String,
        }
    }

    fn schema() -> SchemaInfo {
        let mut schema = SchemaInfo::default();
        schema
            .attributes
            .insert("name".to_string(), attribute("name"));
        schema.entities.insert(
            "person".to_string(),
            EntitySchemaEntry {
                type_name: "person".to_string(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![OwnedAttributeEntry {
                    attr_name: "name".to_string(),
                    value_type: ValueType::String,
                    annotations: vec![Annotation::Key],
                }],
            },
        );
        schema
    }

    #[test]
    fn define_schema_operation_round_trips_json() {
        let operation = OperationSpec::DefineSchema { schema: schema() };

        let json = serde_json::to_string(&operation).unwrap();
        assert!(json.contains("\"kind\":\"define_schema\""));

        let parsed: OperationSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, operation);
    }

    #[test]
    fn schema_bearing_operation_round_trips_json() {
        let operation = OperationSpec::AddOwnership {
            owner_type: "person".to_string(),
            attribute: OwnedAttributeEntry {
                attr_name: "name".to_string(),
                value_type: ValueType::String,
                annotations: vec![Annotation::Key],
            },
        };

        let parsed: OperationSpec =
            serde_json::from_str(&serde_json::to_string(&operation).unwrap()).unwrap();

        assert_eq!(parsed, operation);
    }

    #[test]
    fn run_typeql_operation_round_trips_json() {
        let operation = OperationSpec::RunTypeql {
            forward: "define attribute nickname, value string;".to_string(),
            reverse: Some("undefine attribute nickname;".to_string()),
        };

        let json = serde_json::to_value(&operation).unwrap();
        assert_eq!(json["kind"], "run_typeql");

        let parsed: OperationSpec = serde_json::from_value(json).unwrap();
        assert_eq!(parsed, operation);
    }

    #[test]
    fn copy_attribute_operation_round_trips_json() {
        let operation = OperationSpec::CopyAttribute {
            forward: "match\n  $x isa person, has old-name $v;\n  not { $x has new-name $d; };\ninsert\n  $x has new-name == $v;".to_string(),
            reverse: Some("match $x isa person, has new-name $v;\ndelete $v of $x;".to_string()),
        };

        let json = serde_json::to_value(&operation).unwrap();
        assert_eq!(json["kind"], "copy_attribute");
        assert!(
            json["forward"]
                .as_str()
                .unwrap()
                .contains("has new-name == $v")
        );

        let parsed: OperationSpec = serde_json::from_value(json).unwrap();
        assert_eq!(parsed, operation);
    }

    #[test]
    fn copy_attribute_without_reverse_round_trips_json() {
        let operation = OperationSpec::CopyAttribute {
            forward: "match\n  $x isa company, has legacy-id $v;\n  not { $x has new-id $d; };\ninsert\n  $x has new-id == $v;".to_string(),
            reverse: None,
        };

        let json = serde_json::to_value(&operation).unwrap();
        assert_eq!(json["kind"], "copy_attribute");
        // reverse is None → omitted (serde default).

        let parsed: OperationSpec = serde_json::from_value(json).unwrap();
        assert_eq!(parsed, operation);
    }

    #[test]
    fn graph_preserves_spec_order() {
        let graph = MigrationGraph {
            migrations: vec![
                MigrationSpec {
                    app_label: "app".to_string(),
                    name: "0001_initial".to_string(),
                    dependencies: vec![],
                    operations: vec![OperationSpec::DefineSchema { schema: schema() }],
                    checksum: Some("aaa".to_string()),
                    reversible: true,
                },
                MigrationSpec {
                    app_label: "app".to_string(),
                    name: "0002_custom".to_string(),
                    dependencies: vec![MigrationDependencySpec {
                        app_label: "app".to_string(),
                        migration_name: "0001_initial".to_string(),
                    }],
                    operations: vec![OperationSpec::RunTypeql {
                        forward: "define attribute nickname, value string;".to_string(),
                        reverse: None,
                    }],
                    checksum: Some("bbb".to_string()),
                    reversible: false,
                },
            ],
        };

        let parsed: MigrationGraph =
            serde_json::from_str(&serde_json::to_string(&graph).unwrap()).unwrap();

        assert_eq!(parsed.migrations[0].name, "0001_initial");
        assert_eq!(parsed.migrations[1].name, "0002_custom");
        assert_eq!(parsed, graph);
    }
}
