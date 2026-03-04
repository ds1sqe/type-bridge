//! PyO3 bindings for ORM-style CRUD query building.
//!
//! Exposes [`CrudQueryBuilder`] which generates TypeQL query strings
//! from entity/relation type names and attribute specifications.
//! No database connection is required — this is pure query construction.
//!
//! # Python usage
//!
//! ```python
//! from type_bridge_core import CrudQueryBuilder
//!
//! builder = CrudQueryBuilder()
//! typeql = builder.build_entity_insert("person", [("name", "Alice", "string"), ("age", 30, "long")])
//! print(typeql)
//! # insert
//! # $e isa person, has name "Alice", has age 30;
//! ```

use std::collections::HashMap;

use pyo3::prelude::*;
use pyo3::types::{PyList, PyTuple};

use type_bridge_core_lib::ast::*;
use type_bridge_core_lib::compiler::QueryCompiler;

/// Parse a list of Python (attr_name, value, value_type) tuples into a HashMap
/// of attribute name → (serde_json::Value, value_type string).
fn parse_attr_tuples(
    attrs: &Bound<'_, PyList>,
) -> PyResult<HashMap<String, (serde_json::Value, String)>> {
    let mut map = HashMap::new();
    for item in attrs.iter() {
        let tuple: Bound<'_, PyTuple> = item.downcast_into().map_err(|_| {
            pyo3::exceptions::PyTypeError::new_err(
                "Each attribute must be a (name, value, value_type) tuple",
            )
        })?;
        if tuple.len() != 3 {
            return Err(pyo3::exceptions::PyValueError::new_err(
                "Each attribute tuple must have exactly 3 elements: (name, value, value_type)",
            ));
        }
        let name: String = tuple.get_item(0)?.extract()?;
        let value = py_value_to_json(&tuple.get_item(1)?)?;
        let value_type: String = tuple.get_item(2)?.extract()?;
        map.insert(name, (value, value_type));
    }
    Ok(map)
}

/// Convert a Python value to serde_json::Value.
fn py_value_to_json(value: &Bound<'_, PyAny>) -> PyResult<serde_json::Value> {
    // Check bool before int (Python bool is subclass of int)
    if value.is_instance_of::<pyo3::types::PyBool>() {
        return Ok(serde_json::Value::Bool(value.extract::<bool>()?));
    }
    if value.is_none() {
        return Ok(serde_json::Value::Null);
    }
    if let Ok(s) = value.extract::<String>() {
        return Ok(serde_json::Value::String(s));
    }
    if let Ok(i) = value.extract::<i64>() {
        return Ok(serde_json::json!(i));
    }
    if let Ok(f) = value.extract::<f64>() {
        return Ok(serde_json::json!(f));
    }
    Err(pyo3::exceptions::PyTypeError::new_err(format!(
        "Unsupported value type: {}",
        value.get_type().name()?
    )))
}

/// Python-facing CRUD query builder.
///
/// Generates TypeQL query strings from entity/relation type names
/// and attribute specifications. No database connection required.
#[pyclass]
pub struct CrudQueryBuilder {
    compiler: QueryCompiler,
}

#[pymethods]
impl CrudQueryBuilder {
    /// Create a new CRUD query builder.
    #[new]
    fn new() -> Self {
        CrudQueryBuilder {
            compiler: QueryCompiler::new(),
        }
    }

    /// Build an entity INSERT query.
    ///
    /// Args:
    ///     type_name: The entity type name (e.g. "person").
    ///     attributes: List of (attr_name, value, value_type) tuples.
    ///
    /// Returns:
    ///     The compiled TypeQL INSERT string.
    ///
    /// Example:
    ///     >>> builder.build_entity_insert("person", [("name", "Alice", "string"), ("age", 30, "long")])
    fn build_entity_insert(
        &self,
        type_name: &str,
        attributes: Bound<'_, PyList>,
    ) -> PyResult<String> {
        let attrs = parse_attr_tuples(&attributes)?;

        let mut statements = vec![Statement::Isa {
            variable: "$e".to_string(),
            type_name: type_name.to_string(),
        }];

        for (attr_name, (value, value_type)) in &attrs {
            statements.push(Statement::Has {
                subject_var: "$e".to_string(),
                attr_name: attr_name.clone(),
                value: Value::Literal(LiteralValue {
                    value: value.clone(),
                    value_type: value_type.clone(),
                }),
            });
        }

        let clauses = vec![Clause::Insert(statements)];
        Ok(self.compiler.compile(&clauses))
    }

