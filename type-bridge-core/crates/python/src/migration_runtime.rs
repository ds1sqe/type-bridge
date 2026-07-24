//! PyO3 wrappers for migration spec normalization, graph validation, and the
//! checksum-drift gate.
//!
//! These functions serde-normalize migration IR and run pure graph/checksum
//! checks over it. They open no transactions and execute no TypeQL — migration
//! execution is owned by a later sub-plan.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use pyo3::prelude::*;
use pyo3::types::PyBytes;
use pythonize::{depythonize, pythonize};
use type_bridge_migration::{
    AppliedMigrationRecord, LegacyDirectoryAuthority, LegacyDirectoryEntry, LegacyMetadataRevision,
    MigrationGraph, MigrationRunRecord, MigrationSpec, MigrationStateSchemaKind,
    MigrationStateStore, TypeDbStateStore,
    applied_migration_entity_label as rust_applied_migration_entity_label, check_checksum_drift,
    collect_executor_info, execute_plan, execute_plan_with_run_log,
    is_migration_state_type as rust_is_migration_state_type, load_sidecar, migration_file_checksum,
    migration_state_schema as rust_migration_state_schema, plan, require_legacy_writer_open,
    validate_graph,
};
use type_bridge_orm::ProviderRuntimeOwner;

use crate::orm_runtime::{PyRustDatabase, provider_block_on};

/// Opaque revision token retained by the adoption-only directory authority.
#[pyclass(name = "PyAdoptionRevision", frozen)]
#[derive(Clone)]
struct PyAdoptionRevision {
    inner: LegacyMetadataRevision,
}

/// One no-follow entry captured by the adoption-only directory authority.
#[pyclass(name = "PyAdoptionDirectoryEntry", frozen)]
#[derive(Clone)]
struct PyAdoptionDirectoryEntry {
    inner: LegacyDirectoryEntry,
}

#[pymethods]
impl PyAdoptionDirectoryEntry {
    #[getter]
    fn name(&self) -> PyResult<String> {
        self.inner
            .name()
            .to_str()
            .map(str::to_owned)
            .ok_or_else(|| py_value_error("adoption authority entry name is not valid UTF-8"))
    }

    fn name_bytes<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt as _;
            PyBytes::new(py, self.inner.name().as_bytes())
        }
        #[cfg(not(unix))]
        {
            PyBytes::new(py, self.inner.name().to_string_lossy().as_bytes())
        }
    }

    fn is_file(&self) -> bool {
        self.inner.is_file()
    }

    fn is_directory(&self) -> bool {
        self.inner.is_directory()
    }

    fn is_symlink(&self) -> bool {
        self.inner.is_symlink()
    }

    fn same_identity(&self, other: &PyAdoptionDirectoryEntry) -> bool {
        self.inner == other.inner
    }
}

/// Cross-platform retained authority used only by the legacy converter.
#[pyclass(name = "PyAdoptionDirectoryAuthority")]
struct PyAdoptionDirectoryAuthority {
    inner: LegacyDirectoryAuthority,
}

#[pymethods]
impl PyAdoptionDirectoryAuthority {
    #[staticmethod]
    fn open(path: &str) -> PyResult<Self> {
        LegacyDirectoryAuthority::open_root(Path::new(path))
            .map(|inner| Self { inner })
            .map_err(|error| py_value_error(error.to_string()))
    }

    fn directory_revision(&self) -> PyResult<PyAdoptionRevision> {
        self.inner
            .directory_revision()
            .map(|inner| PyAdoptionRevision { inner })
            .map_err(|error| py_value_error(error.to_string()))
    }

    fn require_directory_revision(&self, revision: &PyAdoptionRevision) -> PyResult<()> {
        self.inner
            .require_directory_revision(&revision.inner)
            .map_err(|error| py_value_error(error.to_string()))
    }

    #[pyo3(signature = (relative, maximum_entries, expected_directory = None))]
    fn entries(
        &self,
        relative: &str,
        maximum_entries: usize,
        expected_directory: Option<&PyAdoptionDirectoryEntry>,
    ) -> PyResult<Vec<PyAdoptionDirectoryEntry>> {
        self.inner
            .entries_relative(
                Path::new(relative),
                maximum_entries,
                expected_directory.map(|entry| &entry.inner),
            )
            .map(|entries| {
                entries
                    .into_iter()
                    .map(|inner| PyAdoptionDirectoryEntry { inner })
                    .collect()
            })
            .map_err(|error| py_value_error(error.to_string()))
    }

