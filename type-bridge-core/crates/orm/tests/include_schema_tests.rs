//! Tests for `include_schema!` proc-macro.

#![cfg(feature = "derive")]

use type_bridge_orm::*;

// Include the test schema from a .tql file
type_bridge_orm::include_schema!("tests/test_schema.tql");

#[test]
fn generated_attribute_types_exist() {
    // define_attribute! generates newtype wrappers with TypeBridgeAttribute
    let name = Name("Alice".to_string());
    let age = Age(30);
    let position = Position("Engineer".to_string());

    assert_eq!(name.0, "Alice");
    assert_eq!(age.0, 30);
    assert_eq!(position.0, "Engineer");
}

#[test]
fn generated_entity_type_name() {
    assert_eq!(Person::TYPE_NAME, "person");
    assert_eq!(Company::TYPE_NAME, "company");
}

#[test]
fn generated_entity_owned_attributes() {
    let attrs = Person::owned_attributes();
    assert_eq!(attrs.len(), 2);
    assert_eq!(attrs[0].attr_name, "name");
    assert_eq!(attrs[1].attr_name, "age");
}

#[test]
fn generated_entity_key_annotation() {
    let attrs = Person::owned_attributes();
    let name_attr = &attrs[0];
    assert!(
        name_attr.annotations.contains(&Annotation::Key),
        "name should be @key"
    );
}

#[test]
fn generated_entity_iid() {
    let mut person = Person {
        iid: None,
        name: Name("Alice".into()),
        age: Age(30),
    };
    assert!(person.iid().is_none());
    person.set_iid("0x123".into());
    assert_eq!(person.iid(), Some("0x123"));
}

#[test]
fn generated_relation_type_name() {
    assert_eq!(Employment::TYPE_NAME, "employment");
}

#[test]
fn generated_relation_roles() {
    let roles = Employment::role_info();
    assert_eq!(roles.len(), 2);
    assert_eq!(roles[0].role_name, "employee");
    assert_eq!(roles[0].player_type_name, "person");
    assert_eq!(roles[1].role_name, "employer");
    assert_eq!(roles[1].player_type_name, "company");
}

#[test]
fn generated_relation_owned_attributes() {
    let attrs = Employment::owned_attributes();
    assert_eq!(attrs.len(), 1);
    assert_eq!(attrs[0].attr_name, "position");
}

#[test]
fn generated_entity_fields_work() {
    let fields = Person::fields();
    let expr = fields.name.eq(Name("Alice".into()));
    match expr {
        Expr::Eq { attr, .. } => assert_eq!(attr, "name"),
        _ => panic!("Expected Eq expr"),
    }
}
