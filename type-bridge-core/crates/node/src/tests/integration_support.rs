use std::env;
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};

use super::*;

static NEXT_SCHEMA_ID: AtomicU64 = AtomicU64::new(1);

pub(super) struct NodeCrudSchema {
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

impl NodeCrudSchema {
    pub fn new(scope: &str) -> Self {
        let id = NEXT_SCHEMA_ID.fetch_add(1, Ordering::SeqCst);
        let suffix = format!("node-{scope}-{}-{id}", process::id());
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

    pub fn define_schema_source(&self) -> String {
        format!(
            r#"define
attribute {name_attr}, value string;
attribute {company_name_attr}, value string;
attribute {age_attr}, value long;
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

    pub fn person_descriptor_json(&self) -> String {
        json!({
            "type_name": self.person_type,
            "is_abstract": false,
            "parent_type": null,
            "owned_attributes": [
                {
                    "field_name": "name",
                    "attr_name": self.name_attr,
                    "value_type": "string",
                    "annotations": ["Key"],
                    "is_optional": false
                },
                {
                    "field_name": "age",
                    "attr_name": self.age_attr,
                    "value_type": "long",
                    "annotations": [{"Card": [0, 5]}],
                    "is_optional": true
                },
                {
                    "field_name": "score",
                    "attr_name": self.score_attr,
                    "value_type": "double",
                    "annotations": [{"Card": [0, 5]}],
                    "is_optional": true
                },
                {
                    "field_name": "active",
                    "attr_name": self.active_attr,
                    "value_type": "boolean",
                    "annotations": [{"Card": [0, 5]}],
                    "is_optional": true
                },
                {
                    "field_name": "birthday",
                    "attr_name": self.birthday_attr,
                    "value_type": "date",
                    "annotations": [{"Card": [0, 5]}],
                    "is_optional": true
                },
                {
                    "field_name": "login_at",
                    "attr_name": self.login_at_attr,
                    "value_type": "datetime",
                    "annotations": [{"Card": [0, 5]}],
                    "is_optional": true
                },
                {
                    "field_name": "seen_at",
                    "attr_name": self.seen_at_attr,
                    "value_type": "datetime-tz",
                    "annotations": [{"Card": [0, 5]}],
                    "is_optional": true
                },
                {
                    "field_name": "balance",
                    "attr_name": self.balance_attr,
                    "value_type": "decimal",
                    "annotations": [{"Card": [0, 5]}],
                    "is_optional": true
                },
                {
                    "field_name": "session_length",
                    "attr_name": self.session_length_attr,
                    "value_type": "duration",
                    "annotations": [{"Card": [0, 5]}],
                    "is_optional": true
                }
            ]
        })
        .to_string()
    }

    pub fn company_descriptor_json(&self) -> String {
        json!({
            "type_name": self.company_type,
            "is_abstract": false,
            "parent_type": null,
            "owned_attributes": [
                {
                    "field_name": "name",
                    "attr_name": self.company_name_attr,
                    "value_type": "string",
                    "annotations": ["Key"],
                    "is_optional": false
                }
            ]
        })
        .to_string()
    }

    pub fn employment_descriptor_json(&self) -> String {
        json!({
            "type_name": self.employment_type,
            "is_abstract": false,
            "parent_type": null,
            "owned_attributes": [
                {
                    "field_name": "since",
                    "attr_name": self.since_attr,
                    "value_type": "date",
                    "annotations": [{"Card": [0, 5]}],
                    "is_optional": true
                }
            ],
            "roles": [
                {
                    "role_name": "employee",
                    "player_type_names": [self.person_type],
                    "cardinality": [1, 1]
                },
                {
                    "role_name": "employer",
                    "player_type_names": [self.company_type],
                    "cardinality": [1, 1]
                }
            ]
        })
        .to_string()
    }
}

pub(super) fn unique_schema_suffix(prefix: &str, scope: &str) -> String {
    let id = NEXT_SCHEMA_ID.fetch_add(1, Ordering::SeqCst);
    format!("{prefix}-{scope}-{}-{id}", process::id())
}

pub(super) fn setup_node_database(scope: &str) -> Option<(NodeRustDatabase, NodeCrudSchema)> {
    let schema = NodeCrudSchema::new(scope);
    let db = setup_node_schema(&schema.define_schema_source())?;

    Some((db, schema))
}

pub(super) fn setup_node_schema(schema_source: &str) -> Option<NodeRustDatabase> {
    let address = env::var("TYPEDB_ADDRESS").unwrap_or_else(|_| "localhost:1730".to_string());
    let database =
        env::var("TYPE_BRIDGE_NODE_INTG_DATABASE").unwrap_or_else(|_| "type_bridge_test".into());
    let username = env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".to_string());
    let password = env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".to_string());

    let db = match connect_rust_database(address, database, Some(username), Some(password)) {
        Ok(db) => db,
        Err(error) => {
            eprintln!("Skipping Node integration test: TypeDB connection unavailable ({error})");
            return None;
        }
    };

    let tx = match db.transaction(Some("schema".to_string())) {
        Ok(tx) => tx,
        Err(error) => {
            eprintln!("Skipping Node integration test: schema transaction unavailable ({error})");
            return None;
        }
    };
    if let Err(error) = tx.query_json(schema_source.to_string()) {
        let _ = tx.close();
        eprintln!("Skipping Node integration test: schema define failed ({error})");
        return None;
    }
    if let Err(error) = tx.commit() {
        eprintln!("Skipping Node integration test: schema commit failed ({error})");
        return None;
    }

    Some(db)
}

pub(super) fn attr_string(value: &str) -> Value {
    json!({"value_type": "string", "value": value})
}

pub(super) fn attr_long(value: i64) -> Value {
    json!({"value_type": "long", "value": value.to_string()})
}

pub(super) fn attr_double(value: f64) -> Value {
    json!({"value_type": "double", "value": value})
}

pub(super) fn attr_boolean(value: bool) -> Value {
    json!({"value_type": "boolean", "value": value})
}

pub(super) fn attr_date(value: &str) -> Value {
    json!({"value_type": "date", "value": value})
}

pub(super) fn attr_datetime(value: &str) -> Value {
    json!({"value_type": "datetime", "value": value})
}

pub(super) fn attr_datetimetz(value: &str) -> Value {
    json!({"value_type": "datetime-tz", "value": value})
}

pub(super) fn attr_decimal(value: &str) -> Value {
    json!({"value_type": "decimal", "value": value})
}

pub(super) fn attr_duration(value: &str) -> Value {
    json!({"value_type": "duration", "value": value})
}

pub(super) fn row_attribute<'a>(row: &'a Value, attr_name: &str) -> Option<&'a Value> {
    row.get("attributes")?
        .as_array()?
        .iter()
        .find(|entry| entry.get(0).and_then(Value::as_str) == Some(attr_name))?
        .get(1)
}

pub(super) fn row_attributes<'a>(row: &'a Value, attr_name: &str) -> Vec<&'a Value> {
    row.get("attributes")
        .and_then(Value::as_array)
        .map(|attributes| {
            attributes
                .iter()
                .filter(|entry| entry.get(0).and_then(Value::as_str) == Some(attr_name))
                .filter_map(|entry| entry.get(1))
                .collect()
        })
        .unwrap_or_default()
}
