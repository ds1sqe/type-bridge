//! TypeQL `define` block generator from [`SchemaInfo`].

use std::collections::{BTreeMap, BTreeSet, HashSet};

use crate::_attribute::ValueType;
use type_bridge_core_lib::__internal as tql;

use super::info::{AttributeSchemaEntry, SchemaInfo};

/// A plays clause collected during relation processing.
///
/// Carries the optional plays-side cardinality overlay so that emission can
/// append `@card(min..max)` when present, and omit it entirely when absent,
/// preserving byte-identical output for the no-card case.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct PlaysClause {
    player: String,
    role_ref: String,
    cardinality: Option<(u32, Option<u32>)>,
}

/// Look up the plays-side cardinality for `(player, role_ref)` in the schema.
///
/// Checks entities first, then relations, so that both entity-players and
/// relation-players are covered (master invariant: relation-as-player).
/// Returns `None` when the player has no `@card` on this plays clause.
fn plays_card_for(info: &SchemaInfo, player: &str, role_ref: &str) -> Option<(u32, Option<u32>)> {
    info.entities
        .get(player)
        .and_then(|e| e.plays_cardinalities.get(role_ref).copied())
        .or_else(|| {
            info.relations
                .get(player)
                .and_then(|r| r.plays_cardinalities.get(role_ref).copied())
        })
}

fn player_parent<'a>(info: &'a SchemaInfo, player: &str) -> Option<&'a str> {
    info.entities
        .get(player)
        .and_then(|entry| entry.parent_type.as_deref())
        .or_else(|| {
            info.relations
                .get(player)
                .and_then(|entry| entry.parent_type.as_deref())
        })
}

/// Generate a schema using checked official-AST construction.
///
/// Invalid structured inputs return an empty string. Use
/// [`try_generate_define_block`] to retain the typed diagnostic.
#[deprecated(
    since = "2.3.0",
    note = "use try_generate_define_block; removed in 3.0"
)]
pub fn generate_define_block(info: &SchemaInfo) -> String {
    try_generate_define_block(info).unwrap_or_default()
}

/// Build and render a schema with typed construction errors.
pub fn try_generate_define_block(info: &SchemaInfo) -> Result<String, super::error::SchemaError> {
    let result = (|| -> Result<String, tql::TqlError> {
        let mut definitions = Vec::new();
        for attribute in info.attributes.values() {
            definitions.push(attribute_node(attribute, true)?);
        }
        for name in topological_sort(&info.entities, |entry| entry.parent_type.as_deref()) {
            let entity = &info.entities[&name];
            let parent = entity
                .parent_type
                .as_ref()
                .and_then(|name| info.entities.get(name));
            definitions.push(entity_node(entity, parent)?);
        }
        let mut plays_clauses = Vec::new();
        for name in topological_sort(&info.relations, |entry| entry.parent_type.as_deref()) {
            let relation = &info.relations[&name];
            let parent = relation
                .parent_type
                .as_ref()
                .and_then(|name| info.relations.get(name));
            definitions.push(relation_node(relation, parent)?);
            let parent_roles: BTreeSet<_> = parent
                .into_iter()
                .flat_map(|parent| &parent.roles)
                .map(|role| role.role_name.as_str())
                .collect();
            let mut seen = BTreeSet::new();
            for role in &relation.roles {
                if parent_roles.contains(role.role_name.as_str()) || !seen.insert(&role.role_name) {
                    continue;
                }
                let role_ref = format!("{}:{}", relation.type_name, role.role_name);
                for player in &role.player_type_names {
                    plays_clauses.push(PlaysClause {
                        player: player.clone(),
                        role_ref: role_ref.clone(),
                        cardinality: plays_card_for(info, player, &role_ref),
                    });
                }
            }
        }
        plays_clauses.sort();
        plays_clauses.dedup();
        for clause in &plays_clauses {
            // Flattened schema IR includes inherited players. TypeDB rejects an
            // unchanged plays declaration repeated on a subtype (SVL42).
            let mut ancestor = player_parent(info, &clause.player);
            let mut visited = BTreeSet::new();
            let mut inherited = false;
            while let Some(parent) = ancestor {
                if !visited.insert(parent) {
                    break;
                }
                if let Some(candidate) = plays_clauses.iter().find(|candidate| {
                    candidate.player == parent && candidate.role_ref == clause.role_ref
                }) {
                    // No local annotation means inheritance, not a reset of
                    // the ancestor's cardinality. An unannotated intermediate
                    // declaration likewise inherits its nearest explicit card.
                    if clause.cardinality.is_none() {
                        inherited = true;
                        break;
                    }
                    if candidate.cardinality.is_some() {
                        inherited = candidate.cardinality == clause.cardinality;
                        break;
                    }
                }
                ancestor = player_parent(info, parent);
            }
            if inherited {
                continue;
            }
            let (relation, role) = clause
                .role_ref
                .split_once(':')
                .ok_or(tql::TqlError::InvalidIdentifier)?;
            let mut annotations = Vec::new();
            if let Some((min, max)) = clause.cardinality {
                annotations.push(tql::schema_card(u64::from(min), max.map(u64::from))?);
            }
            definitions.push(tql::schema_type(
                None,
                &clause.player,
                vec![],
                vec![tql::schema_plays(relation, role, annotations)?],
            )?);
        }
        tql::render_schema_document(
            definitions
                .into_iter()
                .map(|definition| (definition, vec![]))
                .collect(),
        )
    })();
    result.map_err(|error| super::error::SchemaError::Validation {
        message: error.to_string(),
    })
}

/// Build one checked entity definition with its declared parent context.
pub fn entity_node(
    entity: &super::info::EntitySchemaEntry,
    parent: Option<&super::info::EntitySchemaEntry>,
) -> Result<tql::SchemaDefinition, tql::TqlError> {
    let parent_attributes: BTreeMap<_, _> = parent
        .into_iter()
        .flat_map(|parent| &parent.owned_attributes)
        .map(|attribute| (attribute.attr_name.as_str(), attribute))
        .collect();
    let mut capabilities = Vec::new();
    if let Some(parent) = &entity.parent_type {
        capabilities.push(tql::schema_sub(parent)?);
    }
    for attribute in &entity.owned_attributes {
        if parent_attributes
            .get(attribute.attr_name.as_str())
            .is_some_and(|parent| *parent == attribute)
        {
            continue;
        }
        capabilities.push(ownership_node(attribute)?);
    }
    let mut annotations = Vec::new();
    if entity.is_abstract {
        annotations.push(tql::schema_flag("abstract")?);
    }
    doc_meta(&mut annotations, entity.doc.as_deref(), &entity.meta)?;
    tql::schema_type(Some("entity"), &entity.type_name, annotations, capabilities)
}

/// Build one checked relation definition with its declared parent context.
pub fn relation_node(
    relation: &super::info::RelationSchemaEntry,
    parent: Option<&super::info::RelationSchemaEntry>,
) -> Result<tql::SchemaDefinition, tql::TqlError> {
    let parent_attributes: BTreeMap<_, _> = parent
        .into_iter()
        .flat_map(|parent| &parent.owned_attributes)
        .map(|attribute| (attribute.attr_name.as_str(), attribute))
        .collect();
    let parent_roles: BTreeSet<_> = parent
        .into_iter()
        .flat_map(|parent| &parent.roles)
        .map(|role| role.role_name.as_str())
        .collect();
    let mut capabilities = Vec::new();
    if let Some(parent) = &relation.parent_type {
        capabilities.push(tql::schema_sub(parent)?);
    }
    let mut seen = BTreeSet::new();
    for role in &relation.roles {
        if parent_roles.contains(role.role_name.as_str()) || !seen.insert(&role.role_name) {
            continue;
        }
        let mut annotations = Vec::new();
        if role.is_abstract {
            annotations.push(tql::schema_flag("abstract")?);
        }
        if role.distinct {
            annotations.push(tql::schema_flag("distinct")?);
        }
        if let Some((min, max)) = role.cardinality {
            annotations.push(tql::schema_card(u64::from(min), max.map(u64::from))?);
        }
        doc_meta(&mut annotations, role.doc.as_deref(), &role.meta)?;
        capabilities.push(tql::schema_relates_with_parent_source(
            &role.role_name,
            role.ordered,
            role.overrides.as_deref(),
            annotations,
        )?);
    }
    for attribute in &relation.owned_attributes {
        if parent_attributes
            .get(attribute.attr_name.as_str())
            .is_some_and(|parent| *parent == attribute)
        {
            continue;
        }
        capabilities.push(ownership_node(attribute)?);
    }
    let mut annotations = Vec::new();
    if relation.is_abstract {
        annotations.push(tql::schema_flag("abstract")?);
    }
    doc_meta(&mut annotations, relation.doc.as_deref(), &relation.meta)?;
    tql::schema_type(
        Some("relation"),
        &relation.type_name,
        annotations,
        capabilities,
    )
}

