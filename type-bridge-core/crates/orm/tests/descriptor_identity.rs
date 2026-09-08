//! Deterministic descriptor identity and request-relevant fingerprint coverage.

use type_bridge_orm::_registry::DescriptorFingerprintRoot;
use type_bridge_orm::*;

#[path = "support/internal.rs"]
mod internal;
use internal::*;

fn attribute(field_name: &str, attr_name: &str, value_type: ValueType) -> OwnedAttributeDescriptor {
    OwnedAttributeDescriptor {
        field_name: field_name.to_string(),
        attr_name: attr_name.to_string(),
        value_type,
        annotations: vec![],
        is_optional: false,
        is_ordered: false,
        doc: None,
        meta: Default::default(),
    }
}

fn entity(
    type_name: &str,
    parent_type: Option<&str>,
    owned_attributes: Vec<OwnedAttributeDescriptor>,
) -> EntityDescriptor {
    EntityDescriptor {
        type_name: type_name.to_string(),
        is_abstract: type_name == "person",
        parent_type: parent_type.map(str::to_string),
        owned_attributes,
        doc: None,
        meta: Default::default(),
    }
}

fn employment(employee_cardinality: Option<(u32, Option<u32>)>) -> RelationDescriptor {
    RelationDescriptor {
        type_name: "employment".to_string(),
        is_abstract: false,
        parent_type: None,
        owned_attributes: vec![attribute(
            "position",
            "employment-position",
            ValueType::String,
        )],
        roles: vec![
            RoleDescriptor {
                role_name: "employee".to_string(),
                player_type_names: vec!["person".to_string()],
                cardinality: employee_cardinality,
                ..Default::default()
            },
            RoleDescriptor {
                role_name: "employer".to_string(),
                player_type_names: vec!["company".to_string()],
                cardinality: Some((1, Some(1))),
                ..Default::default()
            },
        ],
        doc: None,
        meta: Default::default(),
    }
}

fn register_employment_graph(registry: &DescriptorRegistry, reverse: bool) {
    let person = entity(
        "person",
        None,
        vec![OwnedAttributeDescriptor {
            annotations: vec![Annotation::Key],
            ..attribute("name", "person-name", ValueType::String)
        }],
    );
    let company = entity(
        "company",
        None,
        vec![attribute("name", "company-name", ValueType::String)],
    );
    let relation = employment(Some((1, Some(1))));

    if reverse {
        registry.register_relation(relation).unwrap();
        registry.register_entity(company).unwrap();
        registry.register_entity(person).unwrap();
    } else {
        registry.register_entity(person).unwrap();
        registry.register_entity(company).unwrap();
        registry.register_relation(relation).unwrap();
    }
}

fn root(
    registry: &DescriptorRegistry,
    type_name: &str,
    include_subtypes: bool,
) -> DescriptorFingerprintRoot {
    DescriptorFingerprintRoot::new(
        registry
            .descriptor_id(type_name)
            .unwrap_or_else(|| panic!("missing descriptor {type_name}")),
        include_subtypes,
    )
}

#[test]
fn canonical_ids_and_snapshots_are_kind_and_owner_qualified() {
    let first = DescriptorRegistry::new();
    let second = DescriptorRegistry::new();
    register_employment_graph(&first, false);
    register_employment_graph(&second, true);

    let person_id = first.descriptor_id("person").unwrap();
    let employment_id = first.descriptor_id("employment").unwrap();
    assert_eq!(person_id.as_str(), "entity:person");
    assert_eq!(employment_id.as_str(), "relation:employment");

    let name_id = first.field_id(&person_id, "name").unwrap();
    assert_eq!(name_id.owner, person_id);
    assert_eq!(name_id.name, "name");
    assert_eq!(
        first
            .field_id(&first.descriptor_id("company").unwrap(), "company-name")
            .unwrap()
            .name,
        "name"
    );

    let employee_id = first.role_id(&employment_id, "employee").unwrap();
    assert_eq!(employee_id.owner, employment_id);
    assert_eq!(employee_id.name, "employee");
    assert!(
        first
            .role_id(&first.descriptor_id("person").unwrap(), "employee")
            .is_none()
    );

    assert_eq!(
        first.identity_snapshot().unwrap(),
        second.identity_snapshot().unwrap()
    );
    assert_eq!(
        first.schema_fingerprint().unwrap(),
        second.schema_fingerprint().unwrap()
    );
}

