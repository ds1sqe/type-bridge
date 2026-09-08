//! The canonical `SchemaDiff -> ordered authoring operations` mapping.
//!
//! Exactly one such mapping exists (#166): both the live `makemigrations`
//! flow and the offline authoring API call [`map_schema_diff`]. The emitted
//! order mirrors the historical generator so live output is stable:
//!
//! 1. added attributes, then attribute-rename additions (define new
//!    attribute, staged plain ownerships, data backfill, annotation
//!    tightening)
//! 2. modified attribute type definitions (`RunTypeql` define/redefine)
//! 3. added entities (parents before children)
//! 4. added relations (parents before children)
//! 5. prerequisite player removals for new `sub` edges, then modified
//!    entities (ownerships, then header changes)
//! 6. modified relations (ownerships, roles, role players, cardinality,
//!    then header changes)
//! 7. removed relations — exactly one `RemoveRelation` each (#168)
//! 8. removed entities (ownership detach, then `RemoveEntity`)
//! 9. attribute-rename removals (loosen old annotations, delete old
//!    values, detach, remove), then removed attributes
//!
//! Attribute renames are mapping *directives*, not operations: without
//! them, `base(old)`/`target(new)` maps to an independent remove+add that
//! destroys data. A rename replaces those diff-mapped operations with a
//! staged expansion built from existing primitives.

use std::collections::{BTreeMap, BTreeSet};

use type_bridge_orm::_schema::diff::{AttributeTypeChanges, RelationChanges, SchemaDiff};
use type_bridge_orm::_schema::generator::{
    attribute_constraint_definition, card_annotation, topological_sort,
};
use type_bridge_orm::_schema::info::{
    EntitySchemaEntry, OwnedAttributeEntry, RelationSchemaEntry, SchemaInfo,
};

use crate::error::MigrationError;
use crate::spec::OperationSpec;

