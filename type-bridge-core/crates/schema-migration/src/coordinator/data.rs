//! Atomic data execution; the managed marker resolves commit uncertainty.
use super::*;
use crate::{DataStepDirection, DataStepEventRecord, VerifiedSchemaMigrationManifest};
use type_bridge_contract::migration_data::DataStep;

pub(super) fn events<'a>(
    entries: &'a [JournalEntry<DataStepEventRecord>],
    manifest: &VerifiedSchemaMigrationManifest,
    step: &DataStep,
    direction: DataStepDirection,
) -> Vec<&'a DataStepEventRecord> {
    entries
        .iter()
        .map(JournalEntry::record)
        .filter(|record| {
            record.migration_id() == manifest.id()
                && record.step_id() == step.id()
                && record.direction() == direction
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn execute<S: MigrationExecutionJournal, P: MigrationExecutionProvider>(
    store: &S,
    provider: &P,
    lease: &MigrationLease,
    manifest: &VerifiedSchemaMigrationManifest,
    step: &DataStep,
    direction: DataStepDirection,
    entries: &[JournalEntry<DataStepEventRecord>],
    control: &MigrationExecutionControl,
) -> Result<(), Diagnostic> {
    control.check()?;
    let events = events(entries, manifest, step, direction);
    let last = events.last().map(|record| record.kind());
    let complete = await_interruptible_operation(
        control,
        provider.observe_data_step(lease, manifest, step, direction),
    )
    .await?;
    if last == Some(GroupJournalEventKind::Committed) {
        return if complete {
            Ok(())
        } else {
            Err(failure(
                DiagnosticCategory::Integrity,
                "migration_data_marker_drift",
                "completed data journal disagrees with the managed completion marker",
            ))
        };
    }
    if last == Some(GroupJournalEventKind::DefinitelyAborted) && complete {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "migration_data_marker_drift",
            "aborted data journal disagrees with the managed completion marker",
        ));
    }
    let event = |kind| DataStepEventRecord::new(lease, manifest, step, direction, kind);
    // Recovery may find a committed marker even without a retained attempt.
    // Give the repaired checkpoint the same complete event state machine.
    if last.is_none() || !complete {
        store
            .record_data_event(lease, event(GroupJournalEventKind::BeforeCommit)?)
            .await?;
    }
    if !complete
        && let Err(failure) = provider
            .execute_data_step(lease, manifest, step, direction, control)
            .await
    {
        let kind = match failure.certainty() {
            GroupCommitCertainty::DefinitelyAborted => GroupJournalEventKind::DefinitelyAborted,
            GroupCommitCertainty::Unknown => GroupJournalEventKind::CommitOutcomeUnknown,
        };
        store.record_data_event(lease, event(kind)?).await?;
        if failure.certainty() == GroupCommitCertainty::DefinitelyAborted
            || !provider
                .observe_data_step(lease, manifest, step, direction)
                .await?
        {
            return Err(failure.into_parts().1);
        }
    }
    store
        .record_data_event(lease, event(GroupJournalEventKind::Committed)?)
        .await?;
    Ok(())
}

pub(super) fn validate<'a>(
    lease: &MigrationLease,
    entries: &[JournalEntry<DataStepEventRecord>],
    manifests: impl Iterator<Item = &'a VerifiedSchemaMigrationManifest>,
    direction: DataStepDirection,
) -> Result<(), Diagnostic> {
    let manifests: Vec<_> = manifests.collect();
    for entry in entries {
        let record = entry.record();
        let manifest = manifests
            .iter()
            .find(|manifest| manifest.id() == record.migration_id())
            .ok_or_else(|| {
                failure(
                    DiagnosticCategory::Integrity,
                    "migration_data_event_foreign_manifest",
                    "data event names a manifest outside the open plan",
                )
            })?;
        let step = manifest
            .steps()
            .iter()
            .filter_map(|step| step.as_data())
            .find(|step| step.id() == record.step_id())
            .ok_or_else(|| {
                failure(
                    DiagnosticCategory::Integrity,
                    "migration_data_event_foreign_step",
                    "data event names a step outside the verified manifest",
                )
            })?;
        let historical = MigrationLease::new(
            lease.scope().clone(),
            lease.holder().clone(),
            record.fence(),
        );
        if &DataStepEventRecord::new(&historical, manifest, step, direction, record.kind())?
            != record
        {
            return Err(failure(
                DiagnosticCategory::Integrity,
                "migration_data_event_mismatch",
                "data event differs from verified manifest evidence",
            ));
        }
    }
    Ok(())
}
