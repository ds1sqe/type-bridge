//! The `tb.sdk.entry` stage span and, with the `perf-stages` feature, the
//! private stage recorder the perf drivers read.
//!
//! The span opens at each binding entry that runs a pipeline operation
//! (plan build, stage, commit, answer decode, migration execute), so its
//! exclusive time is the binding's own marshalling. It carries no fields.
//! Without a subscriber it costs one disabled-callsite check.

/// The span over one binding entry.
pub(crate) fn entry() -> tracing::Span {
    tracing::info_span!("tb.sdk.entry")
}

/// `type_bridge_core._perf_stages()`: install the stage layer on first use
/// and return the exclusive nanoseconds per stage since the last call.
///
/// Private to the perf drivers; shipped wheels never enable the feature.
#[cfg(feature = "perf-stages")]
#[pyo3::pyfunction]
fn _perf_stages() -> pyo3::PyResult<std::collections::BTreeMap<&'static str, u64>> {
    type_bridge_perf_stages::recorder()
        .map(type_bridge_perf_stages::StageTimes::take)
        .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))
}

pub(crate) fn register(module: &pyo3::Bound<'_, pyo3::types::PyModule>) -> pyo3::PyResult<()> {
    #[cfg(feature = "perf-stages")]
    {
        use pyo3::types::PyModuleMethods as _;
        module.add_function(pyo3::wrap_pyfunction!(_perf_stages, module)?)?;
    }
    let _ = module;
    Ok(())
}
