//! Canonical single insert/put across direct and proxy targets.

use std::mem::size_of;
use std::sync::Arc;

use type_bridge_contract::sdk_diagnostic::SdkProviderOperation;
use type_bridge_orm::{ProjectedBatchOperation, ProjectedBatchRow, ProjectedCrudExecutor};

use crate::abi::{TypeBridgeStatus, guarded};
use crate::allocation::{AllocationSite, ReservedBox, allocation_exhausted};
use crate::execution_diagnostic::{
    TypeBridgeExecutionDiagnostics, initialize_execution_outputs, return_execution_error,
};
use crate::generated_preflight::{
    GENERATED_INPUT_PROJECTED_CREATE, GENERATED_INPUT_PROJECTED_TOKEN, direct_output_preflight,
};
use crate::manager_target::{RawTarget, policy_preflight};
use crate::projected_batch::TypeBridgeProjectedBatchResult;
use crate::projected_model::TypeBridgeProjectedCreate;
use crate::projected_token::{TypeBridgeProjectedTokenV1, resolve_model_token};
use crate::projected_value::{invalid_brand_diagnostic, same_package_brand};
use crate::query::{TypeBridgeProxyConnection, TypeBridgeQueryExecutionLimitsV1};
use crate::runtime::{TypeBridgeCancellation, TypeBridgeDatabase, TypeBridgeWriteTransaction};

#[allow(clippy::too_many_arguments)]
unsafe fn create(
    target: RawTarget,
    model: *const TypeBridgeProjectedTokenV1,
    input: *const TypeBridgeProjectedCreate,
    operation: ProjectedBatchOperation,
    limits: *const TypeBridgeQueryExecutionLimitsV1,
    cancellation: *const TypeBridgeCancellation,
    out: *mut *mut TypeBridgeProjectedBatchResult,
    diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    guarded(|| {
        let outputs = match direct_output_preflight(&[
            (out.cast(), size_of::<*mut TypeBridgeProjectedBatchResult>()),
            (
                diagnostics.cast(),
                size_of::<*mut TypeBridgeExecutionDiagnostics>(),
            ),
        ]) {
            Ok(value) => value,
            Err(status) => return status,
        };
        // SAFETY: caller retains complete input/target owners for this call.
        if let Err(status) = unsafe { target.preflight(&outputs) }
            .and_then(|()| outputs.check_object_kind(GENERATED_INPUT_PROJECTED_TOKEN, model.cast()))
            .and_then(|()| {
                outputs.check_object_kind(GENERATED_INPUT_PROJECTED_CREATE, input.cast())
            })
            .and_then(|()| policy_preflight(&outputs, limits, cancellation))
        {
            return status;
        }
        if !input.is_null() {
            // SAFETY: complete create owner has been fenced.
            if let Err(status) = unsafe { &*input }.check_borrowed_ranges(&outputs) {
                return status;
            }
        }
        // SAFETY: output slots are pairwise and input disjoint.
        if let Err(status) = unsafe { initialize_execution_outputs(out, diagnostics) } {
            return status;
        }
        // SAFETY: target and input remain live for the synchronous invocation.
        let Some(target) = (unsafe { target.borrow() }) else {
            return TypeBridgeStatus::InvalidArgument;
        };
        if input.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        let input = unsafe { &*input };
        if !same_package_brand(target.package(), &input.package) {
            return return_execution_error(invalid_brand_diagnostic(), diagnostics);
        }
        // Capture before model validation and owned row construction.
        let control = match unsafe { target.batch_control(limits, cancellation) } {
            Ok(value) => value,
            Err(error) => return return_execution_error(error, diagnostics),
        };
        let model = match unsafe { resolve_model_token(target.package(), model) } {
            Ok(value) => value,
            Err(error) => return return_execution_error(error, diagnostics),
        };
        let query_target = match target.query_target() {
            Ok(value) => value,
            Err(error) => return return_execution_error(error, diagnostics),
        };
        let executor = ProjectedCrudExecutor::new(&target.package().installed_projection);
        let batch = match executor.single_controlled_write_batch(
            &model,
            operation,
            ProjectedBatchRow::Create(input.value.clone()),
            &control,
        ) {
            Ok(value) => value,
            Err(error) => return return_execution_error(error, diagnostics),
        };
        let slot = match ReservedBox::try_new(AllocationSite::CrudWriteResultHandle) {
            Ok(value) => value,
            Err(_) => return return_execution_error(allocation_exhausted(), diagnostics),
        };
        let package = Arc::clone(target.package());
        match target.block_on(executor.execute_single_write_on_target(
            query_target,
            &batch,
            control,
            |value| TypeBridgeProjectedBatchResult::from_write(package, model, operation, value),
        )) {
            Ok(value) => {
                // SAFETY: reserved outer owner publishes without allocation after commit.
                unsafe { out.write_unaligned(Box::into_raw(slot.initialize(value))) };
                TypeBridgeStatus::Ok
            }
            Err(error) => return_execution_error(
                error.into_diagnostic(SdkProviderOperation::Write),
                diagnostics,
            ),
        }
    })
}

