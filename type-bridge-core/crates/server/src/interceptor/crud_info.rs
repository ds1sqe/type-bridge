use type_bridge_core_lib::ast::{Clause, Constraint, Pattern, Statement};

/// The kind of CRUD operation detected from query clauses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CrudOperation {
    Insert,
    Read,
    Update,
    Delete,
    Put,
    Other,
}

/// The kind of TypeDB type referenced in the query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeKind {
    Entity,
    Relation,
    Attribute,
}

/// Semantic CRUD metadata extracted from a query's clauses.
///
/// Provides high-level context about what a query does, which type(s)
/// it targets, which attributes it touches, and optional IID lookups.
#[derive(Debug, Clone, PartialEq)]
pub struct CrudInfo {
    pub operation: CrudOperation,
    pub type_name: Option<String>,
    pub type_kind: Option<TypeKind>,
    pub attribute_names: Vec<String>,
    pub iid: Option<String>,
}

/// Analyze a sequence of clauses to extract CRUD operation metadata.
///
/// Scans clauses to determine the operation type (Insert, Read, Update,
/// Delete, Put), then inspects patterns/statements for type information,
/// attribute names, and IID lookups.
pub fn extract_crud_info(clauses: &[Clause]) -> CrudInfo {
    // Step 1: determine operation from clause types
    let mut has_match = false;
    let mut has_insert = false;
    let mut has_delete = false;
    let mut has_update = false;
    let mut has_put = false;
    let mut has_fetch = false;
    let mut has_reduce = false;

    for clause in clauses {
        match clause {
            Clause::Match(_) | Clause::MatchLet(_) => has_match = true,
            Clause::Insert(_) => has_insert = true,
            Clause::Delete(_) => has_delete = true,
            Clause::Update(_) => has_update = true,
            Clause::Put(_) => has_put = true,
            Clause::Fetch(_) => has_fetch = true,
            Clause::Reduce { .. } => has_reduce = true,
            Clause::Sort(_) | Clause::Limit(_) | Clause::Offset(_) => {}
        }
    }

    let operation = if has_insert {
        CrudOperation::Insert
    } else if has_delete {
        CrudOperation::Delete
    } else if has_update {
        CrudOperation::Update
    } else if has_put {
        CrudOperation::Put
    } else if has_match || has_fetch || has_reduce {
        CrudOperation::Read
    } else {
        CrudOperation::Other
    };

    // Step 2: extract type info from the appropriate clauses
    let mut type_name: Option<String> = None;
    let mut type_kind: Option<TypeKind> = None;
    let mut attribute_names: Vec<String> = Vec::new();
    let mut iid: Option<String> = None;

    for clause in clauses {
        match clause {
            Clause::Insert(stmts) | Clause::Put(stmts)
                if matches!(operation, CrudOperation::Insert | CrudOperation::Put) =>
            {
                extract_from_statements(
                    stmts,
                    &mut type_name,
                    &mut type_kind,
                    &mut attribute_names,
                );
            }
            Clause::Delete(stmts) if operation == CrudOperation::Delete => {
                extract_from_statements(
                    stmts,
                    &mut type_name,
                    &mut type_kind,
                    &mut attribute_names,
                );
            }
            Clause::Update(stmts) if operation == CrudOperation::Update => {
                extract_from_statements(
                    stmts,
                    &mut type_name,
                    &mut type_kind,
                    &mut attribute_names,
                );
            }
            Clause::Match(patterns) => {
                extract_from_patterns(
                    patterns,
                    &mut type_name,
                    &mut type_kind,
                    &mut attribute_names,
                    &mut iid,
                );
            }
            _ => {}
        }
    }

    // Step 3: deduplicate attribute names
    attribute_names.sort();
    attribute_names.dedup();

    CrudInfo {
        operation,
        type_name,
        type_kind,
        attribute_names,
        iid,
    }
}

