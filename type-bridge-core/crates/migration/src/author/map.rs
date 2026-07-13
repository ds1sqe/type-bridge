//! The canonical `SchemaDiff -> ordered authoring operations` mapping.
//!
//! Exactly one such mapping exists (#166): both the live `makemigrations`
//! flow and the offline authoring API call [`map_schema_diff`]. The emitted
//! order mirrors the historical generator so live output is stable:
//!
//! 1. added attributes
//! 2. modified attribute type definitions (`RunTypeql` define/redefine)
//! 3. added entities
//! 4. added relations
//! 5. modified entities (ownerships, then header changes)
//! 6. modified relations (ownerships, roles, role players, cardinality,
//!    then header changes)
//! 7. removed relations — exactly one `RemoveRelation` each (#168)
//! 8. removed entities (ownership detach, then `RemoveEntity`)
//! 9. removed attributes

use type_bridge_orm::schema::diff::{AttributeTypeChanges, SchemaDiff};
use type_bridge_orm::schema::generator::{attribute_definition, card_annotation};
use type_bridge_orm::schema::info::{OwnedAttributeEntry, SchemaInfo};

use crate::error::MigrationError;
use crate::spec::OperationSpec;

/// Map a computed [`SchemaDiff`] into the ordered authoring operation list.
///
/// `base` is the schema the diff was computed from; `target` is the schema
/// it was computed to. Both are needed: `Add*` payloads come from `target`,
/// removal detach lists come from `base`.
///
/// # Errors
///
/// - [`MigrationError::AuthoringInput`] – the diff references a type absent
///   from the schema it should exist in (corrupt or mismatched inputs).
/// - [`MigrationError::UnsupportedChange`] – a detected change has no
///   canonical lowering; it is surfaced instead of being silently dropped.
pub fn map_schema_diff(
    base: &SchemaInfo,
    target: &SchemaInfo,
    diff: &SchemaDiff,
) -> crate::Result<Vec<OperationSpec>> {
    let mut operations = Vec::new();

    for attr_name in &diff.added_attributes {
        let attribute = target
            .attributes
            .get(attr_name)
            .ok_or_else(|| missing("added attribute", attr_name, "target"))?;
        operations.push(OperationSpec::AddAttribute {
            attribute: attribute.clone(),
        });
    }

    for (attr_name, changes) in &diff.modified_attributes {
        let attribute = target
            .attributes
            .get(attr_name)
            .ok_or_else(|| missing("modified attribute", attr_name, "target"))?;
        let keyword = attribute_change_keyword(changes);
        operations.push(OperationSpec::RunTypeql {
            forward: format!("{keyword}\n{}", attribute_definition(attribute)),
            reverse: None,
        });
    }

    for entity_name in &diff.added_entities {
        let entity = target
            .entities
            .get(entity_name)
            .ok_or_else(|| missing("added entity", entity_name, "target"))?;
        operations.push(OperationSpec::AddEntity {
            entity: entity.clone(),
        });
    }

    for relation_name in &diff.added_relations {
        let relation = target
            .relations
            .get(relation_name)
            .ok_or_else(|| missing("added relation", relation_name, "target"))?;
        operations.push(OperationSpec::AddRelation {
            relation: relation.clone(),
        });
    }

    for (entity_name, changes) in &diff.modified_entities {
        let owned = |attr_name: &str| -> crate::Result<OwnedAttributeEntry> {
            target
                .entities
                .get(entity_name)
                .and_then(|entity| owned_entry(&entity.owned_attributes, attr_name))
                .ok_or_else(|| missing("added ownership", attr_name, "target"))
        };
        for attr_name in &changes.added_attributes {
            operations.push(OperationSpec::AddOwnership {
                owner_type: entity_name.clone(),
                attribute: owned(attr_name)?,
            });
        }
        for attr_name in &changes.removed_attributes {
            operations.push(OperationSpec::RemoveOwnership {
                owner_type: entity_name.clone(),
                attr_name: attr_name.clone(),
            });
        }
        for (attr_name, old_annotations, new_annotations) in &changes.modified_attributes {
            operations.push(OperationSpec::ModifyOwnership {
                owner_type: entity_name.clone(),
                attr_name: attr_name.clone(),
                old_annotations: old_annotations.clone(),
                new_annotations: new_annotations.clone(),
            });
        }
        push_header_changes(
            &mut operations,
            entity_name,
            changes.abstract_changed,
            &changes.parent_changed,
        )?;
    }

    for (relation_name, changes) in &diff.modified_relations {
        let target_relation = target.relations.get(relation_name);
        let owned = |attr_name: &str| -> crate::Result<OwnedAttributeEntry> {
            target_relation
                .and_then(|relation| owned_entry(&relation.owned_attributes, attr_name))
                .ok_or_else(|| missing("added ownership", attr_name, "target"))
        };
        for attr_name in &changes.added_attributes {
            operations.push(OperationSpec::AddOwnership {
                owner_type: relation_name.clone(),
                attribute: owned(attr_name)?,
            });
        }
        for attr_name in &changes.removed_attributes {
            operations.push(OperationSpec::RemoveOwnership {
                owner_type: relation_name.clone(),
                attr_name: attr_name.clone(),
            });
        }
        for (attr_name, old_annotations, new_annotations) in &changes.modified_attributes {
            operations.push(OperationSpec::ModifyOwnership {
                owner_type: relation_name.clone(),
                attr_name: attr_name.clone(),
                old_annotations: old_annotations.clone(),
                new_annotations: new_annotations.clone(),
            });
        }
        for role_name in &changes.added_roles {
            let role = target_relation
                .and_then(|relation| {
                    relation
                        .roles
                        .iter()
                        .find(|role| role.role_name == *role_name)
                })
                .ok_or_else(|| missing("added role", role_name, "target"))?;
            operations.push(OperationSpec::AddRole {
                relation_type: relation_name.clone(),
                role: role.clone(),
            });
        }
        for role_name in &changes.removed_roles {
            operations.push(OperationSpec::RemoveRole {
                relation_type: relation_name.clone(),
                role_name: role_name.clone(),
            });
        }
        for player_change in &changes.modified_role_players {
            for player_type in &player_change.added_player_types {
                operations.push(OperationSpec::AddRolePlayer {
                    relation_type: relation_name.clone(),
                    role_name: player_change.role_name.clone(),
                    player_type_name: player_type.clone(),
                });
            }
            for player_type in &player_change.removed_player_types {
                operations.push(OperationSpec::RemoveRolePlayer {
                    relation_type: relation_name.clone(),
                    role_name: player_change.role_name.clone(),
                    player_type_name: player_type.clone(),
                });
            }
        }
        for cardinality_change in &changes.modified_role_cardinality {
            operations.push(role_cardinality_change(
                relation_name,
                &cardinality_change.role_name,
                cardinality_change.old_cardinality,
                cardinality_change.new_cardinality,
            )?);
        }
        push_header_changes(
            &mut operations,
            relation_name,
            changes.abstract_changed,
            &changes.parent_changed,
        )?;
    }

    for relation_name in &diff.removed_relations {
        // A whole-relation removal stays a single RemoveRelation. TypeDB
        // cascades declared roles, player capabilities, and ownerships in
        // one schema transaction, and rejects any intermediate schema where
        // a concrete relation relates zero roles (#168).
        operations.push(OperationSpec::RemoveRelation {
            type_name: relation_name.clone(),
        });
    }

    for entity_name in &diff.removed_entities {
        if let Some(entity) = base.entities.get(entity_name) {
            for attribute in &entity.owned_attributes {
                operations.push(OperationSpec::RemoveOwnership {
                    owner_type: entity_name.clone(),
                    attr_name: attribute.attr_name.clone(),
                });
            }
        }
        operations.push(OperationSpec::RemoveEntity {
            type_name: entity_name.clone(),
        });
    }

    for attr_name in &diff.removed_attributes {
        operations.push(OperationSpec::RemoveAttribute {
            attr_name: attr_name.clone(),
        });
    }

    Ok(operations)
}

