//! Integration tests for `#[derive(TypeBridgeAttribute)]` and `#[derive(TypeBridgeEntity)]`.

#![cfg(feature = "derive")]

use type_bridge_orm::*;

// ── Attribute derive tests ───────────────────────────────────────────

#[derive(DeriveAttribute, Debug, Clone, PartialEq)]
#[attribute(name = "name", value_type = "string")]
struct Name(pub String);

#[derive(DeriveAttribute, Debug, Clone, PartialEq)]
#[attribute(name = "age", value_type = "long")]
struct Age(pub i64);

#[derive(DeriveAttribute, Debug, Clone, PartialEq)]
#[attribute(name = "score", value_type = "double")]
struct Score(pub f64);

#[derive(DeriveAttribute, Debug, Clone, PartialEq)]
#[attribute(name = "active", value_type = "boolean")]
struct Active(pub bool);

#[derive(DeriveAttribute, Debug, Clone, PartialEq)]
#[attribute(name = "email", value_type = "string")]
struct Email(pub String);

#[derive(DeriveAttribute, Debug, Clone, PartialEq)]
#[attribute(name = "birthday", value_type = "date")]
struct Birthday(pub String);

#[derive(DeriveAttribute, Debug, Clone, PartialEq)]
#[attribute(name = "created-at", value_type = "datetime")]
struct CreatedAt(pub String);

#[derive(DeriveAttribute, Debug, Clone, PartialEq)]
#[attribute(name = "amount", value_type = "decimal")]
struct Amount(pub String);

#[derive(DeriveAttribute, Debug, Clone, PartialEq)]
#[attribute(name = "interval", value_type = "duration")]
struct Interval(pub String);

#[test]
fn attribute_derive_string() {
    assert_eq!(Name::ATTR_NAME, "name");
    assert_eq!(Name::VALUE_TYPE, "string");
    let n = Name("Alice".into());
    let v = n.to_value();
    assert_eq!(v, AttributeValue::String("Alice".into()));
    assert_eq!(Name::from_value(&v), Some(Name("Alice".into())));
}

#[test]
fn attribute_derive_long() {
    assert_eq!(Age::ATTR_NAME, "age");
    assert_eq!(Age::VALUE_TYPE, "long");
    let a = Age(30);
    let v = a.to_value();
    assert_eq!(v, AttributeValue::Long(30));
    assert_eq!(Age::from_value(&v), Some(Age(30)));
}

#[test]
fn attribute_derive_double() {
    assert_eq!(Score::ATTR_NAME, "score");
    assert_eq!(Score::VALUE_TYPE, "double");
    let s = Score(95.5);
    let v = s.to_value();
    assert_eq!(v, AttributeValue::Double(95.5));
    assert_eq!(Score::from_value(&v), Some(Score(95.5)));
}

#[test]
fn attribute_derive_boolean() {
    assert_eq!(Active::ATTR_NAME, "active");
    assert_eq!(Active::VALUE_TYPE, "boolean");
    let a = Active(true);
    let v = a.to_value();
    assert_eq!(v, AttributeValue::Boolean(true));
    assert_eq!(Active::from_value(&v), Some(Active(true)));
}

#[test]
fn attribute_derive_date() {
    assert_eq!(Birthday::ATTR_NAME, "birthday");
    let b = Birthday("2024-01-15".into());
    assert_eq!(b.to_value(), AttributeValue::Date("2024-01-15".into()));
}

#[test]
fn attribute_derive_datetime() {
    assert_eq!(CreatedAt::ATTR_NAME, "created-at");
    let c = CreatedAt("2024-01-15T10:30:00".into());
    assert_eq!(
        c.to_value(),
        AttributeValue::DateTime("2024-01-15T10:30:00".into())
    );
}

#[test]
fn attribute_derive_decimal() {
    assert_eq!(Amount::ATTR_NAME, "amount");
    let a = Amount("123.45".into());
    assert_eq!(a.to_value(), AttributeValue::Decimal("123.45".into()));
}

