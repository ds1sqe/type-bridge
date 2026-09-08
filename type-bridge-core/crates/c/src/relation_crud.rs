//! Generated-only exact relation CRUD exports.

use crate::abi::{TypeBridgeByteView, TypeBridgeStatus};
use crate::execution_diagnostic::TypeBridgeExecutionDiagnostics;
use crate::projected_model::{TypeBridgeProjectedCreate, TypeBridgeProjectedThing};
use crate::projected_token::TypeBridgeProjectedTokenV1;
use crate::runtime::{
    TypeBridgeCancellation, TypeBridgeDatabase, TypeBridgeReadTransaction,
    TypeBridgeWriteTransaction,
};
use crate::thing_crud::{
    CrudKind, LegacyPolicyTarget, MemoryRange, ReadTarget, WriteTarget,
    prepare_count_outputs_for_legacy_policy_target,
    prepare_diagnostics_output_for_legacy_policy_target,
    prepare_thing_outputs_for_legacy_policy_target, run_count_call, run_delete_call, run_get_call,
    run_put_call, run_write_thing_call,
};

/// Insert one exact projected relation using one owned write transaction and commit.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_database_relation_insert(
    database: *const TypeBridgeDatabase,
    model: *const TypeBridgeProjectedTokenV1,
    create: *const TypeBridgeProjectedCreate,
    cancellation: *const TypeBridgeCancellation,
    out_thing: *mut *mut TypeBridgeProjectedThing,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: all caller input ranges are described before any output write.
    let prepared = match unsafe {
        prepare_thing_outputs_for_legacy_policy_target(
            LegacyPolicyTarget::Database(database),
            &[
                MemoryRange::of_object(database),
                MemoryRange::of_object(model),
                MemoryRange::of_object(create),
                MemoryRange::of_object(cancellation),
            ],
            out_thing,
            out_diagnostics,
        )
    } {
        Ok(value) => value,
        Err(status) => return status,
    };
    if database.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: caller retains the database handle and all delegated inputs for this call.
    unsafe {
        run_write_thing_call(
            CrudKind::Relation,
            WriteTarget::Database(&*database),
            prepared,
            model,
            create,
            None,
            cancellation,
        )
    }
}

/// Put one exact projected relation using one owned write transaction and commit.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_database_relation_put(
    database: *const TypeBridgeDatabase,
    model: *const TypeBridgeProjectedTokenV1,
    create: *const TypeBridgeProjectedCreate,
    cancellation: *const TypeBridgeCancellation,
    out_thing: *mut *mut TypeBridgeProjectedThing,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: all caller input ranges are described before any output write.
    let prepared = match unsafe {
        prepare_thing_outputs_for_legacy_policy_target(
            LegacyPolicyTarget::Database(database),
            &[
                MemoryRange::of_object(database),
                MemoryRange::of_object(model),
                MemoryRange::of_object(create),
                MemoryRange::of_object(cancellation),
            ],
            out_thing,
            out_diagnostics,
        )
    } {
        Ok(value) => value,
        Err(status) => return status,
    };
    if database.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: caller retains the database handle and all delegated inputs for this call.
    unsafe {
        run_put_call(
            CrudKind::Relation,
            WriteTarget::Database(&*database),
            prepared,
            model,
            create,
            cancellation,
        )
    }
}

/// Read one exact projected relation by IID using one owned read transaction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_database_relation_get_by_iid(
    database: *const TypeBridgeDatabase,
    model: *const TypeBridgeProjectedTokenV1,
    iid: TypeBridgeByteView,
    cancellation: *const TypeBridgeCancellation,
    out_thing: *mut *mut TypeBridgeProjectedThing,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: all caller input ranges are described before any output write.
    let prepared = match unsafe {
        prepare_thing_outputs_for_legacy_policy_target(
            LegacyPolicyTarget::Database(database),
            &[
                MemoryRange::of_object(database),
                MemoryRange::of_object(model),
                MemoryRange::of_bytes(iid),
                MemoryRange::of_object(cancellation),
            ],
            out_thing,
            out_diagnostics,
        )
    } {
        Ok(value) => value,
        Err(status) => return status,
    };
    if database.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: caller retains the database handle and all delegated inputs for this call.
    unsafe {
        run_get_call(
            CrudKind::Relation,
            ReadTarget::Database(&*database),
            prepared,
            model,
            iid,
            cancellation,
        )
    }
}