fn missing(what: &str, name: &str, schema: &str) -> MigrationError {
    MigrationError::AuthoringInput {
        message: format!("diff lists {what} {name:?} but the {schema} schema does not define it"),
    }
}

fn owned_entry(entries: &[OwnedAttributeEntry], attr_name: &str) -> Option<OwnedAttributeEntry> {
    entries
        .iter()
        .find(|entry| entry.attr_name == attr_name)
        .cloned()
}

/// Choose the TypeQL keyword for a full attribute type redefinition.
///
/// `define` is only valid when every change adds something that was absent
/// (a fresh constraint or a `false -> true` flag); anything that alters or
/// removes an existing facet needs `redefine`.
fn attribute_change_keyword(changes: &AttributeTypeChanges) -> &'static str {
    if changes.value_type_changed.is_some() || changes.parent_changed.is_some() {
        return "redefine";
    }
    let constraint_changes = [
        changes
            .regex_changed
            .as_ref()
            .map(|(old, new)| (old.is_some(), new.is_some())),
        changes
            .allowed_values_changed
            .as_ref()
            .map(|(old, new)| (old.is_some(), new.is_some())),
        changes
            .range_changed
            .as_ref()
            .map(|(old, new)| (old.is_some(), new.is_some())),
    ];
    for (had_old, has_new) in constraint_changes.into_iter().flatten() {
        if had_old || !has_new {
            return "redefine";
        }
    }
    for (old, new) in [changes.abstract_changed, changes.independent_changed]
        .into_iter()
        .flatten()
    {
        if old || !new {
            return "redefine";
        }
    }
    "define"
}