#[test]
fn attribute_derive_duration() {
    assert_eq!(Interval::ATTR_NAME, "interval");
    let i = Interval("P1Y2M".into());
    assert_eq!(i.to_value(), AttributeValue::Duration("P1Y2M".into()));
}

#[test]
fn attribute_from_value_type_mismatch() {
    let v = AttributeValue::Long(42);
    assert_eq!(Name::from_value(&v), None);
}

// ── Entity derive tests ──────────────────────────────────────────────

#[derive(DeriveEntity, Debug)]
#[entity(name = "person")]
struct Person {
    iid: Option<String>,
    #[field(key)]
    name: Name,
    age: Age,
    email: Option<Email>,
}

#[test]
fn entity_type_name() {
    assert_eq!(Person::TYPE_NAME, "person");
}

#[test]
fn entity_owned_attributes() {
    let attrs = Person::owned_attributes();
    assert_eq!(attrs.len(), 3);

    assert_eq!(attrs[0].attr_name, "name");
    assert_eq!(attrs[0].value_type, ValueType::String);
    assert!(attrs[0].is_key());

    assert_eq!(attrs[1].attr_name, "age");
    assert_eq!(attrs[1].value_type, ValueType::Long);
    assert!(!attrs[1].is_key());

    assert_eq!(attrs[2].attr_name, "email");
    assert_eq!(attrs[2].value_type, ValueType::String);
    assert!(!attrs[2].is_key());
}

#[test]
fn entity_iid_management() {
    let mut p = Person {
        iid: None,
        name: Name("Alice".into()),
        age: Age(30),
        email: None,
    };
    assert_eq!(p.iid(), None);
    p.set_iid("0xabc".into());
    assert_eq!(p.iid(), Some("0xabc"));
}

#[test]
fn entity_to_attribute_values_all_present() {
    let p = Person {
        iid: None,
        name: Name("Alice".into()),
        age: Age(30),
        email: Some(Email("alice@example.com".into())),
    };
    let values = p.to_attribute_values();
    assert_eq!(values.len(), 3);
    assert_eq!(values[0].0, "name");
    assert_eq!(values[0].1, AttributeValue::String("Alice".into()));
    assert_eq!(values[1].0, "age");
    assert_eq!(values[1].1, AttributeValue::Long(30));
    assert_eq!(values[2].0, "email");
    assert_eq!(
        values[2].1,
        AttributeValue::String("alice@example.com".into())
    );
}

#[test]
fn entity_to_attribute_values_optional_none() {
    let p = Person {
        iid: None,
        name: Name("Bob".into()),
        age: Age(25),
        email: None,
    };
    let values = p.to_attribute_values();
    // Optional None should be omitted
    assert_eq!(values.len(), 2);
    assert_eq!(values[0].0, "name");
    assert_eq!(values[1].0, "age");
}

#[test]
fn entity_from_document_all_fields() {
    let mut doc = serde_json::Map::new();
    doc.insert("name".into(), serde_json::json!("Charlie"));
    doc.insert("age".into(), serde_json::json!(35));
    doc.insert("email".into(), serde_json::json!("charlie@test.com"));

    let p = Person::from_document(&doc).unwrap();
    assert_eq!(p.name.0, "Charlie");
    assert_eq!(p.age.0, 35);
    assert_eq!(p.email, Some(Email("charlie@test.com".into())));
    assert_eq!(p.iid(), None);
}

#[test]
fn entity_from_document_optional_missing() {
    let mut doc = serde_json::Map::new();
    doc.insert("name".into(), serde_json::json!("Diana"));
    doc.insert("age".into(), serde_json::json!(28));

    let p = Person::from_document(&doc).unwrap();
    assert_eq!(p.name.0, "Diana");
    assert_eq!(p.age.0, 28);
    assert_eq!(p.email, None);
}

