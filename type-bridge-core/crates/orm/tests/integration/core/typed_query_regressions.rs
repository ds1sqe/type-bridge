//! Live regressions for typed-query hydration at the real TypeDB boundary.

use std::cmp::Ordering;
use std::collections::BTreeSet;

use crate::common::dynamic_crud::{
    attr, setup_dynamic_database, setup_dynamic_typeql, unique_schema_suffix,
};
use crate::internal::*;
use type_bridge_core_lib::decimal::parse_decimal;
use type_bridge_orm::*;

fn exact_one_request(
    registry: &DescriptorRegistry,
    bindings: Vec<MatchBinding>,
    predicate: Option<MatchExpr>,
    selected: Vec<BindingId>,
) -> ValidatedMatchRequest {
    validate_match_request(
        registry,
        MatchRequest::v1(
            MatchPlan {
                bindings,
                predicate,
                allowed_cross_joins: BTreeSet::new(),
            },
            MatchOperation::FetchRows {
                output: FetchShape::Positional {
                    slots: selected
                        .into_iter()
                        .map(|binding| FetchSlot::One { binding })
                        .collect(),
                },
                order: Vec::new(),
                window: Window {
                    offset: 0,
                    limit: 1,
                },
                cardinality: RowCardinality::ExactlyOne,
            },
        ),
    )
    .expect("regression request should pass canonical validation")
}

fn one_selected_thing(result: &ValidatedMatchResult) -> &HydratedThing {
    let MatchResult::Rows { rows } = result.result() else {
        panic!("expected selected rows")
    };
    assert_eq!(rows.len(), 1, "expected one selected row");
    let SlotValue::One(thing) = &rows[0].slots()[0] else {
        panic!("expected one singular selected thing")
    };
    thing
}

#[tokio::test]
async fn typed_hydration_preserves_long_values_above_json_safe_integer_range() {
    let _guard = crate::common::integration_test_guard().await;
    let (db, schema) = setup_dynamic_database("typed-long-precision").await;
    let descriptor = schema.person_descriptor();
    let manager = DynamicEntityManager::new(&db, descriptor.clone());
    const VALUE: i64 = 9_007_199_254_740_993;
    manager
        .insert(&vec![
            ("name".into(), AttributeValue::String("LargeLong".into())),
            ("age".into(), AttributeValue::Long(VALUE)),
        ])
        .await
        .expect("large integer fixture should insert losslessly");

    let registry = DescriptorRegistry::new();
    registry
        .register_entity(descriptor.as_ref().clone())
        .expect("large integer descriptor should register");
    let root = BindingId::new(0);
    let request = exact_one_request(
        &registry,
        vec![MatchBinding {
            id: root,
            descriptor: registry.descriptor_id(&schema.person_type).unwrap(),
            thing_kind: ThingKind::Entity,
            match_mode: MatchMode::Exact,
        }],
        None,
        vec![root],
    );

    let result = db
        .execute_match(&registry, &request)
        .await
        .expect("typed hydration should preserve the full TypeDB integer domain");
    let age = one_selected_thing(&result)
        .attributes()
        .iter()
        .find(|attribute| attribute.field().name == "age")
        .expect("selected thing should contain its large integer field");
    assert_eq!(age.values(), &[AttributeValue::Long(VALUE)]);
}

#[tokio::test]
async fn typed_hydration_accepts_real_driver_fractional_decimal_spelling() {
    let _guard = crate::common::integration_test_guard().await;
    let (db, schema) = setup_dynamic_database("typed-fractional-decimal").await;
    let descriptor = schema.person_descriptor();
    let manager = DynamicEntityManager::new(&db, descriptor.clone());
    manager
        .insert(&vec![
            (
                "name".into(),
                AttributeValue::String("FractionalDecimal".into()),
            ),
            ("balance".into(), AttributeValue::Decimal("1234.56".into())),
        ])
        .await
        .expect("fractional decimal fixture should insert");

    let registry = DescriptorRegistry::new();
    registry
        .register_entity(descriptor.as_ref().clone())
        .expect("fractional decimal descriptor should register");
    let root = BindingId::new(0);
    let request = exact_one_request(
        &registry,
        vec![MatchBinding {
            id: root,
            descriptor: registry.descriptor_id(&schema.person_type).unwrap(),
            thing_kind: ThingKind::Entity,
            match_mode: MatchMode::Exact,
        }],
        None,
        vec![root],
    );

    let result = db
        .execute_match(&registry, &request)
        .await
        .expect("typed hydration should accept the driver's canonical decimal spelling");
    let balance = one_selected_thing(&result)
        .attributes()
        .iter()
        .find(|attribute| attribute.field().name == "balance")
        .expect("selected thing should contain its fractional decimal field");
    let [AttributeValue::Decimal(value)] = balance.values() else {
        panic!("unexpected hydrated decimal: {:?}", balance.values())
    };
    let hydrated = parse_decimal(value).expect("driver decimal spelling should be canonical");
    let inserted = parse_decimal("1234.56").unwrap();
    assert_eq!(hydrated.compare(&inserted), Ordering::Equal);
}

