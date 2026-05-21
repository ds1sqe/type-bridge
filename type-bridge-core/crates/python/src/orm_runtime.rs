//! PyO3 facade over the shared Rust ORM runtime.
//!
//! This module intentionally performs boundary marshalling only. Descriptor
//! validation, query construction, execution, and hydration live in
//! `type_bridge_orm`.

use std::sync::Arc;

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};
use pythonize::{depythonize, pythonize};
use serde_json::{Map, Value};
use tokio::runtime::Runtime;
use type_bridge_orm::{
    AttributeValue, DescriptorRegistry, DynamicAttributeMap, DynamicEntityManager,
    DynamicEntityRow, DynamicRelationManager, DynamicRelationRow, DynamicRolePlayerInput,
    EntityDescriptor, Filter, OrmError, RelationDescriptor, ValueType,
};

/// Python-facing descriptor registry wrapper.
#[pyclass]
pub struct PyDescriptorRegistry {
    inner: Arc<DescriptorRegistry>,
}

#[pymethods]
impl PyDescriptorRegistry {
    /// Create an empty descriptor registry.
    #[new]
    fn new() -> Self {
        Self {
            inner: Arc::new(DescriptorRegistry::new()),
        }
    }

    /// Register an entity descriptor dict and return the canonical descriptor dict.
    fn register_entity(&self, py: Python<'_>, descriptor: Bound<'_, PyAny>) -> PyResult<PyObject> {
        let descriptor: EntityDescriptor = depythonize(&descriptor)
            .map_err(|error| py_value_error(format!("Invalid entity descriptor: {error}")))?;
        let registered = self
            .inner
            .register_entity(descriptor)
            .map_err(py_orm_error)?;
        pythonize(py, registered.as_ref())
            .map(|obj| obj.unbind())
            .map_err(|error| py_value_error(error.to_string()))
    }

    /// Register a relation descriptor dict and return the canonical descriptor dict.
    fn register_relation(
        &self,
        py: Python<'_>,
        descriptor: Bound<'_, PyAny>,
    ) -> PyResult<PyObject> {
        let descriptor: RelationDescriptor = depythonize(&descriptor)
            .map_err(|error| py_value_error(format!("Invalid relation descriptor: {error}")))?;
        let registered = self
            .inner
            .register_relation(descriptor)
            .map_err(py_orm_error)?;
        pythonize(py, registered.as_ref())
            .map(|obj| obj.unbind())
            .map_err(|error| py_value_error(error.to_string()))
    }

    /// Return an entity descriptor dict by type name.
    fn entity(&self, py: Python<'_>, type_name: &str) -> PyResult<PyObject> {
        let descriptor = self.inner.entity(type_name).map_err(py_orm_error)?;
        pythonize(py, descriptor.as_ref())
            .map(|obj| obj.unbind())
            .map_err(|error| py_value_error(error.to_string()))
    }

    /// Return a relation descriptor dict by type name.
    fn relation(&self, py: Python<'_>, type_name: &str) -> PyResult<PyObject> {
        let descriptor = self.inner.relation(type_name).map_err(py_orm_error)?;
        pythonize(py, descriptor.as_ref())
            .map(|obj| obj.unbind())
            .map_err(|error| py_value_error(error.to_string()))
    }

    /// Return a sorted snapshot of all registered descriptors.
    fn snapshot(&self, py: Python<'_>) -> PyResult<PyObject> {
        pythonize(py, &self.inner.snapshot())
            .map(|obj| obj.unbind())
            .map_err(|error| py_value_error(error.to_string()))
    }
}

/// Python-facing Rust database handle.
#[pyclass]
pub struct PyRustDatabase {
    db: Arc<type_bridge_orm::Database>,
    runtime: Arc<Runtime>,
}

#[pymethods]
impl PyRustDatabase {
    /// Connect to TypeDB using the shared Rust ORM session layer.
    #[staticmethod]
    #[pyo3(signature = (address, database, username=None, password=None))]
    fn connect(
        address: &str,
        database: &str,
        username: Option<&str>,
        password: Option<&str>,
    ) -> PyResult<Self> {
        let runtime = Runtime::new().map(Arc::new).map_err(|error| {
            py_runtime_error(format!("Failed to create Tokio runtime: {error}"))
        })?;
        let username = username.unwrap_or("admin").to_string();
        let password = password.unwrap_or("password").to_string();
        let db = runtime
            .block_on(type_bridge_orm::Database::connect(
                address, database, &username, &password,
            ))
            .map_err(py_orm_error)?;

        Ok(Self {
            db: Arc::new(db),
            runtime,
        })
    }