fn extract_from_statements(
    stmts: &[Statement],
    type_name: &mut Option<String>,
    type_kind: &mut Option<TypeKind>,
    attribute_names: &mut Vec<String>,
) {
    for stmt in stmts {
        match stmt {
            Statement::Isa { type_name: tn, .. } if type_name.is_none() => {
                *type_name = Some(tn.clone());
                *type_kind = Some(TypeKind::Entity);
            }
            Statement::Relation {
                type_name: tn,
                attributes,
                ..
            } if type_name.is_none() => {
                *type_name = Some(tn.clone());
                *type_kind = Some(TypeKind::Relation);
                for attr_stmt in attributes {
                    if let Statement::Has { attr_name, .. } = attr_stmt {
                        attribute_names.push(attr_name.clone());
                    }
                }
            }
            Statement::Has { attr_name, .. } => {
                attribute_names.push(attr_name.clone());
            }
            _ => {}
        }
    }
}

fn extract_from_patterns(
    patterns: &[Pattern],
    type_name: &mut Option<String>,
    type_kind: &mut Option<TypeKind>,
    attribute_names: &mut Vec<String>,
    iid: &mut Option<String>,
) {
    for pattern in patterns {
        match pattern {
            Pattern::Entity {
                type_name: tn,
                constraints,
                ..
            } if type_name.is_none() => {
                *type_name = Some(tn.clone());
                *type_kind = Some(TypeKind::Entity);
                extract_from_constraints(constraints, attribute_names, iid);
            }
            Pattern::Relation {
                type_name: tn,
                constraints,
                ..
            } if type_name.is_none() => {
                *type_name = Some(tn.clone());
                *type_kind = Some(TypeKind::Relation);
                extract_from_constraints(constraints, attribute_names, iid);
            }
            Pattern::Attribute { type_name: tn, .. } if type_name.is_none() => {
                *type_name = Some(tn.clone());
                *type_kind = Some(TypeKind::Attribute);
            }
            Pattern::Has { attr_type, .. } => {
                attribute_names.push(attr_type.clone());
            }
            Pattern::Iid { iid: id, .. } if iid.is_none() => {
                *iid = Some(id.clone());
            }
            Pattern::Or(branches) => {
                for branch in branches {
                    extract_from_patterns(branch, type_name, type_kind, attribute_names, iid);
                }
            }
            _ => {}
        }
    }
}

