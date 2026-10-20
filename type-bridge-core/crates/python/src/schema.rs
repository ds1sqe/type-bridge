//! PyO3 wrappers for Rust schema diff/generation.
//!
//! The schema policy lives in `type_bridge_orm::_schema`; this module only
//! marshals serde-compatible Python dicts across the boundary.

use pyo3::prelude::*;
use pythonize::{depythonize, pythonize};
use type_bridge_orm::_schema::SchemaInfo;
use type_bridge_orm::_schema::diff::{ClassifiedChange, SchemaDiff};

/// Generate a TypeQL `define` block from a serialized `SchemaInfo` dict.
#[pyfunction(name = "_archived_generate_define_block")]
fn generate_define_block(py_info: Bound<'_, PyAny>) -> PyResult<String> {
    let info: SchemaInfo = depythonize(&py_info)
        .map_err(|error| py_value_error(format!("Invalid SchemaInfo: {error}")))?;
    info.validate()
        .map_err(|error| py_value_error(error.to_string()))?;
    info.to_typeql()
        .map_err(|error| py_value_error(error.to_string()))
}

/// Compute a serialized `SchemaDiff` dict from two serialized `SchemaInfo` dicts.
#[pyfunction(name = "_archived_compute_schema_diff")]
fn compute_schema_diff(
    py: Python<'_>,
    current: Bound<'_, PyAny>,
    target: Bound<'_, PyAny>,
) -> PyResult<Py<PyAny>> {
    let current: SchemaInfo = depythonize(&current)
        .map_err(|error| py_value_error(format!("Invalid current SchemaInfo: {error}")))?;
    let target: SchemaInfo = depythonize(&target)
        .map_err(|error| py_value_error(format!("Invalid target SchemaInfo: {error}")))?;
    let diff = SchemaDiff::compute(&current, &target);
    pythonize(py, &diff)
        .map(|obj| obj.unbind())
        .map_err(|error| py_value_error(error.to_string()))
}

/// Classify a serialized `SchemaDiff` dict.
#[pyfunction(name = "_archived_classify_schema_diff")]
fn classify_schema_diff(py: Python<'_>, diff: Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
    let diff: SchemaDiff = depythonize(&diff)
        .map_err(|error| py_value_error(format!("Invalid SchemaDiff: {error}")))?;
    let classified: Vec<ClassifiedChange> = diff.classify();
    pythonize(py, &classified)
        .map(|obj| obj.unbind())
        .map_err(|error| py_value_error(error.to_string()))
}

/// Return whether a serialized `SchemaDiff` has breaking changes.
#[pyfunction(name = "_archived_schema_diff_is_breaking")]
fn schema_diff_is_breaking(diff: Bound<'_, PyAny>) -> PyResult<bool> {
    let diff: SchemaDiff = depythonize(&diff)
        .map_err(|error| py_value_error(format!("Invalid SchemaDiff: {error}")))?;
    Ok(diff.has_breaking_changes())
}

fn py_value_error(message: impl Into<String>) -> PyErr {
    pyo3::exceptions::PyValueError::new_err(message.into())
}

/// Register schema facade functions on the Python module.
pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(generate_define_block, m)?)?;
    m.add_function(wrap_pyfunction!(render_schema_annotations, m)?)?;
    m.add_function(wrap_pyfunction!(render_schema_definition, m)?)?;
    m.add_function(wrap_pyfunction!(render_schema_owns, m)?)?;
    m.add_function(wrap_pyfunction!(compute_schema_diff, m)?)?;
    m.add_function(wrap_pyfunction!(classify_schema_diff, m)?)?;
    m.add_function(wrap_pyfunction!(schema_diff_is_breaking, m)?)?;
    Ok(())
}

#[derive(Default, serde::Deserialize)]
#[serde(default)]
struct AnnotationOptions {
    #[serde(rename = "abstract")]
    abstract_: bool,
    independent: bool,
    is_key: bool,
    is_unique: bool,
    is_distinct: bool,
    card_min: Option<u64>,
    card_max: Option<u64>,
    doc: Option<String>,
    meta: std::collections::BTreeMap<String, String>,
}

#[pyfunction(name = "_render_schema_annotations")]
fn render_schema_annotations(options: Bound<'_, PyAny>) -> PyResult<Vec<String>> {
    use type_bridge_core_lib::__internal as tql;
    let options: AnnotationOptions =
        depythonize(&options).map_err(|error| py_value_error(error.to_string()))?;
    let build = || -> Result<Vec<tql::SchemaAnnotation>, tql::TqlError> {
        let mut result = Vec::new();
        for (enabled, name) in [
            (options.abstract_, "abstract"),
            (options.independent, "independent"),
            (options.is_key, "key"),
            (options.is_unique, "unique"),
            (options.is_distinct, "distinct"),
        ] {
            if enabled {
                result.push(tql::schema_flag(name)?);
            }
        }
        if !options.is_key && (options.card_min.is_some() || options.card_max.is_some()) {
            result.push(tql::schema_card(
                options.card_min.unwrap_or(0),
                options.card_max,
            )?);
        }
        if let Some(doc) = &options.doc {
            result.push(tql::schema_doc(doc)?);
        }
        for (key, value) in &options.meta {
            result.push(tql::schema_meta(key, value)?);
        }
        Ok(result)
    };
    build()
        .map(|values| {
            values
                .iter()
                .map(|value| tql::render_schema_annotations(std::slice::from_ref(value)))
                .collect()
        })
        .map_err(|error| py_value_error(error.to_string()))
}

#[pyfunction(name = "_render_schema_definition")]
fn render_schema_definition(kind: &str, entry: Bound<'_, PyAny>) -> PyResult<String> {
    use type_bridge_core_lib::__internal as tql;
    use type_bridge_orm::_schema::{generator, info};
    let definition = match kind {
        "attribute" => {
            let entry: info::AttributeSchemaEntry =
                depythonize(&entry).map_err(|error| py_value_error(error.to_string()))?;
            generator::attribute_node(&entry, true)
        }
        "entity" => {
            let entry: info::EntitySchemaEntry =
                depythonize(&entry).map_err(|error| py_value_error(error.to_string()))?;
            generator::entity_node(&entry, None)
        }
        "relation" => {
            let entry: info::RelationSchemaEntry =
                depythonize(&entry).map_err(|error| py_value_error(error.to_string()))?;
            generator::relation_node(&entry, None)
        }
        _ => return Err(py_value_error("unknown schema definition kind")),
    }
    .map_err(|error| py_value_error(error.to_string()))?;
    tql::render_schema_definition(definition).map_err(|error| py_value_error(error.to_string()))
}

#[pyfunction(name = "_render_schema_owns")]
fn render_schema_owns(entries: Bound<'_, PyAny>) -> PyResult<Vec<String>> {
    use type_bridge_core_lib::__internal as tql;
    use type_bridge_orm::_schema::{generator, info::OwnedAttributeEntry};
    let entries: Vec<OwnedAttributeEntry> =
        depythonize(&entries).map_err(|error| py_value_error(error.to_string()))?;
    entries
        .iter()
        .map(|entry| {
            generator::ownership_node(entry)
                .map(|node| tql::render_schema_capability(&node))
                .map_err(|error| py_value_error(error.to_string()))
        })
        .collect()
}