#[test]
fn entity_from_document_missing_required() {
    let mut doc = serde_json::Map::new();
    doc.insert("name".into(), serde_json::json!("Eve"));
    // Missing age

    let result = Person::from_document(&doc);
    assert!(result.is_err());
    match result.unwrap_err() {
        OrmError::Hydration { type_name, message } => {
            assert_eq!(type_name, "person");
            assert!(message.contains("age"));
        }
        other => panic!("Expected Hydration error, got: {other}"),
    }
}

#[test]
fn entity_roundtrip_to_values_from_document() {
    let original = Person {
        iid: None,
        name: Name("Frank".into()),
        age: Age(42),
        email: Some(Email("frank@test.com".into())),
    };

    // Convert to attribute values, then build a JSON doc, then hydrate back
    let values = original.to_attribute_values();
    let mut doc = serde_json::Map::new();
    for (attr_name, value) in &values {
        let json_val = match value {
            AttributeValue::String(s) => serde_json::json!(s),
            AttributeValue::Long(n) => serde_json::json!(n),
            _ => unreachable!(),
        };
        doc.insert(attr_name.to_string(), json_val);
    }

    let hydrated = Person::from_document(&doc).unwrap();
    assert_eq!(hydrated.name.0, "Frank");
    assert_eq!(hydrated.age.0, 42);
    assert_eq!(hydrated.email, Some(Email("frank@test.com".into())));
}

// ── Entity with default AST methods ──────────────────────────────────

#[test]
fn entity_insert_clauses_work() {
    let p = Person {
        iid: None,
        name: Name("Alice".into()),
        age: Age(30),
        email: None,
    };
    let clauses = p.to_insert_with_iid_fetch("$e");
    // Should produce insert + fetch clauses
    assert!(clauses.len() >= 2);
}

#[test]
fn entity_match_pattern_works() {
    let p = Person {
        iid: Some("0xabc".into()),
        name: Name("Alice".into()),
        age: Age(30),
        email: None,
    };
    let _pattern = p.to_match_pattern("$e");
    // Just verify it doesn't panic
}

// ── Entity with all required fields (no optionals) ───────────────────

#[derive(DeriveEntity, Debug)]
#[entity(name = "company")]
struct Company {
    iid: Option<String>,
    #[field(key)]
    name: Name,
}

#[test]
fn entity_single_field() {
    assert_eq!(Company::TYPE_NAME, "company");
    let attrs = Company::owned_attributes();
    assert_eq!(attrs.len(), 1);
    assert!(attrs[0].is_key());

    let c = Company {
        iid: None,
        name: Name("Acme".into()),
    };
    let values = c.to_attribute_values();
    assert_eq!(values.len(), 1);
}

// ── Relation derive tests ───────────────────────────────────────────

#[derive(DeriveAttribute, Debug, Clone, PartialEq)]
#[attribute(name = "position", value_type = "string")]
struct Position(pub String);

#[derive(DeriveAttribute, Debug, Clone, PartialEq)]
#[attribute(name = "start-date", value_type = "date")]
struct StartDate(pub String);

#[derive(DeriveRelation, Debug)]
#[relation(name = "employment")]
struct Employment {
    iid: Option<String>,
    #[role(name = "employee", player_type = "person")]
    employee: RolePlayerRef,
    #[role(name = "employer", player_type = "company")]
    employer: RolePlayerRef,
    position: Option<Position>,
    start_date: Option<StartDate>,
}

#[test]
fn relation_derive_type_name() {
    assert_eq!(Employment::TYPE_NAME, "employment");
}

#[test]
fn relation_derive_role_info() {
    let roles = Employment::role_info();
    assert_eq!(roles.len(), 2);
    assert_eq!(roles[0].role_name, "employee");
    assert_eq!(roles[0].player_type_name, "person");
    assert_eq!(roles[1].role_name, "employer");
    assert_eq!(roles[1].player_type_name, "company");
}

