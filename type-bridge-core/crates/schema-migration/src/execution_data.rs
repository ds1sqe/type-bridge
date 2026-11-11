//! Fenced journal identities for atomic data steps.
use crate::{
    ExecutionFence, ExecutionScope, GroupJournalEventKind, MigrationLease,
    VerifiedSchemaMigrationManifest, verified_manifest_digest,
};
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory};
use type_bridge_contract::migration::{MigrationId, MigrationManifestDigest, MigrationStepId};
use type_bridge_contract::migration_data::DataStep;

/// Which atomic program is being executed or recovered.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DataStepDirection {
    /// Run forward and insert its completion marker.
    Forward,
    /// Run reverse and delete its completion marker.
    Reverse,
}

/// Identity-only event whose completion is proven by the managed marker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DataStepEventRecord {
    scope: ExecutionScope,
    fence: ExecutionFence,
    manifest_digest: MigrationManifestDigest,
    migration_id: MigrationId,
    step_id: MigrationStepId,
    direction: DataStepDirection,
    kind: GroupJournalEventKind,
}
impl DataStepEventRecord {
    /// Bind an event to an exact verified step and scope. The provider must
    /// establish marker presence (forward) or absence (reverse) before recording
    /// completion; a journal event alone never authorizes a data re-run.
    pub fn new(
        lease: &MigrationLease,
        manifest: &VerifiedSchemaMigrationManifest,
        step: &DataStep,
        direction: DataStepDirection,
        kind: GroupJournalEventKind,
    ) -> Result<Self, Diagnostic> {
        if lease.scope().managed_scope_id() != manifest.managed_scope().id()
            || !manifest
                .steps()
                .iter()
                .any(|candidate| candidate.as_data() == Some(step))
            || (direction == DataStepDirection::Reverse && step.reverse().is_none())
            || kind == GroupJournalEventKind::FormalOnlyAdvanced
        {
            return Err(Diagnostic::new(
                DiagnosticCategory::Integrity,
                type_bridge_contract::diagnostic::DiagnosticCode::new(
                    "migration_data_event_identity",
                )
                .expect("static code"),
                "data event is not bound to a reversible verified step, direction and scope",
            ));
        }
        Ok(Self {
            scope: lease.scope().clone(),
            fence: lease.fence(),
            manifest_digest: verified_manifest_digest(manifest)?,
            migration_id: manifest.id().clone(),
            step_id: step.id().clone(),
            direction,
            kind,
        })
    }
    /// Return the execution scope.
    pub const fn scope(&self) -> &ExecutionScope {
        &self.scope
    }
    /// Return the lease fence.
    pub const fn fence(&self) -> ExecutionFence {
        self.fence
    }
    /// Return the complete manifest digest.
    pub const fn manifest_digest(&self) -> MigrationManifestDigest {
        self.manifest_digest
    }
    /// Return the compound migration identity.
    pub const fn migration_id(&self) -> &MigrationId {
        &self.migration_id
    }
    /// Return the manifest-local step identity.
    pub const fn step_id(&self) -> &MigrationStepId {
        &self.step_id
    }
    /// Return the execution direction.
    pub const fn direction(&self) -> DataStepDirection {
        self.direction
    }
    /// Return the commit-boundary event.
    pub const fn kind(&self) -> GroupJournalEventKind {
        self.kind
    }
}