/// Replace and rehydrate one exact projected relation, then commit once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_database_relation_update(
    database: *const TypeBridgeDatabase,
    model: *const TypeBridgeProjectedTokenV1,
    iid: TypeBridgeByteView,
    create: *const TypeBridgeProjectedCreate,
    cancellation: *const TypeBridgeCancellation,
    out_thing: *mut *mut TypeBridgeProjectedThing,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: all caller input ranges are described before any output write.
    let prepared = match unsafe {
        prepare_thing_outputs_for_legacy_policy_target(
            LegacyPolicyTarget::Database(database),
            &[
                MemoryRange::of_object(database),
                MemoryRange::of_object(model),
                MemoryRange::of_bytes(iid),
                MemoryRange::of_object(create),
                MemoryRange::of_object(cancellation),
            ],
            out_thing,
            out_diagnostics,
        )
    } {
        Ok(value) => value,
        Err(status) => return status,
    };
    if database.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: caller retains the database handle and all delegated inputs for this call.
    unsafe {
        run_write_thing_call(
            CrudKind::Relation,
            WriteTarget::Database(&*database),
            prepared,
            model,
            create,
            Some(iid),
            cancellation,
        )
    }
}

/// Blind-delete one exact projected relation by IID and commit once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_database_relation_delete_by_iid(
    database: *const TypeBridgeDatabase,
    model: *const TypeBridgeProjectedTokenV1,
    iid: TypeBridgeByteView,
    cancellation: *const TypeBridgeCancellation,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: all caller input ranges and the complete target are retained until preparation.
    let prepared = match unsafe {
        prepare_diagnostics_output_for_legacy_policy_target(
            LegacyPolicyTarget::Database(database),
            &[
                MemoryRange::of_object(database),
                MemoryRange::of_object(model),
                MemoryRange::of_bytes(iid),
                MemoryRange::of_object(cancellation),
            ],
            out_diagnostics,
        )
    } {
        Ok(value) => value,
        Err(status) => return status,
    };
    if database.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: caller retains the database handle and all delegated inputs for this call.
    unsafe {
        run_delete_call(
            CrudKind::Relation,
            WriteTarget::Database(&*database),
            prepared,
            model,
            iid,
            cancellation,
        )
    }
}

/// Count exact projected relation instances using one owned read transaction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_database_relation_count(
    database: *const TypeBridgeDatabase,
    model: *const TypeBridgeProjectedTokenV1,
    cancellation: *const TypeBridgeCancellation,
    out_count: *mut u64,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: all caller input ranges and the complete target are retained until preparation.
    let prepared = match unsafe {
        prepare_count_outputs_for_legacy_policy_target(
            LegacyPolicyTarget::Database(database),
            &[
                MemoryRange::of_object(database),
                MemoryRange::of_object(model),
                MemoryRange::of_object(cancellation),
            ],
            out_count,
            out_diagnostics,
        )
    } {
        Ok(value) => value,
        Err(status) => return status,
    };
    if database.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: caller retains the database handle and all delegated inputs for this call.
    unsafe {
        run_count_call(
            CrudKind::Relation,
            ReadTarget::Database(&*database),
            prepared,
            model,
            cancellation,
        )
    }
}

/// Read one exact relation without consuming the caller's read transaction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_read_transaction_relation_get_by_iid(
    transaction: *const TypeBridgeReadTransaction,
    model: *const TypeBridgeProjectedTokenV1,
    iid: TypeBridgeByteView,
    cancellation: *const TypeBridgeCancellation,
    out_thing: *mut *mut TypeBridgeProjectedThing,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: all caller input ranges are described before any output write.
    let prepared = match unsafe {
        prepare_thing_outputs_for_legacy_policy_target(
            LegacyPolicyTarget::ReadTransaction(transaction),
            &[
                MemoryRange::of_object(transaction),
                MemoryRange::of_object(model),
                MemoryRange::of_bytes(iid),
                MemoryRange::of_object(cancellation),
            ],
            out_thing,
            out_diagnostics,
        )
    } {
        Ok(value) => value,
        Err(status) => return status,
    };
    if transaction.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: caller provides serialized access to the live transaction handle.
    unsafe {
        run_get_call(
            CrudKind::Relation,
            ReadTarget::ReadTransaction(&*transaction),
            prepared,
            model,
            iid,
            cancellation,
        )
    }
}

