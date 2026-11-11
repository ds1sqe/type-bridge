//! Python callback marshalling; transaction and migration semantics stay in Rust.
use pyo3::prelude::*;
use std::collections::BTreeMap;
use std::sync::Arc;
use type_bridge_contract::migration::MigrationStepId;
use type_bridge_orm::ProviderRuntimeOwner;
use type_bridge_schema_migration_typedb::MigrationUserSteps;

type Callback = Arc<Py<PyAny>>;

#[derive(Clone, Default)]
pub(super) struct UserSteps(BTreeMap<MigrationStepId, (Callback, Option<Callback>)>);
impl UserSteps {
    pub(super) fn register(
        &mut self,
        py: Python<'_>,
        id: &str,
        forward: Py<PyAny>,
        reverse: Option<Py<PyAny>>,
    ) -> PyResult<()> {
        let id = MigrationStepId::new(id).map_err(super::py_catalog_diagnostic)?;
        if self.0.contains_key(&id) {
            return Err(super::py_catalog_code(
                "migration_user_step_duplicate",
                String::new(),
            ));
        }
        if !forward.bind(py).is_callable()
            || reverse
                .as_ref()
                .is_some_and(|value| !value.bind(py).is_callable())
        {
            return Err(pyo3::exceptions::PyTypeError::new_err(
                "migration callbacks must be callable",
            ));
        }
        self.0
            .insert(id, (Arc::new(forward), reverse.map(Arc::new)));
        Ok(())
    }

    pub(super) fn bind(&self, runtime: &Arc<ProviderRuntimeOwner>) -> PyResult<MigrationUserSteps> {
        if self.0.is_empty() {
            return Ok(Default::default());
        }
        let bind = |callback: &Arc<Py<PyAny>>| {
            let callback = Arc::clone(callback);
            let runtime = Arc::clone(runtime);
            move |tx| {
                let callback = Arc::clone(&callback);
                let runtime = Arc::clone(&runtime);
                async move {
                    tokio::task::spawn_blocking(move || {
                        Python::attach(|py| -> PyResult<()> {
                            let native = crate::orm_runtime::PyTransactionContext::from_borrowed(
                                tx, runtime,
                            );
                            let wrapper = py
                                .import("type_bridge.migration")?
                                .getattr("MigrationTransaction")?
                                .call1((native,))?;
                            let result = callback.bind(py).call1((wrapper,))?;
                            if !result.is_none() {
                                return Err(pyo3::exceptions::PyTypeError::new_err(
                                    "synchronous migration callbacks must return None",
                                ));
                            }
                            Ok(())
                        })
                    })
                    .await
                    .map_err(failed)?
                    .map_err(failed)
                }
            }
        };
        let mut result = MigrationUserSteps::default();
        for (id, (forward, reverse)) in &self.0 {
            match reverse {
                Some(reverse) => {
                    result.register_with_reverse(id.clone(), bind(forward), bind(reverse))
                }
                None => result.register(id.clone(), bind(forward)),
            }
            .map_err(super::py_catalog_diagnostic)?;
        }
        Ok(result)
    }
}
fn failed(error: impl std::fmt::Display) -> type_bridge_contract::diagnostic::Diagnostic {
    let cause = error.to_string();
    use type_bridge_contract::diagnostic::*;
    Diagnostic::new(
        DiagnosticCategory::InvalidContract,
        DiagnosticCode::new("migration_user_step_failed").expect("static diagnostic"),
        format!("Python migration callback failed; its transaction will be rolled back: {cause}"),
    )
    .with_detail("cause", cause)
}