    /// Return whether the Rust database connection is open.
    fn is_connected(&self) -> bool {
        self.db.is_connected()
    }
}

/// Python-facing dynamic entity manager.
#[pyclass]
pub struct PyDynamicEntityManager {
    db: Arc<type_bridge_orm::Database>,
    runtime: Arc<Runtime>,
    descriptor: Arc<EntityDescriptor>,
}

#[pymethods]
impl PyDynamicEntityManager {
    /// Construct a dynamic entity manager from a Rust database and descriptor dict.
    #[new]
    fn new(database: &PyRustDatabase, descriptor: Bound<'_, PyAny>) -> PyResult<Self> {
        let descriptor: EntityDescriptor = depythonize(&descriptor)
            .map_err(|error| py_value_error(format!("Invalid entity descriptor: {error}")))?;
        Ok(Self {
            db: Arc::clone(&database.db),
            runtime: Arc::clone(&database.runtime),
            descriptor: Arc::new(descriptor),
        })
    }

    /// Insert one entity and return its IID.
    fn insert(&self, attributes: Bound<'_, PyAny>) -> PyResult<String> {
        let attributes = entity_attributes_from_py(&self.descriptor, attributes)?;
        let manager = DynamicEntityManager::new(&self.db, Arc::clone(&self.descriptor));
        self.runtime
            .block_on(manager.insert(&attributes))
            .map_err(py_orm_error)
    }

    /// Fetch entities matching equality filters.
    #[pyo3(signature = (filters=None))]
    fn get(&self, py: Python<'_>, filters: Option<Bound<'_, PyAny>>) -> PyResult<PyObject> {
        let filters = entity_filters_from_py(&self.descriptor, filters)?;
        let manager = DynamicEntityManager::new(&self.db, Arc::clone(&self.descriptor));
        let rows = self
            .runtime
            .block_on(manager.get(&filters))
            .map_err(py_orm_error)?;
        entity_rows_to_py(py, &rows)
    }

    /// Fetch all entities for this descriptor.
    fn all(&self, py: Python<'_>) -> PyResult<PyObject> {
        self.get(py, None)
    }

    /// Count entities matching equality filters.
    #[pyo3(signature = (filters=None))]
    fn count(&self, filters: Option<Bound<'_, PyAny>>) -> PyResult<u64> {
        let filters = entity_filters_from_py(&self.descriptor, filters)?;
        let manager = DynamicEntityManager::new(&self.db, Arc::clone(&self.descriptor));
        self.runtime
            .block_on(manager.count_with_filters(&filters))
            .map_err(py_orm_error)
    }

    /// Delete one entity by IID.
    fn delete_by_iid(&self, iid: &str) -> PyResult<()> {
        let manager = DynamicEntityManager::new(&self.db, Arc::clone(&self.descriptor));
        self.runtime
            .block_on(manager.delete_by_iid(iid))
            .map_err(py_orm_error)
    }
}

/// Python-facing dynamic relation manager.
#[pyclass]
pub struct PyDynamicRelationManager {
    db: Arc<type_bridge_orm::Database>,
    runtime: Arc<Runtime>,
    descriptor: Arc<RelationDescriptor>,
}

#[pymethods]
impl PyDynamicRelationManager {
    /// Construct a dynamic relation manager from a Rust database and descriptor dict.
    #[new]
    fn new(database: &PyRustDatabase, descriptor: Bound<'_, PyAny>) -> PyResult<Self> {
        let descriptor: RelationDescriptor = depythonize(&descriptor)
            .map_err(|error| py_value_error(format!("Invalid relation descriptor: {error}")))?;
        Ok(Self {
            db: Arc::clone(&database.db),
            runtime: Arc::clone(&database.runtime),
            descriptor: Arc::new(descriptor),
        })
    }