fn extract_from_constraints(
    constraints: &[Constraint],
    attribute_names: &mut Vec<String>,
    iid: &mut Option<String>,
) {
    for constraint in constraints {
        match constraint {
            Constraint::Has { attr_name, .. } => {
                attribute_names.push(attr_name.clone());
            }
            Constraint::Iid(id) if iid.is_none() => {
                *iid = Some(id.clone());
            }
            _ => {}
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use type_bridge_core_lib::ast::*;

    use super::*;

    fn lit_str(s: &str) -> Value {
        Value::Literal(LiteralValue {
            value: serde_json::json!(s),
            value_type: "string".to_string(),
        })
    }

    fn lit_long(n: i64) -> Value {
        Value::Literal(LiteralValue {
            value: serde_json::json!(n),
            value_type: "long".to_string(),
        })
    }

    // --- Operation detection ---

    #[test]
    fn empty_clauses_returns_other() {
        let info = extract_crud_info(&[]);
        assert_eq!(info.operation, CrudOperation::Other);
        assert!(info.type_name.is_none());
        assert!(info.type_kind.is_none());
        assert!(info.attribute_names.is_empty());
        assert!(info.iid.is_none());
    }

    #[test]
    fn match_fetch_is_read() {
        let clauses = vec![
            Clause::Match(vec![Pattern::Entity {
                variable: "p".into(),
                type_name: "person".into(),
                constraints: vec![],
                is_strict: false,
            }]),
            Clause::Fetch(vec![]),
        ];
        let info = extract_crud_info(&clauses);
        assert_eq!(info.operation, CrudOperation::Read);
    }

    #[test]
    fn match_only_is_read() {
        let clauses = vec![Clause::Match(vec![])];
        let info = extract_crud_info(&clauses);
        assert_eq!(info.operation, CrudOperation::Read);
    }

    #[test]
    fn match_reduce_is_read() {
        let clauses = vec![
            Clause::Match(vec![]),
            Clause::Reduce {
                assignments: vec![],
                group_by: None,
            },
        ];
        let info = extract_crud_info(&clauses);
        assert_eq!(info.operation, CrudOperation::Read);
    }

    #[test]
    fn insert_is_insert() {
        let clauses = vec![Clause::Insert(vec![Statement::Isa {
            variable: "$p".into(),
            type_name: "person".into(),
        }])];
        let info = extract_crud_info(&clauses);
        assert_eq!(info.operation, CrudOperation::Insert);
    }

    #[test]
    fn match_insert_is_insert() {
        let clauses = vec![
            Clause::Match(vec![]),
            Clause::Insert(vec![Statement::Isa {
                variable: "$p".into(),
                type_name: "person".into(),
            }]),
        ];
        let info = extract_crud_info(&clauses);
        assert_eq!(info.operation, CrudOperation::Insert);
    }

    #[test]
    fn match_delete_is_delete() {
        let clauses = vec![
            Clause::Match(vec![]),
            Clause::Delete(vec![Statement::DeleteThing("$p".into())]),
        ];
        let info = extract_crud_info(&clauses);
        assert_eq!(info.operation, CrudOperation::Delete);
    }

    #[test]
    fn match_update_is_update() {
        let clauses = vec![
            Clause::Match(vec![]),
            Clause::Update(vec![Statement::Has {
                subject_var: "$p".into(),
                attr_name: "age".into(),
                value: lit_long(31),
            }]),
        ];
        let info = extract_crud_info(&clauses);
        assert_eq!(info.operation, CrudOperation::Update);
    }

    #[test]
    fn put_is_put() {
        let clauses = vec![Clause::Put(vec![Statement::Isa {
            variable: "$p".into(),
            type_name: "person".into(),
        }])];
        let info = extract_crud_info(&clauses);
        assert_eq!(info.operation, CrudOperation::Put);
    }

    #[test]
    fn insert_takes_priority_over_fetch() {
        let clauses = vec![
            Clause::Match(vec![]),
            Clause::Insert(vec![]),
            Clause::Fetch(vec![]),
        ];
        let info = extract_crud_info(&clauses);
        assert_eq!(info.operation, CrudOperation::Insert);
    }

    // --- Type extraction ---

    #[test]
    fn entity_type_from_match_pattern() {
        let clauses = vec![
            Clause::Match(vec![Pattern::Entity {
                variable: "$p".into(),
                type_name: "person".into(),
                constraints: vec![],
                is_strict: false,
            }]),
            Clause::Fetch(vec![]),
        ];
        let info = extract_crud_info(&clauses);
        assert_eq!(info.type_name.as_deref(), Some("person"));
        assert_eq!(info.type_kind, Some(TypeKind::Entity));
    }

    #[test]
    fn relation_type_from_match_pattern() {
        let clauses = vec![Clause::Match(vec![Pattern::Relation {
            variable: "$r".into(),
            type_name: "employment".into(),
            role_players: vec![],
            constraints: vec![],
        }])];
        let info = extract_crud_info(&clauses);
        assert_eq!(info.type_name.as_deref(), Some("employment"));
        assert_eq!(info.type_kind, Some(TypeKind::Relation));
    }

    #[test]
    fn attribute_type_from_match_pattern() {
        let clauses = vec![Clause::Match(vec![Pattern::Attribute {
            variable: "$n".into(),
            type_name: "name".into(),
            value: None,
        }])];
        let info = extract_crud_info(&clauses);
        assert_eq!(info.type_name.as_deref(), Some("name"));
        assert_eq!(info.type_kind, Some(TypeKind::Attribute));
    }

    #[test]
    fn entity_type_from_insert_statement() {
        let clauses = vec![Clause::Insert(vec![
            Statement::Isa {
                variable: "$p".into(),
                type_name: "person".into(),
            },
            Statement::Has {
                subject_var: "$p".into(),
                attr_name: "name".into(),
                value: lit_str("Alice"),
            },
        ])];
        let info = extract_crud_info(&clauses);
        assert_eq!(info.type_name.as_deref(), Some("person"));
        assert_eq!(info.type_kind, Some(TypeKind::Entity));
    }

    #[test]
    fn relation_type_from_insert_statement() {
        let clauses = vec![Clause::Insert(vec![Statement::Relation {
            variable: "$r".into(),
            type_name: "employment".into(),
            role_players: vec![],
            include_variable: true,
            attributes: vec![Statement::Has {
                subject_var: "$r".into(),
                attr_name: "start-date".into(),
                value: lit_str("2024-01-01"),
            }],
        }])];
        let info = extract_crud_info(&clauses);
        assert_eq!(info.type_name.as_deref(), Some("employment"));
        assert_eq!(info.type_kind, Some(TypeKind::Relation));
        assert!(info.attribute_names.contains(&"start-date".to_string()));
    }

    // --- Attribute extraction ---

    #[test]
    fn attribute_names_from_constraints() {
        let clauses = vec![Clause::Match(vec![Pattern::Entity {
            variable: "$p".into(),
            type_name: "person".into(),
            constraints: vec![
                Constraint::Has {
                    attr_name: "name".into(),
                    value: lit_str("Alice"),
                },
                Constraint::Has {
                    attr_name: "age".into(),
                    value: lit_long(30),
                },
            ],
            is_strict: false,
        }])];
        let info = extract_crud_info(&clauses);
        assert_eq!(info.attribute_names, vec!["age", "name"]); // sorted
    }

    #[test]
    fn attribute_names_from_has_pattern() {
        let clauses = vec![Clause::Match(vec![
            Pattern::Entity {
                variable: "$p".into(),
                type_name: "person".into(),
                constraints: vec![],
                is_strict: false,
            },
            Pattern::Has {
                thing_var: "$p".into(),
                attr_type: "email".into(),
                attr_var: "$e".into(),
            },
        ])];
        let info = extract_crud_info(&clauses);
        assert!(info.attribute_names.contains(&"email".to_string()));
    }

    #[test]
    fn attribute_names_from_insert_has_statements() {
        let clauses = vec![Clause::Insert(vec![
            Statement::Isa {
                variable: "$p".into(),
                type_name: "person".into(),
            },
            Statement::Has {
                subject_var: "$p".into(),
                attr_name: "name".into(),
                value: lit_str("Alice"),
            },
            Statement::Has {
                subject_var: "$p".into(),
                attr_name: "age".into(),
                value: lit_long(30),
            },
        ])];
        let info = extract_crud_info(&clauses);
        assert_eq!(info.attribute_names, vec!["age", "name"]);
    }

    #[test]
    fn duplicate_attribute_names_deduped() {
        let clauses = vec![
            Clause::Match(vec![Pattern::Entity {
                variable: "$p".into(),
                type_name: "person".into(),
                constraints: vec![Constraint::Has {
                    attr_name: "name".into(),
                    value: lit_str("Alice"),
                }],
                is_strict: false,
            }]),
            Clause::Update(vec![Statement::Has {
                subject_var: "$p".into(),
                attr_name: "name".into(),
                value: lit_str("Bob"),
            }]),
        ];
        let info = extract_crud_info(&clauses);
        assert_eq!(
            info.attribute_names.iter().filter(|a| *a == "name").count(),
            1
        );
    }

    // --- IID extraction ---

    #[test]
    fn iid_from_pattern() {
        let clauses = vec![Clause::Match(vec![Pattern::Iid {
            variable: "$p".into(),
            iid: "0x12345".into(),
        }])];
        let info = extract_crud_info(&clauses);
        assert_eq!(info.iid.as_deref(), Some("0x12345"));
    }

    #[test]
    fn iid_from_constraint() {
        let clauses = vec![Clause::Match(vec![Pattern::Entity {
            variable: "$p".into(),
            type_name: "person".into(),
            constraints: vec![Constraint::Iid("0xABCDE".into())],
            is_strict: false,
        }])];
        let info = extract_crud_info(&clauses);
        assert_eq!(info.iid.as_deref(), Some("0xABCDE"));
    }

    // --- Display/Debug ---

    #[test]
    fn crud_info_debug() {
        let info = CrudInfo {
            operation: CrudOperation::Insert,
            type_name: Some("person".into()),
            type_kind: Some(TypeKind::Entity),
            attribute_names: vec!["name".into()],
            iid: None,
        };
        let debug = format!("{:?}", info);
        assert!(debug.contains("Insert"));
        assert!(debug.contains("person"));
    }

    #[test]
    fn crud_info_clone() {
        let info = CrudInfo {
            operation: CrudOperation::Read,
            type_name: Some("person".into()),
            type_kind: Some(TypeKind::Entity),
            attribute_names: vec!["name".into()],
            iid: Some("0x123".into()),
        };
        let cloned = info.clone();
        assert_eq!(info, cloned);
    }

    #[test]
    fn crud_operation_eq() {
        assert_eq!(CrudOperation::Insert, CrudOperation::Insert);
        assert_ne!(CrudOperation::Insert, CrudOperation::Read);
    }

    #[test]
    fn type_kind_eq() {
        assert_eq!(TypeKind::Entity, TypeKind::Entity);
        assert_ne!(TypeKind::Entity, TypeKind::Relation);
    }

    // --- Coverage: Sort/Limit/Offset are no-ops ---

    #[test]
    fn sort_limit_offset_ignored() {
        let clauses = vec![
            Clause::Match(vec![]),
            Clause::Sort(vec![SortField {
                variable: "$age".into(),
                ascending: true,
            }]),
            Clause::Limit(10),
            Clause::Offset(20),
        ];
        let info = extract_crud_info(&clauses);
        assert_eq!(info.operation, CrudOperation::Read);
    }

    // --- Coverage: Or pattern recursion ---

    #[test]
    fn or_pattern_extracts_type_from_branch() {
        let clauses = vec![Clause::Match(vec![Pattern::Or(vec![
            vec![Pattern::Entity {
                variable: "$p".into(),
                type_name: "person".into(),
                constraints: vec![],
                is_strict: false,
            }],
            vec![Pattern::Entity {
                variable: "$c".into(),
                type_name: "company".into(),
                constraints: vec![],
                is_strict: false,
            }],
        ])])];
        let info = extract_crud_info(&clauses);
        assert_eq!(info.type_name.as_deref(), Some("person"));
        assert_eq!(info.type_kind, Some(TypeKind::Entity));
    }

    // --- Coverage: catch-all arms (Not, SubType, ValueComparison, Raw, Constraint::Isa) ---

    #[test]
    fn not_pattern_and_value_comparison_are_skipped() {
        let clauses = vec![Clause::Match(vec![
            Pattern::Not(vec![Pattern::Entity {
                variable: "$x".into(),
                type_name: "admin".into(),
                constraints: vec![],
                is_strict: false,
            }]),
            Pattern::ValueComparison {
                var: "$age".into(),
                operator: ">".into(),
                value: lit_long(18),
            },
            Pattern::Raw("$x has status 'active';".into()),
            Pattern::SubType {
                variable: "$t".into(),
                parent_type: "thing".into(),
            },
        ])];
        let info = extract_crud_info(&clauses);
        // None of these set type_name
        assert!(info.type_name.is_none());
    }

    #[test]
    fn constraint_isa_is_skipped() {
        let clauses = vec![Clause::Match(vec![Pattern::Entity {
            variable: "$p".into(),
            type_name: "person".into(),
            constraints: vec![
                Constraint::Isa {
                    type_name: "person".into(),
                    strict: true,
                },
                Constraint::Has {
                    attr_name: "name".into(),
                    value: lit_str("Alice"),
                },
            ],
            is_strict: false,
        }])];
        let info = extract_crud_info(&clauses);
        assert_eq!(info.type_name.as_deref(), Some("person"));
        assert!(info.attribute_names.contains(&"name".to_string()));
    }
}