pub(crate) fn doc_meta(
    output: &mut Vec<tql::SchemaAnnotation>,
    doc: Option<&str>,
    meta: &BTreeMap<String, String>,
) -> Result<(), tql::TqlError> {
    if let Some(doc) = doc {
        output.push(tql::schema_doc(doc)?);
    }
    for (key, value) in meta {
        output.push(tql::schema_meta(key, value)?);
    }
    Ok(())
}

/// Build one checked ownership capability.
pub fn ownership_node(
    attribute: &super::info::OwnedAttributeEntry,
) -> Result<tql::SchemaCapability, tql::TqlError> {
    tql::schema_owns(
        &attribute.attr_name,
        attribute.is_ordered,
        ownership_annotations(attribute)?,
    )
}

/// Build checked ownership annotations in their declared order.
pub fn ownership_annotations(
    attribute: &super::info::OwnedAttributeEntry,
) -> Result<Vec<tql::SchemaAnnotation>, tql::TqlError> {
    use crate::_entity::Annotation;
    let mut annotations = Vec::new();
    for annotation in &attribute.annotations {
        annotations.push(match annotation {
            Annotation::Key => tql::schema_flag("key")?,
            Annotation::Unique => tql::schema_flag("unique")?,
            Annotation::Distinct => tql::schema_flag("distinct")?,
            Annotation::Card(min, max) => tql::schema_card(u64::from(*min), max.map(u64::from))?,
        });
    }
    doc_meta(&mut annotations, attribute.doc.as_deref(), &attribute.meta)?;
    Ok(annotations)
}

/// Build checked attribute nodes for schema generation and migration authoring.
pub fn attribute_node(
    attribute: &AttributeSchemaEntry,
    include_doc_meta: bool,
) -> Result<tql::SchemaDefinition, tql::TqlError> {
    let mut annotations = Vec::new();
    if attribute.is_abstract {
        annotations.push(tql::schema_flag("abstract")?);
    }
    if attribute.is_independent {
        annotations.push(tql::schema_flag("independent")?);
    }
    if include_doc_meta {
        doc_meta(&mut annotations, attribute.doc.as_deref(), &attribute.meta)?;
    }
    let mut capabilities = Vec::new();
    if let Some(parent) = &attribute.parent_type {
        capabilities.push(tql::schema_sub(parent)?);
    }
    let mut values = Vec::new();
    if let Some((min, max)) = &attribute.range {
        values.push(tql::schema_range(min.as_deref(), max.as_deref())?);
    }
    if let Some(regex) = &attribute.regex {
        values.push(tql::schema_regex_source(regex)?);
    }
    if let Some(allowed) = &attribute.allowed_values {
        values.push(tql::schema_values_source(
            &allowed.iter().map(String::as_str).collect::<Vec<_>>(),
        )?);
    }
    capabilities.push(tql::schema_value(
        typedb_value_type(&attribute.value_type),
        values,
    )?);
    tql::schema_type(
        Some("attribute"),
        &attribute.attr_name,
        annotations,
        capabilities,
    )
}
fn typedb_value_type(value_type: &ValueType) -> &'static str {
    match value_type {
        ValueType::String => "string",
        ValueType::Long => "integer",
        ValueType::Double => "double",
        ValueType::Boolean => "boolean",
        ValueType::Date => "date",
        ValueType::DateTime => "datetime",
        ValueType::DateTimeTz => "datetime-tz",
        ValueType::Decimal => "decimal",
        ValueType::Duration => "duration",
    }
}

/// Render a cardinality annotation. Invalid bounds return an empty string.
#[deprecated(
    since = "2.3.0",
    note = "use checked core schema builders; removed in 3.0"
)]
pub fn card_annotation(min: u32, max: Option<u32>) -> String {
    tql::schema_card(u64::from(min), max.map(u64::from))
        .map(|value| tql::render_schema_annotations(&[value]))
        .unwrap_or_default()
}
/// Render an attribute definition. Invalid structured inputs return an empty string.
#[deprecated(
    since = "2.3.0",
    note = "use checked core schema builders; removed in 3.0"
)]
pub fn attribute_definition(attribute: &AttributeSchemaEntry) -> String {
    attribute_node(attribute, true)
        .and_then(tql::render_schema_definition)
        .unwrap_or_default()
}
/// Render attribute constraints without head documentation annotations.
#[deprecated(
    since = "2.3.0",
    note = "use checked core schema builders; removed in 3.0"
)]
pub fn attribute_constraint_definition(attribute: &AttributeSchemaEntry) -> String {
    attribute_node(attribute, false)
        .and_then(tql::render_schema_definition)
        .unwrap_or_default()
}

/// Topological sort: parents before children, deterministic order within each level.
///
/// Only keys of `map` appear in the result; a parent naming a type outside
/// the map (e.g. defined in an earlier migration) is ignored rather than
/// emitted.
pub fn topological_sort<T, F>(map: &BTreeMap<String, T>, get_parent: F) -> Vec<String>
where
    F: Fn(&T) -> Option<&str>,
{
    let mut result = Vec::new();
    let mut visited = HashSet::new();

    fn visit<T2, F2>(
        name: &str,
        map: &BTreeMap<String, T2>,
        get_parent: &F2,
        visited: &mut HashSet<String>,
        result: &mut Vec<String>,
    ) where
        F2: Fn(&T2) -> Option<&str>,
    {
        if visited.contains(name) {
            return;
        }
        visited.insert(name.to_string());
        // A parent may name a type outside this map (e.g. the singleton
        // SchemaInfo built by migration lowering); such names are not part
        // of this ordering and must not be emitted.
        let Some(entry) = map.get(name) else {
            return;
        };
        if let Some(parent) = get_parent(entry) {
            visit(parent, map, get_parent, visited, result);
        }
        result.push(name.to_string());
    }

    // BTreeMap keys are already sorted alphabetically
    for name in map.keys() {
        visit(name, map, &get_parent, &mut visited, &mut result);
    }
    result
}

#[cfg(test)]
#[allow(deprecated)]
mod tests {
    use super::*;
    use crate::_attribute::ValueType;
    use crate::_descriptor::{
        EntityDescriptor, RelationDescriptor, RoleDescriptor, TypeDescriptor,
    };
    use crate::_entity::Annotation;
    use crate::_schema::info::*;

    #[test]
    fn empty_schema_produces_define_only() {
        let info = SchemaInfo::default();
        let result = generate_define_block(&info);
        assert_eq!(result, "define");
    }

    #[test]
    fn generates_attribute_definitions() {
        let mut info = SchemaInfo::default();
        info.attributes.insert(
            "name".into(),
            AttributeSchemaEntry::new("name", ValueType::String),
        );
        info.attributes.insert(
            "age".into(),
            AttributeSchemaEntry::new("age", ValueType::Long),
        );

        let result = generate_define_block(&info);
        assert!(result.contains("attribute age, value integer;"));
        assert!(result.contains("attribute name, value string;"));
        // age before name (alphabetical)
        let age_pos = result.find("attribute age").unwrap();
        let name_pos = result.find("attribute name").unwrap();
        assert!(age_pos < name_pos);
    }

