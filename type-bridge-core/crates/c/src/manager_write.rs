//! Canonical C manager filter mutations on shared connection targets.

use std::mem::size_of;
use std::ptr;
use std::sync::Arc;

use type_bridge_contract::sdk_diagnostic::SdkProviderOperation;
use type_bridge_orm::{ProjectedBatchOperation, ProjectedManagerFilterExecutor};

use crate::abi::{TypeBridgeStatus, guarded};
use crate::allocation::{AllocationSite, ReservedBox, allocation_exhausted};
use crate::execution_diagnostic::{TypeBridgeExecutionDiagnostics, return_execution_error};
use crate::generated_preflight::{GENERATED_INPUT_PROJECTED_CREATE, direct_output_preflight};
use crate::manager_filter::{TypeBridgeManagerFilter, check_filter};
use crate::manager_target::{RawTarget, policy_preflight};
use crate::projected_batch::TypeBridgeProjectedBatchResult;
use crate::projected_model::TypeBridgeProjectedCreate;
use crate::projected_value::{invalid_brand_diagnostic, same_package_brand};
use crate::query::{TypeBridgeProxyConnection, TypeBridgeQueryExecutionLimitsV1};
use crate::runtime::{TypeBridgeCancellation, TypeBridgeDatabase, TypeBridgeWriteTransaction};

unsafe fn mutate(
    target: RawTarget,
    filter: *const TypeBridgeManagerFilter,
    limits: *const TypeBridgeQueryExecutionLimitsV1,
    cancellation: *const TypeBridgeCancellation,
    update: Option<(
        *const TypeBridgeProjectedCreate,
        *mut *mut TypeBridgeProjectedBatchResult,
    )>,
    diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    guarded(|| {
        let mut ranges = [
            (
                diagnostics.cast(),
                size_of::<*mut TypeBridgeExecutionDiagnostics>(),
            ),
            (
                ptr::null_mut(),
                size_of::<*mut TypeBridgeProjectedBatchResult>(),
            ),
        ];
        if let Some((_, out)) = update {
            ranges[1].0 = out.cast();
        }
        let outputs = match direct_output_preflight(&ranges) {
            Ok(value) => value,
            Err(status) => return status,
        };
        // SAFETY: caller retains the complete target/filter and policy inputs.
        if let Err(status) = unsafe { target.preflight(&outputs) }
            .and_then(|()| unsafe { check_filter(&outputs, filter) })
            .and_then(|()| policy_preflight(&outputs, limits, cancellation))
        {
            return status;
        }
        if let Some((create, _)) = update {
            if let Err(status) =
                outputs.check_object_kind(GENERATED_INPUT_PROJECTED_CREATE, create.cast())
            {
                return status;
            }
            if !create.is_null() {
                // SAFETY: preflight fenced the complete immutable create owner.
                if let Err(status) = unsafe { &*create }.check_borrowed_ranges(&outputs) {
                    return status;
                }
            }
        }
        // SAFETY: every writable output is pairwise and input disjoint.
        if !diagnostics.is_null() {
            unsafe { diagnostics.write_unaligned(ptr::null_mut()) };
        }
        if let Some((_, out)) = update {
            if out.is_null() {
                return TypeBridgeStatus::InvalidArgument;
            }
            unsafe { out.write_unaligned(ptr::null_mut()) };
        }
        if diagnostics.is_null() || filter.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: the target/filter remain live through this synchronous call.
        let Some(target) = (unsafe { target.borrow() }) else {
            return TypeBridgeStatus::InvalidArgument;
        };
        let filter = unsafe { &*filter };
        if !same_package_brand(target.package(), &filter.package) {
            return return_execution_error(invalid_brand_diagnostic(), diagnostics);
        }
        let query_target = match target.query_target() {
            Ok(value) => value,
            Err(error) => return return_execution_error(error, diagnostics),
        };
        // SAFETY: optional descriptor/cancellation owners were fenced above.
        let control = match unsafe { target.control(limits, cancellation) } {
            Ok(value) => value,
            Err(error) => return return_execution_error(error, diagnostics),
        };
        let executor = ProjectedManagerFilterExecutor::new(&filter.package.installed_projection);
        let result = if let Some((create, out)) = update {
            if create.is_null() {
                return TypeBridgeStatus::InvalidArgument;
            }
            // SAFETY: complete create graph was fenced above.
            let create = unsafe { &*create };
            if !same_package_brand(&filter.package, &create.package) {
                return return_execution_error(invalid_brand_diagnostic(), diagnostics);
            }
            let slot = match ReservedBox::try_new(AllocationSite::CrudWriteResultHandle) {
                Ok(value) => value,
                Err(_) => return return_execution_error(allocation_exhausted(), diagnostics),
            };
            let package = Arc::clone(&filter.package);
            let model = filter.value.model().clone();
            target
                .block_on(executor.update_on_target_mapped(
                    query_target,
                    &filter.value,
                    create.value.clone(),
                    control,
                    |value| {
                        TypeBridgeProjectedBatchResult::from_write(
                            package,
                            model,
                            ProjectedBatchOperation::Update,
                            value,
                        )
                    },
                ))
                .map(|value| {
                    // SAFETY: mapping completed under the write lease; publication cannot allocate.
                    unsafe { out.write_unaligned(Box::into_raw(slot.initialize(value))) };
                })
        } else {
            target.block_on(executor.delete_on_target(query_target, &filter.value, control))
        };
        match result {
            Ok(()) => TypeBridgeStatus::Ok,
            Err(error) => return_execution_error(
                error.into_diagnostic(SdkProviderOperation::Write),
                diagnostics,
            ),
        }
    })
}

