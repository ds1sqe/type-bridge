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
    /// Modify `@doc`/`@meta` annotations on an entity, relation, or attribute
    /// type (TypeDB 3.12+).
    ModifyTypeAnnotations {
        /// Type name (entity, relation, or attribute type).
        type_name: String,
        /// Previous `@doc` value.
        #[serde(default)]
        old_doc: Option<String>,
        /// New `@doc` value.
        #[serde(default)]
        new_doc: Option<String>,
        /// Previous `@meta` annotations, keyed by meta key.
        #[serde(default)]
        old_meta: std::collections::BTreeMap<String, String>,
        /// New `@meta` annotations, keyed by meta key.
        #[serde(default)]
        new_meta: std::collections::BTreeMap<String, String>,
    },
    /// Modify `@doc`/`@meta` annotations on a relation role (TypeDB 3.12+).
    ModifyRoleAnnotations {
        /// Relation type name.
        relation_type: String,
        /// Role name.
        role_name: String,
        /// Previous `@doc` value.
        #[serde(default)]
        old_doc: Option<String>,
        /// New `@doc` value.
        #[serde(default)]
        new_doc: Option<String>,
        /// Previous `@meta` annotations, keyed by meta key.
        #[serde(default)]
        old_meta: std::collections::BTreeMap<String, String>,
        /// New `@meta` annotations, keyed by meta key.
        #[serde(default)]
        new_meta: std::collections::BTreeMap<String, String>,
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
    ///
    /// Two wire forms exist. The lowered form carries only `forward`/`reverse`
    /// TypeQL (what `.py` execution lowers to); the structured form carries
    /// `owner`/`source`/`dest`(/`filter`) and lets the authoring core
    /// synthesize the TypeQL via [`copy_attribute_typeql`] and render a
    /// faithful `ops.CopyAttribute(...)` in the generated `.py`. Sidecars
    /// written by the authoring core always carry the synthesized strings, so
    /// the executor keeps running carried TypeQL.
    CopyAttribute {
        /// Owner type label (structured portable form).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        owner: Option<String>,
        /// Source attribute label (structured portable form).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<String>,
        /// Destination attribute label (structured portable form).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dest: Option<String>,
        /// Optional extra match constraint line (structured portable form).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filter: Option<String>,
        /// Forward backfill TypeQL. Absent only in the structured form before
        /// normalization; the executor derives its count queries from this
        /// string's match clause.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        forward: Option<String>,
        /// Reverse (rollback) TypeQL from `to_rollback_typeql()`, or `None` when
        /// the migration is irreversible.
        #[serde(default)]
        reverse: Option<String>,
    },
}

/// Resolve the executable forward/reverse TypeQL of a `copy_attribute` op.
///
/// Carried TypeQL wins when present (it is the frozen Python
/// `CopyAttribute.to_typeql()` output); otherwise the structured
/// `owner`/`source`/`dest` fields synthesize it. The synthesis is pinned
/// byte-identical to the Python template by a parity test on the Python side.
///
/// # Errors
///
/// [`crate::error::MigrationError::AuthoringInput`] when neither the carried
/// TypeQL nor the complete structured form is present.
pub fn copy_attribute_typeql(op: &OperationSpec) -> crate::Result<(String, Option<String>)> {
    let OperationSpec::CopyAttribute {
        owner,
        source,
        dest,
        filter,
        forward,
        reverse,
    } = op
    else {
        return Err(crate::error::MigrationError::AuthoringInput {
            message: "copy_attribute_typeql called on a non-copy_attribute operation".to_string(),
        });
    };
    if let Some(forward) = forward {
        return Ok((forward.clone(), reverse.clone()));
    }
    let (Some(owner), Some(source), Some(dest)) = (owner, source, dest) else {
        return Err(crate::error::MigrationError::AuthoringInput {
            message: "copy_attribute requires either forward TypeQL or the structured \
                      owner/source/dest fields"
                .to_string(),
        });
    };
    let filter_line = match filter {
        Some(filter) => format!("\n  {filter};"),
        None => String::new(),
    };
    let synthesized_forward = format!(
        "match\n  $x isa {owner}, has {source} $v;\n  not {{ $x has {dest} $d; }};{filter_line}\ninsert\n  $x has {dest} == $v;"
    );
    let synthesized_reverse = format!("match $x isa {owner}, has {dest} $v;\ndelete $v of $x;");
    Ok((
        synthesized_forward,
        Some(reverse.clone().unwrap_or(synthesized_reverse)),
    ))
}