    /// Insert one relation and return its IID.
    fn insert(
        &self,
        attributes: Bound<'_, PyAny>,
        role_players: Bound<'_, PyAny>,
    ) -> PyResult<String> {
        let attributes = relation_attributes_from_py(&self.descriptor, attributes)?;
        let role_players = role_players_from_py(&self.descriptor, role_players)?;
        let manager = DynamicRelationManager::new(&self.db, Arc::clone(&self.descriptor));
        self.runtime
            .block_on(manager.insert(&attributes, &role_players))
            .map_err(py_orm_error)
    }

    /// Fetch relations matching equality filters.
    #[pyo3(signature = (filters=None))]
    fn get(&self, py: Python<'_>, filters: Option<Bound<'_, PyAny>>) -> PyResult<PyObject> {
        let filters = relation_filters_from_py(&self.descriptor, filters)?;
        let manager = DynamicRelationManager::new(&self.db, Arc::clone(&self.descriptor));
        let rows = self
            .runtime
            .block_on(manager.get(&filters))
            .map_err(py_orm_error)?;
        relation_rows_to_py(py, &rows)
    }

    /// Fetch all relations for this descriptor.
    fn all(&self, py: Python<'_>) -> PyResult<PyObject> {
        self.get(py, None)
    }

    /// Count relations matching equality filters.
    #[pyo3(signature = (filters=None))]
    fn count(&self, filters: Option<Bound<'_, PyAny>>) -> PyResult<u64> {
        let filters = relation_filters_from_py(&self.descriptor, filters)?;
        let manager = DynamicRelationManager::new(&self.db, Arc::clone(&self.descriptor));
        self.runtime
            .block_on(manager.count_with_filters(&filters))
            .map_err(py_orm_error)
    }

    /// Delete one relation by IID.
    fn delete_by_iid(&self, iid: &str) -> PyResult<()> {
        let manager = DynamicRelationManager::new(&self.db, Arc::clone(&self.descriptor));
        self.runtime
            .block_on(manager.delete_by_iid(iid))
            .map_err(py_orm_error)
    }
}

fn entity_attributes_from_py(
    descriptor: &EntityDescriptor,
    value: Bound<'_, PyAny>,
) -> PyResult<DynamicAttributeMap> {
    attributes_from_py(&descriptor.owned_attributes, value)
}

fn relation_attributes_from_py(
    descriptor: &RelationDescriptor,
    value: Bound<'_, PyAny>,
) -> PyResult<DynamicAttributeMap> {
    attributes_from_py(&descriptor.owned_attributes, value)
}

fn attributes_from_py(
    descriptors: &[type_bridge_orm::OwnedAttributeDescriptor],
    value: Bound<'_, PyAny>,
) -> PyResult<DynamicAttributeMap> {
    if value.is_none() {
        return Ok(vec![]);
    }
    let value = py_to_json(value)?;
    let obj = value
        .as_object()
        .ok_or_else(|| py_type_error("Attributes must be a dict"))?;
    let mut attributes = Vec::new();

    for (key, value) in obj {
        if value.is_null() {
            continue;
        }
        let descriptor = descriptors
            .iter()
            .find(|descriptor| descriptor.field_name == *key || descriptor.attr_name == *key)
            .ok_or_else(|| py_value_error(format!("Unknown attribute '{key}'")))?;
        push_attribute_values(
            &mut attributes,
            &descriptor.attr_name,
            descriptor.value_type,
            value,
        )?;
    }

    Ok(attributes)
}

fn entity_filters_from_py(
    descriptor: &EntityDescriptor,
    filters: Option<Bound<'_, PyAny>>,
) -> PyResult<Vec<Filter>> {
    filters_from_py(&descriptor.owned_attributes, filters)
}

fn relation_filters_from_py(
    descriptor: &RelationDescriptor,
    filters: Option<Bound<'_, PyAny>>,
) -> PyResult<Vec<Filter>> {
    filters_from_py(&descriptor.owned_attributes, filters)
}