    /// Build an entity MATCH+FETCH query.
    ///
    /// Args:
    ///     type_name: The entity type name.
    ///     filters: Optional list of (attr_name, operator, value, value_type) tuples.
    ///     limit: Optional maximum number of results.
    ///     offset: Optional number of results to skip.
    ///
    /// Returns:
    ///     The compiled TypeQL MATCH+FETCH string.
    #[pyo3(signature = (type_name, filters=None, limit=None, offset=None))]
    fn build_entity_fetch(
        &self,
        type_name: &str,
        filters: Option<Bound<'_, PyList>>,
        limit: Option<u64>,
        offset: Option<u64>,
    ) -> PyResult<String> {
        let mut patterns: Vec<Pattern> = vec![Pattern::Entity {
            variable: "$e".to_string(),
            type_name: type_name.to_string(),
            constraints: vec![],
            is_strict: false,
        }];

        if let Some(filter_list) = filters {
            for (i, item) in filter_list.iter().enumerate() {
                let tuple: Bound<'_, PyTuple> = item.downcast_into().map_err(|_| {
                    pyo3::exceptions::PyTypeError::new_err(
                        "Each filter must be a (attr_name, operator, value, value_type) tuple",
                    )
                })?;
                if tuple.len() != 4 {
                    return Err(pyo3::exceptions::PyValueError::new_err(
                        "Each filter tuple must have 4 elements: (attr, op, value, value_type)",
                    ));
                }
                let attr: String = tuple.get_item(0)?.extract()?;
                let op: String = tuple.get_item(1)?.extract()?;
                let value = py_value_to_json(&tuple.get_item(2)?)?;
                let value_type: String = tuple.get_item(3)?.extract()?;

                let attr_var = format!("$_attr_{i}");
                patterns.push(Pattern::Has {
                    thing_var: "$e".to_string(),
                    attr_type: attr,
                    attr_var: attr_var.clone(),
                });
                patterns.push(Pattern::ValueComparison {
                    var: attr_var,
                    operator: op,
                    value: Value::Literal(LiteralValue { value, value_type }),
                });
            }
        }

        let mut clauses = vec![
            Clause::Match(patterns),
            Clause::Fetch(vec![FetchItem::Wildcard {
                key: "$e".to_string(),
                var: "$e".to_string(),
            }]),
        ];

        if let Some(n) = limit {
            clauses.push(Clause::Limit(n));
        }
        if let Some(n) = offset {
            clauses.push(Clause::Offset(n));
        }

        Ok(self.compiler.compile(&clauses))
    }

    /// Build an entity DELETE query by IID.
    ///
    /// Args:
    ///     type_name: The entity type name.
    ///     iid: The internal identifier of the entity.
    ///
    /// Returns:
    ///     The compiled TypeQL MATCH+DELETE string.
    fn build_entity_delete(&self, type_name: &str, iid: &str) -> String {
        let clauses = vec![
            Clause::Match(vec![Pattern::Entity {
                variable: "$e".to_string(),
                type_name: type_name.to_string(),
                constraints: vec![Constraint::Iid(iid.to_string())],
                is_strict: false,
            }]),
            Clause::Delete(vec![Statement::DeleteThing("$e".to_string())]),
        ];
        self.compiler.compile(&clauses)
    }

    /// Build an entity UPDATE query by IID.
    ///
    /// Args:
    ///     type_name: The entity type name.
    ///     iid: The internal identifier of the entity.
    ///     attributes: List of (attr_name, new_value, value_type) tuples.
    ///
    /// Returns:
    ///     The compiled TypeQL MATCH+DELETE+INSERT string.
    fn build_entity_update(
        &self,
        type_name: &str,
        iid: &str,
        attributes: Bound<'_, PyList>,
    ) -> PyResult<String> {
        let attrs = parse_attr_tuples(&attributes)?;

        let mut patterns: Vec<Pattern> = vec![Pattern::Entity {
            variable: "$e".to_string(),
            type_name: type_name.to_string(),
            constraints: vec![Constraint::Iid(iid.to_string())],
            is_strict: false,
        }];

        let mut delete_stmts = Vec::new();
        let mut insert_stmts = Vec::new();

        for (i, (attr_name, (value, value_type))) in attrs.iter().enumerate() {
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
                value: Value::Literal(LiteralValue {
                    value: value.clone(),
                    value_type: value_type.clone(),
                }),
            });
        }

        let clauses = vec![
            Clause::Match(patterns),
            Clause::Delete(delete_stmts),
            Clause::Insert(insert_stmts),
        ];
        Ok(self.compiler.compile(&clauses))
    }

    /// Build an entity COUNT query.
    ///
    /// Args:
    ///     type_name: The entity type name.
    ///
    /// Returns:
    ///     The compiled TypeQL MATCH+REDUCE COUNT string.
    fn build_entity_count(&self, type_name: &str) -> String {
        let clauses = vec![
            Clause::Match(vec![Pattern::Entity {
                variable: "$e".to_string(),
                type_name: type_name.to_string(),
                constraints: vec![],
                is_strict: false,
            }]),
            Clause::Reduce {
                assignments: vec![ReduceAssignment {
                    variable: "$count".to_string(),
                    expression: Value::FunctionCall(FunctionCallValue {
                        function: "count".to_string(),
                        args: vec![Value::Variable("$e".to_string())],
                    }),
                }],
                group_by: None,
            },
        ];
        self.compiler.compile(&clauses)
    }

    /// Build a relation INSERT query.
    ///
    /// Args:
    ///     type_name: The relation type name.
    ///     role_players: List of (role_name, entity_type, key_attr, key_value, key_value_type) tuples.
    ///     attributes: Optional list of (attr_name, value, value_type) tuples.
    ///
    /// Returns:
    ///     The compiled TypeQL query string.
    #[pyo3(signature = (type_name, role_players, attributes=None))]
    fn build_relation_insert(
        &self,
        type_name: &str,
        role_players: Bound<'_, PyList>,
        attributes: Option<Bound<'_, PyList>>,
    ) -> PyResult<String> {
        let mut match_patterns: Vec<Pattern> = Vec::new();
        let mut ast_role_players: Vec<RolePlayer> = Vec::new();

        for (i, item) in role_players.iter().enumerate() {
            let tuple: Bound<'_, PyTuple> = item.downcast_into().map_err(|_| {
                pyo3::exceptions::PyTypeError::new_err(
                    "Each role player must be a (role, entity_type, key_attr, key_value, key_value_type) tuple",
                )
            })?;
            if tuple.len() != 5 {
                return Err(pyo3::exceptions::PyValueError::new_err(
                    "Each role player tuple must have 5 elements",
                ));
            }
            let role: String = tuple.get_item(0)?.extract()?;
            let entity_type: String = tuple.get_item(1)?.extract()?;
            let key_attr: String = tuple.get_item(2)?.extract()?;
            let key_value = py_value_to_json(&tuple.get_item(3)?)?;
            let key_value_type: String = tuple.get_item(4)?.extract()?;

            let player_var = format!("$_player_{i}");
            match_patterns.push(Pattern::Entity {
                variable: player_var.clone(),
                type_name: entity_type,
                constraints: vec![Constraint::Has {
                    attr_name: key_attr,
                    value: Value::Literal(LiteralValue {
                        value: key_value,
                        value_type: key_value_type,
                    }),
                }],
                is_strict: false,
            });
            ast_role_players.push(RolePlayer { role, player_var });
        }

        // Parse optional relation attributes
        let mut relation_attrs: Vec<Statement> = Vec::new();
        if let Some(attr_list) = attributes {
            let attrs = parse_attr_tuples(&attr_list)?;
            for (attr_name, (value, value_type)) in &attrs {
                relation_attrs.push(Statement::Has {
                    subject_var: "$r".to_string(),
                    attr_name: attr_name.clone(),
                    value: Value::Literal(LiteralValue {
                        value: value.clone(),
                        value_type: value_type.clone(),
                    }),
                });
            }
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

        Ok(self.compiler.compile(&clauses))
    }

    /// Build an entity "PUT" (upsert) query.
    ///
    /// If the entity exists (matched by a key attribute), updates it.
    /// Otherwise, inserts a new entity.
    /// Returns the TypeQL insert query string (for simple put semantics).
    ///
    /// Args:
    ///     type_name: The entity type name.
    ///     attributes: List of (attr_name, value, value_type) tuples.
    ///
    /// Returns:
    ///     The compiled TypeQL INSERT string (caller handles put logic).
    fn build_entity_put(&self, type_name: &str, attributes: Bound<'_, PyList>) -> PyResult<String> {
        // Put is semantically an insert — the caller manages existence check
        self.build_entity_insert(type_name, attributes)
    }
}