#[test]
fn relation_derive_owned_attributes() {
    let attrs = Employment::owned_attributes();
    assert_eq!(attrs.len(), 2);
    assert_eq!(attrs[0].attr_name, "position");
    assert_eq!(attrs[0].value_type, ValueType::String);
    assert!(!attrs[0].is_key());
    assert_eq!(attrs[1].attr_name, "start-date");
    assert_eq!(attrs[1].value_type, ValueType::Date);
}

#[test]
fn relation_derive_iid_management() {
    let mut emp = Employment {
        iid: None,
        employee: RolePlayerRef {
            role: "employee",
            entity_type_name: "person",
            iid: None,
            key: None,
        },
        employer: RolePlayerRef {
            role: "employer",
            entity_type_name: "company",
            iid: None,
            key: None,
        },
        position: None,
        start_date: None,
    };
    assert_eq!(emp.iid(), None);
    emp.set_iid("0xrel1".into());
    assert_eq!(emp.iid(), Some("0xrel1"));
}

#[test]
fn relation_derive_to_attribute_values_with_optionals() {
    let emp = Employment {
        iid: None,
        employee: RolePlayerRef {
            role: "employee",
            entity_type_name: "person",
            iid: None,
            key: None,
        },
        employer: RolePlayerRef {
            role: "employer",
            entity_type_name: "company",
            iid: None,
            key: None,
        },
        position: Some(Position("Engineer".into())),
        start_date: None,
    };
    let values = emp.to_attribute_values();
    // Only position, start_date is None
    assert_eq!(values.len(), 1);
    assert_eq!(values[0].0, "position");
    assert_eq!(
        values[0].1,
        AttributeValue::String("Engineer".into())
    );
}

#[test]
fn relation_derive_to_role_player_refs() {
    let emp = Employment {
        iid: None,
        employee: RolePlayerRef {
            role: "employee",
            entity_type_name: "person",
            iid: Some("0xp1".into()),
            key: None,
        },
        employer: RolePlayerRef {
            role: "employer",
            entity_type_name: "company",
            iid: None,
            key: Some(("name", AttributeValue::String("Acme".into()))),
        },
        position: None,
        start_date: None,
    };
    let refs = emp.to_role_player_refs();
    assert_eq!(refs.len(), 2);
    assert_eq!(refs[0].role, "employee");
    assert_eq!(refs[0].iid.as_deref(), Some("0xp1"));
    assert_eq!(refs[1].role, "employer");
    assert!(refs[1].key.is_some());
}

#[test]
fn relation_derive_from_document() {
    let mut doc = serde_json::Map::new();
    doc.insert("position".into(), serde_json::json!("CTO"));
    doc.insert("start-date".into(), serde_json::json!("2024-06-01"));

    let emp = Employment::from_document(&doc).unwrap();
    assert_eq!(emp.position, Some(Position("CTO".into())));
    assert_eq!(emp.start_date, Some(StartDate("2024-06-01".into())));
    // Role players default to empty refs
    assert_eq!(emp.employee.role, "employee");
    assert_eq!(emp.employer.role, "employer");
}

#[test]
fn relation_derive_from_document_optional_missing() {
    let doc = serde_json::Map::new();
    let emp = Employment::from_document(&doc).unwrap();
    assert!(emp.position.is_none());
    assert!(emp.start_date.is_none());
}

#[test]
fn relation_derive_insert_clauses() {
    let emp = Employment {
        iid: None,
        employee: RolePlayerRef {
            role: "employee",
            entity_type_name: "person",
            iid: None,
            key: Some(("name", AttributeValue::String("Alice".into()))),
        },
        employer: RolePlayerRef {
            role: "employer",
            entity_type_name: "company",
            iid: Some("0xcomp".into()),
            key: None,
        },
        position: Some(Position("Engineer".into())),
        start_date: None,
    };
    let clauses = emp.to_insert_with_iid_fetch("$r");
    // match (role players) + insert (relation) + fetch (iid)
    assert!(clauses.len() >= 3);
}

