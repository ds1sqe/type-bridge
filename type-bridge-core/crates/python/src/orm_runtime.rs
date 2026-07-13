//! PyO3 facade over the shared Rust ORM runtime.
//!
//! This module intentionally performs boundary marshalling only. Descriptor
//! validation, query construction, execution, and hydration live in
//! `type_bridge_orm`.

use std::sync::Arc;

use crate::version::VersionError;
use pyo3::prelude::*;
use pyo3::types::{PyBool, PyDict, PyFloat, PyInt, PyList, PyString};
use pythonize::{depythonize, pythonize};
use serde_json::{Map, Value};
use tokio::runtime::Runtime;
use type_bridge_core_lib::version as core_version;
use type_bridge_orm::session::backend::QueryResult;
use type_bridge_orm::{
    AttributeValue, DescriptorRegistry, DynamicAggregate, DynamicAttributeMap, DynamicComparisonOp,
    DynamicEntityManager, DynamicEntityRow, DynamicExpr, DynamicRelationManager,
    DynamicRelationRow, DynamicRolePlayerInput, DynamicSort, EntityDescriptor, Filter,
    GivenRowsSpec, GivenValue, OrmError, RelationDescriptor, SchemaInfo, SortDir,
    TransactionContext, TxType, ValueType,
};

/// Python-facing descriptor registry wrapper.
#[pyclass]
pub struct PyDescriptorRegistry {
    inner: Arc<DescriptorRegistry>,
}

impl PyDescriptorRegistry {
    /// Clone the shared registry for another native-only PyO3 seam.
    pub(crate) fn registry_arc(&self) -> Arc<DescriptorRegistry> {
        Arc::clone(&self.inner)
    }
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

    /// Expose the registered models as migration-facing `SchemaInfo` for the
    /// Python diff / breaking-change path, mirroring `snapshot`'s descriptor view.
    fn schema_info(&self, py: Python<'_>) -> PyResult<PyObject> {
        pythonize(py, &SchemaInfo::from_descriptors(&self.inner.snapshot()))
            .map(|obj| obj.unbind())
            .map_err(|error| py_value_error(error.to_string()))
    }
}

/// Python-facing typed dynamic attribute value.
#[pyclass(name = "DynamicValue")]
#[derive(Clone)]
pub struct PyDynamicValue {
    value: AttributeValue,
}

impl PyDynamicValue {
    /// Clone the typed value without exposing it as an untyped Python DTO.
    pub(crate) fn attribute_value(&self) -> AttributeValue {
        self.value.clone()
    }
}

#[pymethods]
impl PyDynamicValue {
    /// Create a string value.
    #[staticmethod]
    fn string(value: &str) -> Self {
        Self {
            value: AttributeValue::String(value.to_string()),
        }
    }

    /// Create a long value.
    #[staticmethod]
    fn long(value: i64) -> Self {
        Self {
            value: AttributeValue::Long(value),
        }
    }

    /// Create a double value.
    #[staticmethod]
    fn double(value: f64) -> Self {
        Self {
            value: AttributeValue::Double(value),
        }
    }

    /// Create a boolean value.
    #[staticmethod]
    fn boolean(value: bool) -> Self {
        Self {
            value: AttributeValue::Boolean(value),
        }
    }

    /// Create a date value from an ISO-8601 date string.
    #[staticmethod]
    fn date(value: &str) -> Self {
        Self {
            value: AttributeValue::Date(value.to_string()),
        }
    }

    /// Create a datetime value from an ISO-8601 datetime string.
    #[staticmethod]
    fn datetime(value: &str) -> Self {
        Self {
            value: AttributeValue::DateTime(value.to_string()),
        }
    }

    /// Create a datetime-tz value from an ISO-8601 datetime string.
    #[staticmethod]
    fn datetime_tz(value: &str) -> Self {
        Self {
            value: AttributeValue::DateTimeTZ(value.to_string()),
        }
    }

    /// Create a decimal value from its canonical string representation.
    #[staticmethod]
    fn decimal(value: &str) -> Self {
        Self {
            value: AttributeValue::Decimal(value.to_string()),
        }
    }

    /// Create a duration value from an ISO-8601 duration string.
    #[staticmethod]
    fn duration(value: &str) -> Self {
        Self {
            value: AttributeValue::Duration(value.to_string()),
        }
    }
}

/// Python-facing typed dynamic sort direction.
#[pyclass(name = "DynamicSortDir")]
#[derive(Clone, Copy)]
pub struct PyDynamicSortDir {
    direction: SortDir,
}

#[pymethods]
impl PyDynamicSortDir {
    /// Ascending order.
    #[staticmethod]
    fn asc() -> Self {
        Self {
            direction: SortDir::Asc,
        }
    }

    /// Descending order.
    #[staticmethod]
    fn desc() -> Self {
        Self {
            direction: SortDir::Desc,
        }
    }
}

/// Python-facing typed dynamic expression.
#[pyclass(name = "DynamicExpr")]
#[derive(Clone)]
pub struct PyDynamicExpr {
    expr: DynamicExpr,
}

#[pymethods]
impl PyDynamicExpr {
    /// Attribute equality.
    #[staticmethod]
    fn eq(attr_name: &str, value: PyRef<'_, PyDynamicValue>) -> Self {
        compare_expr(attr_name, DynamicComparisonOp::Eq, &value)
    }

    /// Attribute inequality.
    #[staticmethod]
    fn neq(attr_name: &str, value: PyRef<'_, PyDynamicValue>) -> Self {
        compare_expr(attr_name, DynamicComparisonOp::Neq, &value)
    }

    /// Attribute greater-than.
    #[staticmethod]
    fn gt(attr_name: &str, value: PyRef<'_, PyDynamicValue>) -> Self {
        compare_expr(attr_name, DynamicComparisonOp::Gt, &value)
    }

    /// Attribute greater-than-or-equal.
    #[staticmethod]
    fn gte(attr_name: &str, value: PyRef<'_, PyDynamicValue>) -> Self {
        compare_expr(attr_name, DynamicComparisonOp::Gte, &value)
    }

    /// Attribute less-than.
    #[staticmethod]
    fn lt(attr_name: &str, value: PyRef<'_, PyDynamicValue>) -> Self {
        compare_expr(attr_name, DynamicComparisonOp::Lt, &value)
    }

    /// Attribute less-than-or-equal.
    #[staticmethod]
    fn lte(attr_name: &str, value: PyRef<'_, PyDynamicValue>) -> Self {
        compare_expr(attr_name, DynamicComparisonOp::Lte, &value)
    }