fn filters_from_py(
    descriptors: &[type_bridge_orm::OwnedAttributeDescriptor],
    filters: Option<Bound<'_, PyAny>>,
) -> PyResult<Vec<Filter>> {
    let Some(filters) = filters else {
        return Ok(vec![]);
    };
    if filters.is_none() {
        return Ok(vec![]);
    }
    let value = py_to_json(filters)?;
    let obj = value
        .as_object()
        .ok_or_else(|| py_type_error("Filters must be a dict"))?;
    let mut rust_filters = Vec::new();
    for (key, value) in obj {
        if value.is_null() {
            continue;
        }
        if value.is_array() {
            return Err(py_value_error(format!(
                "Filter '{key}' must be a scalar equality value"
            )));
        }
        let descriptor = descriptors
            .iter()
            .find(|descriptor| descriptor.field_name == *key || descriptor.attr_name == *key)
            .ok_or_else(|| py_value_error(format!("Unknown filter attribute '{key}'")))?;
        let attr_value = attribute_value_from_json(value, descriptor.value_type)?;
        rust_filters.push(Filter::eq(descriptor.attr_name.clone(), attr_value));
    }
    Ok(rust_filters)
}

fn push_attribute_values(
    attributes: &mut DynamicAttributeMap,
    attr_name: &str,
    value_type: ValueType,
    value: &Value,
) -> PyResult<()> {
    if let Some(values) = value.as_array() {
        for item in values {
            attributes.push((
                attr_name.to_string(),
                attribute_value_from_json(item, value_type)?,
            ));
        }
        return Ok(());
    }
    attributes.push((
        attr_name.to_string(),
        attribute_value_from_json(value, value_type)?,
    ));
    Ok(())
}

fn attribute_value_from_json(value: &Value, value_type: ValueType) -> PyResult<AttributeValue> {
    AttributeValue::from_json(value, value_type.as_str()).ok_or_else(|| {
        py_value_error(format!(
            "Value {value:?} is not a {} attribute value",
            value_type.as_str()
        ))
    })
}

fn role_players_from_py(
    descriptor: &RelationDescriptor,
    value: Bound<'_, PyAny>,
) -> PyResult<Vec<DynamicRolePlayerInput>> {
    let value = py_to_json(value)?;
    let players = value
        .as_array()
        .ok_or_else(|| py_type_error("Role players must be a list of dicts"))?;
    let mut inputs = Vec::new();

    for player in players {
        let obj = player
            .as_object()
            .ok_or_else(|| py_type_error("Each role player must be a dict"))?;
        let role_name = required_string(obj, "role_name")?;
        let role = descriptor
            .role(&role_name)
            .ok_or_else(|| py_value_error(format!("Unknown role '{role_name}'")))?;
        let player_type_name = required_string(obj, "player_type_name")?;
        if !role
            .player_type_names
            .iter()
            .any(|type_name| type_name == &player_type_name)
        {
            return Err(py_value_error(format!(
                "Role '{role_name}' cannot be played by '{player_type_name}'"
            )));
        }

        let iid = obj
            .get("iid")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        let key = match (obj.get("key_attr"), obj.get("key_value")) {
            (Some(key_attr), Some(key_value)) => {
                let key_attr = key_attr
                    .as_str()
                    .ok_or_else(|| py_type_error("key_attr must be a string"))?;
                let value_type = obj
                    .get("key_value_type")
                    .and_then(Value::as_str)
                    .and_then(ValueType::parse)
                    .ok_or_else(|| {
                        py_value_error("key_value_type must be one of the TypeDB value types")
                    })?;
                Some((
                    key_attr.to_string(),
                    attribute_value_from_json(key_value, value_type)?,
                ))
            }
            _ => None,
        };

        if iid.is_none() && key.is_none() {
            return Err(py_value_error(format!(
                "Role player for role '{role_name}' requires iid or key fields"
            )));
        }

        inputs.push(DynamicRolePlayerInput {
            role_name,
            player_type_name,
            iid,
            key,
        });
    }

    Ok(inputs)
}

fn entity_rows_to_py(py: Python<'_>, rows: &[DynamicEntityRow]) -> PyResult<PyObject> {
    let values: Vec<_> = rows.iter().map(entity_row_to_json).collect();
    pythonize(py, &values)
        .map(|obj| obj.unbind())
        .map_err(|error| py_value_error(error.to_string()))
}