/// Map a computed [`SchemaDiff`] into the ordered authoring operation list.
///
/// `base` is the schema the diff was computed from; `target` is the schema
/// it was computed to. Both are needed: `Add*` payloads come from `target`,
/// removal detach lists come from `base`.
///
/// `renames` are `(old_name, new_name)` attribute-rename directives; each
/// replaces the diff's independent remove+add of that pair with a
/// data-preserving staged expansion.
///
/// # Errors
///
/// - [`MigrationError::AuthoringInput`] – the diff references a type absent
///   from the schema it should exist in (corrupt or mismatched inputs), or
///   a rename directive is inconsistent with the two schemas.
/// - [`MigrationError::UnsupportedChange`] – a detected change has no
///   canonical lowering; it is surfaced instead of being silently dropped.
pub fn map_schema_diff(
    base: &SchemaInfo,
    target: &SchemaInfo,
    diff: &SchemaDiff,
    renames: &[(String, String)],
) -> crate::Result<Vec<OperationSpec>> {
    let mut rename = expand_attribute_renames(base, target, renames)?;
    let rename_additions = std::mem::take(&mut rename.additions);
    let rename_removals = std::mem::take(&mut rename.removals);
    let mut operations = Vec::new();
    let declared_player_changes = declared_role_player_changes(base, target);
    let players_attaching_to_parent: BTreeSet<&str> = diff
        .modified_entities
        .iter()
        .filter_map(|(name, changes)| {
            changes
                .parent_changed
                .as_ref()
                .and_then(|(_, new)| new.as_ref().map(|_| name.as_str()))
        })
        .chain(
            diff.modified_relations
                .iter()
                .filter_map(|(name, changes)| {
                    changes
                        .parent_changed
                        .as_ref()
                        .and_then(|(_, new)| new.as_ref().map(|_| name.as_str()))
                }),
        )
        .collect();
    let relation_players_changing_parent: BTreeSet<&str> = diff
        .modified_relations
        .iter()
        .filter_map(|(name, changes)| changes.parent_changed.as_ref().map(|_| name.as_str()))
        .collect();

    for attr_name in &diff.added_attributes {
        if rename.suppress_added_attributes.contains(attr_name) {
            continue;
        }
        let attribute = target
            .attributes
            .get(attr_name)
            .ok_or_else(|| missing("added attribute", attr_name, "target"))?;
        operations.push(OperationSpec::AddAttribute {
            attribute: attribute.clone(),
        });
    }
    operations.extend(rename_additions);

    for (attr_name, changes) in &diff.modified_attributes {
        let attribute = target
            .attributes
            .get(attr_name)
            .ok_or_else(|| missing("modified attribute", attr_name, "target"))?;
        if !changes.is_annotation_only() {
            let keyword = attribute_change_keyword(changes);
            // @doc/@meta head annotations are rejected inside redefine and
            // collide (DEX16) inside define; annotation changes lower to a
            // dedicated ModifyTypeAnnotations below.
            operations.push(OperationSpec::RunTypeql {
                forward: format!("{keyword}\n{}", attribute_constraint_definition(attribute)),
                reverse: None,
            });
        }
        if let Some(op) =
            type_annotation_change(attr_name, &changes.doc_changed, &changes.meta_changed)
        {
            operations.push(op);
        }
    }

    // Added types are emitted parents-before-children and reduced to their
    // own declarations: each `Add*` lowers to a singleton define with no
    // parent in scope, so a child emitted first would reference an undefined
    // supertype, and a flattened entry would redeclare inherited
    // capabilities, which TypeDB rejects (#190).
    let mut added_entities = BTreeMap::new();
    for entity_name in &diff.added_entities {
        let entity = target
            .entities
            .get(entity_name)
            .ok_or_else(|| missing("added entity", entity_name, "target"))?;
        added_entities.insert(entity_name.clone(), entity);
    }
    for entity_name in topological_sort(&added_entities, |e| e.parent_type.as_deref()) {
        operations.push(OperationSpec::AddEntity {
            entity: declared_entity_entry(added_entities[&entity_name], target),
        });
    }

    let mut added_relations = BTreeMap::new();
    for relation_name in &diff.added_relations {
        let relation = target
            .relations
            .get(relation_name)
            .ok_or_else(|| missing("added relation", relation_name, "target"))?;
        added_relations.insert(relation_name.clone(), relation);
    }
    for relation_name in topological_sort(&added_relations, |r| r.parent_type.as_deref()) {
        operations.push(OperationSpec::AddRelation {
            relation: declared_relation_entry(added_relations[&relation_name], target),
        });
    }

    // A type cannot gain a parent while still declaring a `plays` capability
    // it will inherit from that parent. Emit those removals before any entity
    // or relation `sub` header changes; the remaining player changes stay in
    // the normal modified-relation phase below.
    for (relation_name, changes) in &declared_player_changes {
        for change in changes {
            for player_type in &change.removed_player_types {
                if players_attaching_to_parent.contains(player_type.as_str()) {
                    operations.push(OperationSpec::RemoveRolePlayer {
                        relation_type: relation_name.clone(),
                        role_name: change.role_name.clone(),
                        player_type_name: player_type.clone(),
                    });
                }
            }
        }
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
            if rename.suppresses_added_ownership(entity_name, attr_name) {
                continue;
            }
            operations.push(OperationSpec::AddOwnership {
                owner_type: entity_name.clone(),
                attribute: owned(attr_name)?,
            });
        }
        for attr_name in &changes.removed_attributes {
            if rename.suppresses_removed_ownership(entity_name, attr_name) {
                continue;
            }
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
        if let Some(op) =
            type_annotation_change(entity_name, &changes.doc_changed, &changes.meta_changed)
        {
            operations.push(op);
        }
        push_header_changes(
            &mut operations,
            entity_name,
            changes.abstract_changed,
            &changes.parent_changed,
        )?;
    }

    // Include declared-player changes even when the flattened diff omits the
    // relation. A player hierarchy change can preserve a role's effective
    // player vector while changing which players declare `plays`.
    // Diff-only names remain included so mismatched authoring inputs retain
    // the same validation behaviour as the regular modified-relation path.
    let mut mapped_relation_names: BTreeSet<&String> = diff.modified_relations.keys().collect();
    mapped_relation_names.extend(declared_player_changes.keys());
    for relation_name in mapped_relation_names {
        let no_changes = RelationChanges::default();
        let changes = diff
            .modified_relations
            .get(relation_name)
            .unwrap_or(&no_changes);
        let target_relation = target.relations.get(relation_name);
        let owned = |attr_name: &str| -> crate::Result<OwnedAttributeEntry> {
            target_relation
                .and_then(|relation| owned_entry(&relation.owned_attributes, attr_name))
                .ok_or_else(|| missing("added ownership", attr_name, "target"))
        };
        for attr_name in &changes.added_attributes {
            if rename.suppresses_added_ownership(relation_name, attr_name) {
                continue;
            }
            operations.push(OperationSpec::AddOwnership {
                owner_type: relation_name.clone(),
                attribute: owned(attr_name)?,
            });
        }
        for attr_name in &changes.removed_attributes {
            if rename.suppresses_removed_ownership(relation_name, attr_name) {
                continue;
            }
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
            let mut role = role.clone();
            role.player_type_names = declared_role_players(&role.player_type_names, target);
            operations.push(OperationSpec::AddRole {
                relation_type: relation_name.clone(),
                role,
            });
        }
        for role_name in &changes.removed_roles {
            operations.push(OperationSpec::RemoveRole {
                relation_type: relation_name.clone(),
                role_name: role_name.clone(),
            });
        }
        if let Some(player_changes) = declared_player_changes.get(relation_name) {
            for player_change in player_changes {
                for player_type in &player_change.added_player_types {
                    if relation_players_changing_parent.contains(player_type.as_str()) {
                        continue;
                    }
                    operations.push(OperationSpec::AddRolePlayer {
                        relation_type: relation_name.clone(),
                        role_name: player_change.role_name.clone(),
                        player_type_name: player_type.clone(),
                    });
                }
                for player_type in &player_change.removed_player_types {
                    if players_attaching_to_parent.contains(player_type.as_str()) {
                        continue;
                    }
                    operations.push(OperationSpec::RemoveRolePlayer {
                        relation_type: relation_name.clone(),
                        role_name: player_change.role_name.clone(),
                        player_type_name: player_type.clone(),
                    });
                }
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
        for annotation_change in &changes.modified_role_annotations {
            let (old_doc, new_doc) = annotation_change.doc_changed.clone().unwrap_or_default();
            let (old_meta, new_meta) = annotation_change.meta_changed.clone().unwrap_or_default();
            operations.push(OperationSpec::ModifyRoleAnnotations {
                relation_type: relation_name.clone(),
                role_name: annotation_change.role_name.clone(),
                old_doc,
                new_doc,
                old_meta,
                new_meta,
            });
        }
        if let Some(op) =
            type_annotation_change(relation_name, &changes.doc_changed, &changes.meta_changed)
        {
            operations.push(op);
        }
        push_header_changes(
            &mut operations,
            relation_name,
            changes.abstract_changed,
            &changes.parent_changed,
        )?;
    }

    // Relations can themselves play roles. Their hierarchy changes occur in
    // the modified-relation loop above, so additions that depend on losing an
    // inherited capability must wait until every relation header has changed;
    // lexical relation ordering is not a dependency order.
    for (relation_name, changes) in &declared_player_changes {
        for change in changes {
            for player_type in &change.added_player_types {
                if relation_players_changing_parent.contains(player_type.as_str()) {
                    operations.push(OperationSpec::AddRolePlayer {
                        relation_type: relation_name.clone(),
                        role_name: change.role_name.clone(),
                        player_type_name: player_type.clone(),
                    });
                }
            }
        }
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

    operations.extend(rename_removals);
    for attr_name in &diff.removed_attributes {
        if rename.suppress_removed_attributes.contains(attr_name) {
            continue;
        }
        operations.push(OperationSpec::RemoveAttribute {
            attr_name: attr_name.clone(),
        });
    }

    Ok(operations)
}

/// The staged operations and diff suppressions for the requested attribute
/// renames.
#[derive(Default)]
struct RenameExpansion {
    /// Ops spliced after the diff's added attributes.
    additions: Vec<OperationSpec>,
    /// Ops spliced before the diff's removed attributes.
    removals: Vec<OperationSpec>,
    /// New attribute names whose diff `AddAttribute` the expansion replaces.
    suppress_added_attributes: BTreeSet<String>,
    /// Old attribute names whose diff `RemoveAttribute` the expansion
    /// replaces.
    suppress_removed_attributes: BTreeSet<String>,
    /// `(owner, new_name)` ownerships the expansion defines staged instead.
    suppress_added_ownerships: BTreeSet<(String, String)>,
    /// `(owner, old_name)` ownerships the expansion detaches itself.
    suppress_removed_ownerships: BTreeSet<(String, String)>,
}

impl RenameExpansion {
    fn suppresses_added_ownership(&self, owner: &str, attr_name: &str) -> bool {
        self.suppress_added_ownerships
            .contains(&(owner.to_string(), attr_name.to_string()))
    }

    fn suppresses_removed_ownership(&self, owner: &str, attr_name: &str) -> bool {
        self.suppress_removed_ownerships
            .contains(&(owner.to_string(), attr_name.to_string()))
    }
}

/// One owner of a renamed attribute, with its ownership entry.
struct RenameOwner {
    owner_type: String,
    entry: OwnedAttributeEntry,
    /// Whether the owner type itself still exists in the given schema.
    survives: bool,
}

/// Every entity/relation in `schema` owning `attr_name`, in deterministic
/// (entities-then-relations, name-sorted) order. `other` decides `survives`.
fn owners_of(schema: &SchemaInfo, other: &SchemaInfo, attr_name: &str) -> Vec<RenameOwner> {
    let mut owners = Vec::new();
    for (owner_type, entity) in &schema.entities {
        if let Some(entry) = owned_entry(&entity.owned_attributes, attr_name) {
            owners.push(RenameOwner {
                owner_type: owner_type.clone(),
                entry,
                survives: other.entities.contains_key(owner_type),
            });
        }
    }
    for (owner_type, relation) in &schema.relations {
        if let Some(entry) = owned_entry(&relation.owned_attributes, attr_name) {
            owners.push(RenameOwner {
                owner_type: owner_type.clone(),
                entry,
                survives: other.relations.contains_key(owner_type),
            });
        }
    }
    owners
}

/// Build the staged expansion for every `(old, new)` attribute rename.
///
/// Forward shape per rename (all existing primitives):
///
/// 1. `AddAttribute(new)` with the target's full definition.
/// 2. Plain `AddOwnership(owner, new)` per owner keeping the attribute —
///    staged, because data-dependent annotations (`@key`, `@card(1..)`)
///    fail commit validation while no instance owns the new attribute yet.
/// 3. `CopyAttribute(owner, old -> new)` backfill per keeping owner.
/// 4. `ModifyOwnership` tightening to the target annotations.
/// 5. `ModifyOwnership` loosening the old ownership's annotations —
///    deleting values under a declared `@key`/`@card(1..)` is a
///    commit-time violation.
/// 6. `RunTypeql` deleting the old attribute instances — the
///    irreversible step — then `RemoveOwnership(owner, old)` per
///    surviving owner (undefine refuses while instances exist) and
///    `RemoveAttribute(old)`.
fn expand_attribute_renames(
    base: &SchemaInfo,
    target: &SchemaInfo,
    renames: &[(String, String)],
) -> crate::Result<RenameExpansion> {
    let mut expansion = RenameExpansion::default();
    for (old_name, new_name) in renames {
        validate_rename(base, target, old_name, new_name, &expansion)?;
        expansion
            .suppress_removed_attributes
            .insert(old_name.clone());
        expansion.suppress_added_attributes.insert(new_name.clone());

        let old_owners = owners_of(base, target, old_name);
        let target_owners = owners_of(target, base, new_name);
        // Owners keeping the attribute under its new name, paired with the
        // target-side ownership entry (whose annotations win).
        let keeping: Vec<(&RenameOwner, &OwnedAttributeEntry)> = old_owners
            .iter()
            .filter_map(|owner| {
                target_owners
                    .iter()
                    .find(|candidate| candidate.owner_type == owner.owner_type)
                    .map(|candidate| (owner, &candidate.entry))
            })
            .collect();

        for (owner, target_entry) in &keeping {
            if owner.entry.is_ordered || target_entry.is_ordered {
                return Err(MigrationError::UnsupportedChange {
                    type_name: owner.owner_type.clone(),
                    change: format!(
                        "attribute rename {old_name:?} -> {new_name:?} over an ordered \
                         ownership has no backfill lowering"
                    ),
                });
            }
        }

        let new_attribute = target
            .attributes
            .get(new_name)
            .ok_or_else(|| missing("renamed attribute", new_name, "target"))?;
        expansion.additions.push(OperationSpec::AddAttribute {
            attribute: new_attribute.clone(),
        });
        for (owner, _) in &keeping {
            expansion
                .suppress_added_ownerships
                .insert((owner.owner_type.clone(), new_name.clone()));
            expansion.additions.push(OperationSpec::AddOwnership {
                owner_type: owner.owner_type.clone(),
                attribute: OwnedAttributeEntry {
                    attr_name: new_name.clone(),
                    value_type: new_attribute.value_type,
                    annotations: vec![],
                    is_ordered: false,
                    doc: None,
                    meta: BTreeMap::new(),
                },
            });
        }
        for (owner, _) in &keeping {
            expansion.additions.push(
                OperationSpec::CopyAttribute {
                    owner: Some(owner.owner_type.clone()),
                    source: Some(old_name.clone()),
                    dest: Some(new_name.clone()),
                    filter: None,
                    forward: None,
                    reverse: None,
                }
                .normalized()?,
            );
        }
        for (owner, target_entry) in &keeping {
            let flags = target_entry.flags_string();
            if !flags.is_empty() {
                expansion.additions.push(OperationSpec::ModifyOwnership {
                    owner_type: owner.owner_type.clone(),
                    attr_name: new_name.clone(),
                    old_annotations: String::new(),
                    new_annotations: flags,
                });
            }
        }

        // The old side unwinds in strict order. Deleting values violates a
        // declared @key/@card(1..), and undefining an ownership refuses
        // while instances exist (SVL51) — so: loosen the old annotations,
        // delete the old values, detach the empty ownerships, remove the
        // type.
        for owner in &old_owners {
            let flags = owner.entry.flags_string();
            if owner.survives && !flags.is_empty() {
                expansion.removals.push(OperationSpec::ModifyOwnership {
                    owner_type: owner.owner_type.clone(),
                    attr_name: old_name.clone(),
                    old_annotations: flags,
                    new_annotations: String::new(),
                });
            }
        }
        expansion.removals.push(OperationSpec::RunTypeql {
            forward: format!("match\n  $v isa {old_name};\ndelete\n  $v;"),
            reverse: None,
        });
        for owner in &old_owners {
            expansion
                .suppress_removed_ownerships
                .insert((owner.owner_type.clone(), old_name.clone()));
            // Owners removed by the diff detach their ownerships in the
            // removed-entity/relation flow; detaching here again would
            // reference a type that no longer exists.
            if owner.survives {
                expansion.removals.push(OperationSpec::RemoveOwnership {
                    owner_type: owner.owner_type.clone(),
                    attr_name: old_name.clone(),
                });
            }
        }
        expansion.removals.push(OperationSpec::RemoveAttribute {
            attr_name: old_name.clone(),
        });
    }
    Ok(expansion)
}

fn validate_rename(
    base: &SchemaInfo,
    target: &SchemaInfo,
    old_name: &str,
    new_name: &str,
    expansion: &RenameExpansion,
) -> crate::Result<()> {
    let invalid = |message: String| MigrationError::AuthoringInput { message };
    if old_name == new_name {
        return Err(invalid(format!(
            "attribute rename {old_name:?} -> {new_name:?} does not change the name"
        )));
    }
    if expansion.suppress_removed_attributes.contains(old_name)
        || expansion.suppress_added_attributes.contains(new_name)
    {
        return Err(invalid(format!(
            "attribute rename {old_name:?} -> {new_name:?} overlaps another rename directive"
        )));
    }
    let old_entry = base.attributes.get(old_name).ok_or_else(|| {
        invalid(format!(
            "attribute rename source {old_name:?} does not exist in the base schema"
        ))
    })?;
    let new_entry = target.attributes.get(new_name).ok_or_else(|| {
        invalid(format!(
            "attribute rename destination {new_name:?} does not exist in the target schema"
        ))
    })?;
    if target.attributes.contains_key(old_name) {
        return Err(invalid(format!(
            "attribute rename source {old_name:?} still exists in the target schema; \
             a rename must remove the old name"
        )));
    }
    if base.attributes.contains_key(new_name) {
        return Err(invalid(format!(
            "attribute rename destination {new_name:?} already exists in the base schema"
        )));
    }
    if old_entry.value_type != new_entry.value_type {
        return Err(invalid(format!(
            "attribute rename {old_name:?} -> {new_name:?} cannot change the value type \
             ({:?} -> {:?})",
            old_entry.value_type, new_entry.value_type
        )));
    }
    if let Some(child) = base
        .attributes
        .values()
        .find(|entry| entry.parent_type.as_deref() == Some(old_name))
    {
        return Err(MigrationError::UnsupportedChange {
            type_name: old_name.to_string(),
            change: format!(
                "attribute rename with subtypes ({:?} subs it) has no lowering",
                child.attr_name
            ),
        });
    }
    Ok(())
}

/// `@meta` annotations keyed by meta key (mirrors `diff.rs`).
type MetaMap = BTreeMap<String, String>;

/// An optional `@doc` transition as `(old, new)` (mirrors `diff.rs`).
type DocChange = Option<(Option<String>, Option<String>)>;

/// An optional `@meta` transition as `(old, new)` (mirrors `diff.rs`).
type MetaChange = Option<(MetaMap, MetaMap)>;

/// Build a `ModifyTypeAnnotations` op from a type's `@doc`/`@meta` diff
/// fields, or `None` when neither changed.
fn type_annotation_change(
    type_name: &str,
    doc_changed: &DocChange,
    meta_changed: &MetaChange,
) -> Option<OperationSpec> {
    if doc_changed.is_none() && meta_changed.is_none() {
        return None;
    }
    let (old_doc, new_doc) = doc_changed.clone().unwrap_or_default();
    let (old_meta, new_meta) = meta_changed.clone().unwrap_or_default();
    Some(OperationSpec::ModifyTypeAnnotations {
        type_name: type_name.to_string(),
        old_doc,
        new_doc,
        old_meta,
        new_meta,
    })
}

/// Reduce a flattened entity entry to its own declarations by dropping
/// attributes the parent already owns.
///
/// `SchemaInfo` entries carry the inheritance-resolved effective set. A
/// full-schema define block skips inherited capabilities by consulting the
/// parent entry, but the singleton `AddEntity` lowering has no parent in
/// scope — anything left in the entry is emitted verbatim, and TypeDB
/// rejects redeclaring an inherited capability on a subtype (#190).
fn declared_entity_entry(entity: &EntitySchemaEntry, target: &SchemaInfo) -> EntitySchemaEntry {
    let mut entity = entity.clone();
    if let Some(parent) = entity
        .parent_type
        .as_deref()
        .and_then(|p| target.entities.get(p))
    {
        let parent_attrs: BTreeSet<&str> = parent
            .owned_attributes
            .iter()
            .map(|a| a.attr_name.as_str())
            .collect();
        entity
            .owned_attributes
            .retain(|a| !parent_attrs.contains(a.attr_name.as_str()));
    }
    entity
}

/// Relation counterpart of [`declared_entity_entry`]: drops attributes and
/// roles the parent relation already declares, then reduces every
/// surviving role's player list to its declared players.
fn declared_relation_entry(
    relation: &RelationSchemaEntry,
    target: &SchemaInfo,
) -> RelationSchemaEntry {
    let mut relation = relation.clone();
    if let Some(parent) = relation
        .parent_type
        .as_deref()
        .and_then(|p| target.relations.get(p))
    {
        let parent_attrs: BTreeSet<&str> = parent
            .owned_attributes
            .iter()
            .map(|a| a.attr_name.as_str())
            .collect();
        let parent_roles: BTreeSet<&str> =
            parent.roles.iter().map(|r| r.role_name.as_str()).collect();
        relation
            .owned_attributes
            .retain(|a| !parent_attrs.contains(a.attr_name.as_str()));
        relation
            .roles
            .retain(|r| !parent_roles.contains(r.role_name.as_str()));
    }
    // A role's player list is flattened by `SchemaInfo::from_typeql`
    // regardless of whether the *relation* itself has a parent — a root
    // relation's role can still list both a player and that player's
    // subtype (#190 follow-up). Reduce every surviving role independently
    // of the parent-relation guard above.
    for role in &mut relation.roles {
        role.player_type_names = declared_role_players(&role.player_type_names, target);
    }
    relation
}

struct DeclaredRolePlayerChange {
    role_name: String,
    added_player_types: Vec<String>,
    removed_player_types: Vec<String>,
}

/// Compute the declared-player delta for every declared role present in both
/// schemas.
///
/// `SchemaDiff` compares effective (flattened) player vectors, which means a
/// hierarchy change can alter declarations without producing a role-player
/// diff at all. Comparing the reduced sets from the two schemas both removes
/// phantom changes and recovers declaration changes hidden by equal effective
/// vectors (#190 follow-up).
fn declared_role_player_changes(
    base: &SchemaInfo,
    target: &SchemaInfo,
) -> BTreeMap<String, Vec<DeclaredRolePlayerChange>> {
    let mut changes_by_relation = BTreeMap::new();
    for (relation_name, base_relation) in &base.relations {
        let Some(target_relation) = target.relations.get(relation_name) else {
            continue;
        };
        let base_inherited_roles: BTreeSet<&str> = base_relation
            .parent_type
            .as_deref()
            .and_then(|parent| base.relations.get(parent))
            .map(|parent| {
                parent
                    .roles
                    .iter()
                    .map(|role| role.role_name.as_str())
                    .collect()
            })
            .unwrap_or_default();
        let target_inherited_roles: BTreeSet<&str> = target_relation
            .parent_type
            .as_deref()
            .and_then(|parent| target.relations.get(parent))
            .map(|parent| {
                parent
                    .roles
                    .iter()
                    .map(|role| role.role_name.as_str())
                    .collect()
            })
            .unwrap_or_default();
        let base_roles: BTreeMap<&str, _> = base_relation
            .roles
            .iter()
            .filter(|role| !base_inherited_roles.contains(role.role_name.as_str()))
            .map(|role| (role.role_name.as_str(), role))
            .collect();
        let target_roles: BTreeMap<&str, _> = target_relation
            .roles
            .iter()
            .filter(|role| !target_inherited_roles.contains(role.role_name.as_str()))
            .map(|role| (role.role_name.as_str(), role))
            .collect();

        let mut relation_changes = Vec::new();
        for (role_name, base_role) in base_roles {
            let Some(target_role) = target_roles.get(role_name) else {
                continue;
            };
            let base_players: BTreeSet<String> =
                declared_role_players(&base_role.player_type_names, base)
                    .into_iter()
                    .collect();
            let target_players: BTreeSet<String> =
                declared_role_players(&target_role.player_type_names, target)
                    .into_iter()
                    .collect();
            let added_player_types: Vec<String> =
                target_players.difference(&base_players).cloned().collect();
            let removed_player_types: Vec<String> =
                base_players.difference(&target_players).cloned().collect();
            if !added_player_types.is_empty() || !removed_player_types.is_empty() {
                relation_changes.push(DeclaredRolePlayerChange {
                    role_name: role_name.to_string(),
                    added_player_types,
                    removed_player_types,
                });
            }
        }
        if !relation_changes.is_empty() {
            changes_by_relation.insert(relation_name.clone(), relation_changes);
        }
    }
    changes_by_relation
}

/// Reduce a role's flattened player list to its declared players (#190
/// follow-up).
///
/// `SchemaInfo` role entries carry the inheritance-flattened effective
/// player set: if `Person sub DomainEntity` and `DomainEntity` plays a
/// role, `player_type_names` lists both. A valid TypeDB 3.x schema never
/// lets a subtype plainly redeclare an inherited `plays` capability, so a
/// player whose transitive ancestor (via `parent_type`, walked through
/// both `entities` and `relations` — a relation can play a role too) also
/// appears in the same list can only be flattening, never a genuine
/// declaration; it is dropped. A player whose ancestors are *not* in the
/// list is a real declaration and survives untouched.
fn declared_role_players(players: &[String], schema: &SchemaInfo) -> Vec<String> {
    let present: BTreeSet<&str> = players.iter().map(String::as_str).collect();
    players
        .iter()
        .filter(|player| {
            !ancestors_of(player.as_str(), schema)
                .into_iter()
                .any(|ancestor| present.contains(ancestor))
        })
        .cloned()
        .collect()
}

/// The transitive `parent_type` chain of `type_name` in `schema`, checking
/// both `entities` and `relations` at each step (a relation can play a role
/// too). Does not include `type_name` itself.
fn ancestors_of<'schema>(type_name: &str, schema: &'schema SchemaInfo) -> Vec<&'schema str> {
    let mut chain = Vec::new();
    let mut visited: BTreeSet<&str> = BTreeSet::new();
    let mut current = type_name;
    while let Some(parent) = parent_type_of(current, schema) {
        if !visited.insert(parent) {
            break; // defend against a malformed cyclic `sub` chain
        }
        chain.push(parent);
        current = parent;
    }
    chain
}

/// The declared `parent_type` of `type_name`, whether it is an entity or a
/// relation.
fn parent_type_of<'schema>(type_name: &str, schema: &'schema SchemaInfo) -> Option<&'schema str> {
    schema
        .entities
        .get(type_name)
        .and_then(|entity| entity.parent_type.as_deref())
        .or_else(|| {
            schema
                .relations
                .get(type_name)
                .and_then(|relation| relation.parent_type.as_deref())
        })
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

    use type_bridge_orm::_entity::Annotation;
    use type_bridge_orm::_schema::info::{
        AttributeSchemaEntry, EntitySchemaEntry, OwnedAttributeEntry, RelationSchemaEntry,
        RoleEntry,
    };
    use type_bridge_orm::ValueType;

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
            doc: None,
            meta: BTreeMap::new(),
        }
    }

    fn entity(name: &str, owned_attributes: Vec<OwnedAttributeEntry>) -> EntitySchemaEntry {
        EntitySchemaEntry {
            type_name: name.to_string(),
            is_abstract: false,
            parent_type: None,
            owned_attributes,
            plays_cardinalities: BTreeMap::new(),
            doc: None,
            meta: BTreeMap::new(),
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
            doc: None,
            meta: BTreeMap::new(),
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
            doc: None,
            meta: BTreeMap::new(),
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
        map_schema_diff(base, target, &diff, &[]).expect("mapping should succeed")
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
            OperationSpec::ModifyTypeAnnotations { .. } => "modify_type_annotations",
            OperationSpec::ModifyRoleAnnotations { .. } => "modify_role_annotations",
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
    fn added_sub_entities_emit_parent_first_with_declared_attributes_only() {
        // "ape" sorts before its parent "zebra", and the flattened child
        // entry carries the inherited key attribute (#190).
        let base = SchemaInfo::default();
        let mut child = entity("ape", vec![owned("name", vec![Annotation::Key])]);
        child.parent_type = Some("zebra".to_string());
        let target = schema(
            vec![
                child,
                entity("zebra", vec![owned("name", vec![Annotation::Key])]),
            ],
            vec![],
            vec![attribute("name")],
        );

        let operations = map(&base, &target);

        assert_eq!(
            kinds(&operations),
            vec!["add_attribute", "add_entity", "add_entity"]
        );
        let OperationSpec::AddEntity { entity: first } = &operations[1] else {
            panic!("expected AddEntity: {:?}", operations[1]);
        };
        let OperationSpec::AddEntity { entity: second } = &operations[2] else {
            panic!("expected AddEntity: {:?}", operations[2]);
        };
        assert_eq!(first.type_name, "zebra");
        assert_eq!(second.type_name, "ape");
        assert_eq!(second.parent_type.as_deref(), Some("zebra"));
        assert!(
            second.owned_attributes.is_empty(),
            "inherited attributes must not be redeclared: {:?}",
            second.owned_attributes
        );
    }

    #[test]
    fn added_sub_entity_with_preexisting_parent_drops_inherited_attributes() {
        let parent = entity("zebra", vec![owned("name", vec![Annotation::Key])]);
        let base = schema(vec![parent.clone()], vec![], vec![attribute("name")]);
        let mut child = entity("ape", vec![owned("name", vec![Annotation::Key])]);
        child.parent_type = Some("zebra".to_string());
        let target = schema(vec![child, parent], vec![], vec![attribute("name")]);

        let operations = map(&base, &target);

        assert_eq!(kinds(&operations), vec!["add_entity"]);
        let OperationSpec::AddEntity { entity } = &operations[0] else {
            panic!("expected AddEntity: {:?}", operations[0]);
        };
        assert_eq!(entity.parent_type.as_deref(), Some("zebra"));
        assert!(entity.owned_attributes.is_empty());
    }

    #[test]
    fn added_sub_relations_emit_parent_first_with_declared_roles_only() {
        // "contract" sorts before its parent "work", and the flattened child
        // entry carries the inherited role (#190).
        let base = SchemaInfo::default();
        let parent = relation("work", vec![role("employee", &["person"])], vec![]);
        let mut child = relation(
            "contract",
            vec![
                role("employee", &["person"]),
                role("contractor", &["person"]),
            ],
            vec![],
        );
        child.parent_type = Some("work".to_string());
        let target = schema(vec![entity("person", vec![])], vec![child, parent], vec![]);

        let operations = map(&base, &target);

        assert_eq!(
            kinds(&operations),
            vec!["add_entity", "add_relation", "add_relation"]
        );
        let OperationSpec::AddRelation { relation: first } = &operations[1] else {
            panic!("expected AddRelation: {:?}", operations[1]);
        };
        let OperationSpec::AddRelation { relation: second } = &operations[2] else {
            panic!("expected AddRelation: {:?}", operations[2]);
        };
        assert_eq!(first.type_name, "work");
        assert_eq!(second.type_name, "contract");
        assert_eq!(second.parent_type.as_deref(), Some("work"));
        let role_names: Vec<&str> = second.roles.iter().map(|r| r.role_name.as_str()).collect();
        assert_eq!(
            role_names,
            vec!["contractor"],
            "inherited roles must not be redeclared"
        );
    }

    /// Find the `AddRelation` operation for `type_name`, or panic.
    fn add_relation_entry<'a>(
        operations: &'a [OperationSpec],
        type_name: &str,
    ) -> &'a RelationSchemaEntry {
        operations
            .iter()
            .find_map(|op| match op {
                OperationSpec::AddRelation { relation } if relation.type_name == type_name => {
                    Some(relation)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("expected AddRelation for {type_name:?}: {operations:?}"))
    }

    #[test]
    fn added_relation_role_players_reduced_to_declared_players() {
        // "person" subs "domain-entity", which already plays "owner":
        // flattening lists both in the role's player set, but only
        // "domain-entity" is a genuine declaration (#190 follow-up).
        let base = SchemaInfo::default();
        let mut person = entity("person", vec![]);
        person.parent_type = Some("domain-entity".to_string());
        let target = schema(
            vec![
                entity("domain-entity", vec![]),
                person,
                entity("asset", vec![]),
            ],
            vec![relation(
                "ownership",
                vec![
                    role("owner", &["domain-entity", "person"]),
                    role("asset", &["asset"]),
                ],
                vec![],
            )],
            vec![],
        );

        let operations = map(&base, &target);

        let ownership = add_relation_entry(&operations, "ownership");
        let owner_role = ownership
            .roles
            .iter()
            .find(|r| r.role_name == "owner")
            .expect("owner role present");
        assert_eq!(
            owner_role.player_type_names,
            vec!["domain-entity".to_string()],
            "flattened subtype must not be redeclared: {:?}",
            owner_role.player_type_names
        );
    }

    #[test]
    fn added_role_player_for_flattened_subtype_produces_no_phantom_add() {
        // Adding "person" as a subtype of an already-playing "domain-entity"
        // flattens "person" into "owner"'s player set at the SchemaInfo
        // level, but "person" never declares the `plays` capability itself:
        // only the add_entity should surface, not a phantom
        // add_role_player that TypeDB would reject (#190 follow-up).
        let base = schema(
            vec![entity("domain-entity", vec![]), entity("asset", vec![])],
            vec![relation(
                "ownership",
                vec![role("owner", &["domain-entity"]), role("asset", &["asset"])],
                vec![],
            )],
            vec![],
        );
        let mut person = entity("person", vec![]);
        person.parent_type = Some("domain-entity".to_string());
        let target = schema(
            vec![
                entity("domain-entity", vec![]),
                person,
                entity("asset", vec![]),
            ],
            vec![relation(
                "ownership",
                vec![
                    role("owner", &["domain-entity", "person"]),
                    role("asset", &["asset"]),
                ],
                vec![],
            )],
            vec![],
        );

        let operations = map(&base, &target);

        assert_eq!(
            kinds(&operations),
            vec!["add_entity"],
            "adding a subtype already flattened into an existing player's role \
             must not synthesize a role-player operation: {operations:?}"
        );
    }

    #[test]
    fn detached_subtype_explicit_role_player_is_added_when_flattened_sets_match() {
        // Initially only "parent" declares `plays owner`; flattening also
        // lists "child" because it subs "parent". After detaching, both
        // types declare the capability directly. The effective player vector
        // is identical, so SchemaDiff has no relation change, but the mapper
        // must still add the newly declared child player (#190 follow-up).
        let mut base_child = entity("child", vec![]);
        base_child.parent_type = Some("parent".to_string());
        let base = schema(
            vec![base_child, entity("parent", vec![])],
            vec![relation(
                "ownership",
                vec![role("owner", &["child", "parent"])],
                vec![],
            )],
            vec![],
        );
        let target = schema(
            vec![entity("child", vec![]), entity("parent", vec![])],
            vec![relation(
                "ownership",
                vec![role("owner", &["child", "parent"])],
                vec![],
            )],
            vec![],
        );

        let diff = SchemaDiff::compute(&base, &target);
        assert!(
            diff.modified_relations.is_empty(),
            "equal flattened vectors must expose the regression precondition: {diff:?}"
        );

        let operations = map_schema_diff(&base, &target, &diff, &[])
            .expect("mapping should recover the declared-player change");

        assert_eq!(
            operations,
            vec![
                OperationSpec::RunTypeql {
                    forward: "undefine\nsub parent from child;".to_string(),
                    reverse: None,
                },
                OperationSpec::AddRolePlayer {
                    relation_type: "ownership".to_string(),
                    role_name: "owner".to_string(),
                    player_type_name: "child".to_string(),
                },
            ]
        );
    }

    #[test]
    fn attached_subtype_explicit_role_player_is_removed_before_parent_change() {
        // The inverse transition must remove "child"'s direct declaration
        // before making it a subtype of the already-playing "parent". Each
        // schema operation commits independently, so adding the `sub` edge
        // first would temporarily redeclare an inherited capability.
        let base = schema(
            vec![entity("child", vec![]), entity("parent", vec![])],
            vec![relation(
                "ownership",
                vec![role("owner", &["child", "parent"])],
                vec![],
            )],
            vec![],
        );
        let mut target_child = entity("child", vec![]);
        target_child.parent_type = Some("parent".to_string());
        let target = schema(
            vec![target_child, entity("parent", vec![])],
            vec![relation(
                "ownership",
                vec![role("owner", &["child", "parent"])],
                vec![],
            )],
            vec![],
        );

        let diff = SchemaDiff::compute(&base, &target);
        assert!(diff.modified_relations.is_empty());

        let operations = map_schema_diff(&base, &target, &diff, &[])
            .expect("mapping should recover the declared-player change");

        assert_eq!(
            operations,
            vec![
                OperationSpec::RemoveRolePlayer {
                    relation_type: "ownership".to_string(),
                    role_name: "owner".to_string(),
                    player_type_name: "child".to_string(),
                },
                OperationSpec::RunTypeql {
                    forward: "define\nchild sub parent;".to_string(),
                    reverse: None,
                },
            ]
        );
    }

    #[test]
    fn detached_relation_player_is_added_after_its_parent_change() {
        // Relations can play roles too. "a-holder" sorts before the player
        // relation "z-child", but the mapper must still detach "z-child"
        // before declaring the capability that it previously inherited from
        // "z-parent".
        let outer = relation(
            "a-holder",
            vec![role("member", &["z-child", "z-parent"])],
            vec![],
        );
        let mut parent = relation("z-parent", vec![], vec![]);
        parent.is_abstract = true;
        let mut base_child = relation("z-child", vec![], vec![]);
        base_child.is_abstract = true;
        base_child.parent_type = Some("z-parent".to_string());
        let mut target_child = base_child.clone();
        target_child.parent_type = None;
        let base = schema(
            vec![],
            vec![outer.clone(), parent.clone(), base_child],
            vec![],
        );
        let target = schema(vec![], vec![outer, parent, target_child], vec![]);

        let diff = SchemaDiff::compute(&base, &target);
        assert!(
            !diff.modified_relations.contains_key("a-holder"),
            "equal flattened vectors must hide the outer relation change"
        );

        let operations = map_schema_diff(&base, &target, &diff, &[])
            .expect("mapping should order the declared-player change safely");

        assert_eq!(
            operations,
            vec![
                OperationSpec::RunTypeql {
                    forward: "undefine\nsub z-parent from z-child;".to_string(),
                    reverse: None,
                },
                OperationSpec::AddRolePlayer {
                    relation_type: "a-holder".to_string(),
                    role_name: "member".to_string(),
                    player_type_name: "z-child".to_string(),
                },
            ]
        );
    }

    #[test]
    fn inherited_relation_role_does_not_duplicate_declared_player_change() {
        // "special-ownership" inherits "owner" from "ownership" and its
        // flattened entry repeats the role. The newly declared child player
        // belongs to the role's declaring relation only; emitting the same
        // operation against the child relation would reference an inherited
        // role as though it were declared there.
        let mut base_child = entity("child", vec![]);
        base_child.parent_type = Some("parent".to_string());
        let parent_relation = relation(
            "ownership",
            vec![role("owner", &["child", "parent"])],
            vec![],
        );
        let mut child_relation = relation(
            "special-ownership",
            vec![role("owner", &["child", "parent"])],
            vec![],
        );
        child_relation.parent_type = Some("ownership".to_string());
        let base = schema(
            vec![base_child, entity("parent", vec![])],
            vec![parent_relation.clone(), child_relation.clone()],
            vec![],
        );
        let target = schema(
            vec![entity("child", vec![]), entity("parent", vec![])],
            vec![parent_relation, child_relation],
            vec![],
        );

        let operations = map(&base, &target);

        assert_eq!(kinds(&operations), vec!["run_typeql", "add_role_player"]);
        assert!(matches!(
            &operations[1],
            OperationSpec::AddRolePlayer {
                relation_type,
                role_name,
                player_type_name,
            } if relation_type == "ownership"
                && role_name == "owner"
                && player_type_name == "child"
        ));
    }

    #[test]
    fn removed_subtype_flattened_into_role_players_produces_no_phantom_undefine() {
        // The mirror of the add case: removing "person" (flattened into
        // "owner" alongside its already-playing ancestor) must not
        // synthesize a remove_role_player op — only the remove_entity path
        // applies (#190 follow-up).
        let mut person = entity("person", vec![]);
        person.parent_type = Some("domain-entity".to_string());
        let base = schema(
            vec![
                entity("domain-entity", vec![]),
                person,
                entity("asset", vec![]),
            ],
            vec![relation(
                "ownership",
                vec![
                    role("owner", &["domain-entity", "person"]),
                    role("asset", &["asset"]),
                ],
                vec![],
            )],
            vec![],
        );
        let target = schema(
            vec![entity("domain-entity", vec![]), entity("asset", vec![])],
            vec![relation(
                "ownership",
                vec![role("owner", &["domain-entity"]), role("asset", &["asset"])],
                vec![],
            )],
            vec![],
        );

        let operations = map(&base, &target);

        assert_eq!(
            kinds(&operations),
            vec!["remove_entity"],
            "removing a subtype flattened into a surviving player's role must \
             not synthesize a role-player undefine: {operations:?}"
        );
    }

    #[test]
    fn subtype_sole_declared_player_survives_reduction() {
        // "person" is the *only* declared player of "owner" — its ancestor
        // "domain-entity" does not play the role, so this is a genuine
        // declaration and must survive the reduction untouched (#190
        // follow-up).
        let base = SchemaInfo::default();
        let mut person = entity("person", vec![]);
        person.parent_type = Some("domain-entity".to_string());
        let target = schema(
            vec![entity("domain-entity", vec![]), person],
            vec![relation(
                "ownership",
                vec![role("owner", &["person"])],
                vec![],
            )],
            vec![],
        );

        let operations = map(&base, &target);

        let ownership = add_relation_entry(&operations, "ownership");
        let owner_role = ownership
            .roles
            .iter()
            .find(|r| r.role_name == "owner")
            .expect("owner role present");
        assert_eq!(
            owner_role.player_type_names,
            vec!["person".to_string()],
            "a genuine declaration must not be dropped: {:?}",
            owner_role.player_type_names
        );
    }

    #[test]
    fn transitively_flattened_grandchild_is_dropped() {
        // "grandchild" subs "child" subs "grandparent": only "grandparent"
        // declares `plays`, but flattening lists all three. The ancestor
        // walk must be transitive, not one level deep (#190 follow-up).
        let base = SchemaInfo::default();
        let mut child = entity("child", vec![]);
        child.parent_type = Some("grandparent".to_string());
        let mut grandchild = entity("grandchild", vec![]);
        grandchild.parent_type = Some("child".to_string());
        let target = schema(
            vec![entity("grandparent", vec![]), child, grandchild],
            vec![relation(
                "ownership",
                vec![role("owner", &["grandparent", "child", "grandchild"])],
                vec![],
            )],
            vec![],
        );

        let operations = map(&base, &target);

        let ownership = add_relation_entry(&operations, "ownership");
        let owner_role = ownership
            .roles
            .iter()
            .find(|r| r.role_name == "owner")
            .expect("owner role present");
        assert_eq!(
            owner_role.player_type_names,
            vec!["grandparent".to_string()],
            "transitive ancestors must be dropped: {:?}",
            owner_role.player_type_names
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

        let error = map_schema_diff(&base, &SchemaInfo::default(), &diff, &[])
            .expect_err("missing entry must error");

        assert!(matches!(error, MigrationError::AuthoringInput { .. }));
    }

    // ── attribute renames ────────────────────────────────────────────────

    fn map_renamed(
        base: &SchemaInfo,
        target: &SchemaInfo,
        renames: &[(&str, &str)],
    ) -> crate::Result<Vec<OperationSpec>> {
        let diff = SchemaDiff::compute(base, target);
        let renames: Vec<(String, String)> = renames
            .iter()
            .map(|(old, new)| (old.to_string(), new.to_string()))
            .collect();
        map_schema_diff(base, target, &diff, &renames)
    }

    fn rename_base() -> SchemaInfo {
        schema(
            vec![entity(
                "person",
                vec![owned("legacy-name", vec![Annotation::Key])],
            )],
            vec![],
            vec![attribute("legacy-name")],
        )
    }

    fn rename_target() -> SchemaInfo {
        schema(
            vec![entity(
                "person",
                vec![owned("display-name", vec![Annotation::Key])],
            )],
            vec![],
            vec![attribute("display-name")],
        )
    }

    #[test]
    fn rename_expands_to_the_staged_primitive_sequence() {
        let operations = map_renamed(
            &rename_base(),
            &rename_target(),
            &[("legacy-name", "display-name")],
        )
        .expect("rename must map");

        assert_eq!(
            kinds(&operations),
            vec![
                "add_attribute",    // display-name (target definition)
                "add_ownership",    // person owns display-name — plain, staged
                "copy_attribute",   // backfill legacy-name -> display-name
                "modify_ownership", // tighten to @key after the backfill
                "modify_ownership", // loosen @key on legacy-name pre-delete
                "run_typeql",       // delete legacy-name instances
                "remove_ownership", // detach the emptied ownership
                "remove_attribute", // undefine legacy-name
            ]
        );

        let OperationSpec::AddOwnership { attribute, .. } = &operations[1] else {
            panic!("expected staged ownership");
        };
        assert!(
            attribute.annotations.is_empty(),
            "staged ownership must be plain: @key before the backfill fails \
             commit validation on existing instances"
        );

        let OperationSpec::CopyAttribute {
            forward, reverse, ..
        } = &operations[2]
        else {
            panic!("expected backfill");
        };
        assert_eq!(
            forward.as_deref(),
            Some(
                "match\n  $x isa person, has legacy-name $v;\n  not { $x has display-name $d; };\ninsert\n  $x has display-name == $v;"
            )
        );
        assert!(reverse.is_some());

        let OperationSpec::ModifyOwnership {
            old_annotations,
            new_annotations,
            ..
        } = &operations[3]
        else {
            panic!("expected tightening");
        };
        assert_eq!(old_annotations, "");
        assert_eq!(new_annotations, "@key");

        let OperationSpec::ModifyOwnership {
            attr_name,
            old_annotations,
            new_annotations,
            ..
        } = &operations[4]
        else {
            panic!("expected loosening");
        };
        assert_eq!(attr_name, "legacy-name");
        assert_eq!(old_annotations, "@key");
        assert_eq!(new_annotations, "");

        let OperationSpec::RunTypeql { forward, reverse } = &operations[5] else {
            panic!("expected instance cleanup");
        };
        assert_eq!(forward, "match\n  $v isa legacy-name;\ndelete\n  $v;");
        assert!(
            reverse.is_none(),
            "destroying the old values is the \
             irreversible step"
        );
    }

    #[test]
    fn rename_suppresses_the_diffs_independent_remove_and_add() {
        let operations = map_renamed(
            &rename_base(),
            &rename_target(),
            &[("legacy-name", "display-name")],
        )
        .expect("rename must map");

        // Without the directive the same diff maps to a data-destroying
        // remove+add; with it, none of those independent ops survive.
        let plain = map(&rename_base(), &rename_target());
        assert_eq!(
            kinds(&plain),
            vec![
                "add_attribute",
                "add_ownership",
                "remove_ownership",
                "remove_attribute"
            ]
        );
        assert_eq!(operations.len(), 8, "rename ops fully replace diff ops");
    }

    #[test]
    fn rename_keeps_diff_ops_for_unrelated_owners() {
        // `badge` gains display-name fresh (no legacy-name history): its
        // AddOwnership stays a plain diff op with full annotations and no
        // backfill. `card` owned legacy-name but drops the attribute in the
        // target: no backfill, ownership detached in the rename block.
        let base = schema(
            vec![
                entity("person", vec![owned("legacy-name", vec![])]),
                entity("badge", vec![]),
                entity("card", vec![owned("legacy-name", vec![])]),
            ],
            vec![],
            vec![attribute("legacy-name")],
        );
        let target = schema(
            vec![
                entity("person", vec![owned("display-name", vec![])]),
                entity("badge", vec![owned("display-name", vec![])]),
                entity("card", vec![]),
            ],
            vec![],
            vec![attribute("display-name")],
        );

        let operations = map_renamed(&base, &target, &[("legacy-name", "display-name")])
            .expect("rename must map");

        assert_eq!(
            kinds(&operations),
            vec![
                "add_attribute",
                "add_ownership",    // person (staged, backfilled)
                "copy_attribute",   // person only
                "add_ownership",    // badge (diff op, fresh)
                "run_typeql",       // delete legacy-name instances
                "remove_ownership", // card
                "remove_ownership", // person
                "remove_attribute",
            ]
        );
        let OperationSpec::AddOwnership { owner_type, .. } = &operations[3] else {
            panic!("expected badge ownership");
        };
        assert_eq!(owner_type, "badge");
    }

    #[test]
    fn rename_skips_detach_for_owners_the_diff_removes() {
        // `card` disappears entirely: its ownership detaches in the
        // removed-entity flow, so the rename block must not detach it again.
        let base = schema(
            vec![
                entity("person", vec![owned("legacy-name", vec![])]),
                entity("card", vec![owned("legacy-name", vec![])]),
            ],
            vec![],
            vec![attribute("legacy-name")],
        );
        let target = schema(
            vec![entity("person", vec![owned("display-name", vec![])])],
            vec![],
            vec![attribute("display-name")],
        );

        let operations = map_renamed(&base, &target, &[("legacy-name", "display-name")])
            .expect("rename must map");

        assert_eq!(
            kinds(&operations),
            vec![
                "add_attribute",
                "add_ownership",    // person (staged)
                "copy_attribute",   // person
                "remove_ownership", // card detach (removed-entity flow)
                "remove_entity",    // card
                "run_typeql",       // delete legacy-name instances
                "remove_ownership", // person
                "remove_attribute",
            ]
        );
        let detaches: Vec<&String> = operations
            .iter()
            .filter_map(|op| match op {
                OperationSpec::RemoveOwnership { owner_type, .. } => Some(owner_type),
                _ => None,
            })
            .collect();
        assert_eq!(detaches, vec!["card", "person"]);
    }

    #[test]
    fn rename_without_annotations_skips_the_tightening_step() {
        let base = schema(
            vec![entity("person", vec![owned("legacy-name", vec![])])],
            vec![],
            vec![attribute("legacy-name")],
        );
        let target = schema(
            vec![entity("person", vec![owned("display-name", vec![])])],
            vec![],
            vec![attribute("display-name")],
        );

        let operations = map_renamed(&base, &target, &[("legacy-name", "display-name")])
            .expect("rename must map");

        assert!(!kinds(&operations).contains(&"modify_ownership"));
    }

    #[test]
    fn rename_validation_rejects_inconsistent_directives() {
        let cases: Vec<(SchemaInfo, SchemaInfo, (&str, &str), &str)> = vec![
            (
                rename_base(),
                rename_target(),
                ("legacy-name", "legacy-name"),
                "does not change the name",
            ),
            (
                rename_base(),
                rename_target(),
                ("ghost", "display-name"),
                "does not exist in the base schema",
            ),
            (
                rename_base(),
                rename_target(),
                ("legacy-name", "ghost"),
                "does not exist in the target schema",
            ),
            (
                rename_base(),
                schema(
                    vec![entity("person", vec![owned("display-name", vec![])])],
                    vec![],
                    vec![attribute("legacy-name"), attribute("display-name")],
                ),
                ("legacy-name", "display-name"),
                "still exists in the target schema",
            ),
            (
                schema(
                    vec![entity("person", vec![owned("legacy-name", vec![])])],
                    vec![],
                    vec![attribute("legacy-name"), attribute("display-name")],
                ),
                rename_target(),
                ("legacy-name", "display-name"),
                "already exists in the base schema",
            ),
        ];
        for (base, target, (old, new), expected) in cases {
            let error =
                map_renamed(&base, &target, &[(old, new)]).expect_err("directive must be rejected");
            let message = error.to_string();
            assert!(
                message.contains(expected),
                "expected {expected:?} in {message:?}"
            );
        }
    }

    #[test]
    fn rename_rejects_value_type_change() {
        let mut target = rename_target();
        target.attributes.insert("display-name".to_string(), {
            AttributeSchemaEntry::new("display-name", ValueType::Long)
        });

        let error = map_renamed(&rename_base(), &target, &[("legacy-name", "display-name")])
            .expect_err("value-type change must be rejected");
        assert!(error.to_string().contains("cannot change the value type"));
    }

    #[test]
    fn rename_rejects_attributes_with_subtypes_and_ordered_ownerships() {
        let mut base = rename_base();
        let mut child = attribute("nick-name");
        child.parent_type = Some("legacy-name".to_string());
        base.attributes.insert("nick-name".to_string(), child);
        let mut target = rename_target();
        let mut renamed_child = attribute("nick-name");
        renamed_child.parent_type = Some("display-name".to_string());
        target
            .attributes
            .insert("nick-name".to_string(), renamed_child);
        let error = map_renamed(&base, &target, &[("legacy-name", "display-name")])
            .expect_err("subtyped rename must be rejected");
        assert!(matches!(error, MigrationError::UnsupportedChange { .. }));

        let mut ordered_base = rename_base();
        ordered_base
            .entities
            .get_mut("person")
            .expect("person exists")
            .owned_attributes[0]
            .is_ordered = true;
        let error = map_renamed(
            &ordered_base,
            &rename_target(),
            &[("legacy-name", "display-name")],
        )
        .expect_err("ordered ownership rename must be rejected");
        assert!(matches!(error, MigrationError::UnsupportedChange { .. }));
    }

    #[test]
    fn overlapping_rename_directives_are_rejected() {
        let error = map_renamed(
            &rename_base(),
            &rename_target(),
            &[
                ("legacy-name", "display-name"),
                ("legacy-name", "display-name"),
            ],
        )
        .expect_err("duplicate directives must be rejected");
        assert!(error.to_string().contains("overlaps another rename"));
    }
}
