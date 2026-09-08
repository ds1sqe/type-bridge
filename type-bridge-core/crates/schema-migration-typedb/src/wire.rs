//! Canonical private persistence wire for trusted execution records.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use type_bridge_contract::codec::{from_canonical_json_with_limits, to_canonical_json_with_limits};
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::fingerprint::Fingerprint;
use type_bridge_contract::limits::CodecLimits;
use type_bridge_schema_migration::{
    AppliedRecord, BackfillCompletionEvidence, BackfillEventRecord, BackfillExecutionCounts,
    BackfillExecutionDirection, ExecutionFence, GroupEventRecord, GroupJournalEventKind,
    PlanRecord, RollbackPlanRecord, RollbackStepEventRecord, RolledBackRecord,
};

const EXECUTION_RECORD_V1: &str = "typebridge.migration-execution-record/v1";
const EXECUTION_RECORD_LIMITS: CodecLimits = CodecLimits {
    max_bytes: 1024 * 1024,
    max_depth: 16,
    max_collection_len: 65_536,
    max_string_bytes: 1024 * 1024,
};

#[derive(Serialize)]
struct PlanView<'a> {
    fence: u64,
    format: &'static str,
    kind: &'static str,
    lowering_profile: &'a Fingerprint,
    manifest_digests: Vec<String>,
    manifest_plan_fingerprints: Vec<&'a Fingerprint>,
    migration_ids: &'a [type_bridge_contract::migration::MigrationId],
    observed_live_source: &'a Fingerprint,
    scope: &'a str,
    semantic_profile: &'a Fingerprint,
    source_applied: &'a [type_bridge_contract::migration::MigrationId],
    source_declared: &'a Fingerprint,
    source_frontier: &'a [type_bridge_contract::migration::MigrationId],
    source_semantics: &'a Fingerprint,
    target_declared: &'a Fingerprint,
    target_frontier: &'a [type_bridge_contract::migration::MigrationId],
    target_semantics: &'a Fingerprint,
}

#[derive(Serialize)]
struct EventView<'a> {
    end_step_index: u32,
    event_kind: &'static str,
    fence: u64,
    first_step_index: u32,
    format: &'static str,
    group_ordinal: u32,
    kind: &'static str,
    manifest_digest: String,
    migration_id: &'a type_bridge_contract::migration::MigrationId,
    observed_target: Option<&'a Fingerprint>,
    schema_delta_step_index: u32,
    scope: &'a str,
}

#[derive(Serialize)]
struct BackfillCompletionView<'a> {
    changed: u64,
    direction: &'static str,
    matched: u64,
    plan_fingerprint: &'a Fingerprint,
    skipped: u64,
    transaction_groups: u32,
}

#[derive(Serialize)]
struct BackfillEventView<'a> {
    completion: Option<BackfillCompletionView<'a>>,
    direction: &'static str,
    event_kind: &'static str,
    fence: u64,
    format: &'static str,
    kind: &'static str,
    manifest_digest: String,
    manifest_step_index: u32,
    migration_id: &'a type_bridge_contract::migration::MigrationId,
    operation_ordinal: u32,
    plan_fingerprint: &'a Fingerprint,
    scope: &'a str,
}

