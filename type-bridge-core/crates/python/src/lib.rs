//! # type-bridge-core (Python)
//!
//! PyO3 bindings that expose the **type-bridge** Rust core to Python.
//!
//! This crate re-exports the core AST types as Python classes and provides
//! thin wrappers around [`type_bridge_core_lib::validation::ValidationEngine`],
//! [`type_bridge_core_lib::compiler::QueryCompiler`],
//! [`type_bridge_core_lib::schema::TypeSchema`], and
//! [`type_bridge_core_lib::value_coercion::ValueCoercer`].
//!
//! All complex types cross the Python ↔ Rust boundary via
//! `pythonize` / `depythonize` using serde-tagged-enum dicts.

#![warn(missing_docs)]

pub mod ast;
pub mod migration_runtime;
pub mod orm;
pub mod orm_runtime;
pub mod schema;
pub mod transpiler;
pub mod version;

use type_bridge_core_lib as core;

use pyo3::prelude::*;
use pyo3::types::PyBool;
use pythonize::{depythonize, pythonize};

/// Python-facing wrapper around the Rust [`type_bridge_core_lib::validation::ValidationEngine`].
#[pyclass]
pub struct ValidationEngine {
    inner: core::validation::ValidationEngine,
}

#[pymethods]
impl ValidationEngine {
    #[new]
    fn new() -> Self {
        ValidationEngine {
            inner: core::validation::ValidationEngine::new(),
        }
    }

    fn validate_type_name(&self, name: String, context: String) -> PyResult<bool> {
        let result = self.inner.validate_type_name(&name, &context);
        if !result.is_valid {
            return Err(pyo3::exceptions::PyValueError::new_err(
                result.errors[0].message.clone(),
            ));
        }
        Ok(true)
    }

    fn validate_variable_name(&self, name: String, context: String) -> PyResult<bool> {
        let errors = self.inner.validate_variable_name(&name, &context, "");
        if !errors.is_empty() {
            return Err(pyo3::exceptions::PyValueError::new_err(
                errors[0].message.clone(),
            ));
        }
        Ok(true)
    }

    fn validate_pattern(&self, pattern: Bound<'_, PyAny>) -> PyResult<bool> {
        let core_pattern: core::ast::Pattern = depythonize(&pattern).map_err(|e| {
            pyo3::exceptions::PyValueError::new_err(format!("Failed to deserialize pattern: {}", e))
        })?;
        let result = self.inner.validate_pattern(&core_pattern);
        Ok(result.is_valid)
    }

    fn validate_statement(&self, statement: Bound<'_, PyAny>) -> PyResult<bool> {
        let core_statement: core::ast::Statement = depythonize(&statement).map_err(|e| {
            pyo3::exceptions::PyValueError::new_err(format!(
                "Failed to deserialize statement: {}",
                e
            ))
        })?;
        let result = self.inner.validate_statement(&core_statement);
        Ok(result.is_valid)
    }

    /// Validate a query (as list of clause dicts) against a TypeSchema.
    ///
    /// Returns a dict with `is_valid`, `errors` keys.
    /// Each error has `code`, `message`, `path`, `severity` fields.
    fn validate_query(
        &self,
        py: Python<'_>,
        clauses: Vec<Bound<'_, PyAny>>,
        schema: &TypeSchema,
    ) -> PyResult<PyObject> {
        let mut core_clauses = Vec::new();
        for clause in &clauses {
            let c: core::ast::Clause = depythonize(clause).map_err(|e| {
                pyo3::exceptions::PyValueError::new_err(format!(
                    "Failed to deserialize clause: {}",
                    e
                ))
            })?;
            core_clauses.push(c);
        }

        let result = self.inner.validate_query(&core_clauses, &schema.inner);

        pythonize(py, &result)
            .map(|obj| obj.unbind())
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
    }

    // -------------------------------------------------------------------
    // Custom validation rules (JSON DSL)
    // -------------------------------------------------------------------