    #[pyo3(signature = (relative, expected_parent = None))]
    fn inspect(
        &self,
        relative: &str,
        expected_parent: Option<&PyAdoptionDirectoryEntry>,
    ) -> PyResult<Option<PyAdoptionDirectoryEntry>> {
        self.inner
            .inspect_relative(
                Path::new(relative),
                expected_parent.map(|entry| &entry.inner),
            )
            .map(|entry| entry.map(|inner| PyAdoptionDirectoryEntry { inner }))
            .map_err(|error| py_value_error(error.to_string()))
    }

    #[pyo3(signature = (relative, limit, expected = None))]
    fn read_bounded<'py>(
        &self,
        py: Python<'py>,
        relative: &str,
        limit: usize,
        expected: Option<&PyAdoptionDirectoryEntry>,
    ) -> PyResult<Bound<'py, PyBytes>> {
        self.inner
            .read_relative_bounded(
                Path::new(relative),
                limit,
                expected.map(|entry| &entry.inner),
            )
            .map(|bytes| PyBytes::new(py, &bytes))
            .map_err(|error| py_value_error(error.to_string()))
    }

    fn write_atomic_no_replace(&self, name: &str, contents: &[u8]) -> PyResult<()> {
        self.inner
            .write_atomic_no_replace(name, contents)
            .map_err(|error| py_value_error(error.to_string()))
    }

    fn validate_publication_name(&self, name: &str) -> PyResult<()> {
        self.inner
            .validate_publication_name(name)
            .map_err(|error| py_value_error(error.to_string()))
    }

    fn remove_if_matches(
        &self,
        name: &str,
        expected: &PyAdoptionDirectoryEntry,
        expected_bytes: &[u8],
    ) -> PyResult<bool> {
        self.inner
            .remove_if_matches(name, &expected.inner, expected_bytes)
            .map_err(|error| py_value_error(error.to_string()))
    }

    fn remove_owned_temporary_if_matches(
        &self,
        name: &str,
        target: &str,
        expected: &PyAdoptionDirectoryEntry,
        expected_bytes: &[u8],
    ) -> PyResult<bool> {
        self.inner
            .remove_owned_temporary_if_matches(name, target, &expected.inner, expected_bytes)
            .map_err(|error| py_value_error(error.to_string()))
    }
}

/// Normalize a serialized `MigrationSpec` dict through Rust serde.
#[pyfunction]
fn normalize_migration_spec(py: Python<'_>, spec: Bound<'_, PyAny>) -> PyResult<PyObject> {
    let spec: MigrationSpec = depythonize(&spec)
        .map_err(|error| py_value_error(format!("Invalid MigrationSpec: {error}")))?;
    pythonize(py, &spec)
        .map(|obj| obj.unbind())
        .map_err(|error| py_value_error(error.to_string()))
}

/// Normalize a serialized `MigrationGraph` dict through Rust serde.
#[pyfunction]
fn normalize_migration_graph(py: Python<'_>, graph: Bound<'_, PyAny>) -> PyResult<PyObject> {
    let graph: MigrationGraph = depythonize(&graph)
        .map_err(|error| py_value_error(format!("Invalid MigrationGraph: {error}")))?;
    pythonize(py, &graph)
        .map(|obj| obj.unbind())
        .map_err(|error| py_value_error(error.to_string()))
}

/// Serialize a serialized `MigrationSpec` dict to canonical JSON.
#[pyfunction]
fn migration_spec_to_json(spec: Bound<'_, PyAny>) -> PyResult<String> {
    let spec: MigrationSpec = depythonize(&spec)
        .map_err(|error| py_value_error(format!("Invalid MigrationSpec: {error}")))?;
    serde_json::to_string(&spec).map_err(|error| py_value_error(error.to_string()))
}

/// Deserialize a `MigrationSpec` JSON string and return its normalized dict.
#[pyfunction]
fn migration_spec_from_json(py: Python<'_>, json: &str) -> PyResult<PyObject> {
    let spec: MigrationSpec = serde_json::from_str(json)
        .map_err(|error| py_value_error(format!("Invalid MigrationSpec JSON: {error}")))?;
    pythonize(py, &spec)
        .map(|obj| obj.unbind())
        .map_err(|error| py_value_error(error.to_string()))
}