macro_rules! mutations {
    ($owner:ty, $target:ident, $update:ident, $delete:ident) => {
        /// Replace every matching root through the shared mutation executor.
        #[unsafe(no_mangle)]
        pub(crate) unsafe extern "C" fn $update(
            owner: *const $owner,
            filter: *const TypeBridgeManagerFilter,
            replacement: *const TypeBridgeProjectedCreate,
            limits: *const TypeBridgeQueryExecutionLimitsV1,
            cancellation: *const TypeBridgeCancellation,
            out: *mut *mut TypeBridgeProjectedBatchResult,
            diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
        ) -> TypeBridgeStatus {
            // SAFETY: common mutation preflights all retained inputs and outputs.
            unsafe {
                mutate(
                    RawTarget::$target(owner),
                    filter,
                    limits,
                    cancellation,
                    Some((replacement, out)),
                    diagnostics,
                )
            }
        }
        /// Delete all matches without fetching or returning their objects.
        #[unsafe(no_mangle)]
        pub(crate) unsafe extern "C" fn $delete(
            owner: *const $owner,
            filter: *const TypeBridgeManagerFilter,
            limits: *const TypeBridgeQueryExecutionLimitsV1,
            cancellation: *const TypeBridgeCancellation,
            diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
        ) -> TypeBridgeStatus {
            // SAFETY: common mutation preflights all retained inputs and outputs.
            unsafe {
                mutate(
                    RawTarget::$target(owner),
                    filter,
                    limits,
                    cancellation,
                    None,
                    diagnostics,
                )
            }
        }
    };
}
mutations!(
    TypeBridgeDatabase,
    Database,
    type_bridge_database_manager_update,
    type_bridge_database_manager_delete
);
mutations!(
    TypeBridgeWriteTransaction,
    Write,
    type_bridge_write_transaction_manager_update,
    type_bridge_write_transaction_manager_delete
);
mutations!(
    TypeBridgeProxyConnection,
    Proxy,
    type_bridge_proxy_connection_manager_update,
    type_bridge_proxy_connection_manager_delete
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::allocation::inject_failure;
    use crate::entity_crud::tests::{
        COMMIT_SUCCESS, close_diagnostics, diagnostic, documents, fixture, person_document,
    };
    use crate::iid::TypeBridgeIid;
    use crate::manager_filter::{filter_close, filter_iid, filter_iids, filter_open};
    use crate::projected_batch::{
        type_bridge_projected_batch_result_close_impl,
        type_bridge_projected_batch_result_count_impl,
    };
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
    fn empty_filter_mutations_do_not_open_or_dispatch() {
        let fixture = fixture(vec![], COMMIT_SUCCESS);
        let mut filter = selection(&fixture, true);
        let mut result = ptr::null_mut();
        let mut diagnostics = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_database_manager_update(
                    &*fixture.database,
                    filter,
                    &*fixture.create,
                    ptr::null(),
                    ptr::null(),
                    &mut result,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        let mut count = usize::MAX;
        assert_eq!(
            unsafe {
                type_bridge_projected_batch_result_count_impl(
                    result,
                    &fixture.token,
                    3,
                    &mut count,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(count, 0);
        assert_eq!(
            unsafe { type_bridge_projected_batch_result_close_impl(&mut result) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe {
                type_bridge_database_manager_delete(
                    &*fixture.database,
                    filter,
                    ptr::null(),
                    ptr::null(),
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert!(fixture.state.opened_transactions().is_empty());
        assert_eq!(fixture.state.query_count(), 0);
        assert_eq!(unsafe { filter_close(&mut filter) }, TypeBridgeStatus::Ok);
    }

    #[test]
    fn iid_mutations_share_plans_and_respect_owned_or_borrowed_commit() {
        for borrowed in [false, true] {
            let fixture = fixture(
                vec![documents(vec![person_document("0x10")]), documents(vec![])],
                COMMIT_SUCCESS,
            );
            let mut filter = selection(&fixture, false);
            let mut transaction = if borrowed {
                let context = fixture
                    .database
                    .open_transaction_context(type_bridge_orm::TxType::Write)
                    .unwrap();
                Box::into_raw(Box::new(TypeBridgeWriteTransaction::from_test_context(
                    &fixture.database,
                    context,
                )))
            } else {
                ptr::null_mut()
            };
            let target = if borrowed {
                RawTarget::Write(transaction)
            } else {
                RawTarget::Database(&*fixture.database)
            };
            let mut result = ptr::null_mut();
            let mut diagnostics = ptr::null_mut();
            // SAFETY: target/filter/create owners and output slots remain live.
            let status = unsafe {
                mutate(
                    target,
                    filter,
                    ptr::null(),
                    ptr::null(),
                    Some((&*fixture.create, &mut result)),
                    &mut diagnostics,
                )
            };
            assert_eq!(
                status,
                TypeBridgeStatus::Ok,
                "{:?}",
                (!diagnostics.is_null()).then(|| diagnostic(diagnostics))
            );
            assert_eq!(unsafe { &*result }.test_item().iid(), "0x10");
            assert_eq!(
                unsafe { type_bridge_projected_batch_result_close_impl(&mut result) },
                TypeBridgeStatus::Ok
            );
            assert_eq!(
                unsafe {
                    mutate(
                        target,
                        filter,
                        ptr::null(),
                        ptr::null(),
                        None,
                        &mut diagnostics,
                    )
                },
                TypeBridgeStatus::Ok
            );
            assert_eq!(fixture.state.query_count(), 2);
            assert_eq!(
                fixture.state.commits.load(Ordering::Acquire),
                if borrowed { 0 } else { 2 }
            );
            assert_eq!(fixture.state.rollbacks.load(Ordering::Acquire), 0);
            if borrowed {
                assert_eq!(
                    unsafe {
                        crate::runtime::type_bridge_write_transaction_close(
                            &mut transaction,
                            &mut diagnostics,
                        )
                    },
                    TypeBridgeStatus::Ok
                );
                assert_eq!(fixture.state.rollbacks.load(Ordering::Acquire), 1);
            }
            assert_eq!(unsafe { filter_close(&mut filter) }, TypeBridgeStatus::Ok);
        }
    }

    #[test]
    fn mapped_filter_update_allocation_failure_rolls_back_before_commit() {
        let fixture = fixture(
            vec![documents(vec![person_document("0x10")])],
            COMMIT_SUCCESS,
        );
        let mut filter = selection(&fixture, false);
        let mut result = ptr::dangling_mut();
        let mut diagnostics = ptr::null_mut();
        {
            let _failure = inject_failure(AllocationSite::ProjectedBatchResultThingStorage, 0);
            assert_eq!(
                unsafe {
                    type_bridge_database_manager_update(
                        &*fixture.database,
                        filter,
                        &*fixture.create,
                        ptr::null(),
                        ptr::null(),
                        &mut result,
                        &mut diagnostics,
                    )
                },
                TypeBridgeStatus::ResourceLimit
            );
        }
        assert!(result.is_null());
        assert_eq!(diagnostic(diagnostics).1, "c_allocation_exhausted");
        assert_eq!(fixture.state.query_count(), 1);
        assert_eq!(fixture.state.commits.load(Ordering::Acquire), 0);
        assert_eq!(fixture.state.rollbacks.load(Ordering::Acquire), 1);
        close_diagnostics(&mut diagnostics);
        assert_eq!(unsafe { filter_close(&mut filter) }, TypeBridgeStatus::Ok);
    }
}