macro_rules! creates {
    ($owner:ty, $target:ident, $name:ident, $operation:ident) => {
        /// Execute one create and retain every returned root.
        #[unsafe(no_mangle)]
        pub(crate) unsafe extern "C" fn $name(
            owner: *const $owner,
            model: *const TypeBridgeProjectedTokenV1,
            input: *const TypeBridgeProjectedCreate,
            limits: *const TypeBridgeQueryExecutionLimitsV1,
            cancellation: *const TypeBridgeCancellation,
            out: *mut *mut TypeBridgeProjectedBatchResult,
            diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
        ) -> TypeBridgeStatus {
            // SAFETY: common create preflights the complete retained input graph.
            unsafe {
                create(
                    RawTarget::$target(owner),
                    model,
                    input,
                    ProjectedBatchOperation::$operation,
                    limits,
                    cancellation,
                    out,
                    diagnostics,
                )
            }
        }
    };
}
creates!(
    TypeBridgeDatabase,
    Database,
    type_bridge_database_manager_insert,
    Insert
);
creates!(
    TypeBridgeDatabase,
    Database,
    type_bridge_database_manager_put,
    Put
);
creates!(
    TypeBridgeWriteTransaction,
    Write,
    type_bridge_write_transaction_manager_insert,
    Insert
);
creates!(
    TypeBridgeWriteTransaction,
    Write,
    type_bridge_write_transaction_manager_put,
    Put
);
creates!(
    TypeBridgeProxyConnection,
    Proxy,
    type_bridge_proxy_connection_manager_insert,
    Insert
);
creates!(
    TypeBridgeProxyConnection,
    Proxy,
    type_bridge_proxy_connection_manager_put,
    Put
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::allocation::inject_failure;
    use crate::entity_crud::tests::{
        COMMIT_SUCCESS, close_diagnostics, diagnostic, documents, fixture, person_document,
    };
    use crate::projected_batch::type_bridge_projected_batch_result_close_impl;
    use std::ptr;
    use std::sync::atomic::Ordering;

    #[test]
    fn creates_share_owned_and_borrowed_single_write_execution() {
        for operation in [
            ProjectedBatchOperation::Insert,
            ProjectedBatchOperation::Put,
        ] {
            for borrowed in [false, true] {
                let fixture = fixture(
                    vec![documents(vec![person_document("0x10")])],
                    COMMIT_SUCCESS,
                );
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
                let status = unsafe {
                    create(
                        target,
                        &fixture.token,
                        &*fixture.create,
                        operation,
                        ptr::null(),
                        ptr::null(),
                        &mut result,
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
                assert_eq!(fixture.state.query_count(), 1);
                assert_eq!(
                    fixture.state.commits.load(Ordering::Acquire),
                    usize::from(!borrowed)
                );
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
            }
        }
    }

    #[test]
    fn shared_create_commit_panic_keeps_unknown_certainty() {
        for execute in [
            type_bridge_database_manager_insert,
            type_bridge_database_manager_put,
        ] {
            let fixture = fixture(
                vec![documents(vec![person_document("0x10")])],
                crate::entity_crud::tests::COMMIT_PANIC,
            );
            let mut result = ptr::dangling_mut();
            let mut diagnostics = ptr::null_mut();
            assert_eq!(
                unsafe {
                    execute(
                        &*fixture.database,
                        &fixture.token,
                        &*fixture.create,
                        ptr::null(),
                        ptr::null(),
                        &mut result,
                        &mut diagnostics,
                    )
                },
                TypeBridgeStatus::CommitOutcomeUnknown
            );
            assert!(result.is_null());
            assert_eq!(diagnostic(diagnostics).1, "commit_outcome_unknown");
            assert_eq!(fixture.state.commits.load(Ordering::Acquire), 1);
            assert_eq!(fixture.state.rollbacks.load(Ordering::Acquire), 0);
            close_diagnostics(&mut diagnostics);
        }
    }

    #[test]
    fn create_mapping_failure_rolls_back_before_commit() {
        let fixture = fixture(
            vec![documents(vec![person_document("0x10")])],
            COMMIT_SUCCESS,
        );
        let mut result = ptr::dangling_mut();
        let mut diagnostics = ptr::null_mut();
        {
            let _failure = inject_failure(AllocationSite::ProjectedBatchResultThingStorage, 0);
            assert_eq!(
                unsafe {
                    type_bridge_database_manager_insert(
                        &*fixture.database,
                        &fixture.token,
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
    }
}
