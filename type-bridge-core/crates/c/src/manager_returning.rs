//! Typed IID returning deletes through the canonical batch executor.

use std::{mem::size_of, sync::Arc};

use type_bridge_contract::sdk_diagnostic::SdkProviderOperation;
use type_bridge_orm::projected_batch::ProjectedBatchBindingBudget;
use type_bridge_orm::{
    ProjectedBatch, ProjectedBatchExecutor, ProjectedBatchOperation, ProjectedBatchResult,
    ProjectedBatchRow,
};

use crate::abi::{TypeBridgeStatus, guarded};
use crate::allocation::{AllocationSite, ReservedBox, allocation_exhausted, try_reserve};
use crate::execution_diagnostic::{
    TypeBridgeExecutionDiagnostics, initialize_execution_outputs, return_execution_error,
};
use crate::generated_preflight::{
    GENERATED_INPUT_IID, GENERATED_INPUT_PROJECTED_TOKEN, direct_output_preflight,
};
use crate::iid::TypeBridgeIid;
use crate::manager_target::{RawTarget, policy_preflight};
use crate::projected_batch::{TypeBridgeProjectedBatchResult, copy_iid};
use crate::projected_token::{TypeBridgeProjectedTokenV1, resolve_model_token};
use crate::query::{TypeBridgeProxyConnection, TypeBridgeQueryExecutionLimitsV1};
use crate::runtime::{TypeBridgeCancellation, TypeBridgeDatabase, TypeBridgeWriteTransaction};