/// Load the JSON sidecar for a migration `.py` path and return it as a dict.
///
/// Derives the sidecar path by replacing the `.py` extension with `.json`.
/// Returns the deserialized [`MigrationSpec`] as a Python dict when a valid
/// sidecar exists, or `None` when no sidecar is present.  Raises
/// `ValueError` if the sidecar exists but cannot be read or deserialized.
///
/// Reuses the same serde JSON path as [`migration_spec_from_json`].
#[pyfunction]
fn load_migration_sidecar(py: Python<'_>, py_path: &str) -> PyResult<Option<PyObject>> {
    match load_sidecar(Path::new(py_path)) {
        Ok(None) => Ok(None),
        Ok(Some(spec)) => {
            let obj = pythonize(py, &spec)
                .map(|o| o.unbind())
                .map_err(|err| py_value_error(err.to_string()))?;
            Ok(Some(obj))
        }
        Err(err) => Err(py_value_error(err.to_string())),
    }
}

/// Serialize a serialized `MigrationGraph` dict to canonical JSON.
#[pyfunction]
fn migration_graph_to_json(graph: Bound<'_, PyAny>) -> PyResult<String> {
    let graph: MigrationGraph = depythonize(&graph)
        .map_err(|error| py_value_error(format!("Invalid MigrationGraph: {error}")))?;
    serde_json::to_string(&graph).map_err(|error| py_value_error(error.to_string()))
}

/// Deserialize a `MigrationGraph` JSON string and return its normalized dict.
#[pyfunction]
fn migration_graph_from_json(py: Python<'_>, json: &str) -> PyResult<PyObject> {
    let graph: MigrationGraph = serde_json::from_str(json)
        .map_err(|error| py_value_error(format!("Invalid MigrationGraph JSON: {error}")))?;
    pythonize(py, &graph)
        .map(|obj| obj.unbind())
        .map_err(|error| py_value_error(error.to_string()))
}

/// Calculate the migration-file checksum used for drift detection.
#[pyfunction]
fn calculate_migration_file_checksum(content: &str) -> String {
    migration_file_checksum(content)
}

/// Return TypeBridge's canonical migration-state schema as a `SchemaInfo` dict.
#[pyfunction]
fn migration_state_schema(py: Python<'_>) -> PyResult<PyObject> {
    pythonize(py, rust_migration_state_schema())
        .map(|object| object.unbind())
        .map_err(|error| py_value_error(error.to_string()))
}

/// Return the canonical applied-migration entity label.
#[pyfunction]
fn applied_migration_entity_label() -> &'static str {
    rust_applied_migration_entity_label()
}

/// Return whether a kind/label pair belongs to the migration-state schema.
#[pyfunction]
fn is_migration_state_type(kind: &str, label: &str) -> PyResult<bool> {
    let kind = match kind {
        "entity" => MigrationStateSchemaKind::Entity,
        "relation" => MigrationStateSchemaKind::Relation,
        "attribute" => MigrationStateSchemaKind::Attribute,
        "role" => MigrationStateSchemaKind::Role,
        _ => {
            return Err(py_value_error(format!(
                "Invalid migration state schema kind {kind:?}; expected entity, relation, attribute, or role"
            )));
        }
    };
    Ok(rust_is_migration_state_type(kind, label))
}

/// Validate a serialized migration graph and return structured errors.
#[pyfunction]
#[pyo3(signature = (graph, applied_records = None))]
fn validate_migration_graph(
    py: Python<'_>,
    graph: Bound<'_, PyAny>,
    applied_records: Option<Bound<'_, PyAny>>,
) -> PyResult<PyObject> {
    let graph: MigrationGraph = depythonize(&graph)
        .map_err(|error| py_value_error(format!("Invalid MigrationGraph: {error}")))?;
    let applied_records = depythonize_applied_records(applied_records)?;
    let errors = validate_graph(&graph, &applied_records);
    pythonize(py, &errors)
        .map(|obj| obj.unbind())
        .map_err(|error| py_value_error(error.to_string()))
}