// ── Relation with no attributes ─────────────────────────────────────

#[derive(DeriveRelation, Debug)]
#[relation(name = "friendship")]
struct Friendship {
    iid: Option<String>,
    #[role(name = "friend", player_type = "person")]
    friend_a: RolePlayerRef,
    #[role(name = "friend", player_type = "person")]
    friend_b: RolePlayerRef,
}

#[test]
fn relation_no_attributes() {
    assert_eq!(Friendship::TYPE_NAME, "friendship");
    assert_eq!(Friendship::owned_attributes().len(), 0);
    assert_eq!(Friendship::role_info().len(), 2);
    assert_eq!(Friendship::role_info()[0].role_name, "friend");
    assert_eq!(Friendship::role_info()[1].role_name, "friend");
}

// ── Extended annotation tests ───────────────────────────────────────

#[derive(DeriveAttribute, Debug, Clone, PartialEq)]
#[attribute(name = "username", value_type = "string")]
struct Username(pub String);

#[derive(DeriveAttribute, Debug, Clone, PartialEq)]
#[attribute(name = "tag", value_type = "string")]
struct Tag(pub String);

#[derive(DeriveAttribute, Debug, Clone, PartialEq)]
#[attribute(name = "phone", value_type = "string")]
struct Phone(pub String);

#[derive(DeriveEntity, Debug)]
#[entity(name = "user")]
struct User {
    iid: Option<String>,
    #[field(key)]
    name: Name,
    #[field(unique)]
    username: Username,
    #[field(card_min = 1, card_max = 5)]
    tag: Tag,
    #[field(card_min = 0)]
    phone: Option<Phone>,
}

#[test]
fn entity_with_unique_attribute() {
    let attrs = User::owned_attributes();
    assert_eq!(attrs.len(), 4);

    // name: @key
    assert!(attrs[0].is_key());
    assert!(!attrs[0].is_unique());
    assert!(attrs[0].cardinality().is_none());

    // username: @unique
    assert!(!attrs[1].is_key());
    assert!(attrs[1].is_unique());
    assert!(attrs[1].cardinality().is_none());
}

#[test]
fn entity_with_bounded_cardinality() {
    let attrs = User::owned_attributes();

    // tag: @card(1, 5)
    let tag_attr = &attrs[2];
    assert!(!tag_attr.is_key());
    assert!(!tag_attr.is_unique());
    assert_eq!(tag_attr.cardinality(), Some((1, Some(5))));
}

#[test]
fn entity_with_unbounded_cardinality() {
    let attrs = User::owned_attributes();

    // phone: @card(0, None) — unbounded max
    let phone_attr = &attrs[3];
    assert!(!phone_attr.is_key());
    assert_eq!(phone_attr.cardinality(), Some((0, None)));
}

// ── Abstract type and inheritance tests ─────────────────────────────

#[derive(DeriveAttribute, Debug, Clone, PartialEq)]
#[attribute(name = "breed", value_type = "string")]
struct Breed(pub String);

#[derive(DeriveEntity, Debug)]
#[entity(name = "animal", r#abstract)]
struct Animal {
    iid: Option<String>,
    #[field(key)]
    name: Name,
}

#[derive(DeriveEntity, Debug)]
#[entity(name = "dog", extends = "animal")]
struct Dog {
    iid: Option<String>,
    #[field(key)]
    name: Name,
    breed: Breed,
}

#[allow(clippy::assertions_on_constants)]
#[test]
fn entity_abstract_flag() {
    assert!(Animal::IS_ABSTRACT);
    assert_eq!(Animal::TYPE_NAME, "animal");
}

#[allow(clippy::assertions_on_constants)]
#[test]
fn entity_non_abstract_default() {
    // Person was defined without `abstract`, so IS_ABSTRACT defaults to false
    assert!(!Person::IS_ABSTRACT);
}