    /// Add a single rule from a Python dict.
    fn add_rule(&mut self, rule_dict: Bound<'_, PyAny>) -> PyResult<()> {
        let rule: core::validation::ValidationRule = depythonize(&rule_dict).map_err(|e| {
            pyo3::exceptions::PyValueError::new_err(format!("Failed to deserialize rule: {}", e))
        })?;
        self.inner
            .add_rule(rule)
            .map_err(pyo3::exceptions::PyValueError::new_err)
    }

    /// Load rules from a JSON string. Returns list of warning strings.
    fn load_rules(&mut self, json_str: &str) -> PyResult<Vec<String>> {
        self.inner
            .load_rules(json_str)
            .map_err(pyo3::exceptions::PyValueError::new_err)
    }

    /// Export current rules as a JSON string.
    fn export_rules(&self) -> PyResult<String> {
        self.inner
            .export_rules()
            .map_err(pyo3::exceptions::PyValueError::new_err)
    }

    /// Remove all rules.
    fn clear_rules(&mut self) {
        self.inner.clear_rules();
    }

    /// Get the number of loaded rules.
    fn rule_count(&self) -> usize {
        self.inner.rule_count()
    }

    /// Validate entity data (as a Python dict) against loaded rules.
    ///
    /// Returns a dict with `is_valid` and `errors` keys.
    #[pyo3(signature = (entity_data, schema=None))]
    fn validate_entity(
        &self,
        py: Python<'_>,
        entity_data: Bound<'_, PyAny>,
        schema: Option<&TypeSchema>,
    ) -> PyResult<PyObject> {
        let json_data: serde_json::Value = depythonize(&entity_data).map_err(|e| {
            pyo3::exceptions::PyValueError::new_err(format!(
                "Failed to deserialize entity data: {}",
                e
            ))
        })?;

        let schema_ref = schema.map(|s| &s.inner);
        let result = self.inner.validate_entity(&json_data, schema_ref);

        pythonize(py, &result)
            .map(|obj| obj.unbind())
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
    }
}

/// Python-facing wrapper around the Rust [`type_bridge_core_lib::compiler::QueryCompiler`].
#[pyclass]
pub struct QueryCompiler {
    inner: core::compiler::QueryCompiler,
}

#[pymethods]
impl QueryCompiler {
    #[new]
    fn new() -> Self {
        QueryCompiler {
            inner: core::compiler::QueryCompiler::new(),
        }
    }

    fn compile(&self, clauses: Vec<Bound<'_, PyAny>>) -> PyResult<String> {
        let mut core_clauses = Vec::new();
        for clause in &clauses {
            let c: core::ast::Clause = depythonize(clause).map_err(|e| {
                pyo3::exceptions::PyValueError::new_err(format!(
                    "Failed to deserialize clause: {}",
                    e
                ))
            })?;
            core_clauses.push(c);
        }
        Ok(self.inner.compile(&core_clauses))
    }

    /// Compile from Python dicts matching the serde-tagged-enum format.
    /// This accepts Python dataclasses that have been converted to dicts.
    fn compile_dicts(&self, clauses: Vec<Bound<'_, PyAny>>) -> PyResult<String> {
        let mut core_clauses = Vec::new();
        for clause in &clauses {
            let c: core::ast::Clause = depythonize(clause).map_err(|e| {
                pyo3::exceptions::PyValueError::new_err(format!(
                    "Failed to deserialize clause: {}",
                    e
                ))
            })?;
            core_clauses.push(c);
        }
        Ok(self.inner.compile(&core_clauses))
    }

    /// Parse a TypeQL query string into a list of clause dicts (serde-tagged-enum format).
    fn parse(&self, py: Python<'_>, input: &str) -> PyResult<PyObject> {
        let clauses = core::query_parser::parse_typeql_query(input)
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
        pythonize(py, &clauses)
            .map(|obj| obj.unbind())
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
    }
}

/// Python-facing wrapper around the Rust [`type_bridge_core_lib::schema::TypeSchema`].
#[pyclass]
pub struct TypeSchema {
    inner: core::schema::TypeSchema,
}