/// Fail if any applied migration checksum differs from the loaded graph.
#[pyfunction]
fn check_migration_drift(
    graph: Bound<'_, PyAny>,
    applied_records: Bound<'_, PyAny>,
) -> PyResult<()> {
    let graph: MigrationGraph = depythonize(&graph)
        .map_err(|error| py_value_error(format!("Invalid MigrationGraph: {error}")))?;
    let applied_records: Vec<AppliedMigrationRecord> = depythonize(&applied_records)
        .map_err(|error| py_value_error(format!("Invalid applied migration records: {error}")))?;
    check_checksum_drift(&graph, &applied_records)
        .map_err(|error| py_value_error(error.to_string()))
}

/// Plan a serialized migration graph and return the lowered execution plan.
#[pyfunction]
#[pyo3(signature = (graph, applied_records = None, target = None))]
fn plan_migration_graph(
    py: Python<'_>,
    graph: Bound<'_, PyAny>,
    applied_records: Option<Bound<'_, PyAny>>,
    target: Option<&str>,
) -> PyResult<PyObject> {
    let graph: MigrationGraph = depythonize(&graph)
        .map_err(|error| py_value_error(format!("Invalid MigrationGraph: {error}")))?;
    let applied_records = depythonize_applied_records(applied_records)?;
    let execution_plan = plan(&graph, &applied_records, target)
        .map_err(|error| py_value_error(error.to_string()))?;
    pythonize(py, &execution_plan)
        .map(|obj| obj.unbind())
        .map_err(|error| py_value_error(error.to_string()))
}

/// Rust-owned migration executor bound to a live `PyRustDatabase`.
///
/// Plans a validated migration graph into an ordered execution plan and runs
/// every schema transaction in Rust, on the SAME `Arc<Database>` and
/// `Arc<ProviderRuntimeOwner>` the rest of the Rust ORM path uses. No raw Python
/// transaction crosses this boundary — `apply` takes serde dicts only.
#[pyclass]
pub struct PyMigrationRunner {
    db: Arc<type_bridge_orm::Database>,
    runtime: Arc<ProviderRuntimeOwner>,
}

#[pymethods]
impl PyMigrationRunner {
    /// Build a runner from a `PyRustDatabase`, sharing its database and runtime.
    #[new]
    fn new(db: &PyRustDatabase) -> Self {
        let (db, runtime) = db.handles();
        Self { db, runtime }
    }

    /// Reject an already-adopted target before Python-hosted legacy code or an
    /// externally owned migration ledger can mutate state.
    fn require_legacy_writer_open(&self, py: Python<'_>) -> PyResult<()> {
        provider_block_on(
            py,
            self.runtime.as_ref(),
            require_legacy_writer_open(self.db.as_ref()),
        )
        // This is a provider-backed preflight, not migration-spec validation.
        // Preserve the released runtime-failure surface for connection,
        // transaction, and query errors (including the cutover rejection).
        .map_err(|error| py_runtime_error(error.to_string()))
    }

    /// Plan and execute migrations, returning one result dict per migration.
    ///
    /// `graph` is a serialized `MigrationGraph`; `applied_records` a list of
    /// serialized `AppliedMigrationRecord`s (or `None`); `target` an optional
    /// migration name to migrate to. Planning or drift errors raise
    /// `ValueError` (surfaced as `MigrationError` in Python).
    #[pyo3(signature = (graph, applied_records = None, target = None))]
    fn apply(
        &self,
        py: Python<'_>,
        graph: Bound<'_, PyAny>,
        applied_records: Option<Bound<'_, PyAny>>,
        target: Option<&str>,
    ) -> PyResult<PyObject> {
        let graph: MigrationGraph = depythonize(&graph)
            .map_err(|error| py_value_error(format!("Invalid MigrationGraph: {error}")))?;
        let applied = depythonize_applied_records(applied_records)?;

        let execution_plan =
            plan(&graph, &applied, target).map_err(|error| py_value_error(error.to_string()))?;

        let store = TypeDbStateStore::new(Arc::clone(&self.db));
        let checksums = migration_checksums(&graph);
        let executor = collect_executor_info();
        let results = provider_block_on(
            py,
            self.runtime.as_ref(),
            execute_plan_with_run_log(&self.db, &store, execution_plan, &checksums, &executor),
        )
        .map_err(|error| py_value_error(error.to_string()))?;

        pythonize(py, &results)
            .map(|obj| obj.unbind())
            .map_err(|error| py_value_error(error.to_string()))
    }