#[derive(Serialize)]
struct AppliedView<'a> {
    fence: u64,
    format: &'static str,
    kind: &'static str,
    manifest_digest: String,
    migration_id: &'a type_bridge_contract::migration::MigrationId,
    scope: &'a str,
    source_declared: &'a Fingerprint,
    source_semantics: &'a Fingerprint,
    target_declared: &'a Fingerprint,
    target_semantics: &'a Fingerprint,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PlanWire {
    fence: u64,
    format: String,
    kind: String,
    lowering_profile: Value,
    manifest_digests: Value,
    manifest_plan_fingerprints: Value,
    migration_ids: Value,
    observed_live_source: Value,
    scope: String,
    semantic_profile: Value,
    source_applied: Value,
    source_declared: Value,
    source_frontier: Value,
    source_semantics: Value,
    target_declared: Value,
    target_frontier: Value,
    target_semantics: Value,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct EventWire {
    end_step_index: u32,
    event_kind: String,
    fence: u64,
    first_step_index: u32,
    format: String,
    group_ordinal: u32,
    kind: String,
    manifest_digest: String,
    migration_id: Value,
    observed_target: Option<Value>,
    schema_delta_step_index: u32,
    scope: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct BackfillEventWire {
    completion: Option<BackfillCompletionWire>,
    direction: String,
    event_kind: String,
    fence: u64,
    format: String,
    kind: String,
    manifest_digest: String,
    manifest_step_index: u32,
    migration_id: Value,
    operation_ordinal: u32,
    plan_fingerprint: Value,
    scope: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct BackfillCompletionWire {
    changed: u64,
    direction: String,
    matched: u64,
    plan_fingerprint: Value,
    skipped: u64,
    transaction_groups: u32,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AppliedWire {
    fence: u64,
    format: String,
    kind: String,
    manifest_digest: String,
    migration_id: Value,
    scope: String,
    source_declared: Value,
    source_semantics: Value,
    target_declared: Value,
    target_semantics: Value,
}

pub(crate) fn encode_plan(record: &PlanRecord) -> Result<Vec<u8>, Diagnostic> {
    let view = PlanView {
        fence: record.fence().get(),
        format: EXECUTION_RECORD_V1,
        kind: "plan",
        lowering_profile: record.lowering_profile().as_fingerprint(),
        manifest_digests: record
            .manifest_digests()
            .iter()
            .map(|digest| digest.to_hex())
            .collect(),
        manifest_plan_fingerprints: record
            .manifest_plan_fingerprints()
            .iter()
            .map(|fingerprint| fingerprint.as_fingerprint())
            .collect(),
        migration_ids: record.migration_ids(),
        observed_live_source: record.observed_live_source().as_fingerprint(),
        scope: record.scope().managed_scope_id().as_str(),
        semantic_profile: record.semantic_profile().as_fingerprint(),
        source_applied: record.source_applied(),
        source_declared: record.source_declared().as_fingerprint(),
        source_frontier: record.source_frontier(),
        source_semantics: record.source_semantics().as_fingerprint(),
        target_declared: record.target_declared().as_fingerprint(),
        target_frontier: record.target_frontier(),
        target_semantics: record.target_semantics().as_fingerprint(),
    };
    to_canonical_json_with_limits(&view, EXECUTION_RECORD_LIMITS)
}

pub(crate) fn decode_plan(bytes: &[u8], expected: PlanRecord) -> Result<PlanRecord, Diagnostic> {
    let wire: PlanWire = from_canonical_json_with_limits(bytes, EXECUTION_RECORD_LIMITS)?;
    ensure_header(
        &wire.format,
        &wire.kind,
        &wire.scope,
        wire.fence,
        "plan",
        expected.scope().managed_scope_id().as_str(),
        expected.fence(),
    )?;
    ensure_expected_bytes(bytes, &encode_plan(&expected)?)?;
    Ok(expected)
}

pub(crate) fn encode_event(record: &GroupEventRecord) -> Result<Vec<u8>, Diagnostic> {
    let view = EventView {
        end_step_index: record.end_step_index(),
        event_kind: event_kind(record.kind()),
        fence: record.fence().get(),
        first_step_index: record.first_step_index(),
        format: EXECUTION_RECORD_V1,
        group_ordinal: record.group_ordinal(),
        kind: "event",
        manifest_digest: record.manifest_digest().to_hex(),
        migration_id: record.migration_id(),
        observed_target: record
            .observed_target()
            .map(|fingerprint| fingerprint.as_fingerprint()),
        schema_delta_step_index: record.schema_delta_step_index(),
        scope: record.scope().managed_scope_id().as_str(),
    };
    to_canonical_json_with_limits(&view, EXECUTION_RECORD_LIMITS)
}

pub(crate) fn decode_event(
    bytes: &[u8],
    expected: GroupEventRecord,
) -> Result<GroupEventRecord, Diagnostic> {
    let wire: EventWire = from_canonical_json_with_limits(bytes, EXECUTION_RECORD_LIMITS)?;
    ensure_header(
        &wire.format,
        &wire.kind,
        &wire.scope,
        wire.fence,
        "event",
        expected.scope().managed_scope_id().as_str(),
        expected.fence(),
    )?;
    ensure_expected_bytes(bytes, &encode_event(&expected)?)?;
    Ok(expected)
}

pub(crate) fn encode_backfill_event(record: &BackfillEventRecord) -> Result<Vec<u8>, Diagnostic> {
    let completion = record.completion().map(|evidence| {
        let counts = evidence.counts();
        BackfillCompletionView {
            changed: counts.changed(),
            direction: backfill_direction(evidence.direction()),
            matched: counts.matched(),
            plan_fingerprint: evidence.plan_fingerprint(),
            skipped: counts.skipped(),
            transaction_groups: counts.transaction_groups(),
        }
    });
    let view = BackfillEventView {
        completion,
        direction: backfill_direction(record.direction()),
        event_kind: event_kind(record.kind()),
        fence: record.fence().get(),
        format: EXECUTION_RECORD_V1,
        kind: "backfill-event",
        manifest_digest: record.manifest_digest().to_hex(),
        manifest_step_index: record.manifest_step_index(),
        migration_id: record.migration_id(),
        operation_ordinal: record.operation_ordinal(),
        plan_fingerprint: record.plan_fingerprint(),
        scope: record.scope().managed_scope_id().as_str(),
    };
    to_canonical_json_with_limits(&view, EXECUTION_RECORD_LIMITS)
}

pub(crate) fn decode_backfill_event(
    bytes: &[u8],
    expected: BackfillEventRecord,
) -> Result<BackfillEventRecord, Diagnostic> {
    let wire: BackfillEventWire = from_canonical_json_with_limits(bytes, EXECUTION_RECORD_LIMITS)?;
    ensure_header(
        &wire.format,
        &wire.kind,
        &wire.scope,
        wire.fence,
        "backfill-event",
        expected.scope().managed_scope_id().as_str(),
        expected.fence(),
    )?;
    ensure_expected_bytes(bytes, &encode_backfill_event(&expected)?)?;
    Ok(expected)
}

pub(crate) fn decode_backfill_event_evidence(
    bytes: &[u8],
    expected_plan: &Fingerprint,
    expected_direction: BackfillExecutionDirection,
) -> Result<(GroupJournalEventKind, Option<BackfillCompletionEvidence>), Diagnostic> {
    let wire: BackfillEventWire = from_canonical_json_with_limits(bytes, EXECUTION_RECORD_LIMITS)?;
    let kind = parse_event_kind(&wire.event_kind)?;
    let completion = wire
        .completion
        .map(|completion| {
            if completion.direction != backfill_direction(expected_direction) {
                return Err(identity_mismatch());
            }
            let expected_value =
                serde_json::to_value(expected_plan).map_err(|_| identity_mismatch())?;
            if completion.plan_fingerprint != expected_value {
                return Err(identity_mismatch());
            }
            Ok(BackfillCompletionEvidence::new(
                expected_plan.clone(),
                expected_direction,
                BackfillExecutionCounts::new(
                    completion.matched,
                    completion.changed,
                    completion.skipped,
                    completion.transaction_groups,
                )?,
            ))
        })
        .transpose()?;
    Ok((kind, completion))
}

pub(crate) fn encode_applied(record: &AppliedRecord) -> Result<Vec<u8>, Diagnostic> {
    let view = AppliedView {
        fence: record.fence().get(),
        format: EXECUTION_RECORD_V1,
        kind: "applied",
        manifest_digest: record.manifest_digest().to_hex(),
        migration_id: record.migration_id(),
        scope: record.scope().managed_scope_id().as_str(),
        source_declared: record.source_declared().as_fingerprint(),
        source_semantics: record.source_semantics().as_fingerprint(),
        target_declared: record.target_declared().as_fingerprint(),
        target_semantics: record.target_semantics().as_fingerprint(),
    };
    to_canonical_json_with_limits(&view, EXECUTION_RECORD_LIMITS)
}

pub(crate) fn decode_applied(
    bytes: &[u8],
    expected: AppliedRecord,
) -> Result<AppliedRecord, Diagnostic> {
    let wire: AppliedWire = from_canonical_json_with_limits(bytes, EXECUTION_RECORD_LIMITS)?;
    ensure_header(
        &wire.format,
        &wire.kind,
        &wire.scope,
        wire.fence,
        "applied",
        expected.scope().managed_scope_id().as_str(),
        expected.fence(),
    )?;
    ensure_expected_bytes(bytes, &encode_applied(&expected)?)?;
    Ok(expected)
}

#[derive(Serialize)]
struct RollbackPlanView<'a> {
    fence: u64,
    format: &'static str,
    kind: &'static str,
    lowering_profile: &'a Fingerprint,
    manifest_digests: Vec<String>,
    manifest_plan_fingerprints: Vec<&'a Fingerprint>,
    observed_live_source: &'a Fingerprint,
    remaining_applied: &'a [type_bridge_contract::migration::MigrationId],
    rollback_ids: &'a [type_bridge_contract::migration::MigrationId],
    scope: &'a str,
    semantic_profile: &'a Fingerprint,
    source_applied: &'a [type_bridge_contract::migration::MigrationId],
    source_declared: &'a Fingerprint,
    source_semantics: &'a Fingerprint,
    target_declared: &'a Fingerprint,
    target_semantics: &'a Fingerprint,
}

#[derive(Serialize)]
struct RollbackEventView<'a> {
    event_kind: &'static str,
    fence: u64,
    format: &'static str,
    kind: &'static str,
    manifest_digest: String,
    migration_id: &'a type_bridge_contract::migration::MigrationId,
    observed_target: Option<&'a Fingerprint>,
    scope: &'a str,
    step_ordinal: u32,
}

#[derive(Serialize)]
struct RolledBackView<'a> {
    fence: u64,
    format: &'static str,
    kind: &'static str,
    manifest_digest: String,
    migration_id: &'a type_bridge_contract::migration::MigrationId,
    scope: &'a str,
    source_declared: &'a Fingerprint,
    source_semantics: &'a Fingerprint,
    target_declared: &'a Fingerprint,
    target_semantics: &'a Fingerprint,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RollbackPlanWire {
    fence: u64,
    format: String,
    kind: String,
    lowering_profile: Value,
    manifest_digests: Value,
    manifest_plan_fingerprints: Value,
    observed_live_source: Value,
    remaining_applied: Value,
    rollback_ids: Value,
    scope: String,
    semantic_profile: Value,
    source_applied: Value,
    source_declared: Value,
    source_semantics: Value,
    target_declared: Value,
    target_semantics: Value,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RollbackEventWire {
    event_kind: String,
    fence: u64,
    format: String,
    kind: String,
    manifest_digest: String,
    migration_id: Value,
    observed_target: Option<Value>,
    scope: String,
    step_ordinal: u32,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RolledBackWire {
    fence: u64,
    format: String,
    kind: String,
    manifest_digest: String,
    migration_id: Value,
    scope: String,
    source_declared: Value,
    source_semantics: Value,
    target_declared: Value,
    target_semantics: Value,
}

pub(crate) fn encode_rollback_plan(record: &RollbackPlanRecord) -> Result<Vec<u8>, Diagnostic> {
    let view = RollbackPlanView {
        fence: record.fence().get(),
        format: EXECUTION_RECORD_V1,
        kind: "rollback-plan",
        lowering_profile: record.lowering_profile().as_fingerprint(),
        manifest_digests: record
            .manifest_digests()
            .iter()
            .map(|digest| digest.to_hex())
            .collect(),
        manifest_plan_fingerprints: record
            .manifest_plan_fingerprints()
            .iter()
            .map(|fingerprint| fingerprint.as_fingerprint())
            .collect(),
        observed_live_source: record.observed_live_source().as_fingerprint(),
        remaining_applied: record.remaining_applied(),
        rollback_ids: record.rollback_ids(),
        scope: record.scope().managed_scope_id().as_str(),
        semantic_profile: record.semantic_profile().as_fingerprint(),
        source_applied: record.source_applied(),
        source_declared: record.source_declared().as_fingerprint(),
        source_semantics: record.source_semantics().as_fingerprint(),
        target_declared: record.target_declared().as_fingerprint(),
        target_semantics: record.target_semantics().as_fingerprint(),
    };
    to_canonical_json_with_limits(&view, EXECUTION_RECORD_LIMITS)
}

pub(crate) fn decode_rollback_plan(
    bytes: &[u8],
    expected: RollbackPlanRecord,
) -> Result<RollbackPlanRecord, Diagnostic> {
    let wire: RollbackPlanWire = from_canonical_json_with_limits(bytes, EXECUTION_RECORD_LIMITS)?;
    ensure_header(
        &wire.format,
        &wire.kind,
        &wire.scope,
        wire.fence,
        "rollback-plan",
        expected.scope().managed_scope_id().as_str(),
        expected.fence(),
    )?;
    ensure_expected_bytes(bytes, &encode_rollback_plan(&expected)?)?;
    Ok(expected)
}

pub(crate) fn encode_rollback_event(
    record: &RollbackStepEventRecord,
) -> Result<Vec<u8>, Diagnostic> {
    let view = RollbackEventView {
        event_kind: event_kind(record.kind()),
        fence: record.fence().get(),
        format: EXECUTION_RECORD_V1,
        kind: "rollback-event",
        manifest_digest: record.manifest_digest().to_hex(),
        migration_id: record.migration_id(),
        observed_target: record
            .observed_target()
            .map(|fingerprint| fingerprint.as_fingerprint()),
        scope: record.scope().managed_scope_id().as_str(),
        step_ordinal: record.step_ordinal(),
    };
    to_canonical_json_with_limits(&view, EXECUTION_RECORD_LIMITS)
}

pub(crate) fn decode_rollback_event(
    bytes: &[u8],
    expected: RollbackStepEventRecord,
) -> Result<RollbackStepEventRecord, Diagnostic> {
    let wire: RollbackEventWire = from_canonical_json_with_limits(bytes, EXECUTION_RECORD_LIMITS)?;
    ensure_header(
        &wire.format,
        &wire.kind,
        &wire.scope,
        wire.fence,
        "rollback-event",
        expected.scope().managed_scope_id().as_str(),
        expected.fence(),
    )?;
    ensure_expected_bytes(bytes, &encode_rollback_event(&expected)?)?;
    Ok(expected)
}

pub(crate) fn encode_rolled_back(record: &RolledBackRecord) -> Result<Vec<u8>, Diagnostic> {
    let view = RolledBackView {
        fence: record.fence().get(),
        format: EXECUTION_RECORD_V1,
        kind: "rolled-back",
        manifest_digest: record.manifest_digest().to_hex(),
        migration_id: record.migration_id(),
        scope: record.scope().managed_scope_id().as_str(),
        source_declared: record.source_declared().as_fingerprint(),
        source_semantics: record.source_semantics().as_fingerprint(),
        target_declared: record.target_declared().as_fingerprint(),
        target_semantics: record.target_semantics().as_fingerprint(),
    };
    to_canonical_json_with_limits(&view, EXECUTION_RECORD_LIMITS)
}

pub(crate) fn decode_rolled_back(
    bytes: &[u8],
    expected: RolledBackRecord,
) -> Result<RolledBackRecord, Diagnostic> {
    let wire: RolledBackWire = from_canonical_json_with_limits(bytes, EXECUTION_RECORD_LIMITS)?;
    ensure_header(
        &wire.format,
        &wire.kind,
        &wire.scope,
        wire.fence,
        "rolled-back",
        expected.scope().managed_scope_id().as_str(),
        expected.fence(),
    )?;
    ensure_expected_bytes(bytes, &encode_rolled_back(&expected)?)?;
    Ok(expected)
}

pub(crate) fn persisted_fence(
    bytes: &[u8],
    expected_kind: &'static str,
) -> Result<ExecutionFence, Diagnostic> {
    let value: Value = from_canonical_json_with_limits(bytes, EXECUTION_RECORD_LIMITS)?;
    let object = value.as_object().ok_or_else(|| {
        failure(
            DiagnosticCategory::InvalidContract,
            "migration_typedb_record_header_invalid",
            "persisted migration record has no canonical object header",
        )
    })?;
    let format = object
        .get("format")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            failure(
                DiagnosticCategory::InvalidContract,
                "migration_typedb_record_header_invalid",
                "persisted migration record has no canonical format discriminator",
            )
        })?;
    if format != EXECUTION_RECORD_V1 {
        return Err(failure(
            DiagnosticCategory::InvalidContract,
            "migration_typedb_record_version_unsupported",
            "persisted migration record format is unsupported",
        ));
    }
    if object.get("kind").and_then(Value::as_str) != Some(expected_kind) {
        return Err(failure(
            DiagnosticCategory::InvalidContract,
            "migration_typedb_record_kind_unknown",
            "persisted migration record kind does not match its journal row",
        ));
    }
    let fence = value
        .as_object()
        .and_then(|object| object.get("fence"))
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            failure(
                DiagnosticCategory::InvalidContract,
                "migration_typedb_record_fence_invalid",
                "persisted migration record has no canonical fence",
            )
        })?;
    ExecutionFence::new(fence)
}