/// Count exact entities without consuming the caller's read transaction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_read_transaction_relation_count(
    transaction: *const TypeBridgeReadTransaction,
    model: *const TypeBridgeProjectedTokenV1,
    cancellation: *const TypeBridgeCancellation,
    out_count: *mut u64,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: all caller input ranges and the complete target are retained until preparation.
    let prepared = match unsafe {
        prepare_count_outputs_for_legacy_policy_target(
            LegacyPolicyTarget::ReadTransaction(transaction),
            &[
                MemoryRange::of_object(transaction),
                MemoryRange::of_object(model),
                MemoryRange::of_object(cancellation),
            ],
            out_count,
            out_diagnostics,
        )
    } {
        Ok(value) => value,
        Err(status) => return status,
    };
    if transaction.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: caller provides serialized access to the live transaction handle.
    unsafe {
        run_count_call(
            CrudKind::Relation,
            ReadTarget::ReadTransaction(&*transaction),
            prepared,
            model,
            cancellation,
        )
    }
}

macro_rules! invoke_write_transaction_create {
    (insert, $target:expr, $prepared:expr, $model:expr, $create:expr, $cancellation:expr, $out_thing:expr, $out_diagnostics:expr $(,)?) => {
        run_write_thing_call(
            CrudKind::Relation,
            $target,
            $prepared,
            $model,
            $create,
            None,
            $cancellation,
        )
    };
    (put, $target:expr, $prepared:expr, $model:expr, $create:expr, $cancellation:expr, $out_thing:expr, $out_diagnostics:expr $(,)?) => {
        run_put_call(
            CrudKind::Relation,
            $target,
            $prepared,
            $model,
            $create,
            $cancellation,
        )
    };
}

macro_rules! write_transaction_create_operation {
    ($name:ident, $operation:ident) => {
        #[doc = "Execute one exact relation mutation without terminally consuming the write transaction."]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(
            transaction: *const TypeBridgeWriteTransaction,
            model: *const TypeBridgeProjectedTokenV1,
            create: *const TypeBridgeProjectedCreate,
            cancellation: *const TypeBridgeCancellation,
            out_thing: *mut *mut TypeBridgeProjectedThing,
            out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
        ) -> TypeBridgeStatus {
            // SAFETY: all caller input ranges are described before any output write.
            let prepared = match unsafe {
                prepare_thing_outputs_for_legacy_policy_target(
                    LegacyPolicyTarget::WriteTransaction(transaction),
                    &[
                        MemoryRange::of_object(transaction),
                        MemoryRange::of_object(model),
                        MemoryRange::of_object(create),
                        MemoryRange::of_object(cancellation),
                    ],
                    out_thing,
                    out_diagnostics,
                )
            } {
                Ok(value) => value,
                Err(status) => return status,
            };
            if transaction.is_null() {
                return TypeBridgeStatus::InvalidArgument;
            }
            // SAFETY: caller provides serialized access to the live transaction handle.
            unsafe {
                invoke_write_transaction_create!(
                    $operation,
                    WriteTarget::Transaction(&*transaction),
                    prepared,
                    model,
                    create,
                    cancellation,
                    out_thing,
                    out_diagnostics,
                )
            }
        }
    };
}

write_transaction_create_operation!(type_bridge_write_transaction_relation_insert, insert);
write_transaction_create_operation!(type_bridge_write_transaction_relation_put, put);