    /// Plan and execute migrations without reading or writing migration state.
    ///
    /// This path opens only the transactions required by the migration steps;
    /// it never constructs a [`TypeDbStateStore`] or bootstraps TypeBridge's
    /// state schema. Embedders with an external authoritative ledger use this
    /// method and persist applied state/run logs through that ledger.
    #[pyo3(signature = (graph, applied_records = None, target = None))]
    fn apply_state_free(
        &self,
        py: Python<'_>,
        graph: Bound<'_, PyAny>,
        applied_records: Option<Bound<'_, PyAny>>,
        target: Option<&str>,
    ) -> PyResult<PyObject> {
        let graph: MigrationGraph = depythonize(&graph)
            .map_err(|error| py_value_error(format!("Invalid MigrationGraph: {error}")))?;
        let applied = depythonize_applied_records(applied_records)?;

        let execution_plan =
            plan(&graph, &applied, target).map_err(|error| py_value_error(error.to_string()))?;
        let results = provider_block_on(
            py,
            self.runtime.as_ref(),
            execute_plan(&self.db, execution_plan),
        );

        pythonize(py, &results)
            .map(|obj| obj.unbind())
            .map_err(|error| py_value_error(error.to_string()))
    }
}

/// Construct a `PyMigrationRunner` bound to a `PyRustDatabase`.
#[pyfunction]
fn migration_runner(db: &PyRustDatabase) -> PyMigrationRunner {
    PyMigrationRunner::new(db)
}

/// Rust-owned migration applied-state manager bound to a live `PyRustDatabase`.
///
/// Wraps a [`TypeDbStateStore`] over the same database and provider-runtime
/// owners the rest of the Rust ORM path uses (mirroring
/// [`PyMigrationRunner`]). Every method drives the shared runtime through the
/// GIL-releasing provider helper; no raw Python transaction crosses this
/// boundary (invariant 4) — only serde dicts and scalar strings do.
#[pyclass]
pub struct PyMigrationStateManager {
    store: Arc<TypeDbStateStore>,
    runtime: Arc<ProviderRuntimeOwner>,
}

#[pymethods]
impl PyMigrationStateManager {
    /// Build a state manager from a `PyRustDatabase`, sharing its handles.
    #[new]
    fn new(db: &PyRustDatabase) -> Self {
        let (db, runtime) = db.handles();
        Self {
            store: Arc::new(TypeDbStateStore::new(db)),
            runtime,
        }
    }

    /// Ensure the `type_bridge_migration` schema exists (idempotent).
    fn ensure_schema(&self, py: Python<'_>) -> PyResult<()> {
        provider_block_on(py, self.runtime.as_ref(), self.store.ensure_schema())
            .map_err(|error| py_value_error(error.to_string()))
    }

    /// Load all applied migration records as a list of dicts.
    ///
    /// Each dict carries `app_label`, `name`, `checksum`, and `applied_at`
    /// (the last possibly `None`) — the serde shape of `AppliedMigrationRecord`.
    fn load_applied(&self, py: Python<'_>) -> PyResult<PyObject> {
        let records = provider_block_on(py, self.runtime.as_ref(), self.store.load_applied())
            .map_err(|error| py_value_error(error.to_string()))?;
        pythonize(py, &records)
            .map(|obj| obj.unbind())
            .map_err(|error| py_value_error(error.to_string()))
    }

    /// Load all migration run-log records as a list of dicts.
    fn load_runs(&self, py: Python<'_>) -> PyResult<PyObject> {
        let records = provider_block_on(py, self.runtime.as_ref(), self.store.load_runs())
            .map_err(|error| py_value_error(error.to_string()))?;
        pythonize(py, &records)
            .map(|obj| obj.unbind())
            .map_err(|error| py_value_error(error.to_string()))
    }

    /// Record a migration as applied.
    ///
    /// Accepts a serialized `AppliedMigrationRecord` dict. When `applied_at` is
    /// absent or `None`, Rust stamps the current UTC time in the
    /// Python-compatible `%Y-%m-%dT%H:%M:%S.%f` format.
    fn record_applied(&self, py: Python<'_>, record: Bound<'_, PyAny>) -> PyResult<()> {
        let record: AppliedMigrationRecord = depythonize(&record).map_err(|error| {
            py_value_error(format!("Invalid applied migration record: {error}"))
        })?;
        provider_block_on(py, self.runtime.as_ref(), self.store.record_applied(record))
            .map_err(|error| py_value_error(error.to_string()))
    }