#[allow(clippy::too_many_arguments)]
unsafe fn delete_returning(
    target: RawTarget,
    model: *const TypeBridgeProjectedTokenV1,
    iids: *const *const TypeBridgeIid,
    count: usize,
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
        // SAFETY: the caller retains all complete target, token and policy owners.
        if let Err(status) = unsafe { target.preflight(&outputs) }
            .and_then(|()| outputs.check_object_kind(GENERATED_INPUT_PROJECTED_TOKEN, model.cast()))
            .and_then(|()| policy_preflight(&outputs, limits, cancellation))
        {
            return status;
        }
        let Some(bytes) = count.checked_mul(size_of::<*const TypeBridgeIid>()) else {
            return TypeBridgeStatus::InvalidArgument;
        };
        if count != 0 {
            if iids.is_null() {
                return TypeBridgeStatus::InvalidArgument;
            }
            if let Err(status) = outputs.check_bytes(iids.cast(), bytes) {
                return status;
            }
            for index in 0..count {
                // SAFETY: caller retains the complete pointer array for this call.
                let iid = unsafe { iids.add(index).read_unaligned() };
                if let Err(status) = outputs.check_object_kind(GENERATED_INPUT_IID, iid.cast()) {
                    return status;
                }
            }
        }
        // SAFETY: outputs are disjoint from each other and every retained input.
        if let Err(status) = unsafe { initialize_execution_outputs(out, diagnostics) } {
            return status;
        }
        // SAFETY: complete target was fenced and remains live.
        let Some(target) = (unsafe { target.borrow() }) else {
            return TypeBridgeStatus::InvalidArgument;
        };
        let prepared = (|| {
            // Capture once before model resolution, copying and batch construction.
            let control = unsafe { target.batch_control(limits, cancellation) }?;
            let model = unsafe { resolve_model_token(target.package(), model) }?;
            ProjectedBatch::validate_binding_row_count(count)?;
            let installed = &target.package().installed_projection;
            let mut budget = ProjectedBatchBindingBudget::try_new_for_invocation(
                installed,
                model.clone(),
                ProjectedBatchOperation::Delete,
                &control,
            )?;
            let mut rows = Vec::new();
            for index in 0..count {
                budget.checkpoint()?;
                // SAFETY: preflight fenced this table element and its complete IID owner.
                let iid = unsafe { copy_iid(iids.add(index).read_unaligned()) }?;
                let row = ProjectedBatchRow::Delete { iid };
                budget.try_add_row(installed, &row)?;
                try_reserve(&mut rows, 1, AllocationSite::ProjectedBatchBuilderRows)
                    .map_err(|_| allocation_exhausted())?;
                rows.push(row);
            }
            let batch = ProjectedBatch::try_new_for_invocation(
                installed,
                model.clone(),
                ProjectedBatchOperation::Delete,
                rows,
                &control,
            )?;
            let slot = ReservedBox::try_new(AllocationSite::ProjectedBatchResultHandle)
                .map_err(|_| allocation_exhausted())?;
            let empty = TypeBridgeProjectedBatchResult::from_deleted(
                Arc::clone(target.package()),
                model.clone(),
                count,
                ProjectedBatchResult::Things(Default::default()),
            )?;
            Ok((control, model, batch, slot, empty, target.query_target()?))
        })();
        let (control, model, batch, slot, empty, query_target) = match prepared {
            Ok(value) => value,
            Err(error) => return return_execution_error(error, diagnostics),
        };
        let executor =
            ProjectedBatchExecutor::new(&target.package().installed_projection).returning_deleted();
        let package = Arc::clone(target.package());
        match target.block_on(executor.execute_on_target_mapped(
            query_target,
            &batch,
            control,
            empty,
            |value| TypeBridgeProjectedBatchResult::from_deleted(package, model, count, value),
        )) {
            Ok(value) => {
                // SAFETY: mapping and reservation completed before commit; publication cannot allocate.
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

macro_rules! returning {
    ($owner:ty, $target:ident, $single:ident, $many:ident) => {
        /// Delete one typed IID and return its complete snapshot, or an empty result if absent.
        #[unsafe(no_mangle)]
        pub(crate) unsafe extern "C" fn $single(
            owner: *const $owner,
            model: *const TypeBridgeProjectedTokenV1,
            iid: *const TypeBridgeIid,
            limits: *const TypeBridgeQueryExecutionLimitsV1,
            cancellation: *const TypeBridgeCancellation,
            out: *mut *mut TypeBridgeProjectedBatchResult,
            diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
        ) -> TypeBridgeStatus {
            // SAFETY: shared implementation fences the retained IID and all caller outputs.
            unsafe {
                delete_returning(
                    RawTarget::$target(owner),
                    model,
                    &iid,
                    1,
                    limits,
                    cancellation,
                    out,
                    diagnostics,
                )
            }
        }
        /// Delete an IID list, omitting absent roots; an empty list performs no provider I/O.
        #[unsafe(no_mangle)]
        pub(crate) unsafe extern "C" fn $many(
            owner: *const $owner,
            model: *const TypeBridgeProjectedTokenV1,
            iids: *const *const TypeBridgeIid,
            count: usize,
            limits: *const TypeBridgeQueryExecutionLimitsV1,
            cancellation: *const TypeBridgeCancellation,
            out: *mut *mut TypeBridgeProjectedBatchResult,
            diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
        ) -> TypeBridgeStatus {
            // SAFETY: shared implementation fences the complete retained IID table and outputs.
            unsafe {
                delete_returning(
                    RawTarget::$target(owner),
                    model,
                    iids,
                    count,
                    limits,
                    cancellation,
                    out,
                    diagnostics,
                )
            }
        }
    };
}
returning!(
    TypeBridgeDatabase,
    Database,
    type_bridge_database_manager_delete_returning,
    type_bridge_database_manager_delete_many_returning
);
returning!(
    TypeBridgeWriteTransaction,
    Write,
    type_bridge_write_transaction_manager_delete_returning,
    type_bridge_write_transaction_manager_delete_many_returning
);
returning!(
    TypeBridgeProxyConnection,
    Proxy,
    type_bridge_proxy_connection_manager_delete_returning,
    type_bridge_proxy_connection_manager_delete_many_returning
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::allocation::inject_failure;
    use crate::entity_crud::tests::{
        COMMIT_SUCCESS, close_diagnostics, diagnostic, documents, fixture, relation_fixture,
    };
    use crate::projected_batch::{
        type_bridge_projected_batch_result_close_impl,
        type_bridge_projected_batch_result_count_impl,
    };
    use std::{ptr, sync::atomic::Ordering};

    fn snapshot(relation: bool) -> serde_json::Value {
        if relation {
            return serde_json::json!({"ordinal": 0, "_iid": "0x10", "_type": "membership",
                "attributes": {}, "role_players": [{"role": "member", "iid": "0x20", "type_name": "person", "attributes": {"identifier": [{"value": "player"}]}}]});
        }
        serde_json::json!({"ordinal": 0, "_iid": "0x10", "_type": "person",
            "attributes": {"identifier": [{"value": "ada"}]}, "role_players": []})
    }

    #[test]
    fn snapshots_map_before_commit_and_empty_iids_skip_dispatch() {
        for relation in [false, true] {
            for borrowed in [false, true] {
                for present in [false, true] {
                    let responses = vec![
                        documents(if present {
                            vec![snapshot(relation)]
                        } else {
                            vec![]
                        }),
                        documents(vec![]),
                    ];
                    let (database, token, state) = if relation {
                        let fixture = relation_fixture(responses, COMMIT_SUCCESS);
                        (fixture.database, fixture.token, fixture.state)
                    } else {
                        let fixture = fixture(responses, COMMIT_SUCCESS);
                        (fixture.database, fixture.token, fixture.state)
                    };
                    let mut transaction = if borrowed {
                        Box::into_raw(Box::new(TypeBridgeWriteTransaction::from_test_context(
                            &database,
                            database
                                .open_transaction_context(type_bridge_orm::TxType::Write)
                                .unwrap(),
                        )))
                    } else {
                        ptr::null_mut()
                    };
                    let target = if borrowed {
                        RawTarget::Write(transaction)
                    } else {
                        RawTarget::Database(&*database)
                    };
                    let iid = TypeBridgeIid::new("0x10").unwrap();
                    let missing = TypeBridgeIid::new("0xff").unwrap();
                    let mut result = ptr::null_mut();
                    let mut diagnostics = ptr::null_mut();
                    assert_eq!(
                        unsafe {
                            delete_returning(
                                target,
                                &token,
                                [&iid as *const _, &missing as *const _].as_ptr(),
                                2,
                                ptr::null(),
                                ptr::null(),
                                &mut result,
                                &mut diagnostics,
                            )
                        },
                        TypeBridgeStatus::Ok,
                        "relation={relation} borrowed={borrowed} present={present}: {:?}",
                        (!diagnostics.is_null()).then(|| diagnostic(diagnostics))
                    );
                    let mut count = usize::MAX;
                    assert_eq!(
                        unsafe {
                            type_bridge_projected_batch_result_count_impl(
                                result,
                                &token,
                                4,
                                &mut count,
                                &mut diagnostics,
                            )
                        },
                        TypeBridgeStatus::Ok
                    );
                    assert_eq!(count, usize::from(present));
                    if present {
                        assert!(unsafe { &*result }.test_item().is_deleted());
                    }
                    assert_eq!(
                        unsafe { type_bridge_projected_batch_result_close_impl(&mut result) },
                        TypeBridgeStatus::Ok
                    );
                    assert_eq!(
                        state.commits.load(Ordering::Acquire),
                        usize::from(!borrowed)
                    );
                    let queries = state.query_count();
                    assert_eq!(
                        unsafe {
                            delete_returning(
                                target,
                                &token,
                                ptr::null(),
                                0,
                                ptr::null(),
                                ptr::null(),
                                &mut result,
                                &mut diagnostics,
                            )
                        },
                        TypeBridgeStatus::Ok
                    );
                    assert_eq!(state.query_count(), queries);
                    assert_eq!(
                        unsafe {
                            type_bridge_projected_batch_result_count_impl(
                                result,
                                &token,
                                4,
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
                    }
                }
            }
        }
    }

    #[test]
    fn result_allocation_failure_rolls_back_and_alias_refusal_preserves_inputs() {
        let fixture = fixture(
            vec![documents(vec![snapshot(false)]), documents(vec![])],
            COMMIT_SUCCESS,
        );
        let iid = TypeBridgeIid::new("0x10").unwrap();
        let mut iids = [&iid as *const TypeBridgeIid];
        let mut diagnostics = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_database_manager_delete_many_returning(
                    &*fixture.database,
                    &fixture.token,
                    iids.as_ptr(),
                    1,
                    ptr::null(),
                    ptr::null(),
                    iids.as_mut_ptr().cast(),
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(iids[0], &iid as *const _);
        assert!(diagnostics.is_null());
        assert_eq!(fixture.state.query_count(), 0);
        let mut result = ptr::dangling_mut();
        {
            let _failure = inject_failure(AllocationSite::ProjectedBatchResultThingStorage, 0);
            assert_eq!(
                unsafe {
                    type_bridge_database_manager_delete_returning(
                        &*fixture.database,
                        &fixture.token,
                        &iid,
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
        assert_eq!(fixture.state.commits.load(Ordering::Acquire), 0);
        assert_eq!(fixture.state.rollbacks.load(Ordering::Acquire), 1);
        close_diagnostics(&mut diagnostics);
    }
}
