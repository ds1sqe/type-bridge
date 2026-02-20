//! Dynamic query builder: converts CRUD requests into AST clauses.
//!
//! Uses [`type_bridge_core_lib::ast`] and [`TypeSchema`] to build
//! validated, schema-aware queries without depending on the ORM crate.

use std::collections::HashMap;

use type_bridge_core_lib::ast::*;
use type_bridge_core_lib::schema::TypeSchema;

use crate::error::PipelineError;

use super::types::*;

/// Convert a JSON value + value_type into an AST [`Value::Literal`].
fn to_literal(spec: &AttributeValueSpec) -> Value {
    Value::Literal(LiteralValue {
        value: spec.value.clone(),
        value_type: spec.value_type.clone(),
    })
}

/// Validate that `type_name` exists as an entity in the schema.
fn require_entity(schema: &TypeSchema, type_name: &str) -> Result<(), PipelineError> {
    if schema.get_entity(type_name).is_none() {
        return Err(PipelineError::Validation(format!(
            "Unknown entity type: '{type_name}'"
        )));
    }
    Ok(())
}

/// Validate that `type_name` exists as a relation in the schema.
fn require_relation(schema: &TypeSchema, type_name: &str) -> Result<(), PipelineError> {
    if schema.get_relation(type_name).is_none() {
        return Err(PipelineError::Validation(format!(
            "Unknown relation type: '{type_name}'"
        )));
    }
    Ok(())
}

/// Validate that `attr_name` is owned by `type_name` (entity or relation).
fn require_owned_attribute(
    schema: &TypeSchema,
    type_name: &str,
    attr_name: &str,
) -> Result<(), PipelineError> {
    let owned = schema.get_all_owned_attributes(type_name);
    if !owned.iter().any(|a| a.name == attr_name) {
        return Err(PipelineError::Validation(format!(
            "Type '{type_name}' does not own attribute '{attr_name}'"
        )));
    }
    Ok(())
}

/// Build INSERT clauses for an entity.
///
/// Produces:
/// ```text
/// insert
/// $e isa <type_name>,
///   has <attr1> <val1>,
///   has <attr2> <val2>;
/// ```
pub fn build_entity_insert(
    type_name: &str,
    attributes: &HashMap<String, AttributeValueSpec>,
    schema: &TypeSchema,
) -> Result<Vec<Clause>, PipelineError> {
    require_entity(schema, type_name)?;

    for attr_name in attributes.keys() {
        require_owned_attribute(schema, type_name, attr_name)?;
    }

    let mut statements = vec![Statement::Isa {
        variable: "$e".to_string(),
        type_name: type_name.to_string(),
    }];

    for (attr_name, spec) in attributes {
        statements.push(Statement::Has {
            subject_var: "$e".to_string(),
            attr_name: attr_name.clone(),
            value: to_literal(spec),
        });
    }

    Ok(vec![Clause::Insert(statements)])
}

/// Build MATCH + FETCH clauses for listing entities.
///
/// Produces:
/// ```text
/// match
/// $e isa <type_name>;
/// [filter constraints...]
/// fetch
/// $e: *;
/// [sort/limit/offset]
/// ```
pub fn build_entity_fetch(
    type_name: &str,
    filters: &[FilterSpec],
    sort: &[SortSpec],
    limit: Option<u64>,
    offset: Option<u64>,
    schema: &TypeSchema,
) -> Result<Vec<Clause>, PipelineError> {
    require_entity(schema, type_name)?;

    let mut patterns: Vec<Pattern> = vec![Pattern::Entity {
        variable: "$e".to_string(),
        type_name: type_name.to_string(),
        constraints: vec![],
        is_strict: false,
    }];

    // Add filter constraints as Has patterns + ValueComparison
    for (i, f) in filters.iter().enumerate() {
        require_owned_attribute(schema, type_name, &f.attr)?;
        let attr_var = format!("$_attr_{i}");
        patterns.push(Pattern::Has {
            thing_var: "$e".to_string(),
            attr_type: f.attr.clone(),
            attr_var: attr_var.clone(),
        });
        patterns.push(Pattern::ValueComparison {
            var: attr_var,
            operator: f.op.clone(),
            value: to_literal(&f.value),
        });
    }

    let mut clauses = vec![
        Clause::Match(patterns),
        Clause::Fetch(vec![FetchItem::Wildcard {
            key: "$e".to_string(),
            var: "$e".to_string(),
        }]),
    ];

    // Sort
    if !sort.is_empty() {
        let sort_fields: Vec<SortField> = sort
            .iter()
            .map(|s| SortField {
                variable: format!("$_sort_{}", s.attr),
                ascending: s.dir != "desc",
            })
            .collect();
        clauses.push(Clause::Sort(sort_fields));
    }

    if let Some(n) = limit {
        clauses.push(Clause::Limit(n));
    }
    if let Some(n) = offset {
        clauses.push(Clause::Offset(n));
    }

    Ok(clauses)
}