    /// Remove the applied record identified by `(app_label, name)`.
    fn record_unapplied(&self, py: Python<'_>, app_label: &str, name: &str) -> PyResult<()> {
        let app_label = app_label.to_owned();
        let name = name.to_owned();
        provider_block_on(
            py,
            self.runtime.as_ref(),
            self.store.record_unapplied(&app_label, &name),
        )
        .map_err(|error| py_value_error(error.to_string()))
    }

    /// Insert or replace one migration run-log record.
    fn record_run(&self, py: Python<'_>, record: Bound<'_, PyAny>) -> PyResult<()> {
        let record: MigrationRunRecord = depythonize(&record)
            .map_err(|error| py_value_error(format!("Invalid migration run record: {error}")))?;
        provider_block_on(py, self.runtime.as_ref(), self.store.record_run(record))
            .map_err(|error| py_value_error(error.to_string()))
    }
}

/// Construct a `PyMigrationStateManager` bound to a `PyRustDatabase`.
#[pyfunction]
fn migration_state_manager(db: &PyRustDatabase) -> PyMigrationStateManager {
    PyMigrationStateManager::new(db)
}

fn depythonize_applied_records(
    applied_records: Option<Bound<'_, PyAny>>,
) -> PyResult<Vec<AppliedMigrationRecord>> {
    let Some(applied_records) = applied_records else {
        return Ok(Vec::new());
    };
    // The argument was supplied but may itself be Python `None` (an explicit
    // `apply(graph, None, ...)`), which is distinct from the parameter being
    // absent above; treat it as "no applied records" rather than letting
    // depythonize fail on a None payload.
    if applied_records.is_none() {
        return Ok(Vec::new());
    }
    depythonize(&applied_records)
        .map_err(|error| py_value_error(format!("Invalid applied migration records: {error}")))
}

fn migration_checksums(graph: &MigrationGraph) -> BTreeMap<(String, String), String> {
    graph
        .migrations
        .iter()
        .map(|migration| {
            (
                (migration.app_label.clone(), migration.name.clone()),
                migration.checksum.clone().unwrap_or_default(),
            )
        })
        .collect()
}

fn py_value_error(message: impl Into<String>) -> PyErr {
    pyo3::exceptions::PyValueError::new_err(message.into())
}

fn py_runtime_error(message: impl Into<String>) -> PyErr {
    pyo3::exceptions::PyRuntimeError::new_err(message.into())
}

/// Register migration-runtime facade functions on the Python module.
pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(normalize_migration_spec, m)?)?;
    m.add_function(wrap_pyfunction!(normalize_migration_graph, m)?)?;
    m.add_function(wrap_pyfunction!(migration_spec_to_json, m)?)?;
    m.add_function(wrap_pyfunction!(migration_spec_from_json, m)?)?;
    m.add_function(wrap_pyfunction!(load_migration_sidecar, m)?)?;
    m.add_function(wrap_pyfunction!(migration_graph_to_json, m)?)?;
    m.add_function(wrap_pyfunction!(migration_graph_from_json, m)?)?;
    m.add_function(wrap_pyfunction!(calculate_migration_file_checksum, m)?)?;
    m.add_function(wrap_pyfunction!(migration_state_schema, m)?)?;
    m.add_function(wrap_pyfunction!(applied_migration_entity_label, m)?)?;
    m.add_function(wrap_pyfunction!(is_migration_state_type, m)?)?;
    m.add_function(wrap_pyfunction!(validate_migration_graph, m)?)?;
    m.add_function(wrap_pyfunction!(check_migration_drift, m)?)?;
    m.add_function(wrap_pyfunction!(plan_migration_graph, m)?)?;
    m.add_function(wrap_pyfunction!(migration_runner, m)?)?;
    m.add_function(wrap_pyfunction!(migration_state_manager, m)?)?;
    m.add_class::<PyMigrationRunner>()?;
    m.add_class::<PyMigrationStateManager>()?;
    m.add_class::<PyAdoptionRevision>()?;
    m.add_class::<PyAdoptionDirectoryEntry>()?;
    m.add_class::<PyAdoptionDirectoryAuthority>()?;
    Ok(())
}