fn relation_rows_to_py(py: Python<'_>, rows: &[DynamicRelationRow]) -> PyResult<PyObject> {
    let values: Vec<_> = rows.iter().map(relation_row_to_json).collect();
    pythonize(py, &values)
        .map(|obj| obj.unbind())
        .map_err(|error| py_value_error(error.to_string()))
}

fn entity_row_to_json(row: &DynamicEntityRow) -> Value {
    let mut obj = Map::new();
    for (name, value) in &row.attributes {
        obj.insert(name.clone(), attribute_value_to_json(value));
    }
    if let Some(iid) = &row.iid {
        obj.insert("_iid".into(), Value::String(iid.clone()));
    }
    if let Some(type_name) = &row.type_name {
        obj.insert("_type".into(), Value::String(type_name.clone()));
    }
    Value::Object(obj)
}

fn relation_row_to_json(row: &DynamicRelationRow) -> Value {
    let mut obj = match entity_row_to_json(&DynamicEntityRow {
        iid: row.iid.clone(),
        type_name: row.type_name.clone(),
        attributes: row.attributes.clone(),
    }) {
        Value::Object(obj) => obj,
        _ => Map::new(),
    };
    let role_players: Vec<_> = row
        .role_players
        .iter()
        .map(|player| {
            let mut obj = Map::new();
            obj.insert("role_name".into(), Value::String(player.role_name.clone()));
            if let Some(iid) = &player.player_iid {
                obj.insert("player_iid".into(), Value::String(iid.clone()));
            }
            if let Some(type_name) = &player.player_type_name {
                obj.insert("player_type_name".into(), Value::String(type_name.clone()));
            }
            Value::Object(obj)
        })
        .collect();
    obj.insert("role_players".into(), Value::Array(role_players));
    Value::Object(obj)
}

fn attribute_value_to_json(value: &AttributeValue) -> Value {
    match value {
        AttributeValue::String(value)
        | AttributeValue::Date(value)
        | AttributeValue::DateTime(value)
        | AttributeValue::DateTimeTZ(value)
        | AttributeValue::Decimal(value)
        | AttributeValue::Duration(value) => Value::String(value.clone()),
        AttributeValue::Long(value) => Value::Number((*value).into()),
        AttributeValue::Double(value) => serde_json::json!(value),
        AttributeValue::Boolean(value) => Value::Bool(*value),
    }
}

fn py_to_json(value: Bound<'_, PyAny>) -> PyResult<Value> {
    depythonize(&value)
        .map_err(|error| py_value_error(format!("Expected JSON-compatible value: {error}")))
}

fn required_string(obj: &Map<String, Value>, key: &str) -> PyResult<String> {
    obj.get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| py_value_error(format!("Missing string field '{key}'")))
}

fn py_orm_error(error: OrmError) -> PyErr {
    match error {
        OrmError::DescriptorValidation { .. } | OrmError::DescriptorConflict { .. } => {
            py_value_error(error.to_string())
        }
        OrmError::DescriptorNotFound(_) | OrmError::NotFound(_) => {
            pyo3::exceptions::PyLookupError::new_err(error.to_string())
        }
        OrmError::Connection(_) | OrmError::QueryExecution(_) | OrmError::Transaction(_) => {
            py_runtime_error(error.to_string())
        }
        _ => py_runtime_error(error.to_string()),
    }
}

fn py_value_error(message: impl Into<String>) -> PyErr {
    pyo3::exceptions::PyValueError::new_err(message.into())
}

fn py_type_error(message: impl Into<String>) -> PyErr {
    pyo3::exceptions::PyTypeError::new_err(message.into())
}

fn py_runtime_error(message: impl Into<String>) -> PyErr {
    pyo3::exceptions::PyRuntimeError::new_err(message.into())
}

/// Register ORM runtime facade classes on the Python module.
pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyDescriptorRegistry>()?;
    m.add_class::<PyRustDatabase>()?;
    m.add_class::<PyDynamicEntityManager>()?;
    m.add_class::<PyDynamicRelationManager>()?;
    // Keep these imported so PyO3 validates the signatures at compile time.
    let _ = PyDict::new(m.py());
    let _ = PyList::empty(m.py());
    Ok(())
}
