//! Nominal Python IID values backed by the shared validated identifier.

use std::hash::{Hash, Hasher};

use pyo3::prelude::*;

/// A validated provider Thing IID; plain strings are not IID values.
#[pyclass(name = "Iid", module = "type_bridge", frozen)]
#[derive(Clone)]
pub struct PyIid {
    pub(crate) inner: type_bridge_contract::id::Iid,
}

#[pymethods]
impl PyIid {
    #[new]
    fn new(value: String) -> PyResult<Self> {
        Ok(Self {
            inner: type_bridge_contract::id::Iid::new(value)
                .map_err(|error| crate::plan_runtime::value_error(&error))?,
        })
    }

    fn __str__(&self) -> &str {
        self.inner.as_str()
    }

    fn __repr__(&self) -> String {
        format!("Iid({:?})", self.inner.as_str())
    }

    fn __eq__(&self, other: &Bound<'_, PyAny>) -> bool {
        other
            .extract::<PyRef<'_, Self>>()
            .is_ok_and(|other| self.inner == other.inner)
    }

    fn __hash__(&self) -> u64 {
        let mut state = std::collections::hash_map::DefaultHasher::new();
        self.inner.hash(&mut state);
        state.finish()
    }
}
