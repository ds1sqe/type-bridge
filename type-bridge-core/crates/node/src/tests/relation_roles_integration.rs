use serde_json::{Value, json};

use super::integration_support::{
    attr_long, attr_string, row_attribute, setup_node_schema, unique_schema_suffix,
};

struct MultiRoleSchema {
    document_type: String,
    email_type: String,
    trace_type: String,
    document_id_attr: String,
    subject_attr: String,
    label_attr: String,
}

impl MultiRoleSchema {
    fn new() -> Self {
        let suffix = unique_schema_suffix("node", "multi-role");
        Self {
            document_type: format!("{suffix}-document"),
            email_type: format!("{suffix}-email"),
            trace_type: format!("{suffix}-trace"),
            document_id_attr: format!("{suffix}-document-id"),
            subject_attr: format!("{suffix}-subject"),
            label_attr: format!("{suffix}-label"),
        }
    }

    fn define_schema_source(&self) -> String {
        format!(
            r#"define
attribute {document_id_attr}, value string;
attribute {subject_attr}, value string;
attribute {label_attr}, value string;
entity {document_type}, owns {document_id_attr} @key, plays {trace_type}:origin;
entity {email_type}, owns {subject_attr} @key, plays {trace_type}:origin;
relation {trace_type}, relates origin, owns {label_attr};
"#,
            document_id_attr = self.document_id_attr,
            subject_attr = self.subject_attr,
            label_attr = self.label_attr,
            document_type = self.document_type,
            email_type = self.email_type,
            trace_type = self.trace_type,
        )
    }

    fn document_descriptor_json(&self) -> String {
        json!({
            "type_name": self.document_type,
            "is_abstract": false,
            "parent_type": null,
            "owned_attributes": [
                {
                    "field_name": "document_id",
                    "attr_name": self.document_id_attr,
                    "value_type": "string",
                    "annotations": ["Key"],
                    "is_optional": false
                }
            ]
        })
        .to_string()
    }

    fn email_descriptor_json(&self) -> String {
        json!({
            "type_name": self.email_type,
            "is_abstract": false,
            "parent_type": null,
            "owned_attributes": [
                {
                    "field_name": "subject",
                    "attr_name": self.subject_attr,
                    "value_type": "string",
                    "annotations": ["Key"],
                    "is_optional": false
                }
            ]
        })
        .to_string()
    }

    fn trace_descriptor_json(&self) -> String {
        json!({
            "type_name": self.trace_type,
            "is_abstract": false,
            "parent_type": null,
            "owned_attributes": [
                {
                    "field_name": "label",
                    "attr_name": self.label_attr,
                    "value_type": "string",
                    "annotations": [],
                    "is_optional": true
                }
            ],
            "roles": [
                {
                    "role_name": "origin",
                    "player_type_names": [self.document_type, self.email_type],
                    "cardinality": [1, 1]
                }
            ]
        })
        .to_string()
    }
}

struct AbstractRoleSchema {
    token_type: String,
    symptom_type: String,
    problem_type: String,
    issue_type: String,
    origin_type: String,
    token_text_attr: String,
    issue_key_attr: String,
    confidence_attr: String,
}

impl AbstractRoleSchema {
    fn new() -> Self {
        let suffix = unique_schema_suffix("node", "abstract-role");
        Self {
            token_type: format!("{suffix}-token"),
            symptom_type: format!("{suffix}-symptom"),
            problem_type: format!("{suffix}-problem"),
            issue_type: format!("{suffix}-issue"),
            origin_type: format!("{suffix}-token-origin"),
            token_text_attr: format!("{suffix}-token-text"),
            issue_key_attr: format!("{suffix}-issue-key"),
            confidence_attr: format!("{suffix}-confidence"),
        }
    }

    fn define_schema_source(&self) -> String {
        format!(
            r#"define
attribute {token_text_attr}, value string;
attribute {issue_key_attr}, value string;
attribute {confidence_attr}, value long;
entity {token_type} @abstract, owns {token_text_attr} @key, plays {origin_type}:token;
entity {symptom_type} sub {token_type};
entity {problem_type} sub {token_type};
entity {issue_type}, owns {issue_key_attr} @key, plays {origin_type}:issue;
relation {origin_type}, relates token, relates issue, owns {confidence_attr} @card(0..5);
"#,
            token_text_attr = self.token_text_attr,
            issue_key_attr = self.issue_key_attr,
            confidence_attr = self.confidence_attr,
            token_type = self.token_type,
            symptom_type = self.symptom_type,
            problem_type = self.problem_type,
            issue_type = self.issue_type,
            origin_type = self.origin_type,
        )
    }