/// Lower entity/relation `@abstract` and `sub` header changes.
///
/// The historical Python generator silently ignored these diff fields; the
/// canonical mapper must not (#166). Each change lowers to one reviewable
/// `RunTypeql` statement.
fn push_header_changes(
    operations: &mut Vec<OperationSpec>,
    type_name: &str,
    abstract_changed: Option<(bool, bool)>,
    parent_changed: &Option<(Option<String>, Option<String>)>,
) -> crate::Result<()> {
    if let Some((old, new)) = abstract_changed {
        match (old, new) {
            (false, true) => {
                operations.push(run_schema_typeql(format!("define\n{type_name} @abstract;")))
            }
            (true, false) => operations.push(run_schema_typeql(format!(
                "undefine\n@abstract from {type_name};"
            ))),
            _ => {}
        }
    }
    if let Some((old, new)) = parent_changed {
        match (old, new) {
            (None, Some(new_parent)) => operations.push(run_schema_typeql(format!(
                "define\n{type_name} sub {new_parent};"
            ))),
            (Some(_), Some(new_parent)) => operations.push(run_schema_typeql(format!(
                "redefine\n{type_name} sub {new_parent};"
            ))),
            (Some(old_parent), None) => operations.push(run_schema_typeql(format!(
                "undefine\nsub {old_parent} from {type_name};"
            ))),
            (None, None) => {}
        }
    }
    Ok(())
}

/// Lower a relates-side cardinality change to one reviewable `RunTypeql`.
fn role_cardinality_change(
    relation_name: &str,
    role_name: &str,
    old_cardinality: Option<(u32, Option<u32>)>,
    new_cardinality: Option<(u32, Option<u32>)>,
) -> crate::Result<OperationSpec> {
    let statement = match (old_cardinality, new_cardinality) {
        (None, Some((min, max))) => format!(
            "define\n{relation_name} relates {role_name} {};",
            card_annotation(min, max)
        ),
        (Some(_), Some((min, max))) => format!(
            "redefine\n{relation_name} relates {role_name} {};",
            card_annotation(min, max)
        ),
        (Some(_), None) => format!("undefine\n@card from {relation_name} relates {role_name};"),
        (None, None) => {
            return Err(MigrationError::AuthoringInput {
                message: format!(
                    "role cardinality change on {relation_name}:{role_name} carries no old or new value"
                ),
            });
        }
    };
    Ok(run_schema_typeql(statement))
}