/// Build MATCH + FETCH clauses for fetching a single entity by IID.
pub fn build_entity_fetch_by_iid(
    type_name: &str,
    iid: &str,
    schema: &TypeSchema,
) -> Result<Vec<Clause>, PipelineError> {
    require_entity(schema, type_name)?;

    let patterns = vec![Pattern::Entity {
        variable: "$e".to_string(),
        type_name: type_name.to_string(),
        constraints: vec![Constraint::Iid(iid.to_string())],
        is_strict: false,
    }];

    Ok(vec![
        Clause::Match(patterns),
        Clause::Fetch(vec![FetchItem::Wildcard {
            key: "$e".to_string(),
            var: "$e".to_string(),
        }]),
    ])
}

/// Build MATCH + DELETE clauses for deleting an entity by IID.
pub fn build_entity_delete_by_iid(
    type_name: &str,
    iid: &str,
    schema: &TypeSchema,
) -> Result<Vec<Clause>, PipelineError> {
    require_entity(schema, type_name)?;

    let patterns = vec![Pattern::Entity {
        variable: "$e".to_string(),
        type_name: type_name.to_string(),
        constraints: vec![Constraint::Iid(iid.to_string())],
        is_strict: false,
    }];

    Ok(vec![
        Clause::Match(patterns),
        Clause::Delete(vec![Statement::DeleteThing("$e".to_string())]),
    ])
}

/// Build MATCH + DELETE old attrs + INSERT new attrs for updating an entity by IID.
///
/// Produces:
/// ```text
/// match
/// $e isa <type_name>, iid <iid>;
/// delete
/// $e has <attr1> $old_attr1;
/// insert
/// $e has <attr1> <new_val1>;
/// ```
pub fn build_entity_update_by_iid(
    type_name: &str,
    iid: &str,
    attributes: &HashMap<String, AttributeValueSpec>,
    schema: &TypeSchema,
) -> Result<Vec<Clause>, PipelineError> {
    require_entity(schema, type_name)?;

    for attr_name in attributes.keys() {
        require_owned_attribute(schema, type_name, attr_name)?;
    }

    // Match the entity
    let mut patterns: Vec<Pattern> = vec![Pattern::Entity {
        variable: "$e".to_string(),
        type_name: type_name.to_string(),
        constraints: vec![Constraint::Iid(iid.to_string())],
        is_strict: false,
    }];

    // For each attribute being updated, match the old value
    let mut delete_stmts = Vec::new();
    let mut insert_stmts = Vec::new();

    for (i, (attr_name, spec)) in attributes.iter().enumerate() {
        let old_var = format!("$_old_{i}");
        patterns.push(Pattern::Has {
            thing_var: "$e".to_string(),
            attr_type: attr_name.clone(),
            attr_var: old_var.clone(),
        });
        delete_stmts.push(Statement::Has {
            subject_var: "$e".to_string(),
            attr_name: attr_name.clone(),
            value: Value::Variable(old_var),
        });
        insert_stmts.push(Statement::Has {
            subject_var: "$e".to_string(),
            attr_name: attr_name.clone(),
            value: to_literal(spec),
        });
    }

    Ok(vec![
        Clause::Match(patterns),
        Clause::Delete(delete_stmts),
        Clause::Insert(insert_stmts),
    ])
}