fn event_kind(kind: GroupJournalEventKind) -> &'static str {
    match kind {
        GroupJournalEventKind::BeforeCommit => "before_commit",
        GroupJournalEventKind::Committed => "committed",
        GroupJournalEventKind::CommitOutcomeUnknown => "commit_outcome_unknown",
        GroupJournalEventKind::DefinitelyAborted => "definitely_aborted",
        GroupJournalEventKind::FormalOnlyAdvanced => "formal_only_advanced",
    }
}

fn parse_event_kind(value: &str) -> Result<GroupJournalEventKind, Diagnostic> {
    match value {
        "before_commit" => Ok(GroupJournalEventKind::BeforeCommit),
        "committed" => Ok(GroupJournalEventKind::Committed),
        "commit_outcome_unknown" => Ok(GroupJournalEventKind::CommitOutcomeUnknown),
        "definitely_aborted" => Ok(GroupJournalEventKind::DefinitelyAborted),
        "formal_only_advanced" => Ok(GroupJournalEventKind::FormalOnlyAdvanced),
        _ => Err(failure(
            DiagnosticCategory::InvalidContract,
            "migration_typedb_record_event_kind_unknown",
            "persisted migration record event kind is unsupported",
        )),
    }
}

fn backfill_direction(direction: BackfillExecutionDirection) -> &'static str {
    match direction {
        BackfillExecutionDirection::Forward => "forward",
        BackfillExecutionDirection::Reverse => "reverse",
    }
}