fn run_schema_typeql(forward: String) -> OperationSpec {
    OperationSpec::RunTypeql {
        forward,
        reverse: None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use type_bridge_orm::schema::info::{
        AttributeSchemaEntry, EntitySchemaEntry, OwnedAttributeEntry, RelationSchemaEntry,
        RoleEntry,
    };
    use type_bridge_orm::{Annotation, ValueType};

    use super::*;

    fn attribute(name: &str) -> AttributeSchemaEntry {
        AttributeSchemaEntry::new(name, ValueType::String)
    }

    fn owned(name: &str, annotations: Vec<Annotation>) -> OwnedAttributeEntry {
        OwnedAttributeEntry {
            attr_name: name.to_string(),
            value_type: ValueType::String,
            annotations,
            is_ordered: false,
        }
    }

    fn entity(name: &str, owned_attributes: Vec<OwnedAttributeEntry>) -> EntitySchemaEntry {
        EntitySchemaEntry {
            type_name: name.to_string(),
            is_abstract: false,
            parent_type: None,
            owned_attributes,
            plays_cardinalities: BTreeMap::new(),
        }
    }

    fn role(name: &str, players: &[&str]) -> RoleEntry {
        RoleEntry {
            role_name: name.to_string(),
            player_type_names: players.iter().map(|p| p.to_string()).collect(),
            cardinality: None,
            overrides: None,
            is_abstract: false,
            ordered: false,
            distinct: false,
        }
    }

    fn relation(
        name: &str,
        roles: Vec<RoleEntry>,
        owned_attributes: Vec<OwnedAttributeEntry>,
    ) -> RelationSchemaEntry {
        RelationSchemaEntry {
            type_name: name.to_string(),
            is_abstract: false,
            parent_type: None,
            owned_attributes,
            roles,
            plays_cardinalities: BTreeMap::new(),
        }
    }

    fn schema(
        entities: Vec<EntitySchemaEntry>,
        relations: Vec<RelationSchemaEntry>,
        attributes: Vec<AttributeSchemaEntry>,
    ) -> SchemaInfo {
        let mut info = SchemaInfo::default();
        for entry in entities {
            info.entities.insert(entry.type_name.clone(), entry);
        }
        for entry in relations {
            info.relations.insert(entry.type_name.clone(), entry);
        }
        for entry in attributes {
            info.attributes.insert(entry.attr_name.clone(), entry);
        }
        info
    }

    fn map(base: &SchemaInfo, target: &SchemaInfo) -> Vec<OperationSpec> {
        let diff = SchemaDiff::compute(base, target);
        map_schema_diff(base, target, &diff).expect("mapping should succeed")
    }

    fn kinds(operations: &[OperationSpec]) -> Vec<&'static str> {
        operations.iter().map(kind_name).collect()
    }

    fn kind_name(op: &OperationSpec) -> &'static str {
        match op {
            OperationSpec::DefineSchema { .. } => "define_schema",
            OperationSpec::AddAttribute { .. } => "add_attribute",
            OperationSpec::RemoveAttribute { .. } => "remove_attribute",
            OperationSpec::AddEntity { .. } => "add_entity",
            OperationSpec::RemoveEntity { .. } => "remove_entity",
            OperationSpec::AddRelation { .. } => "add_relation",
            OperationSpec::RemoveRelation { .. } => "remove_relation",
            OperationSpec::AddOwnership { .. } => "add_ownership",
            OperationSpec::RemoveOwnership { .. } => "remove_ownership",
            OperationSpec::ModifyOwnership { .. } => "modify_ownership",
            OperationSpec::AddRole { .. } => "add_role",
            OperationSpec::RemoveRole { .. } => "remove_role",
            OperationSpec::AddRolePlayer { .. } => "add_role_player",
            OperationSpec::RemoveRolePlayer { .. } => "remove_role_player",
            OperationSpec::RunTypeql { .. } => "run_typeql",
            OperationSpec::RenameAttribute { .. } => "rename_attribute",
            OperationSpec::CopyAttribute { .. } => "copy_attribute",
        }
    }

    #[test]
    fn no_changes_maps_to_no_operations() {
        let base = schema(
            vec![entity("person", vec![owned("name", vec![Annotation::Key])])],
            vec![],
            vec![attribute("name")],
        );
        assert!(map(&base, &base.clone()).is_empty());
    }

    #[test]
    fn initial_schema_maps_to_ordered_adds() {
        let base = SchemaInfo::default();
        let target = schema(
            vec![entity("person", vec![owned("name", vec![Annotation::Key])])],
            vec![relation(
                "employment",
                vec![role("employee", &["person"])],
                vec![],
            )],
            vec![attribute("name")],
        );

        let operations = map(&base, &target);

        assert_eq!(
            kinds(&operations),
            vec!["add_attribute", "add_entity", "add_relation"]
        );
    }

    #[test]
    fn removed_relation_maps_to_single_remove_relation() {
        // Relation with roles, players, and an ownership: the wholesale
        // removal must not decompose (#168).
        let base = schema(
            vec![entity("person", vec![]), entity("badge", vec![])],
            vec![relation(
                "legacy-link",
                vec![role("subject", &["person"]), role("badge", &["badge"])],
                vec![owned("link-id", vec![Annotation::Key])],
            )],
            vec![attribute("link-id")],
        );
        let target = schema(
            vec![entity("person", vec![]), entity("badge", vec![])],
            vec![],
            vec![attribute("link-id")],
        );

        let operations = map(&base, &target);

        assert_eq!(kinds(&operations), vec!["remove_relation"]);
        assert!(matches!(
            &operations[0],
            OperationSpec::RemoveRelation { type_name } if type_name == "legacy-link"
        ));
    }

    #[test]
    fn removed_relation_and_attribute_stay_independent_operations() {
        let base = schema(
            vec![entity("person", vec![])],
            vec![relation(
                "legacy-link",
                vec![role("subject", &["person"])],
                vec![owned("link-id", vec![Annotation::Key])],
            )],
            vec![attribute("link-id")],
        );
        let target = schema(vec![entity("person", vec![])], vec![], vec![]);

        let operations = map(&base, &target);

        assert_eq!(
            kinds(&operations),
            vec!["remove_relation", "remove_attribute"]
        );
    }

    #[test]
    fn surviving_relation_changes_stay_granular() {
        let base = schema(
            vec![entity("person", vec![]), entity("contractor", vec![])],
            vec![relation(
                "employment",
                vec![
                    role("employee", &["person", "contractor"]),
                    role("reviewer", &["person"]),
                ],
                vec![owned("note", vec![])],
            )],
            vec![attribute("note")],
        );
        let target = schema(
            vec![entity("person", vec![]), entity("contractor", vec![])],
            vec![relation(
                "employment",
                vec![role("employee", &["person"])],
                vec![],
            )],
            vec![attribute("note")],
        );

        let operations = map(&base, &target);

        assert_eq!(
            kinds(&operations),
            vec!["remove_ownership", "remove_role", "remove_role_player"]
        );
    }

    #[test]
    fn added_ownership_carries_target_entry_annotations() {
        let base = schema(
            vec![entity("person", vec![])],
            vec![],
            vec![attribute("email")],
        );
        let target = schema(
            vec![entity(
                "person",
                vec![owned("email", vec![Annotation::Unique])],
            )],
            vec![],
            vec![attribute("email")],
        );

        let operations = map(&base, &target);

        assert_eq!(kinds(&operations), vec!["add_ownership"]);
        match &operations[0] {
            OperationSpec::AddOwnership {
                owner_type,
                attribute,
            } => {
                assert_eq!(owner_type, "person");
                assert_eq!(attribute.annotations, vec![Annotation::Unique]);
            }
            other => panic!("expected AddOwnership, got {other:?}"),
        }
    }

    #[test]
    fn removed_entity_detaches_ownerships_first() {
        let base = schema(
            vec![entity("customer", vec![owned("email", vec![])])],
            vec![],
            vec![attribute("email")],
        );
        let target = schema(vec![], vec![], vec![attribute("email")]);

        let operations = map(&base, &target);

        assert_eq!(
            kinds(&operations),
            vec!["remove_ownership", "remove_entity"]
        );
    }

    #[test]
    fn modified_attribute_keyword_follows_change_shape() {
        // Fresh regex (None -> Some) is additive: define.
        let mut base_attr = attribute("code");
        base_attr.regex = None;
        let mut target_attr = attribute("code");
        target_attr.regex = Some("^[A-Z]+$".to_string());

        let base = schema(vec![], vec![], vec![base_attr.clone()]);
        let target = schema(vec![], vec![], vec![target_attr.clone()]);
        let operations = map(&base, &target);
        assert_eq!(kinds(&operations), vec!["run_typeql"]);
        match &operations[0] {
            OperationSpec::RunTypeql { forward, .. } => {
                assert!(
                    forward.starts_with("define\n"),
                    "additive change defines: {forward}"
                );
                assert!(forward.contains("@regex"));
            }
            other => panic!("expected RunTypeql, got {other:?}"),
        }

        // Changing an existing regex requires redefine.
        let mut changed = target_attr.clone();
        changed.regex = Some("^[a-z]+$".to_string());
        let base = schema(vec![], vec![], vec![target_attr]);
        let target = schema(vec![], vec![], vec![changed]);
        let operations = map(&base, &target);
        match &operations[0] {
            OperationSpec::RunTypeql { forward, .. } => {
                assert!(
                    forward.starts_with("redefine\n"),
                    "mutating change redefines: {forward}"
                );
            }
            other => panic!("expected RunTypeql, got {other:?}"),
        }
    }

    #[test]
    fn entity_header_changes_lower_to_run_typeql() {
        let base = schema(
            vec![entity("person", vec![]), entity("base", vec![])],
            vec![],
            vec![],
        );
        let mut changed = entity("person", vec![]);
        changed.is_abstract = true;
        changed.parent_type = Some("base".to_string());
        let target = schema(vec![changed, entity("base", vec![])], vec![], vec![]);

        let operations = map(&base, &target);

        let forwards: Vec<&str> = operations
            .iter()
            .map(|op| match op {
                OperationSpec::RunTypeql { forward, .. } => forward.as_str(),
                other => panic!("expected RunTypeql, got {other:?}"),
            })
            .collect();
        assert_eq!(
            forwards,
            vec!["define\nperson @abstract;", "define\nperson sub base;"]
        );
    }

    #[test]
    fn role_cardinality_change_lowers_to_run_typeql() {
        let base = schema(
            vec![entity("person", vec![])],
            vec![relation(
                "employment",
                vec![role("employee", &["person"])],
                vec![],
            )],
            vec![],
        );
        let mut changed_role = role("employee", &["person"]);
        changed_role.cardinality = Some((1, Some(4)));
        let target = schema(
            vec![entity("person", vec![])],
            vec![relation("employment", vec![changed_role], vec![])],
            vec![],
        );

        let operations = map(&base, &target);

        assert_eq!(
            operations,
            vec![OperationSpec::RunTypeql {
                forward: "define\nemployment relates employee @card(1..4);".to_string(),
                reverse: None,
            }]
        );
    }

    #[test]
    fn missing_target_entry_is_an_input_error() {
        let base = SchemaInfo::default();
        let mut diff = SchemaDiff::default();
        diff.added_entities.push("ghost".to_string());

        let error = map_schema_diff(&base, &SchemaInfo::default(), &diff)
            .expect_err("missing entry must error");

        assert!(matches!(error, MigrationError::AuthoringInput { .. }));
    }
}
