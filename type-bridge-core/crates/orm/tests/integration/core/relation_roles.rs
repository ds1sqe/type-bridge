//! Dynamic relation role-shape integration tests against TypeDB.

use std::sync::Arc;

use crate::common::dynamic_crud::*;
use type_bridge_orm::*;

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
        let suffix = unique_schema_suffix("rust", "multi-role");
        Self {
            document_type: format!("{suffix}-document"),
            email_type: format!("{suffix}-email"),
            trace_type: format!("{suffix}-trace"),
            document_id_attr: format!("{suffix}-document-id"),
            subject_attr: format!("{suffix}-subject"),
            label_attr: format!("{suffix}-label"),
        }
    }

    fn define_typeql(&self) -> String {
        format!(
            r#"define
attribute {document_id_attr}, value string;
attribute {subject_attr}, value string;
attribute {label_attr}, value string;
entity {document_type}, owns {document_id_attr} @key, plays {trace_type}:origin;
entity {email_type}, owns {subject_attr} @key, plays {trace_type}:origin;
relation {trace_type}, relates origin @card(0..2), owns {label_attr};
"#,
            document_id_attr = self.document_id_attr,
            subject_attr = self.subject_attr,
            label_attr = self.label_attr,
            document_type = self.document_type,
            email_type = self.email_type,
            trace_type = self.trace_type,
        )
    }

    fn document_descriptor(&self) -> Arc<EntityDescriptor> {
        Arc::new(EntityDescriptor {
            type_name: self.document_type.clone(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: vec![attr(
                "document_id",
                &self.document_id_attr,
                ValueType::String,
                true,
            )],
            doc: None,
            meta: Default::default(),
        })
    }

    fn email_descriptor(&self) -> Arc<EntityDescriptor> {
        Arc::new(EntityDescriptor {
            type_name: self.email_type.clone(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: vec![attr("subject", &self.subject_attr, ValueType::String, true)],
            doc: None,
            meta: Default::default(),
        })
    }

    fn trace_descriptor(&self) -> Arc<RelationDescriptor> {
        Arc::new(RelationDescriptor {
            type_name: self.trace_type.clone(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: vec![attr("label", &self.label_attr, ValueType::String, false)],
            roles: vec![RoleDescriptor {
                role_name: "origin".into(),
                player_type_names: vec![self.document_type.clone(), self.email_type.clone()],
                cardinality: Some((1, Some(2))),
                ..Default::default()
            }],
            doc: None,
            meta: Default::default(),
        })
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
        let suffix = unique_schema_suffix("rust", "abstract-role");
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

    fn define_typeql(&self) -> String {
        format!(
            r#"define
attribute {token_text_attr}, value string;
attribute {issue_key_attr}, value string;
attribute {confidence_attr}, value integer;
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

    fn token_descriptor(&self) -> Arc<EntityDescriptor> {
        Arc::new(EntityDescriptor {
            type_name: self.token_type.clone(),
            is_abstract: true,
            parent_type: None,
            owned_attributes: vec![attr("text", &self.token_text_attr, ValueType::String, true)],
            doc: None,
            meta: Default::default(),
        })
    }

    fn symptom_descriptor(&self) -> Arc<EntityDescriptor> {
        self.token_subtype_descriptor(&self.symptom_type)
    }

    fn problem_descriptor(&self) -> Arc<EntityDescriptor> {
        self.token_subtype_descriptor(&self.problem_type)
    }

    fn token_subtype_descriptor(&self, type_name: &str) -> Arc<EntityDescriptor> {
        Arc::new(EntityDescriptor {
            type_name: type_name.into(),
            is_abstract: false,
            parent_type: Some(self.token_type.clone()),
            owned_attributes: vec![attr("text", &self.token_text_attr, ValueType::String, true)],
            doc: None,
            meta: Default::default(),
        })
    }

    fn issue_descriptor(&self) -> Arc<EntityDescriptor> {
        Arc::new(EntityDescriptor {
            type_name: self.issue_type.clone(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: vec![attr("key", &self.issue_key_attr, ValueType::String, true)],
            doc: None,
            meta: Default::default(),
        })
    }

    fn origin_descriptor(&self) -> Arc<RelationDescriptor> {
        Arc::new(RelationDescriptor {
            type_name: self.origin_type.clone(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: vec![attr(
                "confidence",
                &self.confidence_attr,
                ValueType::Long,
                false,
            )],
            roles: vec![
                RoleDescriptor {
                    role_name: "token".into(),
                    player_type_names: vec![self.token_type.clone()],
                    cardinality: Some((1, Some(1))),
                    ..Default::default()
                },
                RoleDescriptor {
                    role_name: "issue".into(),
                    player_type_names: vec![self.issue_type.clone()],
                    cardinality: Some((1, Some(1))),
                    ..Default::default()
                },
            ],
            doc: None,
            meta: Default::default(),
        })
    }
}

#[tokio::test]
async fn dynamic_relation_single_role_accepts_multiple_player_types_against_typedb() {
    let _guard = crate::common::integration_test_guard().await;
    let schema = MultiRoleSchema::new();
    let db = setup_dynamic_typeql(&schema.define_typeql()).await;
    let document_manager = DynamicEntityManager::new(&db, schema.document_descriptor());
    let email_manager = DynamicEntityManager::new(&db, schema.email_descriptor());
    let trace_manager = DynamicRelationManager::new(&db, schema.trace_descriptor());

    let doc_iid = document_manager
        .insert(&vec![(
            "document_id".into(),
            AttributeValue::String("doc-001".into()),
        )])
        .await
        .expect("document insert should return IID");
    let email_iid = email_manager
        .insert(&vec![(
            "subject".into(),
            AttributeValue::String("Important".into()),
        )])
        .await
        .expect("email insert should return IID");

    let doc_role = vec![role_player("origin", &schema.document_type, &doc_iid)];
    let email_role = vec![role_player("origin", &schema.email_type, &email_iid)];
    trace_manager
        .insert(
            &vec![(
                "label".into(),
                AttributeValue::String("from-document".into()),
            )],
            &doc_role,
        )
        .await
        .expect("document-origin trace insert should work");
    let email_trace_iid = trace_manager
        .insert(
            &vec![("label".into(), AttributeValue::String("from-email".into()))],
            &email_role,
        )
        .await
        .expect("email-origin trace insert should work");

    let doc_rows = trace_manager
        .get_with_role_filters(&[], &doc_role)
        .await
        .expect("document role filter should return rows");
    assert_eq!(doc_rows.len(), 1);
    assert_eq!(doc_rows[0].role_players[0].role_name, "origin");
    assert_eq!(
        doc_rows[0].role_players[0].player_type_name.as_deref(),
        Some(schema.document_type.as_str())
    );

    trace_manager
        .update(
            Some(email_trace_iid.as_str()),
            &vec![(
                "label".into(),
                AttributeValue::String("updated-email".into()),
            )],
            &[],
        )
        .await
        .expect("multi-player-type relation should update by IID");
    let email_rows = trace_manager
        .get_with_role_filters(&[], &email_role)
        .await
        .expect("email role filter should return rows");
    assert_eq!(
        relation_attr_value(&email_rows[0], &schema.label_attr),
        Some(&AttributeValue::String("updated-email".into()))
    );

    trace_manager
        .delete_by_iid(email_trace_iid.as_str())
        .await
        .expect("multi-player-type relation should delete by IID");
    let remaining = trace_manager.all().await.expect("remaining traces fetch");
    assert_eq!(remaining.len(), 1);
    assert_eq!(
        relation_attr_value(&remaining[0], &schema.label_attr),
        Some(&AttributeValue::String("from-document".into()))
    );
}

#[tokio::test]
async fn dynamic_relation_single_role_hydrates_multiple_players_against_typedb() {
    let _guard = crate::common::integration_test_guard().await;
    let schema = MultiRoleSchema::new();
    let db = setup_dynamic_typeql(&schema.define_typeql()).await;
    let document_manager = DynamicEntityManager::new(&db, schema.document_descriptor());
    let email_manager = DynamicEntityManager::new(&db, schema.email_descriptor());
    let trace_manager = DynamicRelationManager::new(&db, schema.trace_descriptor());

    let doc_iid = document_manager
        .insert(&vec![(
            "document_id".into(),
            AttributeValue::String("doc-multi".into()),
        )])
        .await
        .expect("document insert should return IID");
    let email_iid = email_manager
        .insert(&vec![(
            "subject".into(),
            AttributeValue::String("Multi origin".into()),
        )])
        .await
        .expect("email insert should return IID");

    let roles = vec![
        role_player("origin", &schema.document_type, &doc_iid),
        role_player("origin", &schema.email_type, &email_iid),
    ];
    trace_manager
        .insert(
            &vec![("label".into(), AttributeValue::String("multi".into()))],
            &roles,
        )
        .await
        .expect("same-role multi-player trace insert should work");

    let rows = trace_manager
        .get(&[Filter::string_eq("label", "multi")])
        .await
        .expect("same-role multi-player trace should fetch");
    assert!(
        rows.iter().all(|row| row.iid == rows[0].iid),
        "same relation may be returned once per role-player binding"
    );

    let mut origin_types: Vec<_> = rows
        .iter()
        .flat_map(|row| row.role_players.iter())
        .filter(|player| player.role_name == "origin")
        .filter_map(|player| player.player_type_name.as_deref())
        .collect();
    origin_types.sort_unstable();
    origin_types.dedup();
    assert_eq!(origin_types.len(), 2);
    assert!(origin_types.contains(&schema.document_type.as_str()));
    assert!(origin_types.contains(&schema.email_type.as_str()));
}

#[tokio::test]
async fn dynamic_relation_abstract_role_resolves_concrete_players_against_typedb() {
    let _guard = crate::common::integration_test_guard().await;
    let schema = AbstractRoleSchema::new();
    let db = setup_dynamic_typeql(&schema.define_typeql()).await;
    let token_manager = DynamicEntityManager::new(&db, schema.token_descriptor());
    let symptom_manager = DynamicEntityManager::new(&db, schema.symptom_descriptor());
    let problem_manager = DynamicEntityManager::new(&db, schema.problem_descriptor());
    let issue_manager = DynamicEntityManager::new(&db, schema.issue_descriptor());
    let origin_manager = DynamicRelationManager::new(&db, schema.origin_descriptor());

    let symptom_iid = symptom_manager
        .insert(&vec![(
            "text".into(),
            AttributeValue::String("fever".into()),
        )])
        .await
        .expect("symptom insert should return IID");
    let problem_iid = problem_manager
        .insert(&vec![(
            "text".into(),
            AttributeValue::String("infection".into()),
        )])
        .await
        .expect("problem insert should return IID");
    let issue_iid = issue_manager
        .insert(&vec![(
            "key".into(),
            AttributeValue::String("ISSUE-1".into()),
        )])
        .await
        .expect("issue insert should return IID");

    let symptom_roles = vec![
        role_player("token", &schema.symptom_type, &symptom_iid),
        role_player("issue", &schema.issue_type, &issue_iid),
    ];
    let problem_roles = vec![
        role_player("token", &schema.problem_type, &problem_iid),
        role_player("issue", &schema.issue_type, &issue_iid),
    ];
    origin_manager
        .insert(
            &vec![("confidence".into(), AttributeValue::Long(70))],
            &symptom_roles,
        )
        .await
        .expect("symptom-origin insert should work");
    origin_manager
        .insert(
            &vec![("confidence".into(), AttributeValue::Long(90))],
            &problem_roles,
        )
        .await
        .expect("problem-origin insert should work");

    let tokens = token_manager
        .all()
        .await
        .expect("abstract token manager should query concrete subtypes");
    let token_types: Vec<_> = tokens
        .iter()
        .filter_map(|row| row.type_name.as_deref())
        .collect();
    assert!(token_types.contains(&schema.symptom_type.as_str()));
    assert!(token_types.contains(&schema.problem_type.as_str()));

    let symptom_origins = origin_manager
        .get_with_role_filters(&[], &symptom_roles)
        .await
        .expect("abstract role filter should accept concrete symptom");
    assert_eq!(symptom_origins.len(), 1);
    assert_eq!(
        relation_attr_value(&symptom_origins[0], &schema.confidence_attr),
        Some(&AttributeValue::Long(70))
    );

    let all_origins = origin_manager
        .all()
        .await
        .expect("origins should fetch with role players");
    let concrete_token_types: Vec<_> = all_origins
        .iter()
        .flat_map(|row| row.role_players.iter())
        .filter(|player| player.role_name == "token")
        .filter_map(|player| player.player_type_name.as_deref())
        .collect();
    assert!(concrete_token_types.contains(&schema.symptom_type.as_str()));
    assert!(concrete_token_types.contains(&schema.problem_type.as_str()));
}

fn role_player(role_name: &str, player_type_name: &str, iid: &str) -> DynamicRolePlayerInput {
    DynamicRolePlayerInput {
        role_name: role_name.into(),
        player_type_name: player_type_name.into(),
        iid: Some(iid.into()),
        key: None,
    }
}