fn ensure_header(
    format: &str,
    kind: &str,
    scope: &str,
    fence: u64,
    expected_kind: &str,
    expected_scope: &str,
    expected_fence: ExecutionFence,
) -> Result<(), Diagnostic> {
    if format != EXECUTION_RECORD_V1 {
        return Err(failure(
            DiagnosticCategory::InvalidContract,
            "migration_typedb_record_version_unsupported",
            "persisted migration record format is unsupported",
        ));
    }
    if kind != expected_kind || scope != expected_scope || fence != expected_fence.get() {
        return Err(identity_mismatch());
    }
    Ok(())
}

fn ensure_expected_bytes(actual: &[u8], expected: &[u8]) -> Result<(), Diagnostic> {
    if actual == expected {
        Ok(())
    } else {
        Err(identity_mismatch())
    }
}

fn identity_mismatch() -> Diagnostic {
    failure(
        DiagnosticCategory::Integrity,
        "migration_typedb_record_identity_mismatch",
        "persisted migration record does not match independently verified execution evidence",
    )
}

fn failure(category: DiagnosticCategory, code: &'static str, message: &'static str) -> Diagnostic {
    Diagnostic::new(
        category,
        DiagnosticCode::new(code).expect("static diagnostic code is valid"),
        message,
    )
}