    fn token_descriptor_json(&self) -> String {
        entity_descriptor_json(&self.token_type, true, None, "text", &self.token_text_attr)
    }

    fn symptom_descriptor_json(&self) -> String {
        entity_descriptor_json(
            &self.symptom_type,
            false,
            Some(&self.token_type),
            "text",
            &self.token_text_attr,
        )
    }

    fn problem_descriptor_json(&self) -> String {
        entity_descriptor_json(
            &self.problem_type,
            false,
            Some(&self.token_type),
            "text",
            &self.token_text_attr,
        )
    }

    fn issue_descriptor_json(&self) -> String {
        entity_descriptor_json(&self.issue_type, false, None, "key", &self.issue_key_attr)
    }

    fn origin_descriptor_json(&self) -> String {
        json!({
            "type_name": self.origin_type,
            "is_abstract": false,
            "parent_type": null,
            "owned_attributes": [
                {
                    "field_name": "confidence",
                    "attr_name": self.confidence_attr,
                    "value_type": "long",
                    "annotations": [{"Card": [0, 5]}],
                    "is_optional": true
                }
            ],
            "roles": [
                {
                    "role_name": "token",
                    "player_type_names": [self.token_type],
                    "cardinality": [1, 1]
                },
                {
                    "role_name": "issue",
                    "player_type_names": [self.issue_type],
                    "cardinality": [1, 1]
                }
            ]
        })
        .to_string()
    }
}

#[test]
#[ignore = "requires a running TypeDB database; uses TYPEDB_ADDRESS and TYPE_BRIDGE_NODE_INTG_DATABASE"]
fn node_relation_single_role_accepts_multiple_player_types_against_typedb() {
    let schema = MultiRoleSchema::new();
    let Some(db) = setup_node_schema(&schema.define_schema_source()) else {
        return;
    };
    let document_manager = db
        .entity_manager_json(schema.document_descriptor_json())
        .expect("document manager should be created");
    let email_manager = db
        .entity_manager_json(schema.email_descriptor_json())
        .expect("email manager should be created");
    let trace_manager = db
        .relation_manager_json(schema.trace_descriptor_json())
        .expect("trace manager should be created");

    let doc_iid = document_manager
        .insert_json(json!({"document_id": attr_string("doc-001")}).to_string())
        .expect("document insert should return IID");
    let email_iid = email_manager
        .insert_json(json!({"subject": attr_string("Important")}).to_string())
        .expect("email insert should return IID");

    let doc_role = json!([
        {"role_name": "origin", "player_type_name": schema.document_type, "iid": doc_iid}
    ]);
    let email_role = json!([
        {"role_name": "origin", "player_type_name": schema.email_type, "iid": email_iid}
    ]);
    trace_manager
        .insert_json(
            json!({"label": attr_string("from-document")}).to_string(),
            doc_role.to_string(),
        )
        .expect("document-origin trace insert should work");
    let email_trace_iid = trace_manager
        .insert_json(
            json!({"label": attr_string("from-email")}).to_string(),
            email_role.to_string(),
        )
        .expect("email-origin trace insert should work");

    let doc_rows: Value = serde_json::from_str(
        &trace_manager
            .get_with_role_players_json(None, Some(doc_role.to_string()))
            .expect("document role filter should return rows"),
    )
    .expect("document rows should be JSON");
    assert_eq!(doc_rows.as_array().expect("rows are an array").len(), 1);
    assert_eq!(doc_rows[0]["role_players"][0]["role_name"], "origin");
    assert_eq!(
        doc_rows[0]["role_players"][0]["player_type_name"],
        schema.document_type
    );

    trace_manager
        .update_json(
            json!({"label": attr_string("updated-email")}).to_string(),
            json!([]).to_string(),
            Some(email_trace_iid.clone()),
        )
        .expect("multi-player-type relation should update by IID");
    let email_rows: Value = serde_json::from_str(
        &trace_manager
            .get_with_role_players_json(None, Some(email_role.to_string()))
            .expect("email role filter should return rows"),
    )
    .expect("email rows should be JSON");
    assert_eq!(
        row_attribute(&email_rows[0], &schema.label_attr),
        Some(&json!({"String": "updated-email"}))
    );

    trace_manager
        .delete_by_iid(email_trace_iid)
        .expect("multi-player-type relation should delete by IID");
    let remaining: Value = serde_json::from_str(
        &trace_manager
            .get_json(None)
            .expect("remaining traces should fetch"),
    )
    .expect("remaining rows should be JSON");
    assert_eq!(remaining.as_array().expect("rows are an array").len(), 1);
}