#[allow(clippy::assertions_on_constants)]
#[test]
fn entity_extends_parent() {
    assert_eq!(Dog::PARENT_TYPE, Some("animal"));
    assert!(!Dog::IS_ABSTRACT);
    assert_eq!(Dog::TYPE_NAME, "dog");
}

#[test]
fn entity_no_parent_default() {
    assert_eq!(Person::PARENT_TYPE, None);
}

// Abstract relation

#[derive(DeriveRelation, Debug)]
#[relation(name = "connection", r#abstract)]
struct Connection {
    iid: Option<String>,
    #[role(name = "source", player_type = "node")]
    source: RolePlayerRef,
    #[role(name = "target", player_type = "node")]
    target: RolePlayerRef,
}

#[derive(DeriveRelation, Debug)]
#[relation(name = "link", extends = "connection")]
struct Link {
    iid: Option<String>,
    #[role(name = "source", player_type = "node")]
    source: RolePlayerRef,
    #[role(name = "target", player_type = "node")]
    target: RolePlayerRef,
}

#[allow(clippy::assertions_on_constants)]
#[test]
fn relation_abstract_flag() {
    assert!(Connection::IS_ABSTRACT);
    assert_eq!(Connection::TYPE_NAME, "connection");
}

#[allow(clippy::assertions_on_constants)]
#[test]
fn relation_non_abstract_default() {
    assert!(!Employment::IS_ABSTRACT);
}

#[allow(clippy::assertions_on_constants)]
#[test]
fn relation_extends_parent() {
    assert_eq!(Link::PARENT_TYPE, Some("connection"));
    assert!(!Link::IS_ABSTRACT);
}

#[test]
fn relation_no_parent_default() {
    assert_eq!(Employment::PARENT_TYPE, None);
}

// Schema generation with abstract types and inheritance (uses mock backend)

mod mock_backend {
    use type_bridge_orm::session::backend::{
        BoxFuture, DriverBackend, QueryResult, TransactionOps, TxType,
    };
    use type_bridge_orm::OrmError;

    struct NoopTx;

    impl TransactionOps for NoopTx {
        fn query(&mut self, _typeql: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
            Box::pin(async { Ok(QueryResult::Ok) })
        }
        fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            Box::pin(async { Ok(()) })
        }
    }

    pub struct NoopBackend;

    impl DriverBackend for NoopBackend {
        fn open_transaction(
            &self,
            _database: &str,
            _tx_type: TxType,
        ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
            Box::pin(async { Ok(Box::new(NoopTx) as Box<dyn TransactionOps>) })
        }

        fn is_open(&self) -> bool {
            true
        }
    }
}

#[test]
fn schema_registers_abstract_entity() {
    use type_bridge_orm::schema::manager::SchemaManager;
    use type_bridge_orm::session::Database;

    let db = Database::with_backend(Box::new(mock_backend::NoopBackend), "testdb");
    let mut schema = SchemaManager::new(&db);
    schema.register_entity::<Animal>();
    schema.register_entity::<Dog>();

    let info = schema.schema_info();
    let animal = info.get_entity_by_name("animal").unwrap();
    assert!(animal.is_abstract);
    assert_eq!(animal.parent_type, None);

    let dog = info.get_entity_by_name("dog").unwrap();
    assert!(!dog.is_abstract);
    assert_eq!(dog.parent_type.as_deref(), Some("animal"));
}

#[test]
fn schema_generates_abstract_and_sub() {
    use type_bridge_orm::schema::manager::SchemaManager;
    use type_bridge_orm::session::Database;

    let db = Database::with_backend(Box::new(mock_backend::NoopBackend), "testdb");
    let mut schema = SchemaManager::new(&db);
    schema.register_entity::<Animal>();
    schema.register_entity::<Dog>();

    let typeql = schema.generate_schema().unwrap();
    assert!(
        typeql.contains("entity animal @abstract"),
        "should contain @abstract: {typeql}"
    );
    assert!(
        typeql.contains("entity dog sub animal"),
        "should contain sub clause: {typeql}"
    );
    // Dog should own breed (its own) but not re-emit inherited name
    assert!(
        typeql[typeql.find("entity dog").unwrap()..].contains("owns breed"),
        "dog should own breed: {typeql}"
    );
}

