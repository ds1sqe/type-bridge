#![allow(dead_code)]

use std::env;
use std::process;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Map as JsonMap, Value as JsonValue};
use type_bridge_orm::*;

use super::typedb::ensure_database_exists;

static NEXT_SCHEMA_ID: AtomicU64 = AtomicU64::new(1);

pub struct DynamicCrudSchema {
    pub person_type: String,
    pub company_type: String,
    pub employment_type: String,
    pub name_attr: String,
    pub company_name_attr: String,
    pub age_attr: String,
    pub score_attr: String,
    pub active_attr: String,
    pub birthday_attr: String,
    pub login_at_attr: String,
    pub seen_at_attr: String,
    pub balance_attr: String,
    pub session_length_attr: String,
    pub since_attr: String,
}

impl DynamicCrudSchema {
    pub fn new(scope: &str) -> Self {
        let id = NEXT_SCHEMA_ID.fetch_add(1, Ordering::SeqCst);
        let suffix = format!("rust-{scope}-{}-{id}", process::id());
        Self {
            person_type: format!("{suffix}-person"),
            company_type: format!("{suffix}-company"),
            employment_type: format!("{suffix}-employment"),
            name_attr: format!("{suffix}-name"),
            company_name_attr: format!("{suffix}-company-name"),
            age_attr: format!("{suffix}-age"),
            score_attr: format!("{suffix}-score"),
            active_attr: format!("{suffix}-active"),
            birthday_attr: format!("{suffix}-birthday"),
            login_at_attr: format!("{suffix}-login-at"),
            seen_at_attr: format!("{suffix}-seen-at"),
            balance_attr: format!("{suffix}-balance"),
            session_length_attr: format!("{suffix}-session-length"),
            since_attr: format!("{suffix}-since"),
        }
    }

    pub fn define_typeql(&self) -> String {
        format!(
            r#"define
attribute {name_attr}, value string;
attribute {company_name_attr}, value string;
attribute {age_attr}, value integer;
attribute {score_attr}, value double;
attribute {active_attr}, value boolean;
attribute {birthday_attr}, value date;
attribute {login_at_attr}, value datetime;
attribute {seen_at_attr}, value datetime-tz;
attribute {balance_attr}, value decimal;
attribute {session_length_attr}, value duration;
attribute {since_attr}, value date;
entity {person_type}, owns {name_attr} @key, owns {age_attr} @card(0..5), owns {score_attr} @card(0..5), owns {active_attr} @card(0..5), owns {birthday_attr} @card(0..5), owns {login_at_attr} @card(0..5), owns {seen_at_attr} @card(0..5), owns {balance_attr} @card(0..5), owns {session_length_attr} @card(0..5), plays {employment_type}:employee;
entity {company_type}, owns {company_name_attr} @key, plays {employment_type}:employer;
relation {employment_type}, relates employee, relates employer, owns {since_attr} @card(0..5);
"#,
            name_attr = self.name_attr,
            company_name_attr = self.company_name_attr,
            age_attr = self.age_attr,
            score_attr = self.score_attr,
            active_attr = self.active_attr,
            birthday_attr = self.birthday_attr,
            login_at_attr = self.login_at_attr,
            seen_at_attr = self.seen_at_attr,
            balance_attr = self.balance_attr,
            session_length_attr = self.session_length_attr,
            since_attr = self.since_attr,
            person_type = self.person_type,
            company_type = self.company_type,
            employment_type = self.employment_type,
        )
    }