#[test]
#[ignore = "requires a running TypeDB database; uses TYPEDB_ADDRESS and TYPE_BRIDGE_NODE_INTG_DATABASE"]
fn node_relation_abstract_role_resolves_concrete_players_against_typedb() {
    let schema = AbstractRoleSchema::new();
    let Some(db) = setup_node_schema(&schema.define_schema_source()) else {
        return;
    };
    let token_manager = db
        .entity_manager_json(schema.token_descriptor_json())
        .expect("token manager should be created");
    let symptom_manager = db
        .entity_manager_json(schema.symptom_descriptor_json())
        .expect("symptom manager should be created");
    let problem_manager = db
        .entity_manager_json(schema.problem_descriptor_json())
        .expect("problem manager should be created");
    let issue_manager = db
        .entity_manager_json(schema.issue_descriptor_json())
        .expect("issue manager should be created");
    let origin_manager = db
        .relation_manager_json(schema.origin_descriptor_json())
        .expect("origin manager should be created");

    let symptom_iid = symptom_manager
        .insert_json(json!({"text": attr_string("fever")}).to_string())
        .expect("symptom insert should return IID");
    let problem_iid = problem_manager
        .insert_json(json!({"text": attr_string("infection")}).to_string())
        .expect("problem insert should return IID");
    let issue_iid = issue_manager
        .insert_json(json!({"key": attr_string("ISSUE-1")}).to_string())
        .expect("issue insert should return IID");

    let symptom_roles = json!([
        {"role_name": "token", "player_type_name": schema.symptom_type, "iid": symptom_iid},
        {"role_name": "issue", "player_type_name": schema.issue_type, "iid": issue_iid}
    ]);
    let problem_roles = json!([
        {"role_name": "token", "player_type_name": schema.problem_type, "iid": problem_iid},
        {"role_name": "issue", "player_type_name": schema.issue_type, "iid": issue_iid}
    ]);
    origin_manager
        .insert_json(
            json!({"confidence": attr_long(70)}).to_string(),
            symptom_roles.to_string(),
        )
        .expect("symptom-origin insert should work");
    origin_manager
        .insert_json(
            json!({"confidence": attr_long(90)}).to_string(),
            problem_roles.to_string(),
        )
        .expect("problem-origin insert should work");

    let tokens: Value = serde_json::from_str(
        &token_manager
            .get_json(None)
            .expect("abstract token manager should query concrete subtypes"),
    )
    .expect("token rows should be JSON");
    let token_types: Vec<_> = tokens
        .as_array()
        .expect("rows are an array")
        .iter()
        .filter_map(|row| row["type_name"].as_str())
        .collect();
    assert!(token_types.contains(&schema.symptom_type.as_str()));
    assert!(token_types.contains(&schema.problem_type.as_str()));

    let symptom_origins: Value = serde_json::from_str(
        &origin_manager
            .get_with_role_players_json(None, Some(symptom_roles.to_string()))
            .expect("abstract role filter should accept concrete symptom"),
    )
    .expect("origin rows should be JSON");
    assert_eq!(
        row_attribute(&symptom_origins[0], &schema.confidence_attr),
        Some(&json!({"Long": "70"}))
    );

    let all_origins: Value = serde_json::from_str(
        &origin_manager
            .get_json(None)
            .expect("origins should fetch with role players"),
    )
    .expect("all origins should be JSON");
    let token_player_types: Vec<_> = all_origins
        .as_array()
        .expect("rows are an array")
        .iter()
        .flat_map(|row| row["role_players"].as_array().into_iter().flatten())
        .filter(|player| player["role_name"] == "token")
        .filter_map(|player| player["player_type_name"].as_str())
        .collect();
    assert!(token_player_types.contains(&schema.symptom_type.as_str()));
    assert!(token_player_types.contains(&schema.problem_type.as_str()));
}

fn entity_descriptor_json(
    type_name: &str,
    is_abstract: bool,
    parent_type: Option<&str>,
    field_name: &str,
    attr_name: &str,
) -> String {
    json!({
        "type_name": type_name,
        "is_abstract": is_abstract,
        "parent_type": parent_type,
        "owned_attributes": [
            {
                "field_name": field_name,
                "attr_name": attr_name,
                "value_type": "string",
                "annotations": ["Key"],
                "is_optional": false
            }
        ]
    })
    .to_string()
}
