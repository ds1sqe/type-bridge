//! Cross-language golden for a representative public handle-built request.

use std::sync::Arc;

use type_bridge_orm::*;

#[path = "support/internal.rs"]
mod internal;
use internal::*;

fn attribute(field_name: &str, attr_name: &str) -> OwnedAttributeDescriptor {
    OwnedAttributeDescriptor {
        field_name: field_name.into(),
        attr_name: attr_name.into(),
        value_type: ValueType::String,
        annotations: vec![Annotation::Key],
        is_optional: false,
        is_ordered: false,
        doc: None,
        meta: Default::default(),
    }
}

fn registry() -> Arc<DescriptorRegistry> {
    let registry = Arc::new(DescriptorRegistry::new());
    registry
        .register_entity(EntityDescriptor {
            type_name: "person".into(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: vec![attribute("name", "person-name")],
            doc: None,
            meta: Default::default(),
        })
        .unwrap();
    registry
        .register_entity(EntityDescriptor {
            type_name: "company".into(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: vec![attribute("name", "company-name")],
            doc: None,
            meta: Default::default(),
        })
        .unwrap();
    registry
        .register_relation(RelationDescriptor {
            type_name: "employment".into(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: Vec::new(),
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
        })
        .unwrap();
    registry
}

#[test]
fn public_named_page_matches_the_python_and_node_golden() {
    let registry = registry();
    let session = SessionHandle::new(Arc::clone(&registry));
    let person = session.exact("person").unwrap();
    let company = session.exact("company").unwrap();
    let employment = session.exact("employment").unwrap();

    let person_order = person
        .field("name")
        .unwrap()
        .order(SortDirection::Ascending, MissingOrder::Reject);
    let company_order = company
        .field("name")
        .unwrap()
        .order(SortDirection::Ascending, MissingOrder::Reject);
    let companies = company
        .collect()
        .distinct(true)
        .unwrap()
        .order_by(company_order)
        .unwrap();
    let shape = session
        .named([("person", person.one()), ("companies", companies)])
        .unwrap();
    let connected = employment
        .role("employee")
        .unwrap()
        .connects(&person)
        .unwrap()
        .and(
            &employment
                .role("employer")
                .unwrap()
                .connects(&company)
                .unwrap(),
        )
        .unwrap();
    let query = session
        .query(shape)
        .unwrap()
        .add_hidden(employment)
        .unwrap()
        .where_predicate(connected)
        .unwrap();
    let request = query
        .page_by(
            &person,
            &[person_order],
            Window {
                offset: 10,
                limit: 10,
            },
            true,
        )
        .unwrap();
    let actual = String::from_utf8(
        UnvalidatedMatchRequest::from_request(request)
            .unwrap()
            .to_canonical_bytes()
            .unwrap(),
    )
    .unwrap();

    assert_eq!(
        actual,
        include_str!("fixtures/match_request/public-named-page.json").trim()
    );
}