/// Update one exact relation without consuming the caller's write transaction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_write_transaction_relation_update(
    transaction: *const TypeBridgeWriteTransaction,
    model: *const TypeBridgeProjectedTokenV1,
    iid: TypeBridgeByteView,
    create: *const TypeBridgeProjectedCreate,
    cancellation: *const TypeBridgeCancellation,
    out_thing: *mut *mut TypeBridgeProjectedThing,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: all caller input ranges are described before any output write.
    let prepared = match unsafe {
        prepare_thing_outputs_for_legacy_policy_target(
            LegacyPolicyTarget::WriteTransaction(transaction),
            &[
                MemoryRange::of_object(transaction),
                MemoryRange::of_object(model),
                MemoryRange::of_bytes(iid),
                MemoryRange::of_object(create),
                MemoryRange::of_object(cancellation),
            ],
            out_thing,
            out_diagnostics,
        )
    } {
        Ok(value) => value,
        Err(status) => return status,
    };
    if transaction.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: caller provides serialized access to the live transaction handle.
    unsafe {
        run_write_thing_call(
            CrudKind::Relation,
            WriteTarget::Transaction(&*transaction),
            prepared,
            model,
            create,
            Some(iid),
            cancellation,
        )
    }
}

/// Read one exact relation without consuming the caller's write transaction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_write_transaction_relation_get_by_iid(
    transaction: *const TypeBridgeWriteTransaction,
    model: *const TypeBridgeProjectedTokenV1,
    iid: TypeBridgeByteView,
    cancellation: *const TypeBridgeCancellation,
    out_thing: *mut *mut TypeBridgeProjectedThing,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: all caller input ranges are described before any output write.
    let prepared = match unsafe {
        prepare_thing_outputs_for_legacy_policy_target(
            LegacyPolicyTarget::WriteTransaction(transaction),
            &[
                MemoryRange::of_object(transaction),
                MemoryRange::of_object(model),
                MemoryRange::of_bytes(iid),
                MemoryRange::of_object(cancellation),
            ],
            out_thing,
            out_diagnostics,
        )
    } {
        Ok(value) => value,
        Err(status) => return status,
    };
    if transaction.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: caller provides serialized access to the live transaction handle.
    unsafe {
        run_get_call(
            CrudKind::Relation,
            ReadTarget::WriteTransaction(&*transaction),
            prepared,
            model,
            iid,
            cancellation,
        )
    }
}

/// Blind-delete one relation without consuming the caller's write transaction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_write_transaction_relation_delete_by_iid(
    transaction: *const TypeBridgeWriteTransaction,
    model: *const TypeBridgeProjectedTokenV1,
    iid: TypeBridgeByteView,
    cancellation: *const TypeBridgeCancellation,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: all caller input ranges and the complete target are retained until preparation.
    let prepared = match unsafe {
        prepare_diagnostics_output_for_legacy_policy_target(
            LegacyPolicyTarget::WriteTransaction(transaction),
            &[
                MemoryRange::of_object(transaction),
                MemoryRange::of_object(model),
                MemoryRange::of_bytes(iid),
                MemoryRange::of_object(cancellation),
            ],
            out_diagnostics,
        )
    } {
        Ok(value) => value,
        Err(status) => return status,
    };
    if transaction.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: caller provides serialized access to the live transaction handle.
    unsafe {
        run_delete_call(
            CrudKind::Relation,
            WriteTarget::Transaction(&*transaction),
            prepared,
            model,
            iid,
            cancellation,
        )
    }
}

/// Count exact entities without consuming the caller's write transaction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_write_transaction_relation_count(
    transaction: *const TypeBridgeWriteTransaction,
    model: *const TypeBridgeProjectedTokenV1,
    cancellation: *const TypeBridgeCancellation,
    out_count: *mut u64,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: all caller input ranges and the complete target are retained until preparation.
    let prepared = match unsafe {
        prepare_count_outputs_for_legacy_policy_target(
            LegacyPolicyTarget::WriteTransaction(transaction),
            &[
                MemoryRange::of_object(transaction),
                MemoryRange::of_object(model),
                MemoryRange::of_object(cancellation),
            ],
            out_count,
            out_diagnostics,
        )
    } {
        Ok(value) => value,
        Err(status) => return status,
    };
    if transaction.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: caller provides serialized access to the live transaction handle.
    unsafe {
        run_count_call(
            CrudKind::Relation,
            ReadTarget::WriteTransaction(&*transaction),
            prepared,
            model,
            cancellation,
        )
    }
}