    /// String contains.
    #[staticmethod]
    fn contains(attr_name: &str, substring: &str) -> Self {
        Self {
            expr: DynamicExpr::Compare {
                attr_name: attr_name.to_string(),
                operator: DynamicComparisonOp::Contains,
                value: AttributeValue::String(substring.to_string()),
            },
        }
    }

    /// String regex match.
    #[staticmethod]
    fn like(attr_name: &str, pattern: &str) -> Self {
        Self {
            expr: DynamicExpr::Compare {
                attr_name: attr_name.to_string(),
                operator: DynamicComparisonOp::Like,
                value: AttributeValue::String(pattern.to_string()),
            },
        }
    }

    /// IID lookup.
    #[staticmethod]
    fn iid(iid: &str) -> Self {
        Self {
            expr: DynamicExpr::Iid {
                iid: iid.to_string(),
            },
        }
    }

    /// Attribute absence.
    #[staticmethod]
    fn is_null(attr_name: &str) -> Self {
        Self {
            expr: DynamicExpr::IsNull {
                attr_name: attr_name.to_string(),
                is_null: true,
            },
        }
    }

    /// Attribute presence.
    #[staticmethod]
    fn is_not_null(attr_name: &str) -> Self {
        Self {
            expr: DynamicExpr::IsNull {
                attr_name: attr_name.to_string(),
                is_null: false,
            },
        }
    }

    /// Logical AND.
    #[staticmethod]
    fn and_(expressions: Bound<'_, PyList>) -> PyResult<Self> {
        Ok(Self {
            expr: DynamicExpr::And {
                exprs: dynamic_exprs_from_list(&expressions)?,
            },
        })
    }

    /// Logical OR.
    #[staticmethod]
    fn or_(expressions: Bound<'_, PyList>) -> PyResult<Self> {
        Ok(Self {
            expr: DynamicExpr::Or {
                exprs: dynamic_exprs_from_list(&expressions)?,
            },
        })
    }

    /// Logical NOT.
    #[staticmethod]
    fn not_(expression: PyRef<'_, PyDynamicExpr>) -> Self {
        Self {
            expr: DynamicExpr::Not {
                expr: Box::new(expression.expr.clone()),
            },
        }
    }

    /// Apply an expression to a relation role player.
    #[staticmethod]
    fn role_player(role_name: &str, expression: PyRef<'_, PyDynamicExpr>) -> Self {
        Self {
            expr: DynamicExpr::RolePlayer {
                role_name: role_name.to_string(),
                expr: Box::new(expression.expr.clone()),
            },
        }
    }
}

/// Python-facing typed dynamic sort.
#[pyclass(name = "DynamicSort")]
#[derive(Clone)]
pub struct PyDynamicSort {
    sort: DynamicSort,
}

#[pymethods]
impl PyDynamicSort {
    /// Sort by an attribute owned by the queried thing.
    #[staticmethod]
    fn attribute(attr_name: &str, direction: PyRef<'_, PyDynamicSortDir>) -> Self {
        Self {
            sort: DynamicSort::Attribute {
                attr_name: attr_name.to_string(),
                direction: direction.direction,
            },
        }
    }

    /// Sort a relation query by a role player's attribute.
    #[staticmethod]
    fn role_player_attribute(
        role_name: &str,
        attr_name: &str,
        direction: PyRef<'_, PyDynamicSortDir>,
    ) -> Self {
        Self {
            sort: DynamicSort::RolePlayerAttribute {
                role_name: role_name.to_string(),
                attr_name: attr_name.to_string(),
                direction: direction.direction,
            },
        }
    }
}

/// Build the TypeQL for a cross-type or narrowed attribute-owner lookup.
#[pyfunction]
#[pyo3(signature = (kind, attr_name, expression=None, type_name=None))]
fn build_has_lookup_query(
    kind: &str,
    attr_name: &str,
    expression: Option<PyRef<'_, PyDynamicExpr>>,
    type_name: Option<&str>,
) -> PyResult<String> {
    let expression = expression.as_ref().map(|expr| expr.expr.clone());
    type_bridge_orm::manager::query_builder::build_dynamic_has_lookup_query(
        kind,
        attr_name,
        expression.as_ref(),
        type_name,
    )
    .map_err(py_orm_error)
}

fn compare_expr(
    attr_name: &str,
    operator: DynamicComparisonOp,
    value: &PyDynamicValue,
) -> PyDynamicExpr {
    PyDynamicExpr {
        expr: DynamicExpr::Compare {
            attr_name: attr_name.to_string(),
            operator,
            value: value.value.clone(),
        },
    }
}

/// Python-facing Rust database handle.
#[pyclass]
pub struct PyRustDatabase {
    db: Arc<type_bridge_orm::Database>,
    runtime: Arc<Runtime>,
}

impl PyRustDatabase {
    /// Return shared `Arc` clones of the database and runtime handles.
    ///
    /// Exposes the capability to drive work on this database's connection and
    /// runtime without widening the struct fields to `pub`. Callers (e.g. the
    /// migration runner) must `block_on` the returned runtime so every Rust
    /// path shares one connection and one runtime.
    pub(crate) fn handles(&self) -> (Arc<type_bridge_orm::Database>, Arc<Runtime>) {
        (Arc::clone(&self.db), Arc::clone(&self.runtime))
    }
}