    pub fn person_descriptor(&self) -> Arc<EntityDescriptor> {
        Arc::new(EntityDescriptor {
            type_name: self.person_type.clone(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: vec![
                attr("name", &self.name_attr, ValueType::String, true),
                attr("age", &self.age_attr, ValueType::Long, false),
                attr("score", &self.score_attr, ValueType::Double, false),
                attr("active", &self.active_attr, ValueType::Boolean, false),
                attr("birthday", &self.birthday_attr, ValueType::Date, false),
                attr("login_at", &self.login_at_attr, ValueType::DateTime, false),
                attr("seen_at", &self.seen_at_attr, ValueType::DateTimeTz, false),
                attr("balance", &self.balance_attr, ValueType::Decimal, false),
                attr(
                    "session_length",
                    &self.session_length_attr,
                    ValueType::Duration,
                    false,
                ),
            ],
        })
    }

    pub fn company_descriptor(&self) -> Arc<EntityDescriptor> {
        Arc::new(EntityDescriptor {
            type_name: self.company_type.clone(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: vec![attr(
                "name",
                &self.company_name_attr,
                ValueType::String,
                true,
            )],
        })
    }

    pub fn employment_descriptor(&self) -> Arc<RelationDescriptor> {
        Arc::new(RelationDescriptor {
            type_name: self.employment_type.clone(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: vec![attr("since", &self.since_attr, ValueType::Date, false)],
            roles: vec![
                RoleDescriptor {
                    role_name: "employee".into(),
                    player_type_names: vec![self.person_type.clone()],
                    cardinality: Some((1, Some(1))),
                    ..Default::default()
                },
                RoleDescriptor {
                    role_name: "employer".into(),
                    player_type_names: vec![self.company_type.clone()],
                    cardinality: Some((1, Some(1))),
                    ..Default::default()
                },
            ],
        })
    }
}

pub fn unique_schema_suffix(prefix: &str, scope: &str) -> String {
    let id = NEXT_SCHEMA_ID.fetch_add(1, Ordering::SeqCst);
    format!("{prefix}-{scope}-{}-{id}", process::id())
}

pub async fn setup_dynamic_database(scope: &str) -> (Database, DynamicCrudSchema) {
    let schema = DynamicCrudSchema::new(scope);
    let db = connect_dynamic_database().await;

    db.execute_raw(&schema.define_typeql(), TxType::Schema)
        .await
        .unwrap_or_else(|error| panic!("Rust dynamic integration schema define failed: {error}"));

    (db, schema)
}

pub async fn setup_dynamic_typeql(typeql: &str) -> Database {
    let db = connect_dynamic_database().await;

    db.execute_raw(typeql, TxType::Schema)
        .await
        .unwrap_or_else(|error| panic!("Rust dynamic integration schema define failed: {error}"));

    db
}

async fn connect_dynamic_database() -> Database {
    let address = env::var("TYPEDB_ADDRESS").unwrap_or_else(|_| "localhost:1730".to_string());
    let database =
        env::var("TYPE_BRIDGE_RUST_INTG_DATABASE").unwrap_or_else(|_| "type_bridge_test".into());
    let username = env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".to_string());
    let password = env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".to_string());

    ensure_database_exists(
        &address,
        &database,
        &username,
        &password,
        "Rust dynamic integration",
    )
    .await;

    Database::connect(&address, &database, &username, &password)
        .await
        .unwrap_or_else(|error| {
            panic!(
                "Rust dynamic integration requires TypeDB at {address} \
                 database {database}: {error}"
            )
        })
}

pub fn person_attrs(name: &str, age: i64) -> DynamicAttributeMap {
    vec![
        ("name".into(), AttributeValue::String(name.into())),
        ("age".into(), AttributeValue::Long(age)),
    ]
}

pub fn all_value_attrs(name: &str, age: i64) -> DynamicAttributeMap {
    vec![
        ("name".into(), AttributeValue::String(name.into())),
        ("age".into(), AttributeValue::Long(age)),
        ("score".into(), AttributeValue::Double(91.25)),
        ("active".into(), AttributeValue::Boolean(true)),
        ("birthday".into(), AttributeValue::Date("1990-01-02".into())),
        (
            "login_at".into(),
            AttributeValue::DateTime("2026-05-27T10:30:00".into()),
        ),
        (
            "seen_at".into(),
            AttributeValue::DateTimeTZ("2026-05-27T10:30:00+00:00".into()),
        ),
        ("balance".into(), AttributeValue::Decimal("1234.56".into())),
        (
            "session_length".into(),
            AttributeValue::Duration("PT2H30M".into()),
        ),
    ]
}

pub fn company_attrs(name: &str) -> DynamicAttributeMap {
    vec![("name".into(), AttributeValue::String(name.into()))]
}

pub fn relation_attrs(since: &str) -> DynamicAttributeMap {
    vec![("since".into(), AttributeValue::Date(since.into()))]
}

pub fn role_players(
    schema: &DynamicCrudSchema,
    person_iid: String,
    company_iid: String,
) -> Vec<DynamicRolePlayerInput> {
    vec![
        DynamicRolePlayerInput {
            role_name: "employee".into(),
            player_type_name: schema.person_type.clone(),
            iid: Some(person_iid),
            key: None,
        },
        DynamicRolePlayerInput {
            role_name: "employer".into(),
            player_type_name: schema.company_type.clone(),
            iid: Some(company_iid),
            key: None,
        },
    ]
}

pub fn attr_value<'a>(row: &'a DynamicEntityRow, attr_name: &str) -> Option<&'a AttributeValue> {
    row.attributes
        .iter()
        .find(|(name, _)| name == attr_name)
        .map(|(_, value)| value)
}

pub fn attr_values<'a>(row: &'a DynamicEntityRow, attr_name: &str) -> Vec<&'a AttributeValue> {
    row.attributes
        .iter()
        .filter(|(name, _)| name == attr_name)
        .map(|(_, value)| value)
        .collect()
}

pub fn relation_attr_value<'a>(
    row: &'a DynamicRelationRow,
    attr_name: &str,
) -> Option<&'a AttributeValue> {
    row.attributes
        .iter()
        .find(|(name, _)| name == attr_name)
        .map(|(_, value)| value)
}

pub fn count_aggregate() -> DynamicAggregate {
    DynamicAggregate {
        result_key: "count".into(),
        function: "count".into(),
        attr_name: None,
    }
}

pub fn aggregate_i64(row: &JsonMap<String, JsonValue>, result_key: &str) -> Option<i64> {
    let prefixed = format!("${result_key}");
    for key in [prefixed.as_str(), result_key] {
        let Some(value) = row.get(key) else {
            continue;
        };
        if let Some(number) = value.as_i64() {
            return Some(number);
        }
        if let Some(number) = value.get("value").and_then(JsonValue::as_i64) {
            return Some(number);
        }
        if let Some(number) = value
            .get("value")
            .and_then(JsonValue::as_str)
            .and_then(|value| value.parse().ok())
        {
            return Some(number);
        }
    }
    None
}

pub fn mean_age_aggregate() -> DynamicAggregate {
    DynamicAggregate {
        result_key: "avg_age".into(),
        function: "mean".into(),
        attr_name: Some("age".into()),
    }
}

pub fn attr(
    field_name: &str,
    attr_name: &str,
    value_type: ValueType,
    is_key: bool,
) -> OwnedAttributeDescriptor {
    OwnedAttributeDescriptor {
        field_name: field_name.into(),
        attr_name: attr_name.into(),
        value_type,
        annotations: if is_key {
            vec![Annotation::Key]
        } else {
            vec![Annotation::Card(0, Some(5))]
        },
        is_optional: !is_key,
        is_ordered: false,
    }
}
