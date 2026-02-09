use pyo3::prelude::*;
#[pyclass]
pub struct LiteralValue {
    pub value: PyObject,
}
impl Clone for LiteralValue {
    fn clone(&self) -> Self {
        LiteralValue { value: self.value.clone() }
    }
}