#[pymethods]
impl PyRustDatabase {
    /// Connect to TypeDB using the shared Rust ORM session layer.
    #[staticmethod]
    #[pyo3(signature = (address, database, username=None, password=None, http_port=core_version::DEFAULT_HTTP_PORT, server_version=None))]
    fn connect(
        address: &str,
        database: &str,
        username: Option<&str>,
        password: Option<&str>,
        http_port: u16,
        server_version: Option<&str>,
    ) -> PyResult<Self> {
        let runtime = Runtime::new().map(Arc::new).map_err(|error| {
            py_runtime_error(format!("Failed to create Tokio runtime: {error}"))
        })?;
        let username = username.unwrap_or("admin").to_string();
        let password = password.unwrap_or("password").to_string();
        let server_version: Option<core_version::Version> =
            server_version.map(str::parse).transpose().map_err(
                |error: core_version::VersionError| VersionError::new_err(error.to_string()),
            )?;
        let options = type_bridge_orm::ConnectOptions {
            http_port,
            server_version,
            ..type_bridge_orm::ConnectOptions::default()
        };
        let db = runtime
            .block_on(type_bridge_orm::Database::connect_with_options(
                address, database, &username, &password, options,
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

    /// The server version detected at connect time, when known.
    ///
    /// `None` only when the connection was established through the band-7
    /// gRPC fallback, where the server cannot report its version.
    fn server_version(&self) -> Option<String> {
        self.db.server_version().map(|version| version.to_string())
    }

    /// Version-gate schema DDL that uses `@doc`/`@meta` annotations.
    ///
    /// Raises the versioned error when the TypeQL uses schema annotations
    /// (TypeDB 3.12+) and the detected server version predates 3.12.
    fn check_schema_annotation_support(&self, typeql: &str) -> PyResult<()> {
        self.db
            .check_schema_annotation_support(typeql)
            .map_err(py_orm_error)
    }

    /// Whether the server and active negotiated provider support given-stage
    /// parameterized queries. `False` for pre-3.12/unknown servers and when a
    /// 3.12 connection remains on the band-8 discovery provider.
    fn supports_given_stage(&self) -> bool {
        self.db.supports_given_stage()
    }

    /// Execute a `given`-stage TypeQL query over input rows, auto-managing
    /// the transaction lifecycle.
    ///
    /// `variables` are the given variable names without the `$` sigil;
    /// `column_types` are TypeQL value type names (`string`, `integer`,
    /// `double`, `boolean`, `date`, `datetime`, `datetime-tz`) aligned with
    /// `variables`; `rows` is a list of rows, each a list of primitives in
    /// column order (temporal values as ISO-8601 strings).
    #[pyo3(signature = (query, transaction_type, variables, column_types, rows))]
    fn execute_with_rows(
        &self,
        py: Python<'_>,
        query: &str,
        transaction_type: &str,
        variables: Vec<String>,
        column_types: Vec<String>,
        rows: Bound<'_, PyAny>,
    ) -> PyResult<PyObject> {
        let tx_type = parse_tx_type(transaction_type)?;
        let spec = given_rows_from_py(variables, &column_types, &rows)?;
        let result = self
            .runtime
            .block_on(self.db.execute_with_rows(query, tx_type, spec))
            .map_err(py_orm_error)?;
        query_result_to_py(py, result)
    }

    /// Return whether the configured database exists.
    fn database_exists(&self) -> PyResult<bool> {
        self.runtime
            .block_on(self.db.database_exists())
            .map_err(py_orm_error)
    }

    /// Create the configured database if it does not already exist.
    fn create_database(&self) -> PyResult<()> {
        self.runtime
            .block_on(self.db.create_database())
            .map_err(py_orm_error)
    }

    /// Delete the configured database if it exists.
    fn delete_database(&self) -> PyResult<()> {
        self.runtime
            .block_on(self.db.delete_database())
            .map_err(py_orm_error)
    }

    /// Export the live TypeDB schema as TypeQL text.
    fn schema_text(&self) -> PyResult<String> {
        self.runtime
            .block_on(self.db.schema_text())
            .map_err(py_orm_error)
    }

    /// Introspect the live TypeDB schema through the Rust schema manager.
    fn introspect_schema(&self, py: Python<'_>) -> PyResult<PyObject> {
        let manager = type_bridge_orm::SchemaManager::new(self.db.as_ref());
        let info = self
            .runtime
            .block_on(manager.introspect())
            .map_err(py_orm_error)?;
        pythonize(py, &info)
            .map(|obj| obj.unbind())
            .map_err(|error| py_value_error(error.to_string()))
    }

    /// Open a Rust-owned transaction context.
    #[pyo3(signature = (transaction_type="read"))]
    fn transaction(&self, transaction_type: &str) -> PyResult<PyRustTransactionContext> {
        let tx_type = parse_tx_type(transaction_type)?;
        let context = self
            .runtime
            .block_on(self.db.transaction_context(tx_type))
            .map_err(py_orm_error)?;
        Ok(PyRustTransactionContext {
            context,
            runtime: Arc::clone(&self.runtime),
        })
    }
}

/// Python-facing Rust transaction context.
#[pyclass]
pub struct PyRustTransactionContext {
    context: TransactionContext,
    runtime: Arc<Runtime>,
}

impl PyRustTransactionContext {
    pub(crate) fn handles(&self) -> (TransactionContext, Arc<Runtime>) {
        (self.context.clone(), Arc::clone(&self.runtime))
    }
}

#[pymethods]
impl PyRustTransactionContext {
    /// Execute a raw TypeQL query in this Rust transaction.
    fn execute(&self, py: Python<'_>, query: &str) -> PyResult<PyObject> {
        let result = self
            .runtime
            .block_on(self.context.query(query))
            .map_err(py_orm_error)?;
        query_result_to_py(py, result)
    }

    /// Execute a `given`-stage TypeQL query with input rows in this Rust
    /// transaction. See `RustDatabase.execute_with_rows` for the argument
    /// contract; requires a band-9 (TypeDB 3.12+) connection.
    #[pyo3(signature = (query, variables, column_types, rows))]
    fn execute_with_rows(
        &self,
        py: Python<'_>,
        query: &str,
        variables: Vec<String>,
        column_types: Vec<String>,
        rows: Bound<'_, PyAny>,
    ) -> PyResult<PyObject> {
        let spec = given_rows_from_py(variables, &column_types, &rows)?;
        let result = self
            .runtime
            .block_on(self.context.query_with_rows(query, spec))
            .map_err(py_orm_error)?;
        query_result_to_py(py, result)
    }

    /// Commit this Rust transaction.
    fn commit(&self) -> PyResult<()> {
        self.runtime
            .block_on(self.context.commit())
            .map_err(py_orm_error)
    }

    /// Roll back this Rust transaction.
    fn rollback(&self) -> PyResult<()> {
        self.runtime
            .block_on(self.context.rollback())
            .map_err(py_orm_error)
    }

    /// Close this Rust transaction without committing.
    fn close(&self) -> PyResult<()> {
        self.runtime
            .block_on(self.context.close())
            .map_err(py_orm_error)
    }

    /// Return this transaction's type name.
    fn transaction_type(&self) -> &'static str {
        tx_type_name(self.context.tx_type())
    }
}

/// Python-facing dynamic entity manager.
#[pyclass]
pub struct PyDynamicEntityManager {
    db: Option<Arc<type_bridge_orm::Database>>,
    tx: Option<TransactionContext>,
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
            db: Some(Arc::clone(&database.db)),
            tx: None,
            runtime: Arc::clone(&database.runtime),
            descriptor: Arc::new(descriptor),
        })
    }

    /// Construct a dynamic entity manager bound to an existing Rust transaction.
    #[staticmethod]
    fn for_transaction(
        transaction: &PyRustTransactionContext,
        descriptor: Bound<'_, PyAny>,
    ) -> PyResult<Self> {
        let descriptor: EntityDescriptor = depythonize(&descriptor)
            .map_err(|error| py_value_error(format!("Invalid entity descriptor: {error}")))?;
        Ok(Self {
            db: None,
            tx: Some(transaction.context.clone()),
            runtime: Arc::clone(&transaction.runtime),
            descriptor: Arc::new(descriptor),
        })
    }

    /// Insert one entity and return its IID.
    fn insert(&self, attributes: Bound<'_, PyAny>) -> PyResult<String> {
        let attributes = entity_attributes_from_py(&self.descriptor, attributes)?;
        let manager = self.manager()?;
        self.runtime
            .block_on(manager.insert(&attributes))
            .map_err(py_orm_error)
    }

    /// Insert multiple entities in one Rust transaction and return their IIDs.
    fn insert_many(&self, attributes: Bound<'_, PyAny>) -> PyResult<Vec<String>> {
        let attributes = entity_attribute_list_from_py(&self.descriptor, attributes)?;
        let manager = self.manager()?;
        self.runtime
            .block_on(manager.insert_many(&attributes))
            .map_err(py_orm_error)
    }

    /// Put one entity and return its IID.
    fn put(&self, attributes: Bound<'_, PyAny>) -> PyResult<String> {
        let attributes = entity_attributes_from_py(&self.descriptor, attributes)?;
        let manager = self.manager()?;
        self.runtime
            .block_on(manager.put(&attributes))
            .map_err(py_orm_error)
    }

    /// Put multiple entities in one Rust transaction and return their IIDs.
    fn put_many(&self, attributes: Bound<'_, PyAny>) -> PyResult<Vec<String>> {
        let attributes = entity_attribute_list_from_py(&self.descriptor, attributes)?;
        let manager = self.manager()?;
        self.runtime
            .block_on(manager.put_many(&attributes))
            .map_err(py_orm_error)
    }

    /// Update one entity's non-key attributes.
    #[pyo3(signature = (attributes, iid=None))]
    fn update(&self, attributes: Bound<'_, PyAny>, iid: Option<&str>) -> PyResult<()> {
        let attributes = entity_attributes_from_py(&self.descriptor, attributes)?;
        let manager = self.manager()?;
        self.runtime
            .block_on(manager.update(iid, &attributes))
            .map_err(py_orm_error)
    }

    /// Fetch entities matching equality filters.
    #[pyo3(signature = (filters=None))]
    fn get(&self, py: Python<'_>, filters: Option<Bound<'_, PyAny>>) -> PyResult<PyObject> {
        let filters = entity_filters_from_py(&self.descriptor, filters)?;
        let manager = self.manager()?;
        let rows = self
            .runtime
            .block_on(manager.get(&filters))
            .map_err(py_orm_error)?;
        entity_rows_to_py(py, &rows)
    }

    /// Fetch entities matching dynamic expression and sort specs.
    #[pyo3(signature = (expressions=None, sorts=None, limit=None, offset=None))]
    fn get_with_query(
        &self,
        py: Python<'_>,
        expressions: Option<Bound<'_, PyList>>,
        sorts: Option<Bound<'_, PyList>>,
        limit: Option<u64>,
        offset: Option<u64>,
    ) -> PyResult<PyObject> {
        let expressions = dynamic_exprs_from_py_list(expressions)?;
        let sorts = dynamic_sorts_from_py_list(sorts)?;
        let manager = self.manager()?;
        let rows = self
            .runtime
            .block_on(manager.get_with_query(&expressions, &sorts, limit, offset))
            .map_err(py_orm_error)?;
        entity_rows_to_py(py, &rows)
    }

    /// Fetch one entity by TypeDB IID.
    fn get_by_iid(&self, py: Python<'_>, iid: &str) -> PyResult<PyObject> {
        let manager = self.manager()?;
        let row = self
            .runtime
            .block_on(manager.get_by_iid(iid))
            .map_err(py_orm_error)?;
        optional_entity_row_to_py(py, row.as_ref())
    }

    /// Fetch all entities for this descriptor.
    fn all(&self, py: Python<'_>) -> PyResult<PyObject> {
        self.get(py, None)
    }

    /// Count entities matching equality filters.
    #[pyo3(signature = (filters=None))]
    fn count(&self, filters: Option<Bound<'_, PyAny>>) -> PyResult<u64> {
        let filters = entity_filters_from_py(&self.descriptor, filters)?;
        let manager = self.manager()?;
        self.runtime
            .block_on(manager.count_with_filters(&filters))
            .map_err(py_orm_error)
    }

    /// Count entities matching dynamic expression specs.
    #[pyo3(signature = (expressions=None))]
    fn count_with_query(&self, expressions: Option<Bound<'_, PyList>>) -> PyResult<u64> {
        let expressions = dynamic_exprs_from_py_list(expressions)?;
        let manager = self.manager()?;
        self.runtime
            .block_on(manager.count_with_query(&expressions))
            .map_err(py_orm_error)
    }

    /// Run aggregate reductions over entities matching equality filters.
    #[pyo3(signature = (aggregates, filters=None))]
    fn aggregate(
        &self,
        py: Python<'_>,
        aggregates: Bound<'_, PyAny>,
        filters: Option<Bound<'_, PyAny>>,
    ) -> PyResult<PyObject> {
        let filters = entity_filters_from_py(&self.descriptor, filters)?;
        let aggregates = aggregates_from_py(&self.descriptor.owned_attributes, aggregates)?;
        let manager = self.manager()?;
        let rows = self
            .runtime
            .block_on(manager.aggregate(&filters, &aggregates))
            .map_err(py_orm_error)?;
        pythonize(py, &rows)
            .map(|obj| obj.unbind())
            .map_err(|error| py_value_error(error.to_string()))
    }

    /// Run grouped aggregate reductions over entities matching equality filters.
    #[pyo3(signature = (group_fields, aggregates, filters=None))]
    fn group_by_aggregate(
        &self,
        py: Python<'_>,
        group_fields: Bound<'_, PyAny>,
        aggregates: Bound<'_, PyAny>,
        filters: Option<Bound<'_, PyAny>>,
    ) -> PyResult<PyObject> {
        let filters = entity_filters_from_py(&self.descriptor, filters)?;
        let group_fields = group_fields_from_py(&self.descriptor.owned_attributes, group_fields)?;
        let aggregates = aggregates_from_py(&self.descriptor.owned_attributes, aggregates)?;
        let manager = self.manager()?;
        let rows = self
            .runtime
            .block_on(manager.group_by_aggregate(&filters, &group_fields, &aggregates))
            .map_err(py_orm_error)?;
        pythonize(py, &rows)
            .map(|obj| obj.unbind())
            .map_err(|error| py_value_error(error.to_string()))
    }

    /// Delete one entity by IID.
    fn delete_by_iid(&self, iid: &str) -> PyResult<()> {
        let manager = self.manager()?;
        self.runtime
            .block_on(manager.delete_by_iid(iid))
            .map_err(py_orm_error)
    }
}