// ── Field reference tests ───────────────────────────────────────────

#[test]
fn entity_fields_eq() {
    let fields = Person::fields();
    let expr = fields.name.eq(Name("Alice".into()));
    match expr {
        Expr::Eq { attr, value } => {
            assert_eq!(attr, "name");
            assert_eq!(value, AttributeValue::String("Alice".into()));
        }
        _ => panic!("expected Eq"),
    }
}

#[test]
fn entity_fields_comparison_ops() {
    let fields = Person::fields();
    assert!(matches!(fields.age.gt(Age(18)), Expr::Gt { .. }));
    assert!(matches!(fields.age.gte(Age(18)), Expr::Gte { .. }));
    assert!(matches!(fields.age.lt(Age(65)), Expr::Lt { .. }));
    assert!(matches!(fields.age.lte(Age(65)), Expr::Lte { .. }));
    assert!(matches!(fields.age.neq(Age(0)), Expr::Neq { .. }));
}

#[test]
fn entity_fields_string_ops() {
    let fields = Person::fields();
    assert!(matches!(fields.name.contains("Ali"), Expr::Contains { .. }));
    assert!(matches!(fields.name.like("^A.*"), Expr::Like { .. }));
}

#[test]
fn entity_fields_sort() {
    let fields = Person::fields();
    let (attr, dir) = fields.age.asc();
    assert_eq!(attr, "age");
    assert_eq!(dir, SortDir::Asc);
    let (attr, dir) = fields.name.desc();
    assert_eq!(attr, "name");
    assert_eq!(dir, SortDir::Desc);
}

#[test]
fn entity_fields_aggregations() {
    let fields = Person::fields();
    assert!(matches!(fields.age.sum(), Agg::Sum(ref a) if a == "age"));
    assert!(matches!(fields.age.min(), Agg::Min(ref a) if a == "age"));
    assert!(matches!(fields.age.max(), Agg::Max(ref a) if a == "age"));
    assert!(matches!(fields.age.mean(), Agg::Mean(ref a) if a == "age"));
}

#[test]
fn relation_fields_work() {
    let fields = Employment::fields();
    let expr = fields.position.eq(Position("Engineer".into()));
    match expr {
        Expr::Eq { attr, value } => {
            assert_eq!(attr, "position");
            assert_eq!(value, AttributeValue::String("Engineer".into()));
        }
        _ => panic!("expected Eq"),
    }
}

#[test]
fn relation_fields_have_role_refs() {
    let fields = Employment::fields();
    let expr = fields.employee.attr::<Age>("age").gte(Age(30));
    match expr {
        Expr::RolePlayer { role, inner } => {
            assert_eq!(role, "employee");
            match *inner {
                Expr::Gte { ref attr, ref value } => {
                    assert_eq!(attr, "age");
                    assert_eq!(*value, AttributeValue::Long(30));
                }
                _ => panic!("expected Gte inside RolePlayer"),
            }
        }
        _ => panic!("expected RolePlayer"),
    }
}

#[test]
fn relation_fields_employer_role_ref() {
    let fields = Employment::fields();
    let expr = fields.employer.attr::<Name>("name").contains("Corp");
    match expr {
        Expr::RolePlayer { role, inner } => {
            assert_eq!(role, "employer");
            match *inner {
                Expr::Contains { ref attr, ref substring } => {
                    assert_eq!(attr, "name");
                    assert_eq!(substring, "Corp");
                }
                _ => panic!("expected Contains inside RolePlayer"),
            }
        }
        _ => panic!("expected RolePlayer"),
    }
}