impl OperationSpec {
    /// Return the operation with any structured `copy_attribute` filled in
    /// with its synthesized executable TypeQL. Other variants pass through.
    ///
    /// # Errors
    ///
    /// [`crate::error::MigrationError::AuthoringInput`] when a
    /// `copy_attribute` carries neither TypeQL nor the structured fields.
    pub fn normalized(self) -> crate::Result<OperationSpec> {
        if !matches!(self, OperationSpec::CopyAttribute { .. }) {
            return Ok(self);
        }
        let (forward, reverse) = copy_attribute_typeql(&self)?;
        let OperationSpec::CopyAttribute {
            owner,
            source,
            dest,
            filter,
            ..
        } = self
        else {
            unreachable!("guarded by the matches! check above");
        };
        Ok(OperationSpec::CopyAttribute {
            owner,
            source,
            dest,
            filter,
            forward: Some(forward),
            reverse,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use type_bridge_orm::Annotation;
    use type_bridge_orm::ValueType;

    fn attribute(name: &str) -> AttributeSchemaEntry {
        AttributeSchemaEntry::new(name, ValueType::String)
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
                    is_ordered: false,
                    doc: None,
                    meta: Default::default(),
                }],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: Default::default(),
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
                is_ordered: false,
                doc: None,
                meta: Default::default(),
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

    fn lowered_copy_attribute(forward: &str, reverse: Option<&str>) -> OperationSpec {
        OperationSpec::CopyAttribute {
            owner: None,
            source: None,
            dest: None,
            filter: None,
            forward: Some(forward.to_string()),
            reverse: reverse.map(str::to_string),
        }
    }

    fn structured_copy_attribute(filter: Option<&str>) -> OperationSpec {
        OperationSpec::CopyAttribute {
            owner: Some("person".to_string()),
            source: Some("old-name".to_string()),
            dest: Some("new-name".to_string()),
            filter: filter.map(str::to_string),
            forward: None,
            reverse: None,
        }
    }

    #[test]
    fn copy_attribute_operation_round_trips_json() {
        let operation = lowered_copy_attribute(
            "match\n  $x isa person, has old-name $v;\n  not { $x has new-name $d; };\ninsert\n  $x has new-name == $v;",
            Some("match $x isa person, has new-name $v;\ndelete $v of $x;"),
        );

        let json = serde_json::to_value(&operation).unwrap();
        assert_eq!(json["kind"], "copy_attribute");
        assert!(
            json["forward"]
                .as_str()
                .unwrap()
                .contains("has new-name == $v")
        );
        // Absent structured fields stay off the wire so the lowered form
        // serializes exactly as it did before the structured form existed.
        assert!(json.get("owner").is_none());

        let parsed: OperationSpec = serde_json::from_value(json).unwrap();
        assert_eq!(parsed, operation);
    }

    #[test]
    fn copy_attribute_without_reverse_round_trips_json() {
        let operation = lowered_copy_attribute(
            "match\n  $x isa company, has legacy-id $v;\n  not { $x has new-id $d; };\ninsert\n  $x has new-id == $v;",
            None,
        );

        let json = serde_json::to_value(&operation).unwrap();
        assert_eq!(json["kind"], "copy_attribute");
        // reverse is None → omitted (serde default).

        let parsed: OperationSpec = serde_json::from_value(json).unwrap();
        assert_eq!(parsed, operation);
    }

    #[test]
    fn legacy_copy_attribute_sidecar_json_still_parses() {
        // Sidecars written before the structured form carry only the TypeQL.
        let json = r#"{"kind":"copy_attribute","forward":"match ...;","reverse":null}"#;

        let parsed: OperationSpec = serde_json::from_str(json).unwrap();
        assert_eq!(parsed, lowered_copy_attribute("match ...;", None));
    }

    #[test]
    fn structured_copy_attribute_round_trips_json() {
        let operation = structured_copy_attribute(Some("$x has age $a;"));

        let json = serde_json::to_value(&operation).unwrap();
        assert_eq!(json["kind"], "copy_attribute");
        assert_eq!(json["owner"], "person");
        assert!(json.get("forward").is_none());

        let parsed: OperationSpec = serde_json::from_value(json).unwrap();
        assert_eq!(parsed, operation);
    }

    #[test]
    fn structured_copy_attribute_synthesizes_python_shaped_typeql() {
        let (forward, reverse) = copy_attribute_typeql(&structured_copy_attribute(None)).unwrap();

        assert_eq!(
            forward,
            "match\n  $x isa person, has old-name $v;\n  not { $x has new-name $d; };\ninsert\n  $x has new-name == $v;"
        );
        assert_eq!(
            reverse.as_deref(),
            Some("match $x isa person, has new-name $v;\ndelete $v of $x;")
        );
    }

    #[test]
    fn structured_copy_attribute_synthesizes_filter_line() {
        // The template appends the terminating `;`, mirroring the Python
        // `CopyAttribute.to_typeql()` filter line.
        let (forward, _) =
            copy_attribute_typeql(&structured_copy_attribute(Some("$x has age $a"))).unwrap();

        assert_eq!(
            forward,
            "match\n  $x isa person, has old-name $v;\n  not { $x has new-name $d; };\n  $x has age $a;\ninsert\n  $x has new-name == $v;"
        );
    }

    #[test]
    fn carried_typeql_wins_over_structured_fields() {
        let operation = OperationSpec::CopyAttribute {
            owner: Some("person".to_string()),
            source: Some("old-name".to_string()),
            dest: Some("new-name".to_string()),
            filter: None,
            forward: Some("match carried;".to_string()),
            reverse: Some("match carried-reverse;".to_string()),
        };

        let (forward, reverse) = copy_attribute_typeql(&operation).unwrap();
        assert_eq!(forward, "match carried;");
        assert_eq!(reverse.as_deref(), Some("match carried-reverse;"));
    }

    #[test]
    fn copy_attribute_without_typeql_or_fields_is_rejected() {
        let operation = OperationSpec::CopyAttribute {
            owner: Some("person".to_string()),
            source: None,
            dest: Some("new-name".to_string()),
            filter: None,
            forward: None,
            reverse: None,
        };

        let error = copy_attribute_typeql(&operation).unwrap_err();
        assert!(matches!(
            error,
            crate::error::MigrationError::AuthoringInput { .. }
        ));
    }

    #[test]
    fn normalized_fills_structured_copy_attribute_and_passes_others_through() {
        let normalized = structured_copy_attribute(None).normalized().unwrap();
        let OperationSpec::CopyAttribute {
            owner,
            forward,
            reverse,
            ..
        } = &normalized
        else {
            panic!("normalized must stay a copy_attribute");
        };
        assert_eq!(owner.as_deref(), Some("person"));
        assert!(forward.as_deref().unwrap().contains("has new-name == $v"));
        assert!(reverse.as_deref().unwrap().contains("delete $v of $x"));

        let passthrough = OperationSpec::RunTypeql {
            forward: "match $x isa person;".to_string(),
            reverse: None,
        };
        assert_eq!(passthrough.clone().normalized().unwrap(), passthrough);
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

    #[test]
    fn annotation_operations_round_trip_json() {
        let operations = vec![
            OperationSpec::ModifyTypeAnnotations {
                type_name: "person".to_string(),
                old_doc: None,
                new_doc: Some("A person.".to_string()),
                old_meta: BTreeMap::new(),
                new_meta: BTreeMap::from([("owner".to_string(), "core".to_string())]),
            },
            OperationSpec::ModifyRoleAnnotations {
                relation_type: "employment".to_string(),
                role_name: "employee".to_string(),
                old_doc: Some("old".to_string()),
                new_doc: None,
                old_meta: BTreeMap::new(),
                new_meta: BTreeMap::new(),
            },
        ];
        for operation in operations {
            let json = serde_json::to_string(&operation).expect("serialize");
            let back: OperationSpec = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(back, operation);
        }
        // Kind tags follow the snake_case convention of the frozen surface.
        let json = serde_json::to_string(&OperationSpec::ModifyTypeAnnotations {
            type_name: "person".to_string(),
            old_doc: None,
            new_doc: None,
            old_meta: BTreeMap::new(),
            new_meta: BTreeMap::new(),
        })
        .expect("serialize");
        assert!(json.contains("\"kind\":\"modify_type_annotations\""));
    }
}