impl PyDynamicEntityManager {
    fn manager(&self) -> PyResult<DynamicEntityManager<'_>> {
        if let Some(tx) = &self.tx {
            return Ok(DynamicEntityManager::with_transaction(
                tx.clone(),
                Arc::clone(&self.descriptor),
            ));
        }
        let db = self
            .db
            .as_ref()
            .ok_or_else(|| py_runtime_error("Rust entity manager has no execution target"))?;
        Ok(DynamicEntityManager::new(db, Arc::clone(&self.descriptor)))
    }
}

/// Python-facing dynamic relation manager.
#[pyclass]
pub struct PyDynamicRelationManager {
    db: Option<Arc<type_bridge_orm::Database>>,
    tx: Option<TransactionContext>,
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
            db: Some(Arc::clone(&database.db)),
            tx: None,
            runtime: Arc::clone(&database.runtime),
            descriptor: Arc::new(descriptor),
        })
    }

    /// Construct a dynamic relation manager bound to an existing Rust transaction.
    #[staticmethod]
    fn for_transaction(
        transaction: &PyRustTransactionContext,
        descriptor: Bound<'_, PyAny>,
    ) -> PyResult<Self> {
        let descriptor: RelationDescriptor = depythonize(&descriptor)
            .map_err(|error| py_value_error(format!("Invalid relation descriptor: {error}")))?;
        Ok(Self {
            db: None,
            tx: Some(transaction.context.clone()),
            runtime: Arc::clone(&transaction.runtime),
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
        let manager = self.manager()?;
        self.runtime
            .block_on(manager.insert(&attributes, &role_players))
            .map_err(py_orm_error)
    }

    /// Insert multiple relations in one Rust transaction and return their IIDs.
    fn insert_many(&self, items: Bound<'_, PyAny>) -> PyResult<Vec<String>> {
        let items = relation_write_batch_from_py(&self.descriptor, items)?;
        let manager = self.manager()?;
        self.runtime
            .block_on(manager.insert_many(&items))
            .map_err(py_orm_error)
    }

    /// Put one relation and return its IID.
    fn put(
        &self,
        attributes: Bound<'_, PyAny>,
        role_players: Bound<'_, PyAny>,
    ) -> PyResult<String> {
        let attributes = relation_attributes_from_py(&self.descriptor, attributes)?;
        let role_players = role_players_from_py(&self.descriptor, role_players)?;
        let manager = self.manager()?;
        self.runtime
            .block_on(manager.put(&attributes, &role_players))
            .map_err(py_orm_error)
    }

    /// Put multiple relations in one Rust transaction and return their IIDs.
    fn put_many(&self, items: Bound<'_, PyAny>) -> PyResult<Vec<String>> {
        let items = relation_write_batch_from_py(&self.descriptor, items)?;
        let manager = self.manager()?;
        self.runtime
            .block_on(manager.put_many(&items))
            .map_err(py_orm_error)
    }

    /// Update one relation's scalar non-key attributes.
    #[pyo3(signature = (attributes, role_players, iid=None))]
    fn update(
        &self,
        attributes: Bound<'_, PyAny>,
        role_players: Bound<'_, PyAny>,
        iid: Option<&str>,
    ) -> PyResult<()> {
        let attributes = relation_attributes_from_py(&self.descriptor, attributes)?;
        let role_players = role_players_from_py(&self.descriptor, role_players)?;
        let manager = self.manager()?;
        self.runtime
            .block_on(manager.update(iid, &attributes, &role_players))
            .map_err(py_orm_error)
    }

    /// Fetch relations matching equality filters.
    #[pyo3(signature = (filters=None))]
    fn get(&self, py: Python<'_>, filters: Option<Bound<'_, PyAny>>) -> PyResult<PyObject> {
        let filters = relation_filters_from_py(&self.descriptor, filters)?;
        let manager = self.manager()?;
        let rows = self
            .runtime
            .block_on(manager.get(&filters))
            .map_err(py_orm_error)?;
        relation_rows_to_py(py, &rows)
    }

    /// Fetch relations matching dynamic expression and sort specs.
    #[pyo3(signature = (expressions=None, sorts=None, limit=None, offset=None))]
    fn get_with_query(
        &self,
        py: Python<'_>,
        expressions: Option<Bound<'_, PyList>>,
        sorts: Option<Bound<'_, PyList>>,
        limit: Option<u64>,
        offset: Option<u64>,
    ) -> PyResult<PyObject> {
        let expressions = dynamic_exprs_from_py_list(expressions)?;
        let sorts = dynamic_sorts_from_py_list(sorts)?;
        let manager = self.manager()?;
        let rows = self
            .runtime
            .block_on(manager.get_with_query(&expressions, &sorts, limit, offset))
            .map_err(py_orm_error)?;
        relation_rows_to_py(py, &rows)
    }

    /// Fetch relations matching attribute filters and role-player filters.
    #[pyo3(signature = (filters=None, role_players=None))]
    fn get_with_role_players(
        &self,
        py: Python<'_>,
        filters: Option<Bound<'_, PyAny>>,
        role_players: Option<Bound<'_, PyAny>>,
    ) -> PyResult<PyObject> {
        let filters = relation_filters_from_py(&self.descriptor, filters)?;
        let role_players = match role_players {
            Some(role_players) if !role_players.is_none() => {
                role_players_from_py(&self.descriptor, role_players)?
            }
            _ => vec![],
        };
        let manager = self.manager()?;
        let rows = self
            .runtime
            .block_on(manager.get_with_role_filters(&filters, &role_players))
            .map_err(py_orm_error)?;
        relation_rows_to_py(py, &rows)
    }

    /// Fetch one relation by TypeDB IID.
    fn get_by_iid(&self, py: Python<'_>, iid: &str) -> PyResult<PyObject> {
        let manager = self.manager()?;
        let rows = self
            .runtime
            .block_on(manager.get_by_iid(iid))
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
        let manager = self.manager()?;
        self.runtime
            .block_on(manager.count_with_filters(&filters))
            .map_err(py_orm_error)
    }

    /// Count relations matching dynamic expression specs.
    #[pyo3(signature = (expressions=None))]
    fn count_with_query(&self, expressions: Option<Bound<'_, PyList>>) -> PyResult<u64> {
        let expressions = dynamic_exprs_from_py_list(expressions)?;
        let manager = self.manager()?;
        self.runtime
            .block_on(manager.count_with_query(&expressions))
            .map_err(py_orm_error)
    }

    /// Run aggregate reductions over relations matching equality filters.
    #[pyo3(signature = (aggregates, filters=None))]
    fn aggregate(
        &self,
        py: Python<'_>,
        aggregates: Bound<'_, PyAny>,
        filters: Option<Bound<'_, PyAny>>,
    ) -> PyResult<PyObject> {
        let filters = relation_filters_from_py(&self.descriptor, filters)?;
        let aggregates = aggregates_from_py(&self.descriptor.owned_attributes, aggregates)?;
        let manager = self.manager()?;
        let rows = self
            .runtime
            .block_on(manager.aggregate(&filters, &aggregates))
            .map_err(py_orm_error)?;
        pythonize(py, &rows)
            .map(|obj| obj.unbind())
            .map_err(|error| py_value_error(error.to_string()))
    }

    /// Run grouped aggregate reductions over relations matching equality filters.
    #[pyo3(signature = (group_fields, aggregates, filters=None))]
    fn group_by_aggregate(
        &self,
        py: Python<'_>,
        group_fields: Bound<'_, PyAny>,
        aggregates: Bound<'_, PyAny>,
        filters: Option<Bound<'_, PyAny>>,
    ) -> PyResult<PyObject> {
        let filters = relation_filters_from_py(&self.descriptor, filters)?;
        let group_fields = group_fields_from_py(&self.descriptor.owned_attributes, group_fields)?;
        let aggregates = aggregates_from_py(&self.descriptor.owned_attributes, aggregates)?;
        let manager = self.manager()?;
        let rows = self
            .runtime
            .block_on(manager.group_by_aggregate(&filters, &group_fields, &aggregates))
            .map_err(py_orm_error)?;
        pythonize(py, &rows)
            .map(|obj| obj.unbind())
            .map_err(|error| py_value_error(error.to_string()))
    }

    /// Delete one relation by IID.
    fn delete_by_iid(&self, iid: &str) -> PyResult<()> {
        let manager = self.manager()?;
        self.runtime
            .block_on(manager.delete_by_iid(iid))
            .map_err(py_orm_error)
    }
}

impl PyDynamicRelationManager {
    fn manager(&self) -> PyResult<DynamicRelationManager<'_>> {
        if let Some(tx) = &self.tx {
            return Ok(DynamicRelationManager::with_transaction(
                tx.clone(),
                Arc::clone(&self.descriptor),
            ));
        }
        let db = self
            .db
            .as_ref()
            .ok_or_else(|| py_runtime_error("Rust relation manager has no execution target"))?;
        Ok(DynamicRelationManager::new(
            db,
            Arc::clone(&self.descriptor),
        ))
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
    attributes_from_json(descriptors, &value)
}

fn entity_attribute_list_from_py(
    descriptor: &EntityDescriptor,
    value: Bound<'_, PyAny>,
) -> PyResult<Vec<DynamicAttributeMap>> {
    attribute_list_from_py(&descriptor.owned_attributes, value)
}

fn attribute_list_from_py(
    descriptors: &[type_bridge_orm::OwnedAttributeDescriptor],
    value: Bound<'_, PyAny>,
) -> PyResult<Vec<DynamicAttributeMap>> {
    let value = py_to_json(value)?;
    let items = value
        .as_array()
        .ok_or_else(|| py_type_error("Batch attributes must be a list of dicts"))?;
    items
        .iter()
        .map(|item| attributes_from_json(descriptors, item))
        .collect()
}

fn attributes_from_json(
    descriptors: &[type_bridge_orm::OwnedAttributeDescriptor],
    value: &Value,
) -> PyResult<DynamicAttributeMap> {
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
    if let Some(items) = value.as_array() {
        let mut rust_filters = Vec::with_capacity(items.len());
        for item in items {
            let obj = item
                .as_object()
                .ok_or_else(|| py_type_error("Each filter spec must be a dict"))?;
            let attr_name = required_string(obj, "attr_name")?;
            let operator = required_string(obj, "operator")?;
            let value = obj
                .get("value")
                .ok_or_else(|| py_type_error("Filter spec missing value"))?;
            let descriptor = descriptors
                .iter()
                .find(|descriptor| {
                    descriptor.field_name == attr_name || descriptor.attr_name == attr_name
                })
                .ok_or_else(|| py_value_error(format!("Unknown filter attribute '{attr_name}'")))?;
            let attr_value = attribute_value_from_json(value, descriptor.value_type)?;
            rust_filters.push(Filter::compare(
                descriptor.attr_name.clone(),
                operator,
                attr_value,
            ));
        }
        return Ok(rust_filters);
    }
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

fn dynamic_exprs_from_py_list(
    expressions: Option<Bound<'_, PyList>>,
) -> PyResult<Vec<DynamicExpr>> {
    let Some(expressions) = expressions else {
        return Ok(vec![]);
    };
    dynamic_exprs_from_list(&expressions)
}

fn dynamic_exprs_from_list(expressions: &Bound<'_, PyList>) -> PyResult<Vec<DynamicExpr>> {
    expressions
        .iter()
        .map(|item| {
            let expression = item.extract::<PyRef<'_, PyDynamicExpr>>()?;
            Ok(expression.expr.clone())
        })
        .collect()
}

fn dynamic_sorts_from_py_list(sorts: Option<Bound<'_, PyList>>) -> PyResult<Vec<DynamicSort>> {
    let Some(sorts) = sorts else {
        return Ok(vec![]);
    };
    sorts
        .iter()
        .map(|item| {
            let sort = item.extract::<PyRef<'_, PyDynamicSort>>()?;
            Ok(sort.sort.clone())
        })
        .collect()
}

fn aggregates_from_py(
    descriptors: &[type_bridge_orm::OwnedAttributeDescriptor],
    aggregates: Bound<'_, PyAny>,
) -> PyResult<Vec<DynamicAggregate>> {
    let value = py_to_json(aggregates)?;
    let items = value
        .as_array()
        .ok_or_else(|| py_type_error("Aggregates must be a list of dicts"))?;
    let mut rust_aggregates = Vec::with_capacity(items.len());
    for item in items {
        let obj = item
            .as_object()
            .ok_or_else(|| py_type_error("Each aggregate must be a dict"))?;
        let result_key = required_string(obj, "result_key")?;
        let function = required_string(obj, "function")?;
        let attr_name = match obj.get("attr_name") {
            Some(Value::Null) | None => None,
            Some(value) => {
                let attr_name = value
                    .as_str()
                    .ok_or_else(|| py_type_error("Aggregate attr_name must be a string"))?;
                let descriptor = descriptors
                    .iter()
                    .find(|descriptor| {
                        descriptor.field_name == attr_name || descriptor.attr_name == attr_name
                    })
                    .ok_or_else(|| {
                        py_value_error(format!("Unknown aggregate attribute '{attr_name}'"))
                    })?;
                Some(descriptor.attr_name.clone())
            }
        };
        rust_aggregates.push(DynamicAggregate {
            result_key,
            function,
            attr_name,
        });
    }
    Ok(rust_aggregates)
}

fn group_fields_from_py(
    descriptors: &[type_bridge_orm::OwnedAttributeDescriptor],
    fields: Bound<'_, PyAny>,
) -> PyResult<Vec<String>> {
    let value = py_to_json(fields)?;
    let items = value
        .as_array()
        .ok_or_else(|| py_type_error("Group fields must be a list of strings"))?;
    let mut group_fields = Vec::with_capacity(items.len());
    for item in items {
        let field = item
            .as_str()
            .ok_or_else(|| py_type_error("Group field must be a string"))?;
        let descriptor = descriptors
            .iter()
            .find(|descriptor| descriptor.field_name == field || descriptor.attr_name == field)
            .ok_or_else(|| py_value_error(format!("Unknown group field '{field}'")))?;
        group_fields.push(descriptor.attr_name.clone());
    }
    Ok(group_fields)
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
    role_players_from_json(descriptor, &value)
}

fn role_players_from_json(
    descriptor: &RelationDescriptor,
    value: &Value,
) -> PyResult<Vec<DynamicRolePlayerInput>> {
    let players = value
        .as_array()
        .ok_or_else(|| py_type_error("Role players must be a list of dicts"))?;
    let mut inputs = Vec::new();

    for player in players {
        let obj = player
            .as_object()
            .ok_or_else(|| py_type_error("Each role player must be a dict"))?;
        let role_name = required_string(obj, "role_name")?;
        // Validate that the role exists, but not the player's concrete type.
        // A role's declared player_type_names are the (possibly abstract)
        // declared targets; any concrete subtype is a legal player. Subtype
        // compatibility is enforced by TypeDB at insert time — mirroring the
        // orm backend, which performs no player-type membership check here.
        if descriptor.role(&role_name).is_none() {
            return Err(py_value_error(format!("Unknown role '{role_name}'")));
        }
        let player_type_name = required_string(obj, "player_type_name")?;

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

fn relation_write_batch_from_py(
    descriptor: &RelationDescriptor,
    value: Bound<'_, PyAny>,
) -> PyResult<Vec<(DynamicAttributeMap, Vec<DynamicRolePlayerInput>)>> {
    let value = py_to_json(value)?;
    let items = value
        .as_array()
        .ok_or_else(|| py_type_error("Relation batch must be a list of dicts"))?;
    let mut batch = Vec::with_capacity(items.len());
    for item in items {
        let obj = item
            .as_object()
            .ok_or_else(|| py_type_error("Each relation batch item must be a dict"))?;
        let attributes = obj
            .get("attributes")
            .ok_or_else(|| py_value_error("Relation batch item missing attributes"))?;
        let role_players = obj
            .get("role_players")
            .ok_or_else(|| py_value_error("Relation batch item missing role_players"))?;
        batch.push((
            attributes_from_json(&descriptor.owned_attributes, attributes)?,
            role_players_from_json(descriptor, role_players)?,
        ));
    }
    Ok(batch)
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

fn optional_entity_row_to_py(py: Python<'_>, row: Option<&DynamicEntityRow>) -> PyResult<PyObject> {
    match row {
        Some(row) => pythonize(py, &entity_row_to_json(row))
            .map(|obj| obj.unbind())
            .map_err(|error| py_value_error(error.to_string())),
        None => Ok(py.None()),
    }
}

fn entity_row_to_json(row: &DynamicEntityRow) -> Value {
    let mut obj = Map::new();
    for (name, value) in &row.attributes {
        insert_repeated_attribute(&mut obj, name, attribute_value_to_json(value));
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
            let mut attributes = Map::new();
            for (name, value) in &player.attributes {
                insert_repeated_attribute(&mut attributes, name, value.clone());
            }
            obj.insert("attributes".into(), Value::Object(attributes));
            Value::Object(obj)
        })
        .collect();
    obj.insert("role_players".into(), Value::Array(role_players));
    Value::Object(obj)
}

fn insert_repeated_attribute(obj: &mut Map<String, Value>, name: &str, value: Value) {
    match obj.get_mut(name) {
        Some(Value::Array(values)) => values.push(value),
        Some(existing) => {
            let first = std::mem::replace(existing, Value::Null);
            *existing = Value::Array(vec![first, value]);
        }
        None => {
            obj.insert(name.to_string(), value);
        }
    }
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

fn query_result_to_py(py: Python<'_>, result: QueryResult) -> PyResult<PyObject> {
    let values = match result {
        QueryResult::Ok => Vec::new(),
        QueryResult::Documents(values) | QueryResult::Rows(values) => values,
    };
    pythonize(py, &values)
        .map(|obj| obj.unbind())
        .map_err(|error| py_value_error(error.to_string()))
}

fn py_to_json(value: Bound<'_, PyAny>) -> PyResult<Value> {
    depythonize(&value)
        .map_err(|error| py_value_error(format!("Expected JSON-compatible value: {error}")))
}

fn parse_tx_type(value: &str) -> PyResult<TxType> {
    match value.trim().to_ascii_lowercase().as_str() {
        "read" => Ok(TxType::Read),
        "write" => Ok(TxType::Write),
        "schema" => Ok(TxType::Schema),
        other => Err(py_value_error(format!(
            "transaction_type must be 'read', 'write', or 'schema', got {other:?}"
        ))),
    }
}

/// Marshal Python given rows into the ORM's [`GivenRowsSpec`].
///
/// `column_types` drives exact per-cell extraction so Python's numeric subtype
/// coercions cannot silently change a TypeQL value category.
fn given_rows_from_py(
    variables: Vec<String>,
    column_types: &[String],
    rows: &Bound<'_, PyAny>,
) -> PyResult<GivenRowsSpec> {
    if variables.len() != column_types.len() {
        return Err(py_value_error(format!(
            "variables ({}) and column_types ({}) must have the same length",
            variables.len(),
            column_types.len()
        )));
    }
    let rows: Vec<Vec<Bound<'_, PyAny>>> = rows
        .extract()
        .map_err(|error| py_value_error(format!("rows must be a sequence of rows: {error}")))?;
    let mut spec_rows = Vec::with_capacity(rows.len());
    for (row_index, row) in rows.iter().enumerate() {
        if row.len() != column_types.len() {
            return Err(py_value_error(format!(
                "row {row_index} has {} cells; expected {} (one per variable)",
                row.len(),
                column_types.len()
            )));
        }
        let mut cells = Vec::with_capacity(row.len());
        for (cell, column_type) in row.iter().zip(column_types) {
            cells.push(given_value_from_py(cell, column_type).map_err(|error| {
                let message = format!("row {row_index}: {error}");
                if error.is_instance_of::<pyo3::exceptions::PyTypeError>(cell.py()) {
                    py_type_error(message)
                } else {
                    py_value_error(message)
                }
            })?);
        }
        spec_rows.push(cells);
    }
    Ok(GivenRowsSpec {
        variables,
        rows: spec_rows,
    })
}

/// Extract one given cell according to its declared TypeQL column type.
fn given_value_from_py(cell: &Bound<'_, PyAny>, column_type: &str) -> PyResult<GivenValue> {
    Ok(match column_type {
        "boolean" => GivenValue::Boolean(
            cell.downcast_exact::<PyBool>()
                .map_err(|_| py_type_error("given boolean cells require an exact Python bool"))?
                .extract()?,
        ),
        // "long" is the ORM-internal name for the TypeQL "integer" type.
        "integer" | "long" => GivenValue::Integer(
            cell.downcast_exact::<PyInt>()
                .map_err(|_| py_type_error("given integer cells require an exact Python int"))?
                .extract()?,
        ),
        "double" => GivenValue::Double(
            cell.downcast_exact::<PyFloat>()
                .map_err(|_| py_type_error("given double cells require an exact Python float"))?
                .extract()?,
        ),
        "string" => GivenValue::String(exact_given_string(cell, "string")?),
        "date" => GivenValue::Date(exact_given_string(cell, "date")?),
        "datetime" => GivenValue::Datetime(exact_given_string(cell, "datetime")?),
        "datetime-tz" => GivenValue::DatetimeTz(exact_given_string(cell, "datetime-tz")?),
        other => {
            return Err(py_value_error(format!(
                "Unsupported given column type {other:?}; expected one of string, \
                 integer, double, boolean, date, datetime, datetime-tz"
            )));
        }
    })
}

fn exact_given_string(cell: &Bound<'_, PyAny>, column_type: &str) -> PyResult<String> {
    cell.downcast_exact::<PyString>()
        .map_err(|_| {
            py_type_error(format!(
                "given {column_type} cells require an exact Python str"
            ))
        })?
        .extract()
}

fn tx_type_name(tx_type: TxType) -> &'static str {
    match tx_type {
        TxType::Read => "read",
        TxType::Write => "write",
        TxType::Schema => "schema",
    }
}

fn required_string(obj: &Map<String, Value>, key: &str) -> PyResult<String> {
    obj.get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| py_value_error(format!("Missing string field '{key}'")))
}

fn py_orm_error(error: OrmError) -> PyErr {
    match error {
        OrmError::UnsupportedVersion(error) => VersionError::new_err(error.to_string()),
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
    m.add_class::<PyRustTransactionContext>()?;
    m.add_class::<PyDynamicValue>()?;
    m.add_class::<PyDynamicSortDir>()?;
    m.add_class::<PyDynamicExpr>()?;
    m.add_class::<PyDynamicSort>()?;
    m.add_function(wrap_pyfunction!(build_has_lookup_query, m)?)?;
    m.add_class::<PyDynamicEntityManager>()?;
    m.add_class::<PyDynamicRelationManager>()?;
    // Keep these imported so PyO3 validates the signatures at compile time.
    let _ = PyDict::new(m.py());
    let _ = PyList::empty(m.py());
    Ok(())
}

#[cfg(test)]
mod given_rows_tests {
    use pyo3::ffi;

    use super::*;

    #[test]
    fn given_cell_marshalling_requires_exact_python_primitive_types() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let valid = [
                ("True", "boolean", GivenValue::Boolean(true)),
                ("42", "integer", GivenValue::Integer(42)),
                ("1.5", "double", GivenValue::Double(1.5)),
                ("'hello'", "string", GivenValue::String("hello".into())),
                (
                    "'2026-07-13'",
                    "date",
                    GivenValue::Date("2026-07-13".into()),
                ),
                (
                    "'2026-07-13T10:30:00'",
                    "datetime",
                    GivenValue::Datetime("2026-07-13T10:30:00".into()),
                ),
                (
                    "'2026-07-13T10:30:00+09:00'",
                    "datetime-tz",
                    GivenValue::DatetimeTz("2026-07-13T10:30:00+09:00".into()),
                ),
            ];
            for (source, column_type, expected) in valid {
                let source = std::ffi::CString::new(source).unwrap();
                let value = py.eval(source.as_c_str(), None, None).unwrap();
                assert_eq!(given_value_from_py(&value, column_type).unwrap(), expected);
            }

            for (source, column_type) in [
                ("True", "integer"),
                ("1", "boolean"),
                ("1", "double"),
                ("1.0", "integer"),
                ("b'bytes'", "string"),
                ("object()", "date"),
            ] {
                let source = std::ffi::CString::new(source).unwrap();
                let value = py.eval(source.as_c_str(), None, None).unwrap();
                assert!(
                    given_value_from_py(&value, column_type).is_err(),
                    "{source:?} must not coerce into {column_type}"
                );
            }

            let unsupported = py.eval(ffi::c_str!("'value'"), None, None).unwrap();
            assert!(
                given_value_from_py(&unsupported, "decimal")
                    .unwrap_err()
                    .to_string()
                    .contains("Unsupported given column type")
            );
        });
    }

    #[test]
    fn given_rows_marshalling_validates_headers_width_and_row_types() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let rows = py
                .eval(ffi::c_str!("[['Alice', 30], ['Bob', 25]]"), None, None)
                .unwrap();
            let spec = given_rows_from_py(
                vec!["name".into(), "age".into()],
                &["string".into(), "integer".into()],
                &rows,
            )
            .unwrap();
            assert_eq!(spec.variables, vec!["name", "age"]);
            assert_eq!(spec.rows.len(), 2);

            let header_error = given_rows_from_py(
                vec!["name".into()],
                &["string".into(), "integer".into()],
                &rows,
            )
            .unwrap_err();
            assert!(header_error.to_string().contains("same length"));

            let short = py.eval(ffi::c_str!("[['Alice']]"), None, None).unwrap();
            let width_error = given_rows_from_py(
                vec!["name".into(), "age".into()],
                &["string".into(), "integer".into()],
                &short,
            )
            .unwrap_err();
            assert!(
                width_error
                    .to_string()
                    .contains("row 0 has 1 cells; expected 2")
            );

            let wrong = py
                .eval(ffi::c_str!("[['Alice', True]]"), None, None)
                .unwrap();
            let type_error = given_rows_from_py(
                vec!["name".into(), "age".into()],
                &["string".into(), "integer".into()],
                &wrong,
            )
            .unwrap_err();
            assert!(type_error.is_instance_of::<pyo3::exceptions::PyTypeError>(py));
            assert!(type_error.to_string().contains("row 0"));
            assert!(type_error.to_string().contains("exact Python int"));
        });
    }
}
