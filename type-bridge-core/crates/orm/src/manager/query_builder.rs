//! Internal query builder wrapping the core [`QueryCompiler`].
//!
//! Constructs AST clauses from entity trait methods and compiles them
//! to TypeQL strings ready for execution.

use type_bridge_core_lib::ast::{
    Clause, Constraint, FunctionCallValue, Pattern, ReduceAssignment, Statement, Value,
};
use type_bridge_core_lib::compiler::QueryCompiler;

use crate::entity::TypeBridgeEntity;
use crate::error::Result;
use crate::filter::Filter;
use crate::relation::TypeBridgeRelation;

/// Build an insert + fetch-IID query for the given entity.
///
/// Produces TypeQL like:
/// ```text
/// insert $e isa person, has name "Alice", has age 30;
/// fetch { "iid": iid($e) };
/// ```
pub fn build_insert_with_iid<T: TypeBridgeEntity>(entity: &T, var: &str) -> Result<String> {
    let clauses = entity.to_insert_with_iid_fetch(var);
    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

/// Build a polymorphic fetch query with optional filters.
///
/// Produces TypeQL like:
/// ```text
/// match $e isa! $t, has name "Alice"; $t sub person;
/// fetch { "_iid": iid($e), "_type": label($t), "attributes": { $e.* } };
/// ```
pub fn build_fetch<T: TypeBridgeEntity>(filters: &[Filter], var: &str) -> Result<String> {
    let clauses = T::build_polymorphic_fetch(var, T::TYPE_NAME, filters);
    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

/// Build a match + delete query for a specific entity instance.
///
/// Uses IID or @key attributes for identification. Produces:
/// ```text
/// match $e isa person, has name "Alice";
/// delete $e isa person;
/// ```
pub fn build_delete<T: TypeBridgeEntity>(entity: &T, var: &str) -> Result<String> {
    let clauses = vec![
        Clause::Match(vec![entity.to_match_pattern(var)]),
        Clause::Delete(vec![Statement::Isa {
            variable: var.to_string(),
            type_name: T::TYPE_NAME.to_string(),
        }]),
    ];
    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

/// Build a count query with optional filters.
///
/// Produces TypeQL like:
/// ```text
/// match $e isa person, has name "Alice";
/// reduce $count = count($e);
/// ```
pub fn build_count<T: TypeBridgeEntity>(filters: &[Filter], var: &str) -> Result<String> {
    let constraints: Vec<Constraint> = filters
        .iter()
        .map(|f| Constraint::Has {
            attr_name: f.attr_name.clone(),
            value: f.value.to_ast_value(),
        })
        .collect();

    let clauses = vec![
        Clause::Match(vec![Pattern::Entity {
            variable: var.to_string(),
            type_name: T::TYPE_NAME.to_string(),
            constraints,
            is_strict: false,
        }]),
        Clause::Reduce {
            assignments: vec![ReduceAssignment {
                variable: "$count".to_string(),
                expression: Value::FunctionCall(FunctionCallValue {
                    function: "count".into(),
                    args: vec![Value::Variable(var.to_string())],
                }),
            }],
            group_by: None,
        },
    ];
    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

// ------------------------------------------------------------------
// Entity update + put query builders
// ------------------------------------------------------------------

/// Build a match + update query for a specific entity instance.
///
/// Matches the entity by IID or @key attributes, then updates all
/// non-key attribute values. Produces TypeQL like:
/// ```text
/// match $e isa person, has name "Alice";
/// update $e has age 31;
/// ```
pub fn build_update<T: TypeBridgeEntity>(entity: &T, var: &str) -> Result<String> {
    let key_attrs: Vec<&'static str> = T::owned_attributes()
        .iter()
        .filter(|a| a.is_key)
        .map(|a| a.attr_name)
        .collect();

    let update_statements: Vec<Statement> = entity
        .to_attribute_values()
        .into_iter()
        .filter(|(name, _)| !key_attrs.contains(name))
        .map(|(attr_name, value)| Statement::Has {
            subject_var: var.to_string(),
            attr_name: attr_name.to_string(),
            value: value.to_ast_value(),
        })
        .collect();

    if update_statements.is_empty() {
        return Err(crate::error::OrmError::QueryExecution(
            "No non-key attributes to update".into(),
        ));
    }

    let clauses = vec![
        Clause::Match(vec![entity.to_match_pattern(var)]),
        Clause::Update(update_statements),
    ];
    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

/// Build an idempotent put query (insert-or-update) for an entity.
///
/// Produces TypeQL using the `put` keyword instead of `insert`.
/// TypeDB will insert if no matching entity exists, or update if one does.
///
/// ```text
/// put $e isa person, has name "Alice", has age 30;
/// fetch { "iid": iid($e) };
/// ```
pub fn build_put<T: TypeBridgeEntity>(entity: &T, var: &str) -> Result<String> {
    let typeql = build_insert_with_iid::<T>(entity, var)?;
    // Replace the first occurrence of "insert" with "put"
    Ok(typeql.replacen("insert", "put", 1))
}

// ------------------------------------------------------------------
// Relation query builders
// ------------------------------------------------------------------

/// Build an insert + fetch-IID query for a relation.
///
/// Produces TypeQL like:
/// ```text
/// match $rp0 isa person, has name "Alice"; $rp1 isa company, has name "Acme";
/// insert $r isa employment, links (employee: $rp0, employer: $rp1), has position "Engineer";
/// fetch { "iid": iid($r) };
/// ```
pub fn build_relation_insert_with_iid<R: TypeBridgeRelation>(
    relation: &R,
    var: &str,
) -> Result<String> {
    let clauses = relation.to_insert_with_iid_fetch(var);
    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

/// Build a polymorphic fetch query for relations with optional filters.
pub fn build_relation_fetch<R: TypeBridgeRelation>(
    filters: &[Filter],
    var: &str,
) -> Result<String> {
    let clauses = R::build_polymorphic_fetch(var, R::TYPE_NAME, filters);
    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

/// Build a match + delete query for a specific relation instance.
pub fn build_relation_delete<R: TypeBridgeRelation>(
    relation: &R,
    var: &str,
) -> Result<String> {
    let match_patterns = relation.to_match_pattern(var);
    let clauses = vec![
        Clause::Match(match_patterns),
        Clause::Delete(vec![Statement::Isa {
            variable: var.to_string(),
            type_name: R::TYPE_NAME.to_string(),
        }]),
    ];
    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

/// Build a count query for relations with optional filters.
pub fn build_relation_count<R: TypeBridgeRelation>(
    filters: &[Filter],
    var: &str,
) -> Result<String> {
    let constraints: Vec<Constraint> = filters
        .iter()
        .map(|f| Constraint::Has {
            attr_name: f.attr_name.clone(),
            value: f.value.to_ast_value(),
        })
        .collect();

    let clauses = vec![
        Clause::Match(vec![Pattern::Relation {
            variable: var.to_string(),
            type_name: R::TYPE_NAME.to_string(),
            role_players: vec![],
            constraints,
        }]),
        Clause::Reduce {
            assignments: vec![ReduceAssignment {
                variable: "$count".to_string(),
                expression: Value::FunctionCall(FunctionCallValue {
                    function: "count".into(),
                    args: vec![Value::Variable(var.to_string())],
                }),
            }],
            group_by: None,
        },
    ];
    let compiler = QueryCompiler::new();
    Ok(compiler.compile(&clauses))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::OwnedAttributeInfo;
    use crate::value::AttributeValue;

    // Minimal test entity for query builder tests.
    struct TestPerson {
        iid: Option<String>,
        name: String,
        age: i64,
    }

    impl TypeBridgeEntity for TestPerson {
        const TYPE_NAME: &'static str = "person";

        fn owned_attributes() -> &'static [OwnedAttributeInfo] {
            &[
                OwnedAttributeInfo {
                    attr_name: "name",
                    value_type: "string",
                    is_key: true,
                },
                OwnedAttributeInfo {
                    attr_name: "age",
                    value_type: "long",
                    is_key: false,
                },
            ]
        }

        fn iid(&self) -> Option<&str> {
            self.iid.as_deref()
        }

        fn set_iid(&mut self, iid: String) {
            self.iid = Some(iid);
        }

        fn to_attribute_values(&self) -> Vec<(&'static str, AttributeValue)> {
            vec![
                ("name", AttributeValue::String(self.name.clone())),
                ("age", AttributeValue::Long(self.age)),
            ]
        }

        fn from_document(
            doc: &serde_json::Map<String, serde_json::Value>,
        ) -> crate::error::Result<Self> {
            let name = doc
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| crate::error::OrmError::Hydration {
                    type_name: "person".into(),
                    message: "missing name".into(),
                })?
                .to_string();
            let age = doc
                .get("age")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| crate::error::OrmError::Hydration {
                    type_name: "person".into(),
                    message: "missing age".into(),
                })?;
            Ok(Self {
                iid: None,
                name,
                age,
            })
        }
    }

    #[test]
    fn insert_query_contains_isa_and_has() {
        let person = TestPerson {
            iid: None,
            name: "Alice".into(),
            age: 30,
        };
        let q = build_insert_with_iid::<TestPerson>(&person, "$e").unwrap();
        assert!(q.contains("insert"));
        assert!(q.contains("isa person"));
        assert!(q.contains("has name"));
        assert!(q.contains("has age"));
        assert!(q.contains("fetch"));
        assert!(q.contains("iid"));
    }

    #[test]
    fn fetch_query_with_filters() {
        let filters = [Filter::string_eq("name", "Alice")];
        let q = build_fetch::<TestPerson>(&filters, "$e").unwrap();
        assert!(q.contains("match"));
        assert!(q.contains("isa!"));
        assert!(q.contains("has name"));
        assert!(q.contains("fetch"));
    }

    #[test]
    fn fetch_query_without_filters() {
        let q = build_fetch::<TestPerson>(&[], "$e").unwrap();
        assert!(q.contains("match"));
        assert!(q.contains("sub person"));
        assert!(q.contains("fetch"));
    }

    #[test]
    fn delete_query_uses_match_and_delete() {
        let person = TestPerson {
            iid: Some("0xabc".into()),
            name: "Alice".into(),
            age: 30,
        };
        let q = build_delete::<TestPerson>(&person, "$e").unwrap();
        assert!(q.contains("match"));
        assert!(q.contains("delete"));
        assert!(q.contains("isa person"));
    }

    #[test]
    fn count_query_uses_reduce() {
        let q = build_count::<TestPerson>(&[], "$e").unwrap();
        assert!(q.contains("match"));
        assert!(q.contains("isa person"));
        assert!(q.contains("reduce"));
        assert!(q.contains("count"));
    }

    #[test]
    fn count_query_with_filters() {
        let filters = [Filter::long_eq("age", 30)];
        let q = build_count::<TestPerson>(&filters, "$e").unwrap();
        assert!(q.contains("has age"));
        assert!(q.contains("reduce"));
        assert!(q.contains("count"));
    }

    #[test]
    fn update_query_matches_and_updates_non_key() {
        let person = TestPerson {
            iid: Some("0xabc".into()),
            name: "Alice".into(),
            age: 31,
        };
        let q = build_update::<TestPerson>(&person, "$e").unwrap();
        assert!(q.contains("match"));
        assert!(q.contains("update"));
        assert!(q.contains("has age"));
        // Key attributes should NOT appear in the update clause
        assert!(!q.contains("update\n$e has name"));
    }

    #[test]
    fn update_query_without_non_key_attrs_fails() {
        // TestPerson has only name (key) + age (non-key)
        // But a struct with only key attrs would fail
        struct KeyOnly {
            iid: Option<String>,
            name: String,
        }
        impl TypeBridgeEntity for KeyOnly {
            const TYPE_NAME: &'static str = "keyonly";
            fn owned_attributes() -> &'static [OwnedAttributeInfo] {
                &[OwnedAttributeInfo {
                    attr_name: "name",
                    value_type: "string",
                    is_key: true,
                }]
            }
            fn iid(&self) -> Option<&str> {
                self.iid.as_deref()
            }
            fn set_iid(&mut self, iid: String) {
                self.iid = Some(iid);
            }
            fn to_attribute_values(&self) -> Vec<(&'static str, AttributeValue)> {
                vec![("name", AttributeValue::String(self.name.clone()))]
            }
            fn from_document(
                _doc: &serde_json::Map<String, serde_json::Value>,
            ) -> crate::error::Result<Self> {
                unimplemented!()
            }
        }

        let entity = KeyOnly {
            iid: Some("0x1".into()),
            name: "Test".into(),
        };
        let result = build_update::<KeyOnly>(&entity, "$e");
        assert!(result.is_err());
    }

    #[test]
    fn put_query_uses_put_keyword() {
        let person = TestPerson {
            iid: None,
            name: "Alice".into(),
            age: 30,
        };
        let q = build_put::<TestPerson>(&person, "$e").unwrap();
        assert!(q.contains("put"));
        assert!(!q.starts_with("insert"));
        assert!(q.contains("isa person"));
        assert!(q.contains("has name"));
        assert!(q.contains("fetch"));
    }
}
