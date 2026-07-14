//! PyO3 wrappers for the offline migration authoring core (#166).
//!
//! `author_migration` accepts serialized `SchemaInfo` dictionaries and
//! returns the complete in-memory artifact set; `PyAuthoredMigration`
//! carries canonical artifacts, composes namespaced extensions, and publishes
//! the per-version commit manifest through the Rust no-clobber writer.
//! No database connection, model registry, or generated application package
//! is involved at any point.

use std::path::PathBuf;

use pyo3::prelude::*;
use pyo3::types::PyBytes;
use pythonize::{depythonize, pythonize};
use type_bridge_migration::OperationSpec;
use type_bridge_migration::author::{
    AuthorMigrationRequest, AuthoredMigration, ComposedMigration, DeclaredMigrationIntentInput,
    ExistingArtifactPolicy, MigrationMetadata, PositionedOperations, SnapshotContext,
    author_migration as rust_author_migration, publish_composed_migration,
    write_authored_migration,
};
use type_bridge_orm::schema::info::SchemaInfo;

fn py_value_error(message: String) -> PyErr {
    pyo3::exceptions::PyValueError::new_err(message)
}

/// One complete authored migration artifact set.
#[pyclass(name = "AuthoredMigration", module = "type_bridge_core")]
pub struct PyAuthoredMigration {
    inner: AuthoredMigration,
    extensions: Vec<(String, String, Vec<u8>, bool)>,
}

#[pymethods]
impl PyAuthoredMigration {
    /// Full migration stem, e.g. `0003_add_assignment`.
    #[getter]
    fn migration_name(&self) -> &str {
        &self.inner.migration_name
    }

    /// The reviewable `.py` source text.
    #[getter]
    fn python_source(&self) -> &str {
        &self.inner.python_source
    }

    /// The canonical `MigrationSpec` as a normalized dict.
    #[getter]
    fn spec(&self, py: Python<'_>) -> PyResult<PyObject> {
        pythonize(py, &self.inner.spec)
            .map(|obj| obj.unbind())
            .map_err(|error| py_value_error(error.to_string()))
    }

    /// Artifact files as `(relative_path, contents)` pairs.
    #[getter]
    fn files<'py>(&self, py: Python<'py>) -> Vec<(String, Bound<'py, PyBytes>)> {
        self.inner
            .files
            .iter()
            .map(|artifact| {
                (
                    artifact.relative_path.clone(),
                    PyBytes::new(py, &artifact.contents),
                )
            })
            .collect()
    }

    /// Add one namespaced extension before manifest computation.
    #[pyo3(signature = (namespace, relative_path, contents, critical = false))]
    fn add_extension(
        &mut self,
        namespace: String,
        relative_path: String,
        contents: Vec<u8>,
        critical: bool,
    ) -> PyResult<()> {
        let mut extensions = self.extensions.clone();
        extensions.push((namespace, relative_path, contents, critical));
        compose(&self.inner, &extensions).map_err(|error| py_value_error(error.to_string()))?;
        self.extensions = extensions;
        Ok(())
    }

    /// Complete deterministic publication files with the manifest last.
    #[getter]
    fn composed_files<'py>(&self, py: Python<'py>) -> PyResult<Vec<(String, Bound<'py, PyBytes>)>> {
        let composed = compose(&self.inner, &self.extensions)
            .map_err(|error| py_value_error(error.to_string()))?;
        Ok(composed
            .complete_files()
            .into_iter()
            .map(|artifact| (artifact.relative_path, PyBytes::new(py, &artifact.contents)))
            .collect())
    }

    /// Publish canonical files and extensions through the manifest-last
    /// no-clobber writer.
    #[pyo3(signature = (migrations_dir, on_existing = "validate_identical"))]
    fn publish_to(&self, migrations_dir: PathBuf, on_existing: &str) -> PyResult<()> {
        let policy = existing_policy(on_existing)?;
        let composed = compose(&self.inner, &self.extensions)
            .map_err(|error| py_value_error(error.to_string()))?;
        publish_composed_migration(&migrations_dir, &composed, policy)
            .map_err(|error| py_value_error(error.to_string()))
    }

    /// Write every artifact under `migrations_dir` through the validated
    /// writer. `on_existing` is `"validate_identical"` (default) or
    /// `"fail"`.
    #[pyo3(signature = (migrations_dir, on_existing = "validate_identical"))]
    fn write_to(&self, migrations_dir: PathBuf, on_existing: &str) -> PyResult<()> {
        let policy = existing_policy(on_existing)?;
        write_authored_migration(&migrations_dir, &self.inner, policy)
            .map_err(|error| py_value_error(error.to_string()))
    }
}

