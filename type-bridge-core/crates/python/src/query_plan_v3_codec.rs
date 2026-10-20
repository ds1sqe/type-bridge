//! Primitive-only bridge to the shared V3 plan codec.
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use type_bridge_contract::query_plan::v3::decode_query_plan_v3;
use type_bridge_contract::schema::decode_declared_schema;

/// Validate canonical V3 bytes under a declared schema and return bytes and digest.
#[pyfunction]
fn _query_plan_v3_round_trip<'py>(
    py: Python<'py>,
    plan_bytes: &[u8],
    schema_bytes: &[u8],
) -> PyResult<(Bound<'py, PyBytes>, String)> {
    let convert = |error| crate::query_v2_runtime::value_error(&error);
    let schema = decode_declared_schema(schema_bytes).map_err(convert)?;
    let plan = decode_query_plan_v3(plan_bytes, &schema).map_err(convert)?;
    let bytes = plan.canonical_bytes().map_err(convert)?;
    let fingerprint = plan.fingerprint().map_err(convert)?;
    Ok((
        PyBytes::new(py, &bytes),
        fingerprint.as_fingerprint().digest().to_hex(),
    ))
}

pub(crate) fn register(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_function(wrap_pyfunction!(_query_plan_v3_round_trip, module)?)
}