/// Build clauses for inserting a relation with role players.
///
/// First matches each role player (by IID or key attribute), then
/// inserts the relation linking them.
pub fn build_relation_insert(
    type_name: &str,
    role_players: &[RolePlayerSpec],
    attributes: &HashMap<String, AttributeValueSpec>,
    schema: &TypeSchema,
) -> Result<Vec<Clause>, PipelineError> {
    require_relation(schema, type_name)?;

    for attr_name in attributes.keys() {
        require_owned_attribute(schema, type_name, attr_name)?;
    }

    let mut match_patterns: Vec<Pattern> = Vec::new();
    let mut ast_role_players: Vec<RolePlayer> = Vec::new();

    for (i, rp) in role_players.iter().enumerate() {
        let player_var = format!("$_player_{i}");

        // Match the role player entity
        if let Some(ref iid) = rp.iid {
            match_patterns.push(Pattern::Entity {
                variable: player_var.clone(),
                type_name: rp.entity_type.clone(),
                constraints: vec![Constraint::Iid(iid.clone())],
                is_strict: false,
            });
        } else if let (Some(key_attr), Some(key_value)) =
            (&rp.key_attr, &rp.key_value)
        {
            match_patterns.push(Pattern::Entity {
                variable: player_var.clone(),
                type_name: rp.entity_type.clone(),
                constraints: vec![Constraint::Has {
                    attr_name: key_attr.clone(),
                    value: to_literal(key_value),
                }],
                is_strict: false,
            });
        } else {
            return Err(PipelineError::Validation(format!(
                "Role player '{}' must specify either 'iid' or both 'key_attr' and 'key_value'",
                rp.role
            )));
        }

        ast_role_players.push(RolePlayer {
            role: rp.role.clone(),
            player_var,
        });
    }

    // Build insert statement for the relation
    let mut relation_attrs: Vec<Statement> = Vec::new();
    for (attr_name, spec) in attributes {
        relation_attrs.push(Statement::Has {
            subject_var: "$r".to_string(),
            attr_name: attr_name.clone(),
            value: to_literal(spec),
        });
    }

    let insert_stmt = Statement::Relation {
        variable: "$r".to_string(),
        type_name: type_name.to_string(),
        role_players: ast_role_players,
        include_variable: true,
        attributes: relation_attrs,
    };

    let mut clauses = Vec::new();
    if !match_patterns.is_empty() {
        clauses.push(Clause::Match(match_patterns));
    }
    clauses.push(Clause::Insert(vec![insert_stmt]));

    Ok(clauses)
}

/// Build MATCH + FETCH for listing relations.
pub fn build_relation_fetch(
    type_name: &str,
    filters: &[FilterSpec],
    sort: &[SortSpec],
    limit: Option<u64>,
    offset: Option<u64>,
    schema: &TypeSchema,
) -> Result<Vec<Clause>, PipelineError> {
    require_relation(schema, type_name)?;

    let mut patterns: Vec<Pattern> = vec![Pattern::Relation {
        variable: "$r".to_string(),
        type_name: type_name.to_string(),
        role_players: vec![],
        constraints: vec![],
    }];

    for (i, f) in filters.iter().enumerate() {
        require_owned_attribute(schema, type_name, &f.attr)?;
        let attr_var = format!("$_attr_{i}");
        patterns.push(Pattern::Has {
            thing_var: "$r".to_string(),
            attr_type: f.attr.clone(),
            attr_var: attr_var.clone(),
        });
        patterns.push(Pattern::ValueComparison {
            var: attr_var,
            operator: f.op.clone(),
            value: to_literal(&f.value),
        });
    }

    let mut clauses = vec![
        Clause::Match(patterns),
        Clause::Fetch(vec![FetchItem::Wildcard {
            key: "$r".to_string(),
            var: "$r".to_string(),
        }]),
    ];

    if !sort.is_empty() {
        let sort_fields: Vec<SortField> = sort
            .iter()
            .map(|s| SortField {
                variable: format!("$_sort_{}", s.attr),
                ascending: s.dir != "desc",
            })
            .collect();
        clauses.push(Clause::Sort(sort_fields));
    }

    if let Some(n) = limit {
        clauses.push(Clause::Limit(n));
    }
    if let Some(n) = offset {
        clauses.push(Clause::Offset(n));
    }

    Ok(clauses)
}

