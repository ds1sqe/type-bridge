use pyo3::prelude::*;
#[pyclass]
pub struct Test { pub v: PyObject }
impl Clone for Test {
    fn clone(&self) -> Self {
        Test { v: self.v.clone() }
    }
}