#[test]
fn subtype_inclusion_is_explicit_and_detects_new_registered_subtypes() {
    let registry = DescriptorRegistry::new();
    registry
        .register_entity(entity(
            "person",
            None,
            vec![attribute("name", "person-name", ValueType::String)],
        ))
        .unwrap();

    let exact_before = registry
        .request_relevant_fingerprint(&[root(&registry, "person", false)])
        .unwrap();
    let polymorphic_before = registry
        .request_relevant_fingerprint(&[root(&registry, "person", true)])
        .unwrap();

    registry
        .register_entity(entity(
            "employee",
            Some("person"),
            vec![attribute("level", "employee-level", ValueType::Long)],
        ))
        .unwrap();

    let exact_after = registry
        .request_relevant_fingerprint(&[root(&registry, "person", false)])
        .unwrap();
    let polymorphic_after = registry
        .request_relevant_fingerprint(&[root(&registry, "person", true)])
        .unwrap();

    assert_eq!(exact_before, exact_after);
    assert_ne!(polymorphic_before, polymorphic_after);
}

#[test]
fn ancestors_fields_roles_and_compatible_players_affect_closure_fingerprints() {
    let parent_string = DescriptorRegistry::new();
    let parent_long = DescriptorRegistry::new();
    for (registry, value_type) in [
        (&parent_string, ValueType::String),
        (&parent_long, ValueType::Long),
    ] {
        registry
            .register_entity(entity(
                "person",
                None,
                vec![attribute("name", "person-name", value_type)],
            ))
            .unwrap();
        registry
            .register_entity(entity(
                "employee",
                Some("person"),
                vec![attribute("level", "employee-level", ValueType::Long)],
            ))
            .unwrap();
    }
    assert_ne!(
        parent_string
            .request_relevant_fingerprint(&[root(&parent_string, "employee", false)])
            .unwrap(),
        parent_long
            .request_relevant_fingerprint(&[root(&parent_long, "employee", false)])
            .unwrap()
    );

    let role_one = DescriptorRegistry::new();
    let role_many = DescriptorRegistry::new();
    for registry in [&role_one, &role_many] {
        registry
            .register_entity(entity(
                "person",
                None,
                vec![attribute("name", "person-name", ValueType::String)],
            ))
            .unwrap();
        registry
            .register_entity(entity(
                "company",
                None,
                vec![attribute("name", "company-name", ValueType::String)],
            ))
            .unwrap();
    }
    role_one
        .register_relation(employment(Some((1, Some(1)))))
        .unwrap();
    role_many
        .register_relation(employment(Some((1, None))))
        .unwrap();

    assert_ne!(
        role_one
            .request_relevant_fingerprint(&[root(&role_one, "employment", false)])
            .unwrap(),
        role_many
            .request_relevant_fingerprint(&[root(&role_many, "employment", false)])
            .unwrap()
    );

    let compatible_before = role_one
        .request_relevant_fingerprint(&[root(&role_one, "employment", false)])
        .unwrap();
    role_one
        .register_entity(entity(
            "contractor",
            Some("person"),
            vec![attribute("rate", "contractor-rate", ValueType::Decimal)],
        ))
        .unwrap();
    let with_compatible_subtype = role_one
        .request_relevant_fingerprint(&[root(&role_one, "employment", false)])
        .unwrap();
    assert_ne!(compatible_before, with_compatible_subtype);
}

#[test]
fn unrelated_registration_does_not_stale_request_closure() {
    let registry = DescriptorRegistry::new();
    register_employment_graph(&registry, false);

    let before = registry
        .request_relevant_fingerprint(&[root(&registry, "employment", false)])
        .unwrap();
    let full_before = registry.schema_fingerprint().unwrap();

    registry
        .register_entity(entity(
            "unrelated-skill",
            None,
            vec![attribute("label", "skill-label", ValueType::String)],
        ))
        .unwrap();

    let after = registry
        .request_relevant_fingerprint(&[root(&registry, "employment", false)])
        .unwrap();
    let full_after = registry.schema_fingerprint().unwrap();

    assert_eq!(before, after);
    assert_ne!(full_before, full_after);
    assert!(after.as_str().starts_with("schema-sha256-v1:"));
    assert_eq!(after.as_str().len(), "schema-sha256-v1:".len() + 64);
}

#[test]
fn incomplete_relevant_hierarchy_fails_closed() {
    let registry = DescriptorRegistry::new();
    registry
        .register_entity(entity(
            "employee",
            Some("missing-person"),
            vec![attribute("level", "employee-level", ValueType::Long)],
        ))
        .unwrap();

    assert!(matches!(
        registry.request_relevant_fingerprint(&[root(&registry, "employee", false)]),
        Err(OrmError::DescriptorNotFound(type_name)) if type_name == "missing-person"
    ));
}
