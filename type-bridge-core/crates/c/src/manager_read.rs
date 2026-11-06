//! Canonical C manager reads on direct, borrowed and proxy targets.

use std::ffi::c_void;
use std::mem::size_of;
use std::ptr;
use std::sync::Arc;

use type_bridge_contract::id::TypeId;
use type_bridge_contract::sdk_diagnostic::{
    SdkDiagnosticCode, SdkDiagnosticMessage, SdkExecutionDiagnostic, SdkProviderOperation,
};
use type_bridge_orm::{ProjectedManagerFilterExecutor, ProjectedThing};

use crate::abi::{SchemaPackageState, TypeBridgeStatus, close_box, guarded};
use crate::allocation::{AllocationSite, ReservedBox, allocation_exhausted};
use crate::execution_diagnostic::{TypeBridgeExecutionDiagnostics, return_execution_error};
use crate::generated_preflight::{
    DirectOutputPreflight, GENERATED_INPUT_PROJECTED_TOKEN, direct_output_preflight,
};
use crate::manager_filter::{TypeBridgeManagerFilter, check_filter};
use crate::manager_target::{RawTarget, policy_preflight};
use crate::projected_model::{TypeBridgeProjectedThing, check_projected_thing_ranges};
use crate::projected_token::{TypeBridgeProjectedTokenV1, resolve_model_token};
use crate::projected_value::{invalid_brand_diagnostic, same_package_brand};
use crate::query::{TypeBridgeProxyConnection, TypeBridgeQueryExecutionLimitsV1};
use crate::runtime::{
    TypeBridgeCancellation, TypeBridgeDatabase, TypeBridgeReadTransaction,
    TypeBridgeWriteTransaction,
};

/// Owned exact-model read results; individual things can outlive this owner.
pub struct TypeBridgeManagerResult {
    package: Arc<SchemaPackageState>,
    model: TypeId,
    things: Vec<Arc<ProjectedThing>>,
}

