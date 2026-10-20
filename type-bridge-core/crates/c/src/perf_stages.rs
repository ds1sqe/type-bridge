//! The `tb.sdk.entry` stage span and, with the `perf-stages` feature, the
//! private stage recorder the perf drivers read.
//!
//! The span opens at each C entry that runs a pipeline operation (plan
//! build, stage, commit, answer, migration execute), so its exclusive time
//! is the runtime's own marshalling. It carries no fields. Without a
//! subscriber it costs one disabled-callsite check.
//!
//! The recorder is not part of the C ABI: no header declares it, the ABI
//! contract and pkg-config do not list it, and shipped archives are built
//! without the feature, so the symbol does not exist there.

/// The span over one C entry.
pub(crate) fn entry() -> tracing::Span {
    tracing::info_span!("tb.sdk.entry")
}

/// Install the stage layer on first use and write the exclusive
/// nanoseconds per stage since the last call as one JSON object
/// (`{"tb.commit":123,...}`) into `buffer`, NUL-terminated.
///
/// Returns the JSON length without the NUL, like `snprintf`; when it is not
/// below `capacity` nothing is written, the totals are kept for the next
/// call, and the caller retries with a larger buffer. Returns `SIZE_MAX`
/// when another global subscriber prevents the recorder.
///
/// # Safety
///
/// `buffer` is null with `capacity` 0, or valid for `capacity` writes.
#[cfg(feature = "perf-stages")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tb_perf_stages_json(buffer: *mut u8, capacity: usize) -> usize {
    let Ok(times) = type_bridge_perf::stages::recorder() else {
        return usize::MAX;
    };
    let stages = times.take();
    let json = stages_json(&stages);
    if json.len() >= capacity || buffer.is_null() {
        times.restore(stages);
        return json.len();
    }
    // SAFETY: `buffer` holds `capacity > json.len()` bytes by contract.
    unsafe {
        std::ptr::copy_nonoverlapping(json.as_ptr(), buffer, json.len());
        buffer.add(json.len()).write(0);
    }
    json.len()
}

/// Stage names are fixed `tb.*` identifiers, so they need no escaping.
#[cfg(feature = "perf-stages")]
fn stages_json(stages: &std::collections::BTreeMap<&'static str, u64>) -> String {
    let fields = stages
        .iter()
        .map(|(stage, nanos)| format!("\"{stage}\":{nanos}"))
        .collect::<Vec<_>>();
    format!("{{{}}}", fields.join(","))
}

#[cfg(all(test, feature = "perf-stages"))]
mod tests {
    use super::*;

    fn read() -> String {
        // SAFETY: a null buffer with capacity 0 only measures.
        let needed = unsafe { tb_perf_stages_json(std::ptr::null_mut(), 0) };
        let mut buffer = vec![0_u8; needed + 1];
        // SAFETY: the buffer holds `needed + 1` bytes.
        let written = unsafe { tb_perf_stages_json(buffer.as_mut_ptr(), buffer.len()) };
        assert_eq!(written, needed);
        String::from_utf8(buffer[..written].to_vec()).unwrap()
    }

    #[test]
    fn the_recorder_reports_entry_and_core_stages_of_one_operation() {
        read();
        {
            let _entry = entry().entered();
            let _commit = tracing::info_span!("tb.commit").entered();
        }
        let json = read();
        assert!(json.starts_with("{\"tb.commit\":"), "{json}");
        assert!(json.contains(",\"tb.sdk.entry\":"), "{json}");
        assert_eq!(read(), "{}", "each read takes the totals");
    }
}