#[pymethods]
impl TypeSchema {
    /// Parse a TypeQL define-block string and resolve inheritance.
    #[staticmethod]
    fn from_typeql(input: &str) -> PyResult<Self> {
        let schema = core::schema::TypeSchema::from_typeql(input)
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
        Ok(TypeSchema { inner: schema })
    }

    /// Deserialize from a JSON string.
    #[staticmethod]
    fn from_json(json: &str) -> PyResult<Self> {
        let schema = core::schema::TypeSchema::from_json(json)
            .map_err(pyo3::exceptions::PyValueError::new_err)?;
        Ok(TypeSchema { inner: schema })
    }

    /// Serialize to a JSON string.
    fn to_json(&self) -> PyResult<String> {
        self.inner
            .to_json()
            .map_err(pyo3::exceptions::PyValueError::new_err)
    }

    /// Render generated model files as a JSON package for a target language.
    #[pyo3(signature = (target, options_json=None))]
    fn render_models_json(&self, target: &str, options_json: Option<&str>) -> PyResult<String> {
        let target = parse_bindgen_target(target)?;
        let options = parse_bindgen_options(options_json)?;
        core::bindgen::BindgenPlan::from_schema(&self.inner)
            .render(target, &options)
            .to_json()
            .map_err(pyo3::exceptions::PyValueError::new_err)
    }

    /// Check if a type (entity, relation, or attribute) is abstract.
    fn is_abstract(&self, type_name: &str) -> bool {
        self.inner.is_abstract(type_name)
    }

    /// Get all owned attributes for an entity or relation (as list of dicts).
    fn get_all_owned_attributes(&self, py: Python<'_>, name: &str) -> PyResult<PyObject> {
        let attrs = self.inner.get_all_owned_attributes(name);
        pythonize(py, &attrs)
            .map(|obj| obj.unbind())
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
    }

    /// Get all played roles for an entity or relation (as list of dicts).
    fn get_all_plays_roles(&self, py: Python<'_>, name: &str) -> PyResult<PyObject> {
        let roles = self.inner.get_all_plays_roles(name);
        pythonize(py, &roles)
            .map(|obj| obj.unbind())
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
    }

    /// Get all relates (role specs) for a relation (as list of dicts).
    fn get_all_relates(&self, py: Python<'_>, name: &str) -> PyResult<PyObject> {
        let roles = self.inner.get_all_relates(name);
        pythonize(py, &roles)
            .map(|obj| obj.unbind())
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
    }

    /// Get the entities map as a Python dict.
    #[getter]
    fn entities(&self, py: Python<'_>) -> PyResult<PyObject> {
        pythonize(py, &self.inner.entities)
            .map(|obj| obj.unbind())
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
    }

    /// Get the relations map as a Python dict.
    #[getter]
    fn relations(&self, py: Python<'_>) -> PyResult<PyObject> {
        pythonize(py, &self.inner.relations)
            .map(|obj| obj.unbind())
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
    }

    /// Get the attributes map as a Python dict.
    #[getter]
    fn attributes(&self, py: Python<'_>) -> PyResult<PyObject> {
        pythonize(py, &self.inner.attributes)
            .map(|obj| obj.unbind())
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
    }

    /// Get the functions map as a Python dict.
    #[getter]
    fn functions(&self, py: Python<'_>) -> PyResult<PyObject> {
        pythonize(py, &self.inner.functions)
            .map(|obj| obj.unbind())
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
    }

    /// Get the structs map as a Python dict.
    #[getter]
    fn structs(&self, py: Python<'_>) -> PyResult<PyObject> {
        pythonize(py, &self.inner.structs)
            .map(|obj| obj.unbind())
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
    }

