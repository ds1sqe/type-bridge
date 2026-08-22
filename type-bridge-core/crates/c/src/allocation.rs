//! Fallible native-boundary allocations and deterministic failure probes.

use std::alloc::{Layout, alloc, dealloc};
use std::mem::{ManuallyDrop, MaybeUninit};
use std::ptr::NonNull;

use type_bridge_contract::sdk_diagnostic::{
    SdkDiagnosticCode, SdkDiagnosticMessage, SdkExecutionDiagnostic,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AllocationSite {
    ProjectedValueHandle,
    ProjectedReferenceHandle,
    ProjectedCreateHandle,
    ProjectedThingHandle,
    ProjectedCreateBuilderHandle,
    ProjectedCreateBuilderChunk,
    ProjectedBatchBuilderHandle,
    ProjectedBatchBuilderRows,
    ProjectedBatchBuilderIidBytes,
    ProjectedBatchBuilderCreateClone,
    ProjectedBatchFinishRows,
    ProjectedBatchFinishTargets,
    ProjectedBatchFinishKeys,
    ProjectedBatchHandle,
    ProjectedBatchResultThings,
    ProjectedBatchResultThingStorage,
    ProjectedBatchResultHandle,
    ProjectedBatchResultThingHandle,
    DatabaseHandle,
    ReadTransactionHandle,
    WriteTransactionHandle,
    SchemaPackageChunkAssembly,
    QuerySessionHandle,
    QueryBindingHandle,
    QueryFieldHandle,
    QueryRoleHandle,
    QueryPredicateHandle,
    QueryOrderHandle,
    QuerySelectionHandle,
    QueryHandle,
    QueryTerminalHandle,
    QueryResultHandle,
    QueryThingHandle,
    QueryValueHandle,
    QueryRemoteContextHandle,
    QueryRemoteAdvertisementBytes,
    QueryRemotePendingHandle,
    QueryRemoteClaimHandle,
    QueryRemoteResponseBytes,
    QueryFunctionHandle,
    QueryFunctionValueHandle,
    QueryFunctionCallHandle,
    QueryFunctionArguments,
    MigrationAdministrationHandle,
    MigrationDeletionPlanHandle,
    MigrationCatalogHandle,
    MigrationPlanHandle,
    MigrationSnapshotHandle,
    MigrationApprovalBuilderHandle,
    MigrationApprovalSetHandle,
    MigrationExecutionOutcomeHandle,
    MigrationBackfillObservationHandle,
    MigrationVerificationReportHandle,
    MigrationVerificationFindingHandle,
    MigrationDiagnosticsHandle,
    MigrationCancellationHandle,
    CanonicalBytesHandle,
    CanonicalArchiveBuilderHandle,
    CanonicalArchiveHandle,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct AllocationFailure;

pub(crate) fn allocation_exhausted() -> SdkExecutionDiagnostic {
    SdkExecutionDiagnostic::resource_limit(
        SdkDiagnosticCode::new("c_allocation_exhausted")
            .expect("static allocation diagnostic code is canonical"),
        SdkDiagnosticMessage::new("The C runtime could not allocate bounded projected storage")
            .expect("static allocation diagnostic message is valid"),
    )
}

#[cfg(test)]
#[derive(Clone, Copy)]
struct InjectedFailure {
    site: AllocationSite,
    matches_before_failure: usize,
}

#[cfg(test)]
std::thread_local! {
    static INJECTED_FAILURE: std::cell::Cell<Option<InjectedFailure>> = const {
        std::cell::Cell::new(None)
    };
}

pub(crate) fn allocation_checkpoint(_site: AllocationSite) -> Result<(), AllocationFailure> {
    #[cfg(test)]
    {
        let site = _site;
        let failed = INJECTED_FAILURE.with(|state| {
            let Some(mut failure) = state.get() else {
                return false;
            };
            if failure.site != site {
                return false;
            }
            if failure.matches_before_failure == 0 {
                state.set(None);
                true
            } else {
                failure.matches_before_failure -= 1;
                state.set(Some(failure));
                false
            }
        });
        if failed {
            return Err(AllocationFailure);
        }
    }
    Ok(())
}

pub(crate) struct ReservedBox<T> {
    pointer: NonNull<MaybeUninit<T>>,
}

impl<T> ReservedBox<T> {
    pub(crate) fn try_new(site: AllocationSite) -> Result<Self, AllocationFailure> {
        allocation_checkpoint(site)?;
        let layout = Layout::new::<T>();
        let pointer = if layout.size() == 0 {
            NonNull::dangling()
        } else {
            // SAFETY: the layout exactly describes `MaybeUninit<T>` storage.
            NonNull::new(unsafe { alloc(layout) }.cast::<MaybeUninit<T>>())
                .ok_or(AllocationFailure)?
        };
        Ok(Self { pointer })
    }

    pub(crate) fn initialize(self, value: T) -> Box<T> {
        let this = ManuallyDrop::new(self);
        let pointer = this.pointer.cast::<T>();
        // SAFETY: this reservation uniquely owns properly aligned storage for
        // one `T`; initialization occurs exactly once before Box adoption.
        unsafe {
            pointer.as_ptr().write(value);
            Box::from_raw(pointer.as_ptr())
        }
    }
}

impl<T> Drop for ReservedBox<T> {
    fn drop(&mut self) {
        let layout = Layout::new::<T>();
        if layout.size() != 0 {
            // SAFETY: an uninitialized reservation owns this exact allocation.
            unsafe { dealloc(self.pointer.as_ptr().cast(), layout) };
        }
    }
}

pub(crate) fn try_box<T>(site: AllocationSite, value: T) -> Result<Box<T>, AllocationFailure> {
    ReservedBox::try_new(site).map(|reservation| reservation.initialize(value))
}

pub(crate) fn try_reserve<T>(
    values: &mut Vec<T>,
    additional: usize,
    site: AllocationSite,
) -> Result<(), AllocationFailure> {
    allocation_checkpoint(site)?;
    values
        .try_reserve(additional)
        .map_err(|_| AllocationFailure)
}

#[cfg(test)]
pub(crate) struct InjectedFailureGuard;

#[cfg(test)]
impl Drop for InjectedFailureGuard {
    fn drop(&mut self) {
        INJECTED_FAILURE.with(|state| state.set(None));
    }
}

#[cfg(test)]
pub(crate) fn inject_failure(
    site: AllocationSite,
    matches_before_failure: usize,
) -> InjectedFailureGuard {
    INJECTED_FAILURE.with(|state| {
        assert!(
            state.get().is_none(),
            "allocation failure probe already active"
        );
        state.set(Some(InjectedFailure {
            site,
            matches_before_failure,
        }));
    });
    InjectedFailureGuard
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reservation_failure_is_deterministic_and_raii_is_single_owner() {
        let _failure = inject_failure(AllocationSite::ProjectedThingHandle, 0);
        assert!(ReservedBox::<String>::try_new(AllocationSite::ProjectedThingHandle).is_err());
        let reservation = ReservedBox::<String>::try_new(AllocationSite::ProjectedThingHandle)
            .expect("one-shot injected failure was consumed");
        let value = reservation.initialize("retained".to_owned());
        assert_eq!(&*value, "retained");
    }
}
