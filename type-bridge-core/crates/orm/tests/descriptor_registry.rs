//! Public API tests for runtime descriptors and registry behavior.

use std::sync::Arc;

use type_bridge_orm::*;

#[path = "support/internal.rs"]
mod internal;
use internal::*;

fn attr(name: &str, value_type: ValueType) -> OwnedAttributeDescriptor {
    OwnedAttributeDescriptor {
        field_name: name.replace('-', "_"),
        attr_name: name.to_string(),
        value_type,
        annotations: vec![],
        is_optional: false,
        is_ordered: false,
        doc: None,
        meta: Default::default(),
    }
}

fn person_descriptor() -> EntityDescriptor {
    EntityDescriptor {
        type_name: "person".into(),
        is_abstract: false,
        parent_type: None,
        owned_attributes: vec![
            OwnedAttributeDescriptor {
                field_name: "name".into(),
                attr_name: "name".into(),
                value_type: ValueType::String,
                annotations: vec![Annotation::Key],
                is_optional: false,
                is_ordered: false,
                doc: None,
                meta: Default::default(),
            },
            OwnedAttributeDescriptor {
                field_name: "email".into(),
                attr_name: "email".into(),
                value_type: ValueType::String,
                annotations: vec![Annotation::Unique, Annotation::Card(0, Some(1))],
                is_optional: true,
                is_ordered: false,
                doc: None,
                meta: Default::default(),
            },
        ],
        doc: None,
        meta: Default::default(),
    }
}

fn employment_descriptor() -> RelationDescriptor {
    RelationDescriptor {
        type_name: "employment".into(),
        is_abstract: false,
        parent_type: None,
        owned_attributes: vec![OwnedAttributeDescriptor {
            field_name: "position".into(),
            attr_name: "position".into(),
            value_type: ValueType::String,
            annotations: vec![],
            is_optional: true,
            is_ordered: false,
            doc: None,
            meta: Default::default(),
        }],
        roles: vec![
            RoleDescriptor {
                role_name: "employee".into(),
                player_type_names: vec!["person".into()],
                cardinality: Some((1, Some(1))),
                ..Default::default()
            },
            RoleDescriptor {
                role_name: "employer".into(),
                player_type_names: vec!["company".into()],
                cardinality: Some((1, Some(1))),
                ..Default::default()
            },
        ],
        doc: None,
        meta: Default::default(),
    }
}

#[test]
fn descriptor_serde_roundtrips_all_value_types() {
    let descriptor = EntityDescriptor {
        type_name: "all-values".into(),
        is_abstract: false,
        parent_type: Some("thing".into()),
        owned_attributes: vec![
            attr("string-value", ValueType::String),
            attr("long-value", ValueType::Long),
            attr("double-value", ValueType::Double),
            attr("boolean-value", ValueType::Boolean),
            attr("date-value", ValueType::Date),
            attr("datetime-value", ValueType::DateTime),
            attr("datetime-tz-value", ValueType::DateTimeTz),
            attr("decimal-value", ValueType::Decimal),
            attr("duration-value", ValueType::Duration),
        ],
        doc: None,
        meta: Default::default(),
    };

    let json = serde_json::to_string(&descriptor).unwrap();
    let parsed: EntityDescriptor = serde_json::from_str(&json).unwrap();
    let normalized = serde_json::to_string(&parsed).unwrap();

    assert_eq!(parsed, descriptor);
    assert_eq!(normalized, json);
}

#[test]
fn descriptor_helpers_find_keys_attributes_and_roles() {
    let entity = person_descriptor();
    assert_eq!(entity.key_attribute().unwrap().attr_name, "name");
    assert!(entity.attribute("name").unwrap().is_key());
    assert!(entity.attribute("email").unwrap().is_unique());
    assert_eq!(
        entity.attribute("email").unwrap().cardinality(),
        Some((0, Some(1)))
    );

    let relation = employment_descriptor();
    assert_eq!(
        relation.role("employee").unwrap().player_type_names,
        vec!["person"]
    );
    assert_eq!(
        relation.role("employee").unwrap().cardinality,
        Some((1, Some(1)))
    );
}

#[test]
fn registry_registration_is_standalone_and_idempotent() {
    let registry = DescriptorRegistry::new();
    let first = registry.register_entity(person_descriptor()).unwrap();
    let second = registry.register_entity(person_descriptor()).unwrap();

    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(registry.entity("person").unwrap().type_name, "person");
    assert!(matches!(
        registry.get("person"),
        Some(TypeDescriptorRef::Entity(_))
    ));
    assert_eq!(registry.snapshot().len(), 1);
}

#[test]
fn registry_rejects_kind_and_shape_conflicts() {
    let registry = DescriptorRegistry::new();
    registry.register_entity(person_descriptor()).unwrap();

    let mut changed = person_descriptor();
    changed.owned_attributes.push(attr("age", ValueType::Long));
    assert!(matches!(
        registry.register_entity(changed).unwrap_err(),
        OrmError::DescriptorConflict { .. }
    ));

    let mut relation = employment_descriptor();
    relation.type_name = "person".into();
    assert!(matches!(
        registry.register_relation(relation).unwrap_err(),
        OrmError::DescriptorConflict { .. }
    ));
}