/// Build MATCH + DELETE clauses for deleting a relation by IID.
pub fn build_relation_delete_by_iid(
    type_name: &str,
    iid: &str,
    schema: &TypeSchema,
) -> Result<Vec<Clause>, PipelineError> {
    require_relation(schema, type_name)?;

    let patterns = vec![Pattern::Relation {
        variable: "$r".to_string(),
        type_name: type_name.to_string(),
        role_players: vec![],
        constraints: vec![Constraint::Iid(iid.to_string())],
    }];

    Ok(vec![
        Clause::Match(patterns),
        Clause::Delete(vec![Statement::DeleteThing("$r".to_string())]),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use type_bridge_core_lib::compiler::QueryCompiler;
    use type_bridge_core_lib::schema::TypeSchema;

    fn test_schema() -> TypeSchema {
        TypeSchema::from_typeql(
            r#"
            define
                attribute name, value string;
                attribute age, value long;
                attribute start-date, value date;
                entity person,
                    owns name @key,
                    owns age;
                entity company,
                    owns name @key;
                relation employment,
                    relates employee,
                    relates employer,
                    owns start-date;
            "#,
        )
        .unwrap()
    }

    fn compile(clauses: &[Clause]) -> String {
        QueryCompiler::new().compile(clauses)
    }

    // =============================================
    // Entity insert tests
    // =============================================

    #[test]
    fn entity_insert_basic() {
        let schema = test_schema();
        let mut attrs = HashMap::new();
        attrs.insert(
            "name".to_string(),
            AttributeValueSpec {
                value: serde_json::json!("Alice"),
                value_type: "string".to_string(),
            },
        );
        attrs.insert(
            "age".to_string(),
            AttributeValueSpec {
                value: serde_json::json!(30),
                value_type: "long".to_string(),
            },
        );
        let clauses = build_entity_insert("person", &attrs, &schema).unwrap();
        assert_eq!(clauses.len(), 1);
        let typeql = compile(&clauses);
        assert!(typeql.contains("insert"));
        assert!(typeql.contains("person"));
    }

    #[test]
    fn entity_insert_unknown_type() {
        let schema = test_schema();
        let attrs = HashMap::new();
        let result = build_entity_insert("nonexistent", &attrs, &schema);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unknown entity type"));
    }

    #[test]
    fn entity_insert_unknown_attribute() {
        let schema = test_schema();
        let mut attrs = HashMap::new();
        attrs.insert(
            "email".to_string(),
            AttributeValueSpec {
                value: serde_json::json!("a@b.com"),
                value_type: "string".to_string(),
            },
        );
        let result = build_entity_insert("person", &attrs, &schema);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not own attribute"));
    }

    // =============================================
    // Entity fetch tests
    // =============================================

    #[test]
    fn entity_fetch_basic() {
        let schema = test_schema();
        let clauses =
            build_entity_fetch("person", &[], &[], None, None, &schema).unwrap();
        assert!(clauses.len() >= 2);
        let typeql = compile(&clauses);
        assert!(typeql.contains("match"));
        assert!(typeql.contains("person"));
        assert!(typeql.contains("fetch"));
    }

    #[test]
    fn entity_fetch_with_limit_offset() {
        let schema = test_schema();
        let clauses =
            build_entity_fetch("person", &[], &[], Some(10), Some(5), &schema).unwrap();
        let typeql = compile(&clauses);
        assert!(typeql.contains("limit 10"));
        assert!(typeql.contains("offset 5"));
    }

    #[test]
    fn entity_fetch_with_filter() {
        let schema = test_schema();
        let filters = vec![FilterSpec {
            attr: "age".to_string(),
            op: ">=".to_string(),
            value: AttributeValueSpec {
                value: serde_json::json!(18),
                value_type: "long".to_string(),
            },
        }];
        let clauses =
            build_entity_fetch("person", &filters, &[], None, None, &schema).unwrap();
        let typeql = compile(&clauses);
        assert!(typeql.contains(">="));
    }

    #[test]
    fn entity_fetch_unknown_filter_attr() {
        let schema = test_schema();
        let filters = vec![FilterSpec {
            attr: "email".to_string(),
            op: "==".to_string(),
            value: AttributeValueSpec {
                value: serde_json::json!("a@b.com"),
                value_type: "string".to_string(),
            },
        }];
        let result = build_entity_fetch("person", &filters, &[], None, None, &schema);
        assert!(result.is_err());
    }

    // =============================================
    // Entity fetch by IID
    // =============================================

    #[test]
    fn entity_fetch_by_iid() {
        let schema = test_schema();
        let clauses = build_entity_fetch_by_iid("person", "0xabc123", &schema).unwrap();
        let typeql = compile(&clauses);
        assert!(typeql.contains("0xabc123"));
        assert!(typeql.contains("person"));
    }

    #[test]
    fn entity_fetch_by_iid_unknown_type() {
        let schema = test_schema();
        let result = build_entity_fetch_by_iid("nonexistent", "0x1", &schema);
        assert!(result.is_err());
    }

    // =============================================
    // Entity delete by IID
    // =============================================

    #[test]
    fn entity_delete_by_iid() {
        let schema = test_schema();
        let clauses = build_entity_delete_by_iid("person", "0xabc123", &schema).unwrap();
        let typeql = compile(&clauses);
        assert!(typeql.contains("delete"));
        assert!(typeql.contains("0xabc123"));
    }

    #[test]
    fn entity_delete_unknown_type() {
        let schema = test_schema();
        let result = build_entity_delete_by_iid("nonexistent", "0x1", &schema);
        assert!(result.is_err());
    }

    // =============================================
    // Entity update by IID
    // =============================================

    #[test]
    fn entity_update_by_iid() {
        let schema = test_schema();
        let mut attrs = HashMap::new();
        attrs.insert(
            "age".to_string(),
            AttributeValueSpec {
                value: serde_json::json!(31),
                value_type: "long".to_string(),
            },
        );
        let clauses =
            build_entity_update_by_iid("person", "0xabc", &attrs, &schema).unwrap();
        let typeql = compile(&clauses);
        assert!(typeql.contains("match"));
        assert!(typeql.contains("delete"));
        assert!(typeql.contains("insert"));
        assert!(typeql.contains("0xabc"));
    }

    #[test]
    fn entity_update_unknown_attr() {
        let schema = test_schema();
        let mut attrs = HashMap::new();
        attrs.insert(
            "email".to_string(),
            AttributeValueSpec {
                value: serde_json::json!("x@y.com"),
                value_type: "string".to_string(),
            },
        );
        let result = build_entity_update_by_iid("person", "0x1", &attrs, &schema);
        assert!(result.is_err());
    }

    // =============================================
    // Relation insert tests
    // =============================================

    #[test]
    fn relation_insert_with_key_attr() {
        let schema = test_schema();
        let role_players = vec![
            RolePlayerSpec {
                role: "employee".to_string(),
                entity_type: "person".to_string(),
                iid: None,
                key_attr: Some("name".to_string()),
                key_value: Some(AttributeValueSpec {
                    value: serde_json::json!("Alice"),
                    value_type: "string".to_string(),
                }),
            },
            RolePlayerSpec {
                role: "employer".to_string(),
                entity_type: "company".to_string(),
                iid: None,
                key_attr: Some("name".to_string()),
                key_value: Some(AttributeValueSpec {
                    value: serde_json::json!("Acme"),
                    value_type: "string".to_string(),
                }),
            },
        ];
        let attrs = HashMap::new();
        let clauses =
            build_relation_insert("employment", &role_players, &attrs, &schema).unwrap();
        let typeql = compile(&clauses);
        assert!(typeql.contains("match"));
        assert!(typeql.contains("insert"));
        assert!(typeql.contains("employment"));
    }

    #[test]
    fn relation_insert_with_iid() {
        let schema = test_schema();
        let role_players = vec![RolePlayerSpec {
            role: "employee".to_string(),
            entity_type: "person".to_string(),
            iid: Some("0xabc".to_string()),
            key_attr: None,
            key_value: None,
        }];
        let clauses =
            build_relation_insert("employment", &role_players, &HashMap::new(), &schema)
                .unwrap();
        let typeql = compile(&clauses);
        assert!(typeql.contains("0xabc"));
    }

    #[test]
    fn relation_insert_missing_player_id() {
        let schema = test_schema();
        let role_players = vec![RolePlayerSpec {
            role: "employee".to_string(),
            entity_type: "person".to_string(),
            iid: None,
            key_attr: None,
            key_value: None,
        }];
        let result =
            build_relation_insert("employment", &role_players, &HashMap::new(), &schema);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("must specify either 'iid'"));
    }

    #[test]
    fn relation_insert_unknown_type() {
        let schema = test_schema();
        let result = build_relation_insert("nonexistent", &[], &HashMap::new(), &schema);
        assert!(result.is_err());
    }

    #[test]
    fn relation_insert_unknown_attr() {
        let schema = test_schema();
        let mut attrs = HashMap::new();
        attrs.insert(
            "salary".to_string(),
            AttributeValueSpec {
                value: serde_json::json!(50000),
                value_type: "long".to_string(),
            },
        );
        let result = build_relation_insert("employment", &[], &attrs, &schema);
        assert!(result.is_err());
    }

    // =============================================
    // Relation fetch tests
    // =============================================

    #[test]
    fn relation_fetch_basic() {
        let schema = test_schema();
        let clauses =
            build_relation_fetch("employment", &[], &[], None, None, &schema).unwrap();
        let typeql = compile(&clauses);
        assert!(typeql.contains("match"));
        assert!(typeql.contains("employment"));
        assert!(typeql.contains("fetch"));
    }

    #[test]
    fn relation_fetch_unknown_type() {
        let schema = test_schema();
        let result = build_relation_fetch("nonexistent", &[], &[], None, None, &schema);
        assert!(result.is_err());
    }

    // =============================================
    // Relation delete tests
    // =============================================

    #[test]
    fn relation_delete_by_iid() {
        let schema = test_schema();
        let clauses =
            build_relation_delete_by_iid("employment", "0xdef", &schema).unwrap();
        let typeql = compile(&clauses);
        assert!(typeql.contains("delete"));
        assert!(typeql.contains("0xdef"));
    }

    #[test]
    fn relation_delete_unknown_type() {
        let schema = test_schema();
        let result = build_relation_delete_by_iid("nonexistent", "0x1", &schema);
        assert!(result.is_err());
    }

    // =============================================
    // Edge cases
    // =============================================

    #[test]
    fn entity_insert_empty_attributes() {
        let schema = test_schema();
        let clauses = build_entity_insert("person", &HashMap::new(), &schema).unwrap();
        assert_eq!(clauses.len(), 1);
    }

    #[test]
    fn entity_fetch_with_sort() {
        let schema = test_schema();
        let sort = vec![SortSpec {
            attr: "name".to_string(),
            dir: "desc".to_string(),
        }];
        let clauses =
            build_entity_fetch("person", &[], &sort, None, None, &schema).unwrap();
        let typeql = compile(&clauses);
        assert!(typeql.contains("sort"));
    }

    #[test]
    fn relation_insert_with_attrs() {
        let schema = test_schema();
        let role_players = vec![RolePlayerSpec {
            role: "employee".to_string(),
            entity_type: "person".to_string(),
            iid: Some("0x1".to_string()),
            key_attr: None,
            key_value: None,
        }];
        let mut attrs = HashMap::new();
        attrs.insert(
            "start-date".to_string(),
            AttributeValueSpec {
                value: serde_json::json!("2024-01-01"),
                value_type: "date".to_string(),
            },
        );
        let clauses =
            build_relation_insert("employment", &role_players, &attrs, &schema).unwrap();
        let typeql = compile(&clauses);
        assert!(typeql.contains("start-date"));
    }
}