    /// Validate a query (as list of clause dicts) against this schema.
    ///
    /// Convenience method equivalent to `ValidationEngine().validate_query(clauses, schema)`.
    fn validate_query(&self, py: Python<'_>, clauses: Vec<Bound<'_, PyAny>>) -> PyResult<PyObject> {
        let engine = core::validation::ValidationEngine::new();
        let mut core_clauses = Vec::new();
        for clause in &clauses {
            let c: core::ast::Clause = depythonize(clause).map_err(|e| {
                pyo3::exceptions::PyValueError::new_err(format!(
                    "Failed to deserialize clause: {}",
                    e
                ))
            })?;
            core_clauses.push(c);
        }

        let result = engine.validate_query(&core_clauses, &self.inner);

        pythonize(py, &result)
            .map(|obj| obj.unbind())
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
    }
}

/// Python-facing wrapper around the Rust [`type_bridge_core_lib::value_coercion::ValueCoercer`].
#[pyclass]
pub struct ValueCoercer {
    inner: core::value_coercion::ValueCoercer,
}

#[pymethods]
impl ValueCoercer {
    #[new]
    fn new() -> Self {
        ValueCoercer {
            inner: core::value_coercion::ValueCoercer::new(),
        }
    }

    /// Coerce a value to a target TypeDB type. Returns dict with "value" and "value_type".
    fn coerce(
        &self,
        py: Python<'_>,
        value: Bound<'_, PyAny>,
        target_type: &str,
    ) -> PyResult<PyObject> {
        let json_val = py_to_json_value(&value)?;
        let coerced = self
            .inner
            .coerce(&json_val, target_type)
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
        pythonize(py, &coerced)
            .map(|obj| obj.unbind())
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
    }

    /// Batch coerce. Takes list of (value, type) tuples, returns list of dicts.
    fn coerce_batch(
        &self,
        py: Python<'_>,
        pairs: Vec<(Bound<'_, PyAny>, String)>,
    ) -> PyResult<PyObject> {
        let json_pairs: Vec<(serde_json::Value, String)> = pairs
            .iter()
            .map(|(v, t)| Ok((py_to_json_value(v)?, t.clone())))
            .collect::<PyResult<Vec<_>>>()?;
        let results = self.inner.coerce_batch(&json_pairs);
        let py_results: Vec<PyObject> = results
            .into_iter()
            .map(|r| match r {
                Ok(cv) => pythonize(py, &cv)
                    .map(|obj| obj.unbind())
                    .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string())),
                Err(e) => Err(pyo3::exceptions::PyValueError::new_err(e.to_string())),
            })
            .collect::<PyResult<Vec<_>>>()?;
        Ok(pyo3::types::PyList::new(py, &py_results)?
            .into_any()
            .unbind())
    }

    /// Format a value for TypeQL given its known type. Returns TypeQL literal string.
    fn format_typeql(&self, value: Bound<'_, PyAny>, value_type: &str) -> PyResult<String> {
        let json_val = py_to_json_value(&value)?;
        let coerced = core::value_coercion::CoercedValue {
            value: json_val,
            value_type: value_type.to_string(),
        };
        Ok(self.inner.format_typeql(&coerced))
    }
}

