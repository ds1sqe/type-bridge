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
}
