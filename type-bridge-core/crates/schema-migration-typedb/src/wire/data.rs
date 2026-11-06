//! Closed canonical wire for atomic data-step events.
use super::*;
use type_bridge_contract::reserved::DATA_STEP_EVENT_RECORD_KIND;
use type_bridge_schema_migration::{
    DataStepDirection, DataStepEventRecord, MigrationLease, VerifiedSchemaMigrationManifest,
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Wire {
    format: String,
    kind: String,
    scope: String,
    fence: u64,
    manifest_digest: String,
    migration_id: Value,
    step_id: String,
    direction: String,
    event_kind: String,
}
fn direction(value: DataStepDirection) -> &'static str {
    match value {
        DataStepDirection::Forward => "forward",
        DataStepDirection::Reverse => "reverse",
    }
}
pub(crate) fn encode_data_event(record: &DataStepEventRecord) -> Result<Vec<u8>, Diagnostic> {
    let wire = Wire {
        format: EXECUTION_RECORD_V1.into(),
        kind: DATA_STEP_EVENT_RECORD_KIND.into(),
        scope: record.scope().managed_scope_id().as_str().into(),
        fence: record.fence().get(),
        manifest_digest: record.manifest_digest().to_hex(),
        migration_id: serde_json::to_value(record.migration_id())
            .expect("serializable migration identity"),
        step_id: record.step_id().as_str().into(),
        direction: direction(record.direction()).into(),
        event_kind: event_kind(record.kind()).into(),
    };
    to_canonical_json_with_limits(&wire, EXECUTION_RECORD_LIMITS)
}
pub(crate) fn decode_data_event<'a>(
    bytes: &[u8],
    lease: &MigrationLease,
    manifests: impl Iterator<Item = &'a VerifiedSchemaMigrationManifest>,
    expected_direction: DataStepDirection,
) -> Result<DataStepEventRecord, Diagnostic> {
    let wire: Wire = from_canonical_json_with_limits(bytes, EXECUTION_RECORD_LIMITS)?;
    let fence = persisted_fence(bytes, DATA_STEP_EVENT_RECORD_KIND)?;
    if fence > lease.fence() {
        return Err(mismatch());
    }
    let historical = MigrationLease::new(lease.scope().clone(), lease.holder().clone(), fence);
    for manifest in manifests {
        for step in manifest.steps().iter().filter_map(|step| step.as_data()) {
            if step.id().as_str() != wire.step_id {
                continue;
            }
            let expected = DataStepEventRecord::new(
                &historical,
                manifest,
                step,
                expected_direction,
                parse_event_kind(&wire.event_kind)?,
            )?;
            if encode_data_event(&expected)? == bytes {
                return Ok(expected);
            }
        }
    }
    Err(mismatch())
}
fn mismatch() -> Diagnostic {
    Diagnostic::new(
        DiagnosticCategory::Integrity,
        DiagnosticCode::new("migration_data_event_mismatch").expect("static code"),
        "stored data event differs from the verified plan",
    )
}