    #[test]
    fn generates_attribute_type_annotations() {
        let mut info = SchemaInfo::default();
        let mut email = AttributeSchemaEntry::new("email", ValueType::String);
        email.regex = Some(r"^[a-z]+@[a-z]+\.[a-z]+$".into());
        let mut status = AttributeSchemaEntry::new("status", ValueType::String);
        status.allowed_values = Some(vec!["active".into(), "inactive".into(), "pending".into()]);
        let mut age = AttributeSchemaEntry::new("age", ValueType::Long);
        age.range = Some((Some("0".into()), Some("150".into())));
        let mut language = AttributeSchemaEntry::new("language", ValueType::String);
        language.is_independent = true;

        info.attributes.insert("email".into(), email);
        info.attributes.insert("status".into(), status);
        info.attributes.insert("age".into(), age);
        info.attributes.insert("language".into(), language);

        let result = generate_define_block(&info);

        assert!(
            result.contains(r#"attribute email, value string @regex("^[a-z]+@[a-z]+\.[a-z]+$");"#)
        );
        assert!(result.contains(
            r#"attribute status, value string @values("active", "inactive", "pending");"#
        ));
        assert!(result.contains("attribute age, value integer @range(0..150);"));
        assert!(result.contains("attribute language @independent, value string;"));
    }

    #[test]
    fn generates_abstract_attribute_sub_clause_in_typedb_order() {
        let mut info = SchemaInfo::default();
        let mut attr = AttributeSchemaEntry::new("child-id", ValueType::String);
        attr.is_abstract = true;
        attr.parent_type = Some("base-id".into());
        info.attributes.insert("child-id".into(), attr);

        let result = generate_define_block(&info);

        assert!(result.contains("attribute child-id @abstract, sub base-id, value string;"));
        assert!(!result.contains("attribute child-id, sub base-id @abstract"));
    }

    #[test]
    fn generates_entity_with_key() {
        let mut info = SchemaInfo::default();
        info.attributes.insert(
            "name".into(),
            AttributeSchemaEntry::new("name", ValueType::String),
        );
        info.entities.insert(
            "person".into(),
            EntitySchemaEntry {
                type_name: "person".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![OwnedAttributeEntry {
                    attr_name: "name".into(),
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

        let result = generate_define_block(&info);
        assert!(result.contains("entity person,"));
        assert!(result.contains("owns name @key;"));
    }

    #[test]
    fn generates_entity_with_multiple_attrs() {
        let mut info = SchemaInfo::default();
        info.entities.insert(
            "person".into(),
            EntitySchemaEntry {
                type_name: "person".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![
                    OwnedAttributeEntry {
                        attr_name: "name".into(),
                        value_type: ValueType::String,
                        annotations: vec![Annotation::Key],
                        is_ordered: false,
                        doc: None,
                        meta: Default::default(),
                    },
                    OwnedAttributeEntry {
                        attr_name: "age".into(),
                        value_type: ValueType::Long,
                        annotations: vec![],
                        is_ordered: false,
                        doc: None,
                        meta: Default::default(),
                    },
                ],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );

        let result = generate_define_block(&info);
        assert!(result.contains("owns name @key,"));
        assert!(result.contains("owns age;"));
    }

    #[test]
    fn generates_relation_with_roles() {
        let mut info = SchemaInfo::default();
        info.relations.insert(
            "employment".into(),
            RelationSchemaEntry {
                type_name: "employment".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![OwnedAttributeEntry {
                    attr_name: "position".into(),
                    value_type: ValueType::String,
                    annotations: vec![],
                    is_ordered: false,
                    doc: None,
                    meta: Default::default(),
                }],
                roles: vec![
                    RoleEntry {
                        role_name: "employee".into(),
                        player_type_names: vec!["person".into()],
                        cardinality: None,
                        ..Default::default()
                    },
                    RoleEntry {
                        role_name: "employer".into(),
                        player_type_names: vec!["company".into()],
                        cardinality: None,
                        ..Default::default()
                    },
                ],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );

        let result = generate_define_block(&info);
        assert!(result.contains("relation employment,"));
        assert!(result.contains("relates employee,"));
        assert!(result.contains("relates employer,"));
        assert!(result.contains("owns position;"));
        assert!(result.contains("company, plays employment:employer;"));
        assert!(result.contains("person, plays employment:employee;"));
    }

    #[test]
    fn generates_cardinality_annotation() {
        let mut info = SchemaInfo::default();
        info.entities.insert(
            "person".into(),
            EntitySchemaEntry {
                type_name: "person".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![OwnedAttributeEntry {
                    attr_name: "tag".into(),
                    value_type: ValueType::String,
                    annotations: vec![Annotation::Card(2, Some(5))],
                    is_ordered: false,
                    doc: None,
                    meta: Default::default(),
                }],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );

        let result = generate_define_block(&info);
        assert!(result.contains("owns tag @card(2..5);"));
    }

    #[test]
    fn generates_unique_annotation() {
        let mut info = SchemaInfo::default();
        info.entities.insert(
            "person".into(),
            EntitySchemaEntry {
                type_name: "person".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![OwnedAttributeEntry {
                    attr_name: "email".into(),
                    value_type: ValueType::String,
                    annotations: vec![Annotation::Unique],
                    is_ordered: false,
                    doc: None,
                    meta: Default::default(),
                }],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );

        let result = generate_define_block(&info);
        assert!(result.contains("owns email @unique;"));
    }

    #[test]
    fn generates_abstract_entity() {
        let mut info = SchemaInfo::default();
        info.entities.insert(
            "animal".into(),
            EntitySchemaEntry {
                type_name: "animal".into(),
                is_abstract: true,
                parent_type: None,
                owned_attributes: vec![OwnedAttributeEntry {
                    attr_name: "name".into(),
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

        let result = generate_define_block(&info);
        assert!(
            result.contains("entity animal @abstract,"),
            "should have @abstract: {result}"
        );
        assert!(result.contains("owns name @key;"));
    }

    #[test]
    fn generates_sub_clause() {
        let mut info = SchemaInfo::default();
        info.entities.insert(
            "animal".into(),
            EntitySchemaEntry {
                type_name: "animal".into(),
                is_abstract: true,
                parent_type: None,
                owned_attributes: vec![OwnedAttributeEntry {
                    attr_name: "name".into(),
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
        info.entities.insert(
            "dog".into(),
            EntitySchemaEntry {
                type_name: "dog".into(),
                is_abstract: false,
                parent_type: Some("animal".into()),
                owned_attributes: vec![
                    OwnedAttributeEntry {
                        attr_name: "name".into(),
                        value_type: ValueType::String,
                        annotations: vec![Annotation::Key],
                        is_ordered: false,
                        doc: None,
                        meta: Default::default(),
                    },
                    OwnedAttributeEntry {
                        attr_name: "breed".into(),
                        value_type: ValueType::String,
                        annotations: vec![],
                        is_ordered: false,
                        doc: None,
                        meta: Default::default(),
                    },
                ],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );

        let result = generate_define_block(&info);
        assert!(
            result.contains("entity dog, sub animal,"),
            "should have sub clause: {result}"
        );
    }

    #[test]
    fn generates_abstract_entity_sub_clause_in_typedb_order() {
        let mut info = SchemaInfo::default();
        info.entities.insert(
            "animal".into(),
            EntitySchemaEntry {
                type_name: "animal".into(),
                is_abstract: true,
                parent_type: None,
                owned_attributes: vec![],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );
        info.entities.insert(
            "mammal".into(),
            EntitySchemaEntry {
                type_name: "mammal".into(),
                is_abstract: true,
                parent_type: Some("animal".into()),
                owned_attributes: vec![],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );

        let result = generate_define_block(&info);

        assert!(result.contains("entity mammal @abstract, sub animal;"));
        assert!(!result.contains("entity mammal, sub animal @abstract"));
    }

    #[test]
    fn topological_sort_parents_before_children() {
        let mut info = SchemaInfo::default();
        // Insert child first alphabetically (dog before animal is not the case,
        // but let's use "cat" which comes before "mammal")
        info.entities.insert(
            "cat".into(),
            EntitySchemaEntry {
                type_name: "cat".into(),
                is_abstract: false,
                parent_type: Some("mammal".into()),
                owned_attributes: vec![],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );
        info.entities.insert(
            "mammal".into(),
            EntitySchemaEntry {
                type_name: "mammal".into(),
                is_abstract: true,
                parent_type: None,
                owned_attributes: vec![],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );

        let result = generate_define_block(&info);
        let mammal_pos = result.find("entity mammal").unwrap();
        let cat_pos = result.find("entity cat").unwrap();
        assert!(
            mammal_pos < cat_pos,
            "parent (mammal) should appear before child (cat): {result}"
        );
    }

    #[test]
    fn entity_with_parent_outside_schema_keeps_sub_clause() {
        // Migration lowering builds singleton SchemaInfos whose entries may
        // reference parents defined elsewhere; generation must not panic and
        // must preserve the `sub` clause (#190).
        let mut info = SchemaInfo::default();
        info.entities.insert(
            "person".into(),
            EntitySchemaEntry {
                type_name: "person".into(),
                is_abstract: false,
                parent_type: Some("animal".into()),
                owned_attributes: vec![],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );

        let result = generate_define_block(&info);

        assert!(result.contains("entity person, sub animal;"), "{result}");
        assert!(!result.contains("entity animal"), "{result}");
    }

    #[test]
    fn relation_with_parent_outside_schema_keeps_sub_clause() {
        let mut info = SchemaInfo::default();
        info.relations.insert(
            "part-time-employment".into(),
            RelationSchemaEntry {
                type_name: "part-time-employment".into(),
                is_abstract: false,
                parent_type: Some("employment".into()),
                owned_attributes: vec![],
                roles: vec![],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );

        let result = generate_define_block(&info);

        assert!(
            result.contains("relation part-time-employment, sub employment;"),
            "{result}"
        );
        assert!(!result.contains("relation employment"), "{result}");
    }

    #[test]
    fn skips_inherited_attributes() {
        let mut info = SchemaInfo::default();
        info.entities.insert(
            "animal".into(),
            EntitySchemaEntry {
                type_name: "animal".into(),
                is_abstract: true,
                parent_type: None,
                owned_attributes: vec![OwnedAttributeEntry {
                    attr_name: "name".into(),
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
        info.entities.insert(
            "dog".into(),
            EntitySchemaEntry {
                type_name: "dog".into(),
                is_abstract: false,
                parent_type: Some("animal".into()),
                owned_attributes: vec![
                    OwnedAttributeEntry {
                        attr_name: "name".into(),
                        value_type: ValueType::String,
                        annotations: vec![Annotation::Key],
                        is_ordered: false,
                        doc: None,
                        meta: Default::default(),
                    },
                    OwnedAttributeEntry {
                        attr_name: "breed".into(),
                        value_type: ValueType::String,
                        annotations: vec![],
                        is_ordered: false,
                        doc: None,
                        meta: Default::default(),
                    },
                ],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );

        let result = generate_define_block(&info);
        // "dog" section should have "owns breed" but NOT "owns name" (inherited)
        let dog_section_start = result.find("entity dog").unwrap();
        let dog_section = &result[dog_section_start..];
        // Find end of dog section (next blank line or end)
        let dog_section_end = dog_section.find("\n\n").unwrap_or(dog_section.len());
        let dog_section = &dog_section[..dog_section_end];

        assert!(
            dog_section.contains("owns breed"),
            "dog should own breed: {dog_section}"
        );
        assert!(
            !dog_section.contains("owns name"),
            "dog should NOT re-emit inherited name: {dog_section}"
        );
    }

    #[test]
    fn emits_changed_ordered_ownership_redeclaration_on_subtype() {
        let mut info = SchemaInfo::default();
        info.attributes.insert(
            "tag".into(),
            AttributeSchemaEntry::new("tag", ValueType::String),
        );
        info.entities.insert(
            "base".into(),
            EntitySchemaEntry {
                type_name: "base".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![OwnedAttributeEntry {
                    attr_name: "tag".into(),
                    value_type: ValueType::String,
                    annotations: vec![Annotation::Distinct, Annotation::Card(0, Some(3))],
                    is_ordered: true,
                    doc: None,
                    meta: Default::default(),
                }],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );
        info.entities.insert(
            "child".into(),
            EntitySchemaEntry {
                type_name: "child".into(),
                is_abstract: false,
                parent_type: Some("base".into()),
                owned_attributes: vec![OwnedAttributeEntry {
                    attr_name: "tag".into(),
                    value_type: ValueType::String,
                    annotations: vec![Annotation::Card(0, Some(3))],
                    is_ordered: true,
                    doc: None,
                    meta: Default::default(),
                }],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );

        let emitted = generate_define_block(&info);
        assert!(
            emitted.contains("entity child, sub base, owns tag[] @card(0..3);"),
            "the distinct drop must remain an explicit subtype redeclaration: {emitted}"
        );
        typeql::parse_query(&emitted).expect("TypeQL 3.12 accepts the ordered redeclaration");
    }

    #[test]
    fn generates_abstract_relation() {
        let mut info = SchemaInfo::default();
        info.relations.insert(
            "connection".into(),
            RelationSchemaEntry {
                type_name: "connection".into(),
                is_abstract: true,
                parent_type: None,
                owned_attributes: vec![],
                roles: vec![RoleEntry {
                    role_name: "source".into(),
                    player_type_names: vec!["node".into()],
                    ..Default::default()
                }],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );

        let result = generate_define_block(&info);
        assert!(
            result.contains("relation connection @abstract,"),
            "should have @abstract: {result}"
        );
    }

    #[test]
    fn generates_relation_sub_clause() {
        let mut info = SchemaInfo::default();
        info.relations.insert(
            "connection".into(),
            RelationSchemaEntry {
                type_name: "connection".into(),
                is_abstract: true,
                parent_type: None,
                owned_attributes: vec![],
                roles: vec![],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );
        info.relations.insert(
            "employment".into(),
            RelationSchemaEntry {
                type_name: "employment".into(),
                is_abstract: false,
                parent_type: Some("connection".into()),
                owned_attributes: vec![],
                roles: vec![RoleEntry {
                    role_name: "employee".into(),
                    player_type_names: vec!["person".into()],
                    ..Default::default()
                }],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );

        let result = generate_define_block(&info);
        assert!(
            result.contains("relation employment, sub connection,"),
            "should have sub clause: {result}"
        );
    }

    #[test]
    fn generates_abstract_relation_sub_clause_in_typedb_order() {
        let mut info = SchemaInfo::default();
        info.relations.insert(
            "connection".into(),
            RelationSchemaEntry {
                type_name: "connection".into(),
                is_abstract: true,
                parent_type: None,
                owned_attributes: vec![],
                roles: vec![],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );
        info.relations.insert(
            "interaction".into(),
            RelationSchemaEntry {
                type_name: "interaction".into(),
                is_abstract: true,
                parent_type: Some("connection".into()),
                owned_attributes: vec![],
                roles: vec![],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );

        let result = generate_define_block(&info);

        assert!(result.contains("relation interaction @abstract, sub connection;"));
        assert!(!result.contains("relation interaction, sub connection @abstract"));
    }

    #[test]
    fn deduplicates_relation_roles() {
        let mut info = SchemaInfo::default();
        info.relations.insert(
            "friendship".into(),
            RelationSchemaEntry {
                type_name: "friendship".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![],
                roles: vec![
                    RoleEntry {
                        role_name: "friend".into(),
                        player_type_names: vec!["person".into()],
                        ..Default::default()
                    },
                    RoleEntry {
                        role_name: "friend".into(),
                        player_type_names: vec!["person".into()],
                        ..Default::default()
                    },
                ],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );

        let result = generate_define_block(&info);
        // "relates friend" should appear only once
        let count = result.matches("relates friend").count();
        assert_eq!(count, 1, "should deduplicate role names: {result}");
    }

    #[test]
    fn descriptors_emit_relates_only_role_without_plays() {
        let descriptors = vec![
            TypeDescriptor::Entity(EntityDescriptor {
                type_name: "player".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![],
                doc: None,
                meta: Default::default(),
            }),
            TypeDescriptor::Relation(RelationDescriptor {
                type_name: "rel".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![],
                roles: vec![
                    RoleDescriptor {
                        role_name: "definition".into(),
                        player_type_names: vec![],
                        cardinality: None,
                        ..Default::default()
                    },
                    RoleDescriptor {
                        role_name: "actor".into(),
                        player_type_names: vec!["player".into()],
                        cardinality: None,
                        ..Default::default()
                    },
                ],
                doc: None,
                meta: Default::default(),
            }),
        ];
        let info = SchemaInfo::from_descriptors(&descriptors);

        let result = generate_define_block(&info);

        assert_eq!(
            result,
            "define entity player; relation rel, relates definition, relates actor; player, plays rel:actor;"
        );
    }

    #[test]
    fn relates_only_role_survives_schema_info_serde_roundtrip() {
        let descriptors = vec![
            TypeDescriptor::Entity(EntityDescriptor {
                type_name: "player".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![],
                doc: None,
                meta: Default::default(),
            }),
            TypeDescriptor::Relation(RelationDescriptor {
                type_name: "rel".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![],
                roles: vec![
                    RoleDescriptor {
                        role_name: "definition".into(),
                        player_type_names: vec![],
                        cardinality: None,
                        ..Default::default()
                    },
                    RoleDescriptor {
                        role_name: "actor".into(),
                        player_type_names: vec!["player".into()],
                        cardinality: None,
                        ..Default::default()
                    },
                ],
                doc: None,
                meta: Default::default(),
            }),
        ];
        let info = SchemaInfo::from_descriptors(&descriptors);
        let json = serde_json::to_string(&info).unwrap();
        let parsed: SchemaInfo = serde_json::from_str(&json).unwrap();

        let result = generate_define_block(&parsed);

        assert_eq!(
            result,
            "define entity player; relation rel, relates definition, relates actor; player, plays rel:actor;"
        );
    }

    /// When plays-cardinality is present the emitter appends `@card(min..max)`.
    #[test]
    fn plays_card_emitted_when_present() {
        let mut info = SchemaInfo::default();
        let mut plays_cards = BTreeMap::new();
        plays_cards.insert("r:role".to_string(), (0u32, Some(1u32)));

        info.entities.insert(
            "player".into(),
            EntitySchemaEntry {
                type_name: "player".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![],
                plays_cardinalities: plays_cards,
                doc: None,
                meta: Default::default(),
            },
        );
        info.relations.insert(
            "r".into(),
            RelationSchemaEntry {
                type_name: "r".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![],
                roles: vec![RoleEntry {
                    role_name: "role".into(),
                    player_type_names: vec!["player".into()],
                    ..Default::default()
                }],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );

        let result = generate_define_block(&info);
        assert!(
            result.contains("player, plays r:role @card(0..1);"),
            "expected plays @card annotation, got:\n{result}"
        );
    }

    /// When no plays-card is present, the emitted plays clause has no annotation.
    #[test]
    fn plays_clause_has_no_card_when_absent() {
        let mut info = SchemaInfo::default();
        info.entities.insert(
            "person".into(),
            EntitySchemaEntry {
                type_name: "person".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );
        info.relations.insert(
            "employment".into(),
            RelationSchemaEntry {
                type_name: "employment".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![],
                roles: vec![RoleEntry {
                    role_name: "employee".into(),
                    player_type_names: vec!["person".into()],
                    ..Default::default()
                }],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );

        let result = generate_define_block(&info);
        // Assert full line, not just contains — no trailing @card
        assert!(
            result.contains("person, plays employment:employee;"),
            "expected bare plays line, got:\n{result}"
        );
    }

    /// Relates-side `@card` and plays-side `@card` coexist independently.
    #[test]
    fn relates_card_and_plays_card_coexist() {
        let mut info = SchemaInfo::default();
        let mut plays_cards = BTreeMap::new();
        plays_cards.insert("employment:employee".to_string(), (0u32, Some(1u32)));

        info.entities.insert(
            "person".into(),
            EntitySchemaEntry {
                type_name: "person".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![],
                plays_cardinalities: plays_cards,
                doc: None,
                meta: Default::default(),
            },
        );
        info.relations.insert(
            "employment".into(),
            RelationSchemaEntry {
                type_name: "employment".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![],
                roles: vec![RoleEntry {
                    role_name: "employee".into(),
                    player_type_names: vec!["person".into()],
                    cardinality: Some((1, Some(1))),
                    ..Default::default()
                }],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );

        let result = generate_define_block(&info);
        assert!(
            result.contains("relates employee @card(1..1)"),
            "expected relates @card(1..1), got:\n{result}"
        );
        assert!(
            result.contains("person, plays employment:employee @card(0..1);"),
            "expected plays @card(0..1), got:\n{result}"
        );
    }

    /// A relation that plays a role in another relation emits `@card` correctly.
    #[test]
    fn relation_as_player_emits_plays_card() {
        let mut info = SchemaInfo::default();
        let mut plays_cards = BTreeMap::new();
        plays_cards.insert("a:role".to_string(), (1u32, Some(1u32)));

        info.relations.insert(
            "a".into(),
            RelationSchemaEntry {
                type_name: "a".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![],
                roles: vec![RoleEntry {
                    role_name: "role".into(),
                    player_type_names: vec!["b".into()],
                    ..Default::default()
                }],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );
        info.relations.insert(
            "b".into(),
            RelationSchemaEntry {
                type_name: "b".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![],
                roles: vec![],
                plays_cardinalities: plays_cards,
                doc: None,
                meta: Default::default(),
            },
        );

        let result = generate_define_block(&info);
        assert!(
            result.contains("b, plays a:role @card(1..1);"),
            "expected relation-as-player plays @card, got:\n{result}"
        );
    }

    /// Unbounded cardinality `(0, None)` renders as `@card(0..)` (TypeDB rejects `*`).
    #[test]
    fn plays_card_unbounded_renders_open_upper_bound() {
        let mut info = SchemaInfo::default();
        let mut plays_cards = BTreeMap::new();
        plays_cards.insert("r:slot".to_string(), (0u32, None));

        info.entities.insert(
            "thing".into(),
            EntitySchemaEntry {
                type_name: "thing".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![],
                plays_cardinalities: plays_cards,
                doc: None,
                meta: Default::default(),
            },
        );
        info.relations.insert(
            "r".into(),
            RelationSchemaEntry {
                type_name: "r".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![],
                roles: vec![RoleEntry {
                    role_name: "slot".into(),
                    player_type_names: vec!["thing".into()],
                    ..Default::default()
                }],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );

        let result = generate_define_block(&info);
        assert!(
            result.contains("thing, plays r:slot @card(0..);"),
            "expected @card(0..), got:\n{result}"
        );
    }

    /// A repeated role (same name twice) whose player carries plays-card must
    /// still emit the annotated plays line exactly once — the sort+dedup over
    /// `PlaysClause` collapses the duplicate without dropping the `@card`.
    #[test]
    fn plays_card_deduplicated_when_role_repeated() {
        let mut info = SchemaInfo::default();
        let mut plays_cards = BTreeMap::new();
        plays_cards.insert("friendship:friend".to_string(), (0u32, Some(5u32)));
        info.entities.insert(
            "person".into(),
            EntitySchemaEntry {
                type_name: "person".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![],
                plays_cardinalities: plays_cards,
                doc: None,
                meta: Default::default(),
            },
        );
        info.relations.insert(
            "friendship".into(),
            RelationSchemaEntry {
                type_name: "friendship".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![],
                roles: vec![
                    RoleEntry {
                        role_name: "friend".into(),
                        player_type_names: vec!["person".into()],
                        ..Default::default()
                    },
                    RoleEntry {
                        role_name: "friend".into(),
                        player_type_names: vec!["person".into()],
                        ..Default::default()
                    },
                ],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );

        let result = generate_define_block(&info);
        let count = result
            .matches("person, plays friendship:friend @card(0..5);")
            .count();
        assert_eq!(
            count, 1,
            "annotated plays line must dedupe to one:\n{result}"
        );
    }

    /// Serde backward-compatibility: a role entry without the new keys
    /// deserializes correctly (defaults take over), and the round-tripped
    /// serialized form includes both `overrides` and `is_abstract`.
    #[test]
    fn role_descriptor_serde_defaults_on_missing_keys() {
        // Old-format role JSON without `overrides` or `is_abstract`.
        let json = r#"{
            "role_name": "participant",
            "player_type_names": ["person"],
            "cardinality": null
        }"#;
        let role: RoleDescriptor = serde_json::from_str(json).expect("deserialize failed");
        assert_eq!(role.role_name, "participant");
        assert!(role.overrides.is_none());
        assert!(!role.is_abstract);

        // Serialized form must include the new keys.
        let serialized = serde_json::to_string(&role).expect("serialize failed");
        assert!(
            serialized.contains("\"overrides\":null"),
            "serialized role must include overrides: {serialized}"
        );
        assert!(
            serialized.contains("\"is_abstract\":false"),
            "serialized role must include is_abstract: {serialized}"
        );
    }

    /// Specializing role emits `relates author as contributor`.
    #[test]
    fn specializing_role_emits_as_clause() {
        let mut info = SchemaInfo::default();
        info.relations.insert(
            "contribution".into(),
            RelationSchemaEntry {
                type_name: "contribution".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![],
                roles: vec![RoleEntry {
                    role_name: "contributor".into(),
                    player_type_names: vec!["person".into()],
                    ..Default::default()
                }],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );
        info.relations.insert(
            "authoring".into(),
            RelationSchemaEntry {
                type_name: "authoring".into(),
                is_abstract: false,
                parent_type: Some("contribution".into()),
                owned_attributes: vec![],
                roles: vec![
                    // Inherited role (will be skipped by the generator)
                    RoleEntry {
                        role_name: "contributor".into(),
                        player_type_names: vec!["person".into()],
                        ..Default::default()
                    },
                    // Own specializing role
                    RoleEntry {
                        role_name: "author".into(),
                        player_type_names: vec!["author-entity".into()],
                        overrides: Some("contributor".into()),
                        ..Default::default()
                    },
                ],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );

        let result = generate_define_block(&info);
        assert!(
            result.contains("relates author as contributor"),
            "specializing role must emit 'as contributor': {result}"
        );
        // Inherited role must not be re-emitted on the child.
        let authoring_section = result
            .split(';')
            .find(|part| part.contains("relation authoring"))
            .expect("authoring declaration");
        assert!(
            !authoring_section.contains("relates contributor"),
            "inherited role must not be re-emitted: {authoring_section}"
        );
    }

    #[test]
    fn ordered_specialized_role_marks_both_labels_and_parses_as_typeql_3_12() {
        let mut info = SchemaInfo::default();
        info.relations.insert(
            "contribution".into(),
            RelationSchemaEntry {
                type_name: "contribution".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![],
                roles: vec![RoleEntry {
                    role_name: "contributor".into(),
                    ordered: true,
                    ..Default::default()
                }],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );
        info.relations.insert(
            "authoring".into(),
            RelationSchemaEntry {
                type_name: "authoring".into(),
                is_abstract: false,
                parent_type: Some("contribution".into()),
                owned_attributes: vec![],
                roles: vec![RoleEntry {
                    role_name: "author".into(),
                    overrides: Some("contributor".into()),
                    ordered: true,
                    ..Default::default()
                }],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );

        let emitted = generate_define_block(&info);
        assert_eq!(
            emitted,
            concat!(
                "define ",
                "relation contribution, ",
                "relates contributor[]; ",
                "relation authoring, sub contribution, ",
                "relates author[] as contributor[];",
            )
        );
        typeql::parse_query(&emitted).expect("TypeQL 3.12 accepts ordered specialization");

        let parsed = SchemaInfo::from_typeql(&emitted)
            .expect("the schema compatibility parser accepts ordered specialization");
        let author = &parsed.relations["authoring"].roles[0];
        assert!(author.ordered);
        assert_eq!(author.overrides.as_deref(), Some("contributor"));
    }

    /// Abstract own role emits `relates participant @abstract`.
    #[test]
    fn abstract_role_emits_abstract_annotation() {
        let mut info = SchemaInfo::default();
        info.relations.insert(
            "presence".into(),
            RelationSchemaEntry {
                type_name: "presence".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![],
                roles: vec![RoleEntry {
                    role_name: "participant".into(),
                    player_type_names: vec![],
                    is_abstract: true,
                    ..Default::default()
                }],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );

        let result = generate_define_block(&info);
        assert!(
            result.contains("relates participant @abstract"),
            "abstract role must include @abstract annotation: {result}"
        );
    }

    /// Combined: `relates worker as assignee @abstract @card(0..1)` matches canon exactly.
    #[test]
    fn combined_overrides_abstract_card_role_matches_canon() {
        let mut info = SchemaInfo::default();
        // Parent relation declaring the base role.
        info.relations.insert(
            "task".into(),
            RelationSchemaEntry {
                type_name: "task".into(),
                is_abstract: true,
                parent_type: None,
                owned_attributes: vec![],
                roles: vec![RoleEntry {
                    role_name: "assignee".into(),
                    player_type_names: vec![],
                    ..Default::default()
                }],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );
        // Child relation specializing the role.
        info.relations.insert(
            "restricted-task".into(),
            RelationSchemaEntry {
                type_name: "restricted-task".into(),
                is_abstract: false,
                parent_type: Some("task".into()),
                owned_attributes: vec![],
                roles: vec![
                    // Inherited (will be skipped)
                    RoleEntry {
                        role_name: "assignee".into(),
                        player_type_names: vec![],
                        ..Default::default()
                    },
                    // Own specializing + abstract + card
                    RoleEntry {
                        role_name: "worker".into(),
                        player_type_names: vec![],
                        overrides: Some("assignee".into()),
                        is_abstract: true,
                        cardinality: Some((0, Some(1))),
                        ..Default::default()
                    },
                ],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );

        let result = generate_define_block(&info);
        assert!(
            result.contains("relates worker as assignee @abstract @card(0..1)"),
            "combined role must match canon exactly: {result}"
        );
    }

    /// Emit→parse round trip: `generate_define_block` output feeds back through the
    /// core parser and produces the same `overrides`/`is_abstract`/cardinality values.
    #[test]
    fn specializing_role_emit_parse_roundtrip() {
        use crate::_schema::info::SchemaInfo;

        let mut info = SchemaInfo::default();
        info.relations.insert(
            "base-rel".into(),
            RelationSchemaEntry {
                type_name: "base-rel".into(),
                is_abstract: true,
                parent_type: None,
                owned_attributes: vec![],
                roles: vec![RoleEntry {
                    role_name: "contributor".into(),
                    player_type_names: vec![],
                    ..Default::default()
                }],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );
        info.relations.insert(
            "derived-rel".into(),
            RelationSchemaEntry {
                type_name: "derived-rel".into(),
                is_abstract: false,
                parent_type: Some("base-rel".into()),
                owned_attributes: vec![],
                roles: vec![
                    RoleEntry {
                        role_name: "contributor".into(),
                        player_type_names: vec![],
                        ..Default::default()
                    },
                    RoleEntry {
                        role_name: "author".into(),
                        player_type_names: vec![],
                        overrides: Some("contributor".into()),
                        cardinality: Some((1, Some(1))),
                        ..Default::default()
                    },
                ],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );

        let emitted = generate_define_block(&info);
        // Parse the emitted block back through the core schema parser.
        let parsed = SchemaInfo::from_typeql(&emitted).expect("round-trip parse failed");

        let derived = parsed
            .relations
            .get("derived-rel")
            .expect("derived-rel must be present");
        let author_role = derived
            .roles
            .iter()
            .find(|r| r.role_name == "author")
            .expect("author role must survive round-trip");

        assert_eq!(
            author_role.overrides.as_deref(),
            Some("contributor"),
            "overrides must survive emit→parse"
        );
        assert_eq!(
            author_role.cardinality,
            Some((1, Some(1))),
            "cardinality must survive emit→parse"
        );
    }

    /// Ordered `owns name[]` emits the `[]` suffix on the attribute name token.
    #[test]
    fn ordered_owns_emits_list_marker() {
        let mut info = SchemaInfo::default();
        info.entities.insert(
            "person".into(),
            EntitySchemaEntry {
                type_name: "person".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![OwnedAttributeEntry {
                    attr_name: "tag".into(),
                    value_type: ValueType::String,
                    annotations: vec![],
                    is_ordered: true,
                    doc: None,
                    meta: Default::default(),
                }],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );

        let result = generate_define_block(&info);
        assert!(
            result.contains("owns tag[]"),
            "ordered attr must emit [] marker: {result}"
        );
    }

    /// `@distinct` annotation emits correctly on an ordered owns clause.
    #[test]
    fn distinct_annotation_emits_on_ordered_owns() {
        let mut info = SchemaInfo::default();
        info.entities.insert(
            "person".into(),
            EntitySchemaEntry {
                type_name: "person".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![OwnedAttributeEntry {
                    attr_name: "nickname".into(),
                    value_type: ValueType::String,
                    annotations: vec![Annotation::Distinct, Annotation::Card(0, Some(3))],
                    is_ordered: true,
                    doc: None,
                    meta: Default::default(),
                }],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );

        let result = generate_define_block(&info);
        assert!(
            result.contains("owns nickname[] @distinct @card(0..3)"),
            "ordered distinct attr must match canon exactly: {result}"
        );
    }

    /// Ordered `relates member[]` emits the `[]` suffix on the role name token.
    #[test]
    fn ordered_role_emits_list_marker() {
        let mut info = SchemaInfo::default();
        info.relations.insert(
            "feed".into(),
            RelationSchemaEntry {
                type_name: "feed".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![],
                roles: vec![RoleEntry {
                    role_name: "member".into(),
                    player_type_names: vec!["item".into()],
                    ordered: true,
                    ..Default::default()
                }],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );

        let result = generate_define_block(&info);
        assert!(
            result.contains("relates member[]"),
            "ordered role must emit [] marker: {result}"
        );
    }

    /// `@distinct` on a role emits after `@abstract` per the emission canon.
    #[test]
    fn distinct_role_emits_after_abstract() {
        let mut info = SchemaInfo::default();
        info.relations.insert(
            "collection".into(),
            RelationSchemaEntry {
                type_name: "collection".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![],
                roles: vec![RoleEntry {
                    role_name: "member".into(),
                    player_type_names: vec![],
                    is_abstract: true,
                    distinct: true,
                    ..Default::default()
                }],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );

        let result = generate_define_block(&info);
        assert!(
            result.contains("relates member @abstract @distinct"),
            "distinct role must emit @abstract @distinct in order: {result}"
        );
    }

    /// Emit→parse round trip for `owns nickname[] @distinct @card(0..3)`:
    /// the generated TypeQL parses back to identical IR flags.
    #[test]
    fn list_owns_emit_parse_roundtrip() {
        use crate::_schema::info::SchemaInfo;

        let mut info = SchemaInfo::default();
        info.attributes.insert(
            "nickname".into(),
            AttributeSchemaEntry::new("nickname", ValueType::String),
        );
        info.attributes.insert(
            "plain".into(),
            AttributeSchemaEntry::new("plain", ValueType::String),
        );
        info.entities.insert(
            "person".into(),
            EntitySchemaEntry {
                type_name: "person".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![
                    OwnedAttributeEntry {
                        attr_name: "nickname".into(),
                        value_type: ValueType::String,
                        annotations: vec![Annotation::Distinct, Annotation::Card(0, Some(3))],
                        is_ordered: true,
                        doc: None,
                        meta: Default::default(),
                    },
                    OwnedAttributeEntry {
                        attr_name: "plain".into(),
                        value_type: ValueType::String,
                        annotations: vec![],
                        is_ordered: false,
                        doc: None,
                        meta: Default::default(),
                    },
                ],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );

        let emitted = generate_define_block(&info);
        let parsed = SchemaInfo::from_typeql(&emitted).expect("list-owns round-trip parse failed");

        let person = parsed
            .entities
            .get("person")
            .expect("person entity must survive");
        let nick = person
            .owned_attributes
            .iter()
            .find(|a| a.attr_name == "nickname")
            .expect("nickname must survive round-trip");
        let plain = person
            .owned_attributes
            .iter()
            .find(|a| a.attr_name == "plain")
            .expect("plain must survive round-trip");

        assert!(nick.is_ordered, "nickname must be ordered after round-trip");
        assert!(
            nick.annotations.contains(&Annotation::Distinct),
            "nickname must carry @distinct after round-trip"
        );
        assert_eq!(
            nick.annotations
                .iter()
                .find(|a| matches!(a, Annotation::Card(..))),
            Some(&Annotation::Card(0, Some(3))),
            "nickname must carry @card(0..3) after round-trip"
        );
        assert!(!plain.is_ordered, "plain must not be ordered");
    }

    /// Emit→parse round trip for `relates member[] @distinct`:
    /// the generated TypeQL parses back to identical IR flags.
    #[test]
    fn list_role_emit_parse_roundtrip() {
        use crate::_schema::info::SchemaInfo;

        let mut info = SchemaInfo::default();
        info.relations.insert(
            "feed".into(),
            RelationSchemaEntry {
                type_name: "feed".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![],
                roles: vec![RoleEntry {
                    role_name: "member".into(),
                    player_type_names: vec![],
                    ordered: true,
                    distinct: true,
                    ..Default::default()
                }],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );

        let emitted = generate_define_block(&info);
        let parsed = SchemaInfo::from_typeql(&emitted).expect("list-role round-trip parse failed");

        let feed = parsed
            .relations
            .get("feed")
            .expect("feed relation must survive");
        let member = feed
            .roles
            .iter()
            .find(|r| r.role_name == "member")
            .expect("member role must survive round-trip");

        assert!(member.ordered, "member must be ordered after round-trip");
        assert!(member.distinct, "member must be distinct after round-trip");
    }

    /// `RoleDescriptor` serde: new fields `ordered` and `distinct` appear in
    /// serialized form with default values even when absent in source JSON.
    #[test]
    fn role_descriptor_serde_includes_ordered_and_distinct() {
        // Old-format role JSON without the new keys.
        let json = r#"{
            "role_name": "participant",
            "player_type_names": ["person"],
            "cardinality": null
        }"#;
        let role: RoleDescriptor = serde_json::from_str(json).expect("deserialize failed");
        assert!(!role.ordered, "ordered must default to false");
        assert!(!role.distinct, "distinct must default to false");

        let serialized = serde_json::to_string(&role).expect("serialize failed");
        assert!(
            serialized.contains("\"ordered\":false"),
            "serialized role must include ordered: {serialized}"
        );
        assert!(
            serialized.contains("\"distinct\":false"),
            "serialized role must include distinct: {serialized}"
        );
    }

    /// `OwnedAttributeDescriptor` serde: new field `is_ordered` appears in
    /// serialized form with default value even when absent in source JSON.
    #[test]
    fn owned_attr_descriptor_serde_includes_is_ordered() {
        let json = r#"{
            "field_name": "tag",
            "attr_name": "tag",
            "value_type": "string",
            "annotations": [],
            "is_optional": false
        }"#;
        use crate::_descriptor::OwnedAttributeDescriptor;
        let attr: OwnedAttributeDescriptor =
            serde_json::from_str(json).expect("deserialize failed");
        assert!(!attr.is_ordered, "is_ordered must default to false");

        let serialized = serde_json::to_string(&attr).expect("serialize failed");
        assert!(
            serialized.contains("\"is_ordered\":false"),
            "serialized attr descriptor must include is_ordered: {serialized}"
        );
    }

    /// `SchemaInfo::from_descriptors` carries `ordered`, `distinct`, and
    /// `is_ordered` from descriptors into IR entries.
    #[test]
    fn from_descriptors_carries_list_interface_flags() {
        use crate::_descriptor::{
            EntityDescriptor, OwnedAttributeDescriptor, RelationDescriptor, RoleDescriptor,
            TypeDescriptor,
        };

        let descriptors = vec![
            TypeDescriptor::Entity(EntityDescriptor {
                type_name: "person".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![OwnedAttributeDescriptor {
                    field_name: "tag".into(),
                    attr_name: "tag".into(),
                    value_type: ValueType::String,
                    annotations: vec![Annotation::Distinct],
                    is_optional: false,
                    is_ordered: true,
                    doc: None,
                    meta: Default::default(),
                }],
                doc: None,
                meta: Default::default(),
            }),
            TypeDescriptor::Relation(RelationDescriptor {
                type_name: "feed".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![],
                roles: vec![RoleDescriptor {
                    role_name: "member".into(),
                    player_type_names: vec!["person".into()],
                    ordered: true,
                    distinct: true,
                    ..Default::default()
                }],
                doc: None,
                meta: Default::default(),
            }),
        ];

        let info = SchemaInfo::from_descriptors(&descriptors);

        let person = info.entities.get("person").expect("person must be present");
        let tag = &person.owned_attributes[0];
        assert!(tag.is_ordered, "tag must be ordered in IR");
        assert!(
            tag.annotations.contains(&Annotation::Distinct),
            "tag must carry @distinct in IR"
        );

        let feed = info.relations.get("feed").expect("feed must be present");
        let member = &feed.roles[0];
        assert!(member.ordered, "member role must be ordered in IR");
        assert!(member.distinct, "member role must be distinct in IR");
    }

    #[test]
    fn generates_doc_meta_on_entity_header() {
        let mut info = SchemaInfo::default();
        info.entities.insert(
            "person".into(),
            EntitySchemaEntry {
                type_name: "person".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![],
                plays_cardinalities: BTreeMap::new(),
                doc: Some("an individual client".into()),
                meta: BTreeMap::from([("icon".to_string(), "silhouette.png".to_string())]),
            },
        );

        let result = generate_define_block(&info);
        assert!(result.contains(
            "entity person @doc(\"an individual client\") @meta(\"icon\", \"silhouette.png\");"
        ));
    }

    #[test]
    fn generates_doc_meta_header_before_sub_with_comma() {
        // Annotated headers use the same `, sub` comma rule as @abstract.
        let mut info = SchemaInfo::default();
        info.entities.insert(
            "base".into(),
            EntitySchemaEntry {
                type_name: "base".into(),
                is_abstract: true,
                parent_type: None,
                owned_attributes: vec![],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: BTreeMap::new(),
            },
        );
        info.entities.insert(
            "child".into(),
            EntitySchemaEntry {
                type_name: "child".into(),
                is_abstract: false,
                parent_type: Some("base".into()),
                owned_attributes: vec![],
                plays_cardinalities: BTreeMap::new(),
                doc: Some("d".into()),
                meta: BTreeMap::new(),
            },
        );

        let result = generate_define_block(&info);
        assert!(result.contains("entity child @doc(\"d\"), sub base;"));
    }

    #[test]
    fn generates_doc_meta_on_attribute_definition() {
        let mut info = SchemaInfo::default();
        let mut attr = AttributeSchemaEntry::new("name", ValueType::String);
        attr.doc = Some("a personal name".into());
        attr.meta.insert("pii".into(), "true".into());
        info.attributes.insert("name".into(), attr);

        let result = generate_define_block(&info);
        assert!(result.contains(
            "attribute name @doc(\"a personal name\") @meta(\"pii\", \"true\"), value string;"
        ));
    }

    #[test]
    fn generates_doc_meta_on_owns_after_constraints() {
        let mut info = SchemaInfo::default();
        info.attributes.insert(
            "name".into(),
            AttributeSchemaEntry::new("name", ValueType::String),
        );
        info.entities.insert(
            "person".into(),
            EntitySchemaEntry {
                type_name: "person".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![OwnedAttributeEntry {
                    attr_name: "name".into(),
                    value_type: ValueType::String,
                    annotations: vec![Annotation::Key],
                    is_ordered: false,
                    doc: Some("full legal name".into()),
                    meta: BTreeMap::from([("column".to_string(), "name".to_string())]),
                }],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: BTreeMap::new(),
            },
        );

        let result = generate_define_block(&info);
        assert!(
            result
                .contains("owns name @key @doc(\"full legal name\") @meta(\"column\", \"name\");")
        );
    }

    #[test]
    fn generates_doc_meta_on_relates_after_card() {
        let mut info = SchemaInfo::default();
        info.relations.insert(
            "friendship".into(),
            RelationSchemaEntry {
                type_name: "friendship".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![],
                roles: vec![super::super::info::RoleEntry {
                    role_name: "friend".into(),
                    player_type_names: vec![],
                    cardinality: Some((0, Some(2))),
                    overrides: None,
                    is_abstract: false,
                    ordered: false,
                    distinct: false,
                    doc: Some("one side of the bond".into()),
                    meta: BTreeMap::from([("endpoint".to_string(), "true".to_string())]),
                }],
                plays_cardinalities: BTreeMap::new(),
                doc: Some("a mutual bond".into()),
                meta: BTreeMap::new(),
            },
        );

        let result = generate_define_block(&info);
        assert!(result.contains("relation friendship @doc(\"a mutual bond\"),"));
        assert!(result.contains(
            "relates friend @card(0..2) @doc(\"one side of the bond\") @meta(\"endpoint\", \"true\");"
        ));
    }

    #[test]
    fn doc_escapes_round_trip_through_emission() {
        let mut info = SchemaInfo::default();
        info.entities.insert(
            "escbox".into(),
            EntitySchemaEntry {
                type_name: "escbox".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![],
                plays_cardinalities: BTreeMap::new(),
                doc: Some("line1\nline2 with \"quotes\" and back\\slash".into()),
                meta: BTreeMap::new(),
            },
        );

        let result = generate_define_block(&info);
        // Emitted exactly as TypeDB's export renders escapes.
        assert!(result.contains(
            "entity escbox @doc(\"line1\\nline2 with \\\"quotes\\\" and back\\\\slash\");"
        ));
    }

    #[test]
    fn from_typeql_to_define_round_trips_annotations() {
        let source = "define\n\nattribute name @doc(\"a personal name\") @meta(\"pii\", \"true\"), value string;\n\nentity person @doc(\"an individual\"),\n    owns name @key @doc(\"full name\") @meta(\"column\", \"name\");\n";
        let info = SchemaInfo::from_typeql(source).expect("parse");
        let regenerated = generate_define_block(&info);
        assert_eq!(
            typeql::parse_query(&regenerated).unwrap(),
            typeql::parse_query(&typeql::parse_query(source).unwrap().to_string()).unwrap()
        );
    }
}