#[test]
fn registry_rejects_duplicate_attributes_and_roles() {
    let registry = DescriptorRegistry::new();

    let mut entity = person_descriptor();
    entity.owned_attributes.push(OwnedAttributeDescriptor {
        field_name: "display_name".into(),
        attr_name: "name".into(),
        value_type: ValueType::String,
        annotations: vec![],
        is_optional: false,
        is_ordered: false,
        doc: None,
        meta: Default::default(),
    });
    assert!(matches!(
        registry.register_entity(entity).unwrap_err(),
        OrmError::DescriptorValidation { .. }
    ));

    let mut relation = employment_descriptor();
    relation.roles.push(RoleDescriptor {
        role_name: "employee".into(),
        player_type_names: vec!["person".into()],
        ..Default::default()
    });
    assert!(matches!(
        registry.register_relation(relation).unwrap_err(),
        OrmError::DescriptorValidation { .. }
    ));
}

#[test]
fn registry_rejects_cross_namespace_field_aliases_before_lookup() {
    let mut entity = person_descriptor();
    entity.owned_attributes = vec![
        OwnedAttributeDescriptor {
            field_name: "preferred_name".into(),
            attr_name: "name".into(),
            value_type: ValueType::String,
            annotations: vec![],
            is_optional: false,
            is_ordered: false,
            doc: None,
            meta: Default::default(),
        },
        OwnedAttributeDescriptor {
            field_name: "name".into(),
            attr_name: "legal-name".into(),
            value_type: ValueType::String,
            annotations: vec![],
            is_optional: false,
            is_ordered: false,
            doc: None,
            meta: Default::default(),
        },
    ];
    let mut reversed = entity.clone();
    reversed.owned_attributes.reverse();

    for descriptor in [entity, reversed] {
        let registry = DescriptorRegistry::new();
        let OrmError::DescriptorValidation { type_name, message } =
            registry.register_entity(descriptor).unwrap_err()
        else {
            panic!("expected cross-namespace descriptor validation error")
        };
        assert_eq!(type_name, "person");
        assert_eq!(
            message,
            "field name 'name' (attribute 'legal-name') conflicts with attribute name 'name' declared by field 'preferred_name'"
        );
        assert!(registry.snapshot().is_empty());
    }
}

#[test]
fn registry_rejects_hostile_typeql_labels_before_registration() {
    let hostile = [
        "person; match $x isa secret",
        "person name",
        "person\"",
        "match",
        "person::admin",
    ];

    for value in hostile {
        let registry = DescriptorRegistry::new();
        let mut entity = person_descriptor();
        entity.type_name = value.into();
        assert!(matches!(
            registry.register_entity(entity).unwrap_err(),
            OrmError::DescriptorValidation { .. }
        ));
        assert!(registry.snapshot().is_empty());
    }

    let registry = DescriptorRegistry::new();
    let mut entity = person_descriptor();
    entity.owned_attributes[0].attr_name = "name; delete $x".into();
    assert!(matches!(
        registry.register_entity(entity).unwrap_err(),
        OrmError::DescriptorValidation { .. }
    ));

    let registry = DescriptorRegistry::new();
    let mut relation = employment_descriptor();
    relation.roles[0].role_name = "employee) isa secret".into();
    assert!(matches!(
        registry.register_relation(relation).unwrap_err(),
        OrmError::DescriptorValidation { .. }
    ));

    let registry = DescriptorRegistry::new();
    let mut relation = employment_descriptor();
    relation.roles[0].player_type_names = vec!["person; delete $x".into()];
    assert!(matches!(
        registry.register_relation(relation).unwrap_err(),
        OrmError::DescriptorValidation { .. }
    ));
}

#[test]
fn registry_accepts_canonical_hyphenated_typeql_labels() {
    let registry = DescriptorRegistry::new();
    let mut entity = person_descriptor();
    entity.type_name = "work-person".into();
    entity.owned_attributes[0].attr_name = "display-name".into();

    let registered = registry.register_entity(entity).unwrap();

    assert_eq!(registered.type_name, "work-person");
    assert_eq!(registered.owned_attributes[0].attr_name, "display-name");
}

#[test]
fn registry_accepts_relates_only_role() {
    let registry = DescriptorRegistry::new();
    let mut relation = employment_descriptor();
    relation.roles = vec![RoleDescriptor {
        role_name: "definition".into(),
        player_type_names: vec![],
        ..Default::default()
    }];

    let registered = registry.register_relation(relation).unwrap();

    assert_eq!(
        registered
            .role("definition")
            .unwrap()
            .player_type_names
            .len(),
        0
    );
}

#[test]
fn concurrent_identical_registration_converges() {
    let registry = Arc::new(DescriptorRegistry::new());
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let registry = Arc::clone(&registry);
            std::thread::spawn(move || registry.register_entity(person_descriptor()).unwrap())
        })
        .collect();

    let descriptors: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect();
    let first = &descriptors[0];

    assert!(
        descriptors
            .iter()
            .all(|descriptor| Arc::ptr_eq(first, descriptor))
    );
    assert_eq!(registry.snapshot().len(), 1);
}

#[test]
fn registry_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<DescriptorRegistry>();
}