/// Convert a Python object to a serde_json::Value for Rust processing.
fn py_to_json_value(value: &Bound<'_, PyAny>) -> PyResult<serde_json::Value> {
    // Check bool before int (Python bool is subclass of int)
    if value.is_instance_of::<PyBool>() {
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
    // Fallback: convert to string
    let s = value.str()?.to_string();
    Ok(serde_json::Value::String(s))
}

/// Detect the TypeDB value type from a Python object and return (formatted_string, type_hint).
/// This handles the Python type dispatch that cannot be done in pure Rust.
fn detect_type_and_format(value: &Bound<'_, PyAny>) -> PyResult<String> {
    let py = value.py();

    // 1. Unwrap Attribute instances (extract .value)
    let value = if value.hasattr("value")? {
        let inner = value.getattr("value")?;
        // Guard: don't unwrap if .value is a method or the object is a string/bool/int
        // (strings have .value... no they don't in Python, but be safe)
        if inner.is_none() {
            return Ok("\"None\"".to_string());
        }
        inner
    } else {
        value.clone()
    };

    // 2. Check bool BEFORE int (Python bool is subclass of int)
    if value.is_instance_of::<PyBool>() {
        let b: bool = value.extract()?;
        return Ok(if b {
            "true".to_string()
        } else {
            "false".to_string()
        });
    }

    // 3. Check for Decimal (from decimal module)
    let decimal_mod = py.import("decimal")?;
    let decimal_type = decimal_mod.getattr("Decimal")?;
    if value.is_instance(&decimal_type)? {
        let s = value.str()?.to_string();
        return Ok(format!("{}dec", s));
    }

    // 4. Check float BEFORE int (0.0 must format as "0.0", not "0")
    //    Python's str(0.0) returns "0.0" but Rust's f64::to_string() returns "0"
    //    So we must match Python's behavior by calling Python's str() on the float.
    if value.is_instance_of::<pyo3::types::PyFloat>() {
        let s = value.str()?.to_string();
        return Ok(s);
    }

    // 5. Check int
    if let Ok(i) = value.extract::<i64>() {
        return Ok(i.to_string());
    }

    // 6. Check datetime (before date, since datetime is subclass of date)
    let datetime_mod = py.import("datetime")?;
    let datetime_type = datetime_mod.getattr("datetime")?;
    if value.is_instance(&datetime_type)? {
        let iso: String = value.call_method0("isoformat")?.extract()?;
        return Ok(iso);
    }

    // 7. Check date
    let date_type = datetime_mod.getattr("date")?;
    if value.is_instance(&date_type)? {
        let iso: String = value.call_method0("isoformat")?.extract()?;
        return Ok(iso);
    }

    // 8. Check timedelta / isodate.Duration
    let timedelta_type = datetime_mod.getattr("timedelta")?;
    if value.is_instance(&timedelta_type)? {
        // Use isodate.duration_isoformat() for formatting
        let isodate_mod = py.import("isodate")?;
        let formatted: String = isodate_mod
            .call_method1("duration_isoformat", (&value,))?
            .extract()?;
        return Ok(formatted);
    }

    // 9. Check for isodate.Duration explicitly (it may not be a timedelta subclass)
    if let Ok(isodate_mod) = py.import("isodate")
        && let Ok(duration_type) = isodate_mod.getattr("Duration")
        && value.is_instance(&duration_type)?
    {
        let formatted: String = isodate_mod
            .call_method1("duration_isoformat", (&value,))?
            .extract()?;
        return Ok(formatted);
    }

    // 10. String (check last among common types)
    if let Ok(s) = value.extract::<String>() {
        return Ok(core::value_coercion::format_string_literal(&s));
    }

    // 11. Fallback: convert to string, then quote+escape
    let s = value.str()?.to_string();
    Ok(core::value_coercion::format_string_literal(&s))
}

/// Standalone function: format a Python value for TypeQL (infers type).
/// Direct replacement for Python's format_value().
#[pyfunction]
fn format_value(value: Bound<'_, PyAny>) -> PyResult<String> {
    detect_type_and_format(&value)
}

/// Standalone coerce function.
#[pyfunction]
fn coerce_value(py: Python<'_>, value: Bound<'_, PyAny>, target_type: &str) -> PyResult<PyObject> {
    let coercer = core::value_coercion::ValueCoercer::new();
    let json_val = py_to_json_value(&value)?;
    let coerced = coercer
        .coerce(&json_val, target_type)
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
    pythonize(py, &coerced)
        .map(|obj| obj.unbind())
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
}

/// Parse a TypeQL query string into a list of clause dicts.
///
/// Standalone function equivalent to `QueryCompiler().parse(input)`.
#[pyfunction]
fn parse_typeql_query(py: Python<'_>, input: &str) -> PyResult<PyObject> {
    let clauses = core::query_parser::parse_typeql_query(input)
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
    pythonize(py, &clauses)
        .map(|obj| obj.unbind())
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
}

fn parse_bindgen_target(target: &str) -> PyResult<core::bindgen::TargetLanguage> {
    target
        .parse()
        .map_err(pyo3::exceptions::PyValueError::new_err)
}

fn parse_bindgen_options(options_json: Option<&str>) -> PyResult<core::bindgen::BindgenOptions> {
    match options_json {
        Some(options_json) => serde_json::from_str(options_json).map_err(|e| {
            pyo3::exceptions::PyValueError::new_err(format!(
                "Failed to deserialize bindgen options: {e}"
            ))
        }),
        None => Ok(core::bindgen::BindgenOptions::default()),
    }
}

/// Render generated model files as a JSON package from a TypeQL schema string.
#[pyfunction]
#[pyo3(signature = (input, target, options_json=None))]
fn render_models_json(input: &str, target: &str, options_json: Option<&str>) -> PyResult<String> {
    let target = parse_bindgen_target(target)?;
    let options = parse_bindgen_options(options_json)?;
    core::bindgen::generate_json_from_typeql(input, target, &options)
        .map_err(pyo3::exceptions::PyValueError::new_err)
}

#[pymodule]
fn type_bridge_core(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<ValidationEngine>()?;
    m.add_class::<QueryCompiler>()?;
    m.add_class::<TypeSchema>()?;
    m.add_class::<ValueCoercer>()?;
    m.add_function(wrap_pyfunction!(parse_typeql_query, m)?)?;
    m.add_function(wrap_pyfunction!(render_models_json, m)?)?;
    m.add_function(wrap_pyfunction!(format_value, m)?)?;
    m.add_function(wrap_pyfunction!(coerce_value, m)?)?;
    m.add_function(wrap_pyfunction!(transpiler::toml_to_typeql, m)?)?;

    // Values
    m.add_class::<ast::LiteralValue>()?;
    m.add_class::<ast::FunctionCallValue>()?;
    m.add_class::<ast::ArithmeticValue>()?;
    m.add_class::<ast::RolePlayer>()?;

    // Constraints
    m.add_class::<ast::IidConstraint>()?;
    m.add_class::<ast::HasConstraint>()?;
    m.add_class::<ast::IsaConstraint>()?;

    // Patterns
    m.add_class::<ast::EntityPattern>()?;
    m.add_class::<ast::RelationPattern>()?;
    m.add_class::<ast::SubTypePattern>()?;
    m.add_class::<ast::AttributePattern>()?;
    m.add_class::<ast::HasPattern>()?;
    m.add_class::<ast::ValueComparisonPattern>()?;
    m.add_class::<ast::NotPattern>()?;
    m.add_class::<ast::OrPattern>()?;
    m.add_class::<ast::IidPattern>()?;
    m.add_class::<ast::RawPattern>()?;

    // Statements
    m.add_class::<ast::HasStatement>()?;
    m.add_class::<ast::IsaStatement>()?;
    m.add_class::<ast::RelationStatement>()?;
    m.add_class::<ast::DeleteThingStatement>()?;
    m.add_class::<ast::RawStatement>()?;

    // Clauses
    m.add_class::<ast::MatchClause>()?;
    m.add_class::<ast::MatchLetClause>()?;
    m.add_class::<ast::LetAssignment>()?;
    m.add_class::<ast::InsertClause>()?;
    m.add_class::<ast::DeleteClause>()?;
    m.add_class::<ast::UpdateClause>()?;

    // Fetch Items
    m.add_class::<ast::FetchAttribute>()?;
    m.add_class::<ast::FetchVariable>()?;
    m.add_class::<ast::FetchAttributeList>()?;
    m.add_class::<ast::FetchFunction>()?;
    m.add_class::<ast::FetchWildcard>()?;
    m.add_class::<ast::FetchNestedWildcard>()?;
    m.add_class::<ast::FetchClause>()?;

    // Aggregations
    m.add_class::<ast::AggregateExpr>()?;
    m.add_class::<ast::ReduceAssignment>()?;
    m.add_class::<ast::ReduceClause>()?;

    // ORM CRUD query builder
    m.add_class::<orm::CrudQueryBuilder>()?;
    orm_runtime::register(m)?;
    migration_runtime::register(m)?;
    schema::register(m)?;
    version::register(m)?;

    Ok(())
}