impl TypeBridgeManagerResult {
    pub(crate) fn check_borrowed_ranges(
        &self,
        outputs: &DirectOutputPreflight,
    ) -> Result<(), TypeBridgeStatus> {
        outputs.check_package_borrowed_ranges(&self.package)?;
        outputs.check_bytes(
            self.things.as_ptr().cast(),
            self.things.len() * size_of::<Arc<ProjectedThing>>(),
        )?;
        for thing in &self.things {
            check_projected_thing_ranges(thing, outputs)?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum Output {
    All(*mut *mut TypeBridgeManagerResult),
    First(*mut *mut TypeBridgeProjectedThing),
    Count(*mut u64),
    Exists(*mut u8),
}

impl Output {
    fn range(self) -> (*mut c_void, usize) {
        match self {
            Self::All(out) => (out.cast(), size_of::<*mut TypeBridgeManagerResult>()),
            Self::First(out) => (out.cast(), size_of::<*mut TypeBridgeProjectedThing>()),
            Self::Count(out) => (out.cast(), size_of::<u64>()),
            Self::Exists(out) => (out.cast(), size_of::<u8>()),
        }
    }

    unsafe fn clear(
        self,
        diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> Result<(), TypeBridgeStatus> {
        if !diagnostics.is_null() {
            // SAFETY: caller preflight fenced the writable diagnostics slot.
            unsafe { diagnostics.write_unaligned(ptr::null_mut()) };
        }
        if self.range().0.is_null() {
            return Err(TypeBridgeStatus::InvalidArgument);
        }
        // SAFETY: caller preflight fenced this output's exact type/extent.
        unsafe {
            match self {
                Self::All(out) => out.write_unaligned(ptr::null_mut()),
                Self::First(out) => out.write_unaligned(ptr::null_mut()),
                Self::Count(out) => out.write_unaligned(0),
                Self::Exists(out) => out.write_unaligned(0),
            }
        };
        if diagnostics.is_null() {
            Err(TypeBridgeStatus::InvalidArgument)
        } else {
            Ok(())
        }
    }
}

unsafe fn read(
    raw_target: RawTarget,
    filter: *const TypeBridgeManagerFilter,
    limits: *const TypeBridgeQueryExecutionLimitsV1,
    cancellation: *const TypeBridgeCancellation,
    output: Output,
    diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    guarded(|| {
        let outputs = match direct_output_preflight(&[
            output.range(),
            (
                diagnostics.cast(),
                size_of::<*mut TypeBridgeExecutionDiagnostics>(),
            ),
        ]) {
            Ok(value) => value,
            Err(status) => return status,
        };
        // SAFETY: caller retains the complete target and immutable filter graph.
        if let Err(status) = unsafe { raw_target.preflight(&outputs) }
            .and_then(|()| unsafe { check_filter(&outputs, filter) })
            .and_then(|()| policy_preflight(&outputs, limits, cancellation))
        {
            return status;
        }
        // SAFETY: all outputs are pairwise and input disjoint.
        if let Err(status) = unsafe { output.clear(diagnostics) } {
            return status;
        }
        // SAFETY: preflight fenced the complete target.
        let Some(target) = (unsafe { raw_target.borrow() }) else {
            return TypeBridgeStatus::InvalidArgument;
        };
        if filter.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains the preflighted immutable filter.
        let filter = unsafe { &*filter };
        if !same_package_brand(target.package(), &filter.package) {
            return return_execution_error(invalid_brand_diagnostic(), diagnostics);
        }
        let query_target = match target.query_target() {
            Ok(value) => value,
            Err(error) => return return_execution_error(error, diagnostics),
        };
        // SAFETY: optional policy/cancellation inputs were preflighted.
        let control = match unsafe { target.control(limits, cancellation) } {
            Ok(value) => value,
            Err(error) => return return_execution_error(error, diagnostics),
        };
        let executor = ProjectedManagerFilterExecutor::new(&filter.package.installed_projection);
        macro_rules! execute {
            ($future:expr) => {
                match target.block_on($future) {
                    Ok(value) => value,
                    Err(error) => {
                        return return_execution_error(
                            error.into_diagnostic(SdkProviderOperation::Read),
                            diagnostics,
                        )
                    }
                }
            };
        }
        match output {
            Output::All(out) => {
                let slot = match ReservedBox::try_new(AllocationSite::ManagerResultHandle) {
                    Ok(slot) => slot,
                    Err(_) => return return_execution_error(allocation_exhausted(), diagnostics),
                };
                let things = execute!(executor.all_on_target(query_target, &filter.value, control));
                let result = TypeBridgeManagerResult {
                    package: Arc::clone(&filter.package),
                    model: filter.value.model().clone(),
                    things,
                };
                // SAFETY: output was fenced and initialized; publication is infallible.
                unsafe { out.write_unaligned(Box::into_raw(slot.initialize(result))) };
            }
            Output::First(out) => {
                let slot = match ReservedBox::try_new(AllocationSite::ProjectedThingHandle) {
                    Ok(slot) => slot,
                    Err(_) => return return_execution_error(allocation_exhausted(), diagnostics),
                };
                if let Some(thing) =
                    execute!(executor.first_on_target(query_target, &filter.value, control))
                {
                    // SAFETY: output was fenced and initialized; publication is infallible.
                    unsafe {
                        out.write_unaligned(Box::into_raw(slot.initialize(
                            TypeBridgeProjectedThing::from_arc(Arc::clone(&filter.package), thing),
                        )))
                    };
                }
            }
            Output::Count(out) => {
                let count =
                    execute!(executor.count_on_target(query_target, &filter.value, control));
                // SAFETY: exact scalar output was fenced and initialized.
                unsafe { out.write_unaligned(count) };
            }
            Output::Exists(out) => {
                let exists =
                    execute!(executor.exists_on_target(query_target, &filter.value, control));
                // SAFETY: exact scalar output was fenced and initialized.
                unsafe { out.write_unaligned(u8::from(exists)) };
            }
        }
        TypeBridgeStatus::Ok
    })
}

macro_rules! reads {
    ($owner:ty, $target:ident, $all:ident, $first:ident, $count:ident, $exists:ident) => {
        /// Read all matches on this connection target.
        #[unsafe(no_mangle)]
        pub(crate) unsafe extern "C" fn $all(
            owner: *const $owner,
            filter: *const TypeBridgeManagerFilter,
            limits: *const TypeBridgeQueryExecutionLimitsV1,
            cancellation: *const TypeBridgeCancellation,
            out: *mut *mut TypeBridgeManagerResult,
            diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
        ) -> TypeBridgeStatus {
            // SAFETY: common read preflights every retained input and output.
            unsafe {
                read(
                    RawTarget::$target(owner),
                    filter,
                    limits,
                    cancellation,
                    Output::All(out),
                    diagnostics,
                )
            }
        }
        /// Read the optional identity-proven first match on this target.
        #[unsafe(no_mangle)]
        pub(crate) unsafe extern "C" fn $first(
            owner: *const $owner,
            filter: *const TypeBridgeManagerFilter,
            limits: *const TypeBridgeQueryExecutionLimitsV1,
            cancellation: *const TypeBridgeCancellation,
            out: *mut *mut TypeBridgeProjectedThing,
            diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
        ) -> TypeBridgeStatus {
            // SAFETY: common read preflights every retained input and output.
            unsafe {
                read(
                    RawTarget::$target(owner),
                    filter,
                    limits,
                    cancellation,
                    Output::First(out),
                    diagnostics,
                )
            }
        }
        /// Count matches without hydrating objects.
        #[unsafe(no_mangle)]
        pub(crate) unsafe extern "C" fn $count(
            owner: *const $owner,
            filter: *const TypeBridgeManagerFilter,
            limits: *const TypeBridgeQueryExecutionLimitsV1,
            cancellation: *const TypeBridgeCancellation,
            out: *mut u64,
            diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
        ) -> TypeBridgeStatus {
            // SAFETY: common read preflights every retained input and output.
            unsafe {
                read(
                    RawTarget::$target(owner),
                    filter,
                    limits,
                    cancellation,
                    Output::Count(out),
                    diagnostics,
                )
            }
        }
        /// Test whether any match exists without hydrating objects.
        #[unsafe(no_mangle)]
        pub(crate) unsafe extern "C" fn $exists(
            owner: *const $owner,
            filter: *const TypeBridgeManagerFilter,
            limits: *const TypeBridgeQueryExecutionLimitsV1,
            cancellation: *const TypeBridgeCancellation,
            out: *mut u8,
            diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
        ) -> TypeBridgeStatus {
            // SAFETY: common read preflights every retained input and output.
            unsafe {
                read(
                    RawTarget::$target(owner),
                    filter,
                    limits,
                    cancellation,
                    Output::Exists(out),
                    diagnostics,
                )
            }
        }
    };
}
reads!(
    TypeBridgeDatabase,
    Database,
    type_bridge_database_manager_all,
    type_bridge_database_manager_first,
    type_bridge_database_manager_count,
    type_bridge_database_manager_exists
);
reads!(
    TypeBridgeReadTransaction,
    Read,
    type_bridge_read_transaction_manager_all,
    type_bridge_read_transaction_manager_first,
    type_bridge_read_transaction_manager_count,
    type_bridge_read_transaction_manager_exists
);
reads!(
    TypeBridgeWriteTransaction,
    Write,
    type_bridge_write_transaction_manager_all,
    type_bridge_write_transaction_manager_first,
    type_bridge_write_transaction_manager_count,
    type_bridge_write_transaction_manager_exists
);
reads!(
    TypeBridgeProxyConnection,
    Proxy,
    type_bridge_proxy_connection_manager_all,
    type_bridge_proxy_connection_manager_first,
    type_bridge_proxy_connection_manager_count,
    type_bridge_proxy_connection_manager_exists
);

fn invalid_result() -> SdkExecutionDiagnostic {
    SdkExecutionDiagnostic::invalid_input(
        SdkDiagnosticCode::new("c_manager_result_invalid").unwrap(),
        SdkDiagnosticMessage::new("The manager result model or index does not match this access")
            .unwrap(),
    )
}

unsafe fn result_preflight(
    result: *const TypeBridgeManagerResult,
    model: *const TypeBridgeProjectedTokenV1,
    outputs: &DirectOutputPreflight,
) -> Result<(), TypeBridgeStatus> {
    outputs.check_object_kind(GENERATED_INPUT_PROJECTED_TOKEN, model.cast())?;
    if !result.is_null() {
        outputs.check_bytes(result.cast(), size_of::<TypeBridgeManagerResult>())?;
        // SAFETY: complete immutable result owner was fenced above.
        unsafe { &*result }.check_borrowed_ranges(outputs)?;
    }
    Ok(())
}

unsafe fn validate_result(
    result: &TypeBridgeManagerResult,
    model: *const TypeBridgeProjectedTokenV1,
) -> Result<(), SdkExecutionDiagnostic> {
    // SAFETY: caller preflight fenced the nominal token.
    if unsafe { resolve_model_token(&result.package, model) }? != result.model {
        return Err(invalid_result());
    }
    Ok(())
}

/// Count one nominal manager result.
#[unsafe(export_name = "type_bridge_manager_result_count")]
pub(crate) unsafe extern "C" fn result_count(
    result: *const TypeBridgeManagerResult,
    model: *const TypeBridgeProjectedTokenV1,
    out: *mut usize,
    diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    guarded(|| {
        let outputs = match direct_output_preflight(&[
            (out.cast(), size_of::<usize>()),
            (
                diagnostics.cast(),
                size_of::<*mut TypeBridgeExecutionDiagnostics>(),
            ),
        ]) {
            Ok(value) => value,
            Err(status) => return status,
        };
        // SAFETY: caller retains the complete result graph/token.
        if let Err(status) = unsafe { result_preflight(result, model, &outputs) } {
            return status;
        }
        if !out.is_null() {
            unsafe { out.write_unaligned(0) };
        }
        if !diagnostics.is_null() {
            unsafe { diagnostics.write_unaligned(ptr::null_mut()) };
        }
        if result.is_null() || out.is_null() || diagnostics.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: live immutable result and token were preflighted.
        let result = unsafe { &*result };
        if let Err(error) = unsafe { validate_result(result, model) } {
            return return_execution_error(error, diagnostics);
        }
        unsafe { out.write_unaligned(result.things.len()) };
        TypeBridgeStatus::Ok
    })
}

/// Retain one nominal result thing independently of the result owner.
#[unsafe(export_name = "type_bridge_manager_result_at")]
pub(crate) unsafe extern "C" fn result_at(
    result: *const TypeBridgeManagerResult,
    model: *const TypeBridgeProjectedTokenV1,
    index: usize,
    out: *mut *mut TypeBridgeProjectedThing,
    diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    guarded(|| {
        let output = Output::First(out);
        let outputs = match direct_output_preflight(&[
            output.range(),
            (
                diagnostics.cast(),
                size_of::<*mut TypeBridgeExecutionDiagnostics>(),
            ),
        ]) {
            Ok(value) => value,
            Err(status) => return status,
        };
        // SAFETY: caller retains the complete result graph/token.
        if let Err(status) = unsafe { result_preflight(result, model, &outputs) } {
            return status;
        }
        // SAFETY: all output ranges are pairwise and input disjoint.
        if let Err(status) = unsafe { output.clear(diagnostics) } {
            return status;
        }
        if result.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: the complete immutable result was preflighted.
        let result = unsafe { &*result };
        if let Err(error) = unsafe { validate_result(result, model) } {
            return return_execution_error(error, diagnostics);
        }
        let Some(thing) = result.things.get(index) else {
            return return_execution_error(invalid_result(), diagnostics);
        };
        let slot = match ReservedBox::try_new(AllocationSite::ProjectedThingHandle) {
            Ok(slot) => slot,
            Err(_) => return return_execution_error(allocation_exhausted(), diagnostics),
        };
        // SAFETY: successful validation and allocation precede publication.
        unsafe {
            out.write_unaligned(Box::into_raw(slot.initialize(
                TypeBridgeProjectedThing::from_arc(Arc::clone(&result.package), Arc::clone(thing)),
            )))
        };
        TypeBridgeStatus::Ok
    })
}

/// Close one result; individually retained things remain live.
#[unsafe(export_name = "type_bridge_manager_result_close")]
pub(crate) unsafe extern "C" fn result_close(
    result: *mut *mut TypeBridgeManagerResult,
) -> TypeBridgeStatus {
    // SAFETY: caller owns this slot or supplies null.
    unsafe { close_box(result) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity_crud::tests::{
        COMMIT_SUCCESS, Response, close_diagnostics, close_thing, diagnostic, documents, fixture,
        person_document,
    };
    use crate::iid::TypeBridgeIid;
    use crate::manager_filter::{filter_close, filter_iid, filter_iids, filter_open};
    use std::sync::atomic::Ordering;

    fn selection(
        fixture: &crate::entity_crud::tests::Fixture,
        empty: bool,
    ) -> *mut TypeBridgeManagerFilter {
        let mut source = ptr::null_mut();
        let mut result = ptr::null_mut();
        let mut diagnostics = ptr::null_mut();
        // SAFETY: retained fixture owners and independent output slots.
        unsafe {
            assert_eq!(
                filter_open(
                    &fixture.package,
                    &fixture.token,
                    &mut source,
                    &mut diagnostics
                ),
                TypeBridgeStatus::Ok
            );
            assert_eq!(
                if empty {
                    filter_iids(source, ptr::null(), 0, &mut result, &mut diagnostics)
                } else {
                    filter_iid(
                        source,
                        &TypeBridgeIid::new("0x10").unwrap(),
                        &mut result,
                        &mut diagnostics,
                    )
                },
                TypeBridgeStatus::Ok
            );
            assert_eq!(filter_close(&mut source), TypeBridgeStatus::Ok);
        }
        result
    }

    #[test]
    fn empty_filters_skip_all_provider_reads_on_owned_and_borrowed_targets() {
        let fixture = fixture(vec![], COMMIT_SUCCESS);
        let mut filter = selection(&fixture, true);
        let mut diagnostics = ptr::null_mut();
        let mut all = ptr::dangling_mut();
        let mut first = ptr::dangling_mut();
        let mut count = u64::MAX;
        let mut exists = 255;
        let outputs = [
            Output::All(&mut all),
            Output::First(&mut first),
            Output::Count(&mut count),
            Output::Exists(&mut exists),
        ];
        for output in outputs {
            // SAFETY: fixture/filter and independent output slots remain live.
            assert_eq!(
                unsafe {
                    read(
                        RawTarget::Database(&*fixture.database),
                        filter,
                        ptr::null(),
                        ptr::null(),
                        output,
                        &mut diagnostics,
                    )
                },
                TypeBridgeStatus::Ok
            );
        }
        assert!(unsafe { &*all }.things.is_empty());
        assert!(first.is_null());
        assert_eq!((count, exists), (0, 0));
        assert!(fixture.state.opened_transactions().is_empty());
        assert_eq!(unsafe { result_close(&mut all) }, TypeBridgeStatus::Ok);
        for write in [false, true] {
            let kind = if write {
                type_bridge_orm::TxType::Write
            } else {
                type_bridge_orm::TxType::Read
            };
            let context = fixture.database.open_transaction_context(kind).unwrap();
            let mut reader = ptr::null_mut();
            let mut writer = ptr::null_mut();
            let target = if write {
                writer = Box::into_raw(Box::new(TypeBridgeWriteTransaction::from_test_context(
                    &fixture.database,
                    context,
                )));
                RawTarget::Write(writer)
            } else {
                reader = Box::into_raw(Box::new(TypeBridgeReadTransaction::from_test_context(
                    &fixture.database,
                    context,
                )));
                RawTarget::Read(reader)
            };
            for output in outputs {
                assert_eq!(
                    unsafe {
                        read(
                            target,
                            filter,
                            ptr::null(),
                            ptr::null(),
                            output,
                            &mut diagnostics,
                        )
                    },
                    TypeBridgeStatus::Ok
                );
            }
            assert_eq!(unsafe { result_close(&mut all) }, TypeBridgeStatus::Ok);
            assert_eq!(fixture.state.query_count(), 0);
            assert_eq!(fixture.state.commits.load(Ordering::Acquire), 0);
            assert_eq!(
                unsafe {
                    if write {
                        crate::runtime::type_bridge_write_transaction_close(
                            &mut writer,
                            &mut diagnostics,
                        )
                    } else {
                        crate::runtime::type_bridge_read_transaction_close(
                            &mut reader,
                            &mut diagnostics,
                        )
                    }
                },
                TypeBridgeStatus::Ok
            );
        }
        assert_eq!(unsafe { filter_close(&mut filter) }, TypeBridgeStatus::Ok);
    }

    #[test]
    fn first_and_all_use_shared_iid_reads_and_retain_independent_owners() {
        for first_only in [false, true] {
            let fixture = fixture(
                vec![documents(vec![person_document("0x10")])],
                COMMIT_SUCCESS,
            );
            let mut filter = selection(&fixture, false);
            let mut all = ptr::null_mut();
            let mut thing = ptr::null_mut();
            let mut diagnostics = ptr::null_mut();
            let output = if first_only {
                Output::First(&mut thing)
            } else {
                Output::All(&mut all)
            };
            // SAFETY: fixture and filter own every input and the outputs are independent.
            let status = unsafe {
                read(
                    RawTarget::Database(&*fixture.database),
                    filter,
                    ptr::null(),
                    ptr::null(),
                    output,
                    &mut diagnostics,
                )
            };
            assert_eq!(
                status,
                TypeBridgeStatus::Ok,
                "{:?}",
                (!diagnostics.is_null()).then(|| diagnostic(diagnostics))
            );
            assert_eq!(fixture.state.query_count(), 1);
            if !first_only {
                let mut count = usize::MAX;
                assert_eq!(
                    unsafe { result_count(all, &fixture.token, &mut count, &mut diagnostics) },
                    TypeBridgeStatus::Ok
                );
                assert_eq!(count, 1);
                assert_eq!(
                    unsafe { result_at(all, &fixture.token, 0, &mut thing, &mut diagnostics) },
                    TypeBridgeStatus::Ok
                );
                assert_eq!(unsafe { result_close(&mut all) }, TypeBridgeStatus::Ok);
            }
            assert_eq!(unsafe { filter_close(&mut filter) }, TypeBridgeStatus::Ok);
            assert_eq!(unsafe { &*thing }.value.iid(), "0x10");
            close_thing(&mut thing);
        }
    }

    #[test]
    fn borrowed_provider_panic_poisoning_survives_c_boundary_containment() {
        let fixture = fixture(vec![Response::Panic], COMMIT_SUCCESS);
        let mut filter = selection(&fixture, false);
        let context = fixture
            .database
            .open_transaction_context(type_bridge_orm::TxType::Write)
            .unwrap();
        let mut writer = Box::into_raw(Box::new(TypeBridgeWriteTransaction::from_test_context(
            &fixture.database,
            context,
        )));
        let mut result = ptr::null_mut();
        let mut diagnostics = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_write_transaction_manager_all(
                    writer,
                    filter,
                    ptr::null(),
                    ptr::null(),
                    &mut result,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Panic
        );
        assert!(result.is_null());
        assert!(unsafe { &*writer }.is_poisoned());
        assert_eq!(
            unsafe {
                type_bridge_write_transaction_manager_all(
                    writer,
                    filter,
                    ptr::null(),
                    ptr::null(),
                    &mut result,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ExecutionFailed
        );
        close_diagnostics(&mut diagnostics);
        assert_eq!(fixture.state.query_count(), 1);
        assert_eq!(
            unsafe {
                crate::runtime::type_bridge_write_transaction_close(&mut writer, &mut diagnostics)
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(unsafe { filter_close(&mut filter) }, TypeBridgeStatus::Ok);
    }
}
