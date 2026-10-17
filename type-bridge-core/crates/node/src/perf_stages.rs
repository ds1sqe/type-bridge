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

/// `_perfStages()`: install the stage layer on first use and return the
/// exclusive nanoseconds per stage since the last call.
///
/// Private to the perf drivers; shipped packages never enable the feature,
/// so the export and its declaration exist only in perf builds.
#[cfg(feature = "perf-stages")]
#[napi_derive::napi(js_name = "_perfStages")]
pub fn perf_stages() -> napi::Result<std::collections::HashMap<String, i64>> {
    let times = type_bridge_perf::stages::recorder()
        .map_err(|error| napi::Error::new(napi::Status::GenericFailure, error.to_string()))?;
    Ok(times
        .take()
        .into_iter()
        .map(|(stage, nanos)| (stage.to_owned(), i64::try_from(nanos).unwrap_or(i64::MAX)))
        .collect())
}