fn existing_policy(on_existing: &str) -> PyResult<ExistingArtifactPolicy> {
    match on_existing {
        "validate_identical" => Ok(ExistingArtifactPolicy::ValidateIdentical),
        "fail" => Ok(ExistingArtifactPolicy::Fail),
        other => Err(py_value_error(format!(
            "on_existing must be 'validate_identical' or 'fail', got {other:?}"
        ))),
    }
}

fn compose(
    authored: &AuthoredMigration,
    extensions: &[(String, String, Vec<u8>, bool)],
) -> type_bridge_migration::Result<ComposedMigration> {
    let mut composer = authored.composer();
    for (namespace, relative_path, contents, critical) in extensions {
        composer.add_extension(
            namespace.clone(),
            relative_path.clone(),
            contents.clone(),
            *critical,
        )?;
    }
    composer.compose()
}

fn operations_from(ops: Option<Bound<'_, PyAny>>, argument: &str) -> PyResult<Vec<OperationSpec>> {
    match ops {
        None => Ok(Vec::new()),
        Some(ops) => depythonize(&ops)
            .map_err(|error| py_value_error(format!("Invalid {argument} operation list: {error}"))),
    }
}

/// Author the complete migration artifact set from serialized schemas.
///
/// Returns `None` when the schema diff is empty and no explicit operations
/// were supplied.
#[pyfunction]
#[pyo3(signature = (
    base,
    target,
    *,
    app_label,
    name,
    dependencies,
    snapshot_version,
    generated_at,
    type_bridge_version,
    type_bridge_core_version,
    previous_snapshot_version = None,
    before_schema = None,
    after_schema = None,
    declared_intent = None,
    attribute_renames = None,
))]
#[allow(clippy::too_many_arguments)]
fn author_migration(
    base: Bound<'_, PyAny>,
    target: Bound<'_, PyAny>,
    app_label: String,
    name: String,
    dependencies: Vec<(String, String)>,
    snapshot_version: String,
    generated_at: String,
    type_bridge_version: String,
    type_bridge_core_version: String,
    previous_snapshot_version: Option<String>,
    before_schema: Option<Bound<'_, PyAny>>,
    after_schema: Option<Bound<'_, PyAny>>,
    declared_intent: Option<Vec<u8>>,
    attribute_renames: Option<Vec<(String, String)>>,
) -> PyResult<Option<PyAuthoredMigration>> {
    let base: SchemaInfo = depythonize(&base)
        .map_err(|error| py_value_error(format!("Invalid base SchemaInfo: {error}")))?;
    let target: SchemaInfo = depythonize(&target)
        .map_err(|error| py_value_error(format!("Invalid target SchemaInfo: {error}")))?;

    let request = AuthorMigrationRequest {
        base,
        target,
        metadata: MigrationMetadata {
            app_label,
            name,
            dependencies,
            generated_at,
            type_bridge_version,
            type_bridge_core_version,
        },
        snapshot: SnapshotContext {
            version: snapshot_version,
            previous_version: previous_snapshot_version,
        },
        extra_operations: PositionedOperations {
            before_schema: operations_from(before_schema, "before_schema")?,
            after_schema: operations_from(after_schema, "after_schema")?,
        },
        declared_intent: declared_intent.map(|contents| DeclaredMigrationIntentInput { contents }),
        attribute_renames: attribute_renames.unwrap_or_default(),
    };

    let authored =
        rust_author_migration(&request).map_err(|error| py_value_error(error.to_string()))?;
    Ok(authored.map(|inner| PyAuthoredMigration {
        inner,
        extensions: Vec::new(),
    }))
}

/// Register authoring functions and classes on the parent module.
pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyAuthoredMigration>()?;
    m.add_function(wrap_pyfunction!(author_migration, m)?)?;
    Ok(())
}