#[tokio::test]
async fn typed_role_edge_accepts_exact_subtype_player_of_role_declared_base() {
    let _guard = crate::common::integration_test_guard().await;
    let suffix = unique_schema_suffix("rust", "typed-exact-subtype-role");
    let person_type = format!("{suffix}-person");
    let employee_type = format!("{suffix}-employee");
    let employment_type = format!("{suffix}-employment");
    let person_key_attr = format!("{suffix}-person-key");
    let employment_key_attr = format!("{suffix}-employment-key");
    let db = setup_dynamic_typeql(&format!(
        r#"define
attribute {person_key_attr}, value string;
attribute {employment_key_attr}, value string;
entity {person_type}, owns {person_key_attr} @key, plays {employment_type}:employee;
entity {employee_type} sub {person_type};
relation {employment_type}, relates employee @card(1), owns {employment_key_attr} @key;
"#
    ))
    .await;
    db.execute_raw(
        &format!(
            r#"insert
$employee isa {employee_type}, has {person_key_attr} "employee-1";
$employment isa {employment_type}, links (employee: $employee), has {employment_key_attr} "employment-1";
"#
        ),
        TxType::Write,
    )
    .await
    .expect("exact subtype role-edge fixture should insert");

    let person_key = attr("key", &person_key_attr, ValueType::String, true);
    let employment_key = attr("key", &employment_key_attr, ValueType::String, true);
    let registry = DescriptorRegistry::new();
    registry
        .register_entity(EntityDescriptor {
            type_name: person_type.clone(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: vec![person_key.clone()],
            doc: None,
            meta: Default::default(),
        })
        .expect("base player descriptor should register");
    registry
        .register_entity(EntityDescriptor {
            type_name: employee_type.clone(),
            is_abstract: false,
            parent_type: Some(person_type.clone()),
            owned_attributes: vec![person_key],
            doc: None,
            meta: Default::default(),
        })
        .expect("exact subtype player descriptor should register");
    registry
        .register_relation(RelationDescriptor {
            type_name: employment_type.clone(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: vec![employment_key],
            roles: vec![RoleDescriptor {
                role_name: "employee".into(),
                player_type_names: vec![person_type.clone()],
                cardinality: Some((1, Some(1))),
                ..Default::default()
            }],
            doc: None,
            meta: Default::default(),
        })
        .expect("relation descriptor should register");

    let employee = BindingId::new(0);
    let employment = BindingId::new(1);
    let person_descriptor = registry.descriptor_id(&person_type).unwrap();
    let employee_descriptor = registry.descriptor_id(&employee_type).unwrap();
    let employment_descriptor = registry.descriptor_id(&employment_type).unwrap();
    let request = exact_one_request(
        &registry,
        vec![
            MatchBinding {
                id: employee,
                descriptor: employee_descriptor.clone(),
                thing_kind: ThingKind::Entity,
                match_mode: MatchMode::Exact,
            },
            MatchBinding {
                id: employment,
                descriptor: employment_descriptor.clone(),
                thing_kind: ThingKind::Relation,
                match_mode: MatchMode::Exact,
            },
        ],
        Some(MatchExpr::RoleEdge {
            id: RoleEdgeId::new(0),
            relation: employment,
            role: registry
                .role_id(&employment_descriptor, "employee")
                .unwrap(),
            player: employee,
        }),
        vec![employee, employment],
    );

    let result = db
        .execute_match(&registry, &request)
        .await
        .expect("a base-typed role must accept an exact compatible subtype player");
    let MatchResult::Rows { rows } = result.result() else {
        panic!("expected selected rows")
    };
    assert_eq!(rows.len(), 1);
    let [
        SlotValue::One(selected_employee),
        SlotValue::One(selected_employment),
    ] = rows[0].slots()
    else {
        panic!("expected employee and employment slots")
    };
    let employee_role = selected_employment
        .roles()
        .iter()
        .find(|role| role.role().name == "employee")
        .expect("employment should hydrate its employee role");
    let [nested_employee] = employee_role.players() else {
        panic!("employment should contain exactly one employee")
    };
    assert_eq!(nested_employee.concept_id(), selected_employee.concept_id());
    assert_eq!(nested_employee.declared_descriptor(), &person_descriptor);
    assert_eq!(nested_employee.concrete_descriptor(), &employee_descriptor);
    assert_eq!(
        nested_employee.concrete_descriptor(),
        selected_employee.concrete_descriptor()
    );
}
