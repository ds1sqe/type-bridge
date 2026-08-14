//! Private policy-aware C CRUD implementations activated atomically by ABI 1.4.

use std::{mem::size_of, ptr};

use crate::abi::{TypeBridgeByteView, TypeBridgeStatus};
use crate::execution_diagnostic::TypeBridgeExecutionDiagnostics;
use crate::generated_preflight::{
    DirectOutputPreflight, GENERATED_INPUT_CANCELLATION, GENERATED_INPUT_DATABASE,
    GENERATED_INPUT_PROJECTED_CREATE, GENERATED_INPUT_PROJECTED_TOKEN,
    GENERATED_INPUT_READ_TRANSACTION, GENERATED_INPUT_WRITE_TRANSACTION, direct_output_preflight,
};
use crate::projected_model::{TypeBridgeProjectedCreate, TypeBridgeProjectedThing};
use crate::projected_token::TypeBridgeProjectedTokenV1;
use crate::query::TypeBridgeQueryExecutionLimitsV1;
use crate::runtime::{
    TypeBridgeCancellation, TypeBridgeDatabase, TypeBridgeReadTransaction,
    TypeBridgeWriteTransaction, check_database_borrowed_ranges,
    check_read_transaction_borrowed_ranges, check_write_transaction_borrowed_ranges,
};
use crate::thing_crud::{
    CrudKind, MemoryRange, ReadTarget, WriteTarget, prepare_count_outputs,
    prepare_diagnostics_output, prepare_thing_outputs, run_count_call_controlled,
    run_delete_call_controlled, run_get_call_controlled, run_put_call_controlled,
    run_write_thing_call_controlled,
};

#[derive(Clone, Copy)]
enum CreateOperation {
    Insert,
    Put,
}

#[derive(Clone, Copy)]
enum RawWriteTarget {
    Database(*const TypeBridgeDatabase),
    Transaction(*const TypeBridgeWriteTransaction),
}

impl RawWriteTarget {
    fn memory_range(self) -> Option<MemoryRange> {
        match self {
            Self::Database(value) => MemoryRange::of_object(value),
            Self::Transaction(value) => MemoryRange::of_object(value),
        }
    }

    fn is_null(self) -> bool {
        match self {
            Self::Database(value) => value.is_null(),
            Self::Transaction(value) => value.is_null(),
        }
    }

    unsafe fn check_ranges(
        self,
        preflight: &DirectOutputPreflight,
    ) -> Result<(), TypeBridgeStatus> {
        match self {
            Self::Database(value) => {
                preflight.check_object_kind(GENERATED_INPUT_DATABASE, value.cast())?;
                if !value.is_null() {
                    // SAFETY: the complete outer database handle was fenced above.
                    check_database_borrowed_ranges(preflight, unsafe { &*value })?;
                }
            }
            Self::Transaction(value) => {
                preflight.check_object_kind(GENERATED_INPUT_WRITE_TRANSACTION, value.cast())?;
                if !value.is_null() {
                    // SAFETY: the complete outer transaction handle was fenced above.
                    check_write_transaction_borrowed_ranges(preflight, unsafe { &*value })?;
                }
            }
        }
        Ok(())
    }

    unsafe fn resolved<'handle>(self) -> WriteTarget<'handle> {
        match self {
            Self::Database(value) => {
                // SAFETY: the caller checked this complete non-null handle.
                WriteTarget::Database(unsafe { &*value })
            }
            Self::Transaction(value) => {
                // SAFETY: the caller checked this complete non-null handle.
                WriteTarget::Transaction(unsafe { &*value })
            }
        }
    }
}

#[derive(Clone, Copy)]
enum RawReadTarget {
    Database(*const TypeBridgeDatabase),
    ReadTransaction(*const TypeBridgeReadTransaction),
    WriteTransaction(*const TypeBridgeWriteTransaction),
}

impl RawReadTarget {
    fn memory_range(self) -> Option<MemoryRange> {
        match self {
            Self::Database(value) => MemoryRange::of_object(value),
            Self::ReadTransaction(value) => MemoryRange::of_object(value),
            Self::WriteTransaction(value) => MemoryRange::of_object(value),
        }
    }

    fn is_null(self) -> bool {
        match self {
            Self::Database(value) => value.is_null(),
            Self::ReadTransaction(value) => value.is_null(),
            Self::WriteTransaction(value) => value.is_null(),
        }
    }

    unsafe fn check_ranges(
        self,
        preflight: &DirectOutputPreflight,
    ) -> Result<(), TypeBridgeStatus> {
        match self {
            Self::Database(value) => {
                preflight.check_object_kind(GENERATED_INPUT_DATABASE, value.cast())?;
                if !value.is_null() {
                    // SAFETY: the complete outer database handle was fenced above.
                    check_database_borrowed_ranges(preflight, unsafe { &*value })?;
                }
            }
            Self::ReadTransaction(value) => {
                preflight.check_object_kind(GENERATED_INPUT_READ_TRANSACTION, value.cast())?;
                if !value.is_null() {
                    // SAFETY: the complete outer transaction handle was fenced above.
                    check_read_transaction_borrowed_ranges(preflight, unsafe { &*value })?;
                }
            }
            Self::WriteTransaction(value) => {
                preflight.check_object_kind(GENERATED_INPUT_WRITE_TRANSACTION, value.cast())?;
                if !value.is_null() {
                    // SAFETY: the complete outer transaction handle was fenced above.
                    check_write_transaction_borrowed_ranges(preflight, unsafe { &*value })?;
                }
            }
        }
        Ok(())
    }

    unsafe fn resolved<'handle>(self) -> ReadTarget<'handle> {
        match self {
            Self::Database(value) => {
                // SAFETY: the caller checked this complete non-null handle.
                ReadTarget::Database(unsafe { &*value })
            }
            Self::ReadTransaction(value) => {
                // SAFETY: the caller checked this complete non-null handle.
                ReadTarget::ReadTransaction(unsafe { &*value })
            }
            Self::WriteTransaction(value) => {
                // SAFETY: the caller checked this complete non-null handle.
                ReadTarget::WriteTransaction(unsafe { &*value })
            }
        }
    }
}

fn check_optional_object<T>(
    preflight: &DirectOutputPreflight,
    value: *const T,
) -> Result<(), TypeBridgeStatus> {
    if value.is_null() {
        Ok(())
    } else {
        preflight.check_bytes(value.cast(), size_of::<T>())
    }
}

unsafe fn check_crud_inputs(
    preflight: &DirectOutputPreflight,
    model: *const TypeBridgeProjectedTokenV1,
    create: *const TypeBridgeProjectedCreate,
    iid: Option<TypeBridgeByteView>,
    limits: *const TypeBridgeQueryExecutionLimitsV1,
    cancellation: *const TypeBridgeCancellation,
) -> Result<(), TypeBridgeStatus> {
    preflight.check_object_kind(GENERATED_INPUT_PROJECTED_TOKEN, model.cast())?;
    preflight.check_object_kind(GENERATED_INPUT_PROJECTED_CREATE, create.cast())?;
    if !create.is_null() {
        // SAFETY: the complete outer create handle was fenced above.
        unsafe { &*create }.check_borrowed_ranges(preflight)?;
    }
    if let Some(iid) = iid {
        preflight.check_bytes(iid.data.cast(), iid.length)?;
    }
    check_optional_object(preflight, limits)?;
    preflight.check_object_kind(GENERATED_INPUT_CANCELLATION, cancellation.cast())
}

fn thing_output_preflight(
    out_thing: *mut *mut TypeBridgeProjectedThing,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> Result<DirectOutputPreflight, TypeBridgeStatus> {
    direct_output_preflight(&[
        (out_thing.cast(), size_of::<*mut TypeBridgeProjectedThing>()),
        (
            out_diagnostics.cast(),
            size_of::<*mut TypeBridgeExecutionDiagnostics>(),
        ),
    ])
}

fn diagnostics_output_preflight(
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> Result<DirectOutputPreflight, TypeBridgeStatus> {
    direct_output_preflight(&[(
        out_diagnostics.cast(),
        size_of::<*mut TypeBridgeExecutionDiagnostics>(),
    )])
}

fn count_output_preflight(
    out_count: *mut u64,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> Result<DirectOutputPreflight, TypeBridgeStatus> {
    direct_output_preflight(&[
        (out_count.cast(), size_of::<u64>()),
        (
            out_diagnostics.cast(),
            size_of::<*mut TypeBridgeExecutionDiagnostics>(),
        ),
    ])
}

#[allow(clippy::too_many_arguments)]
unsafe fn create_impl(
    kind: CrudKind,
    operation: CreateOperation,
    target: RawWriteTarget,
    model: *const TypeBridgeProjectedTokenV1,
    create: *const TypeBridgeProjectedCreate,
    limits: *const TypeBridgeQueryExecutionLimitsV1,
    cancellation: *const TypeBridgeCancellation,
    out_thing: *mut *mut TypeBridgeProjectedThing,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match thing_output_preflight(out_thing, out_diagnostics) {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: the C caller retains every complete input through this call.
    if let Err(status) = unsafe { target.check_ranges(&preflight) } {
        return status;
    }
    // SAFETY: outer handles are fenced before any nested borrowed-range walk.
    if let Err(status) =
        unsafe { check_crud_inputs(&preflight, model, create, None, limits, cancellation) }
    {
        return status;
    }
    // SAFETY: all caller input ranges are described before any output write.
    let prepared = match unsafe {
        prepare_thing_outputs(
            &[
                target.memory_range(),
                MemoryRange::of_object(model),
                MemoryRange::of_object(create),
                MemoryRange::of_object(limits),
                MemoryRange::of_object(cancellation),
            ],
            out_thing,
            out_diagnostics,
        )
    } {
        Ok(value) => value,
        Err(status) => return status,
    };
    if target.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: preflight fenced every complete active input, and the caller
    // retains the target and immutable arguments throughout this call.
    let target = unsafe { target.resolved() };
    match operation {
        CreateOperation::Insert => {
            // SAFETY: the shared controlled implementation owns all snapshots.
            unsafe {
                run_write_thing_call_controlled(
                    kind,
                    target,
                    prepared,
                    model,
                    create,
                    None,
                    limits,
                    cancellation,
                )
            }
        }
        CreateOperation::Put => {
            // SAFETY: the shared controlled implementation owns all snapshots.
            unsafe {
                run_put_call_controlled(kind, target, prepared, model, create, limits, cancellation)
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn update_impl(
    kind: CrudKind,
    target: RawWriteTarget,
    model: *const TypeBridgeProjectedTokenV1,
    iid: TypeBridgeByteView,
    create: *const TypeBridgeProjectedCreate,
    limits: *const TypeBridgeQueryExecutionLimitsV1,
    cancellation: *const TypeBridgeCancellation,
    out_thing: *mut *mut TypeBridgeProjectedThing,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match thing_output_preflight(out_thing, out_diagnostics) {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: the C caller retains every complete input through this call.
    if let Err(status) = unsafe { target.check_ranges(&preflight) } {
        return status;
    }
    // SAFETY: outer handles are fenced before any nested borrowed-range walk.
    if let Err(status) =
        unsafe { check_crud_inputs(&preflight, model, create, Some(iid), limits, cancellation) }
    {
        return status;
    }
    // SAFETY: all caller input ranges are described before any output write.
    let prepared = match unsafe {
        prepare_thing_outputs(
            &[
                target.memory_range(),
                MemoryRange::of_object(model),
                MemoryRange::of_bytes(iid),
                MemoryRange::of_object(create),
                MemoryRange::of_object(limits),
                MemoryRange::of_object(cancellation),
            ],
            out_thing,
            out_diagnostics,
        )
    } {
        Ok(value) => value,
        Err(status) => return status,
    };
    if target.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: preflight fenced every complete active input for this call.
    unsafe {
        run_write_thing_call_controlled(
            kind,
            target.resolved(),
            prepared,
            model,
            create,
            Some(iid),
            limits,
            cancellation,
        )
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn get_impl(
    kind: CrudKind,
    target: RawReadTarget,
    model: *const TypeBridgeProjectedTokenV1,
    iid: TypeBridgeByteView,
    limits: *const TypeBridgeQueryExecutionLimitsV1,
    cancellation: *const TypeBridgeCancellation,
    out_thing: *mut *mut TypeBridgeProjectedThing,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match thing_output_preflight(out_thing, out_diagnostics) {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: the C caller retains every complete input through this call.
    if let Err(status) = unsafe { target.check_ranges(&preflight) } {
        return status;
    }
    // SAFETY: outer handles are fenced before any nested borrowed-range walk.
    if let Err(status) = unsafe {
        check_crud_inputs(
            &preflight,
            model,
            ptr::null(),
            Some(iid),
            limits,
            cancellation,
        )
    } {
        return status;
    }
    // SAFETY: all caller input ranges are described before any output write.
    let prepared = match unsafe {
        prepare_thing_outputs(
            &[
                target.memory_range(),
                MemoryRange::of_object(model),
                MemoryRange::of_bytes(iid),
                MemoryRange::of_object(limits),
                MemoryRange::of_object(cancellation),
            ],
            out_thing,
            out_diagnostics,
        )
    } {
        Ok(value) => value,
        Err(status) => return status,
    };
    if target.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: preflight fenced every complete active input for this call.
    unsafe {
        run_get_call_controlled(
            kind,
            target.resolved(),
            prepared,
            model,
            iid,
            limits,
            cancellation,
        )
    }
}

unsafe fn delete_impl(
    kind: CrudKind,
    target: RawWriteTarget,
    model: *const TypeBridgeProjectedTokenV1,
    iid: TypeBridgeByteView,
    limits: *const TypeBridgeQueryExecutionLimitsV1,
    cancellation: *const TypeBridgeCancellation,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match diagnostics_output_preflight(out_diagnostics) {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: the C caller retains every complete input through this call.
    if let Err(status) = unsafe { target.check_ranges(&preflight) } {
        return status;
    }
    // SAFETY: outer handles are fenced before any nested borrowed-range walk.
    if let Err(status) = unsafe {
        check_crud_inputs(
            &preflight,
            model,
            ptr::null(),
            Some(iid),
            limits,
            cancellation,
        )
    } {
        return status;
    }
    let prepared = match prepare_diagnostics_output(
        &[
            target.memory_range(),
            MemoryRange::of_object(model),
            MemoryRange::of_bytes(iid),
            MemoryRange::of_object(limits),
            MemoryRange::of_object(cancellation),
        ],
        out_diagnostics,
    ) {
        Ok(value) => value,
        Err(status) => return status,
    };
    if target.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: preflight fenced every complete active input for this call.
    unsafe {
        run_delete_call_controlled(
            kind,
            target.resolved(),
            prepared,
            model,
            iid,
            limits,
            cancellation,
        )
    }
}

unsafe fn count_impl(
    kind: CrudKind,
    target: RawReadTarget,
    model: *const TypeBridgeProjectedTokenV1,
    limits: *const TypeBridgeQueryExecutionLimitsV1,
    cancellation: *const TypeBridgeCancellation,
    out_count: *mut u64,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match count_output_preflight(out_count, out_diagnostics) {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: the C caller retains every complete input through this call.
    if let Err(status) = unsafe { target.check_ranges(&preflight) } {
        return status;
    }
    // SAFETY: outer handles are fenced before any nested borrowed-range walk.
    if let Err(status) =
        unsafe { check_crud_inputs(&preflight, model, ptr::null(), None, limits, cancellation) }
    {
        return status;
    }
    let prepared = match prepare_count_outputs(
        &[
            target.memory_range(),
            MemoryRange::of_object(model),
            MemoryRange::of_object(limits),
            MemoryRange::of_object(cancellation),
        ],
        out_count,
        out_diagnostics,
    ) {
        Ok(value) => value,
        Err(status) => return status,
    };
    if target.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: preflight fenced every complete active input for this call.
    unsafe {
        run_count_call_controlled(
            kind,
            target.resolved(),
            prepared,
            model,
            limits,
            cancellation,
        )
    }
}

macro_rules! define_database_create {
    ($name:ident, $export_name:literal, $kind:expr, $operation:expr) => {
        #[unsafe(export_name = $export_name)]
        pub(crate) unsafe extern "C" fn $name(
            database: *const TypeBridgeDatabase,
            model: *const TypeBridgeProjectedTokenV1,
            create: *const TypeBridgeProjectedCreate,
            limits: *const TypeBridgeQueryExecutionLimitsV1,
            cancellation: *const TypeBridgeCancellation,
            out_thing: *mut *mut TypeBridgeProjectedThing,
            out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
        ) -> TypeBridgeStatus {
            // SAFETY: this exact-prototype implementation delegates complete preflight.
            unsafe {
                create_impl(
                    $kind,
                    $operation,
                    RawWriteTarget::Database(database),
                    model,
                    create,
                    limits,
                    cancellation,
                    out_thing,
                    out_diagnostics,
                )
            }
        }
        const _: unsafe extern "C" fn(
            *const TypeBridgeDatabase,
            *const TypeBridgeProjectedTokenV1,
            *const TypeBridgeProjectedCreate,
            *const TypeBridgeQueryExecutionLimitsV1,
            *const TypeBridgeCancellation,
            *mut *mut TypeBridgeProjectedThing,
            *mut *mut TypeBridgeExecutionDiagnostics,
        ) -> TypeBridgeStatus = $name;
    };
}

macro_rules! define_write_transaction_create {
    ($name:ident, $export_name:literal, $kind:expr, $operation:expr) => {
        #[unsafe(export_name = $export_name)]
        pub(crate) unsafe extern "C" fn $name(
            transaction: *const TypeBridgeWriteTransaction,
            model: *const TypeBridgeProjectedTokenV1,
            create: *const TypeBridgeProjectedCreate,
            limits: *const TypeBridgeQueryExecutionLimitsV1,
            cancellation: *const TypeBridgeCancellation,
            out_thing: *mut *mut TypeBridgeProjectedThing,
            out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
        ) -> TypeBridgeStatus {
            // SAFETY: this exact-prototype implementation delegates complete preflight.
            unsafe {
                create_impl(
                    $kind,
                    $operation,
                    RawWriteTarget::Transaction(transaction),
                    model,
                    create,
                    limits,
                    cancellation,
                    out_thing,
                    out_diagnostics,
                )
            }
        }
        const _: unsafe extern "C" fn(
            *const TypeBridgeWriteTransaction,
            *const TypeBridgeProjectedTokenV1,
            *const TypeBridgeProjectedCreate,
            *const TypeBridgeQueryExecutionLimitsV1,
            *const TypeBridgeCancellation,
            *mut *mut TypeBridgeProjectedThing,
            *mut *mut TypeBridgeExecutionDiagnostics,
        ) -> TypeBridgeStatus = $name;
    };
}

macro_rules! define_update {
    ($name:ident, $export_name:literal, $kind:expr, $target_type:ty, $target:ident) => {
        #[unsafe(export_name = $export_name)]
        pub(crate) unsafe extern "C" fn $name(
            target: *const $target_type,
            model: *const TypeBridgeProjectedTokenV1,
            iid: TypeBridgeByteView,
            create: *const TypeBridgeProjectedCreate,
            limits: *const TypeBridgeQueryExecutionLimitsV1,
            cancellation: *const TypeBridgeCancellation,
            out_thing: *mut *mut TypeBridgeProjectedThing,
            out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
        ) -> TypeBridgeStatus {
            // SAFETY: this exact-prototype implementation delegates complete preflight.
            unsafe {
                update_impl(
                    $kind,
                    RawWriteTarget::$target(target),
                    model,
                    iid,
                    create,
                    limits,
                    cancellation,
                    out_thing,
                    out_diagnostics,
                )
            }
        }
        const _: unsafe extern "C" fn(
            *const $target_type,
            *const TypeBridgeProjectedTokenV1,
            TypeBridgeByteView,
            *const TypeBridgeProjectedCreate,
            *const TypeBridgeQueryExecutionLimitsV1,
            *const TypeBridgeCancellation,
            *mut *mut TypeBridgeProjectedThing,
            *mut *mut TypeBridgeExecutionDiagnostics,
        ) -> TypeBridgeStatus = $name;
    };
}

macro_rules! define_get {
    ($name:ident, $export_name:literal, $kind:expr, $target_type:ty, $target:ident) => {
        #[unsafe(export_name = $export_name)]
        pub(crate) unsafe extern "C" fn $name(
            target: *const $target_type,
            model: *const TypeBridgeProjectedTokenV1,
            iid: TypeBridgeByteView,
            limits: *const TypeBridgeQueryExecutionLimitsV1,
            cancellation: *const TypeBridgeCancellation,
            out_thing: *mut *mut TypeBridgeProjectedThing,
            out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
        ) -> TypeBridgeStatus {
            // SAFETY: this exact-prototype implementation delegates complete preflight.
            unsafe {
                get_impl(
                    $kind,
                    RawReadTarget::$target(target),
                    model,
                    iid,
                    limits,
                    cancellation,
                    out_thing,
                    out_diagnostics,
                )
            }
        }
        const _: unsafe extern "C" fn(
            *const $target_type,
            *const TypeBridgeProjectedTokenV1,
            TypeBridgeByteView,
            *const TypeBridgeQueryExecutionLimitsV1,
            *const TypeBridgeCancellation,
            *mut *mut TypeBridgeProjectedThing,
            *mut *mut TypeBridgeExecutionDiagnostics,
        ) -> TypeBridgeStatus = $name;
    };
}

macro_rules! define_delete {
    ($name:ident, $export_name:literal, $kind:expr, $target_type:ty, $target:ident) => {
        #[unsafe(export_name = $export_name)]
        pub(crate) unsafe extern "C" fn $name(
            target: *const $target_type,
            model: *const TypeBridgeProjectedTokenV1,
            iid: TypeBridgeByteView,
            limits: *const TypeBridgeQueryExecutionLimitsV1,
            cancellation: *const TypeBridgeCancellation,
            out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
        ) -> TypeBridgeStatus {
            // SAFETY: this exact-prototype implementation delegates complete preflight.
            unsafe {
                delete_impl(
                    $kind,
                    RawWriteTarget::$target(target),
                    model,
                    iid,
                    limits,
                    cancellation,
                    out_diagnostics,
                )
            }
        }
        const _: unsafe extern "C" fn(
            *const $target_type,
            *const TypeBridgeProjectedTokenV1,
            TypeBridgeByteView,
            *const TypeBridgeQueryExecutionLimitsV1,
            *const TypeBridgeCancellation,
            *mut *mut TypeBridgeExecutionDiagnostics,
        ) -> TypeBridgeStatus = $name;
    };
}

macro_rules! define_count {
    ($name:ident, $export_name:literal, $kind:expr, $target_type:ty, $target:ident) => {
        #[unsafe(export_name = $export_name)]
        pub(crate) unsafe extern "C" fn $name(
            target: *const $target_type,
            model: *const TypeBridgeProjectedTokenV1,
            limits: *const TypeBridgeQueryExecutionLimitsV1,
            cancellation: *const TypeBridgeCancellation,
            out_count: *mut u64,
            out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
        ) -> TypeBridgeStatus {
            // SAFETY: this exact-prototype implementation delegates complete preflight.
            unsafe {
                count_impl(
                    $kind,
                    RawReadTarget::$target(target),
                    model,
                    limits,
                    cancellation,
                    out_count,
                    out_diagnostics,
                )
            }
        }
        const _: unsafe extern "C" fn(
            *const $target_type,
            *const TypeBridgeProjectedTokenV1,
            *const TypeBridgeQueryExecutionLimitsV1,
            *const TypeBridgeCancellation,
            *mut u64,
            *mut *mut TypeBridgeExecutionDiagnostics,
        ) -> TypeBridgeStatus = $name;
    };
}

define_database_create!(
    type_bridge_database_entity_insert_v2_impl,
    "type_bridge_database_entity_insert_v2",
    CrudKind::Entity,
    CreateOperation::Insert
);
define_database_create!(
    type_bridge_database_entity_put_v2_impl,
    "type_bridge_database_entity_put_v2",
    CrudKind::Entity,
    CreateOperation::Put
);
define_get!(
    type_bridge_database_entity_get_by_iid_v2_impl,
    "type_bridge_database_entity_get_by_iid_v2",
    CrudKind::Entity,
    TypeBridgeDatabase,
    Database
);
define_update!(
    type_bridge_database_entity_update_v2_impl,
    "type_bridge_database_entity_update_v2",
    CrudKind::Entity,
    TypeBridgeDatabase,
    Database
);
define_delete!(
    type_bridge_database_entity_delete_by_iid_v2_impl,
    "type_bridge_database_entity_delete_by_iid_v2",
    CrudKind::Entity,
    TypeBridgeDatabase,
    Database
);
define_count!(
    type_bridge_database_entity_count_v2_impl,
    "type_bridge_database_entity_count_v2",
    CrudKind::Entity,
    TypeBridgeDatabase,
    Database
);
define_get!(
    type_bridge_read_transaction_entity_get_by_iid_v2_impl,
    "type_bridge_read_transaction_entity_get_by_iid_v2",
    CrudKind::Entity,
    TypeBridgeReadTransaction,
    ReadTransaction
);
define_count!(
    type_bridge_read_transaction_entity_count_v2_impl,
    "type_bridge_read_transaction_entity_count_v2",
    CrudKind::Entity,
    TypeBridgeReadTransaction,
    ReadTransaction
);
define_write_transaction_create!(
    type_bridge_write_transaction_entity_insert_v2_impl,
    "type_bridge_write_transaction_entity_insert_v2",
    CrudKind::Entity,
    CreateOperation::Insert
);
define_write_transaction_create!(
    type_bridge_write_transaction_entity_put_v2_impl,
    "type_bridge_write_transaction_entity_put_v2",
    CrudKind::Entity,
    CreateOperation::Put
);
define_get!(
    type_bridge_write_transaction_entity_get_by_iid_v2_impl,
    "type_bridge_write_transaction_entity_get_by_iid_v2",
    CrudKind::Entity,
    TypeBridgeWriteTransaction,
    WriteTransaction
);
define_update!(
    type_bridge_write_transaction_entity_update_v2_impl,
    "type_bridge_write_transaction_entity_update_v2",
    CrudKind::Entity,
    TypeBridgeWriteTransaction,
    Transaction
);
define_delete!(
    type_bridge_write_transaction_entity_delete_by_iid_v2_impl,
    "type_bridge_write_transaction_entity_delete_by_iid_v2",
    CrudKind::Entity,
    TypeBridgeWriteTransaction,
    Transaction
);
define_count!(
    type_bridge_write_transaction_entity_count_v2_impl,
    "type_bridge_write_transaction_entity_count_v2",
    CrudKind::Entity,
    TypeBridgeWriteTransaction,
    WriteTransaction
);

define_database_create!(
    type_bridge_database_relation_insert_v2_impl,
    "type_bridge_database_relation_insert_v2",
    CrudKind::Relation,
    CreateOperation::Insert
);
define_database_create!(
    type_bridge_database_relation_put_v2_impl,
    "type_bridge_database_relation_put_v2",
    CrudKind::Relation,
    CreateOperation::Put
);
define_get!(
    type_bridge_database_relation_get_by_iid_v2_impl,
    "type_bridge_database_relation_get_by_iid_v2",
    CrudKind::Relation,
    TypeBridgeDatabase,
    Database
);
define_update!(
    type_bridge_database_relation_update_v2_impl,
    "type_bridge_database_relation_update_v2",
    CrudKind::Relation,
    TypeBridgeDatabase,
    Database
);
define_delete!(
    type_bridge_database_relation_delete_by_iid_v2_impl,
    "type_bridge_database_relation_delete_by_iid_v2",
    CrudKind::Relation,
    TypeBridgeDatabase,
    Database
);
define_count!(
    type_bridge_database_relation_count_v2_impl,
    "type_bridge_database_relation_count_v2",
    CrudKind::Relation,
    TypeBridgeDatabase,
    Database
);
define_get!(
    type_bridge_read_transaction_relation_get_by_iid_v2_impl,
    "type_bridge_read_transaction_relation_get_by_iid_v2",
    CrudKind::Relation,
    TypeBridgeReadTransaction,
    ReadTransaction
);
define_count!(
    type_bridge_read_transaction_relation_count_v2_impl,
    "type_bridge_read_transaction_relation_count_v2",
    CrudKind::Relation,
    TypeBridgeReadTransaction,
    ReadTransaction
);
define_write_transaction_create!(
    type_bridge_write_transaction_relation_insert_v2_impl,
    "type_bridge_write_transaction_relation_insert_v2",
    CrudKind::Relation,
    CreateOperation::Insert
);
define_write_transaction_create!(
    type_bridge_write_transaction_relation_put_v2_impl,
    "type_bridge_write_transaction_relation_put_v2",
    CrudKind::Relation,
    CreateOperation::Put
);
define_get!(
    type_bridge_write_transaction_relation_get_by_iid_v2_impl,
    "type_bridge_write_transaction_relation_get_by_iid_v2",
    CrudKind::Relation,
    TypeBridgeWriteTransaction,
    WriteTransaction
);
define_update!(
    type_bridge_write_transaction_relation_update_v2_impl,
    "type_bridge_write_transaction_relation_update_v2",
    CrudKind::Relation,
    TypeBridgeWriteTransaction,
    Transaction
);
define_delete!(
    type_bridge_write_transaction_relation_delete_by_iid_v2_impl,
    "type_bridge_write_transaction_relation_delete_by_iid_v2",
    CrudKind::Relation,
    TypeBridgeWriteTransaction,
    Transaction
);
define_count!(
    type_bridge_write_transaction_relation_count_v2_impl,
    "type_bridge_write_transaction_relation_count_v2",
    CrudKind::Relation,
    TypeBridgeWriteTransaction,
    WriteTransaction
);

#[cfg(test)]
mod tests {
    use std::mem::size_of;
    use std::ptr;
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    use type_bridge_orm::QueryExecutionResourceLimits;
    use type_bridge_orm::session::backend::QueryResult;

    use super::*;
    use crate::allocation::{AllocationSite, inject_failure};
    use crate::entity_crud::tests::{
        COMMIT_SUCCESS, FakeState, Response, byte_view, close_diagnostics, close_thing, diagnostic,
        documents, fixture, fixture_with_answer_ceiling, membership_document, person_document,
        relation_fixture, relation_fixture_with_answer_ceiling,
    };
    use crate::entity_crud::{
        type_bridge_database_entity_insert, type_bridge_write_transaction_entity_delete_by_iid,
    };
    use crate::relation_crud::type_bridge_read_transaction_relation_count;
    use crate::runtime::{
        type_bridge_cancellation_close, type_bridge_cancellation_open,
        type_bridge_cancellation_request, type_bridge_database_server_version,
        type_bridge_read_transaction_close, type_bridge_read_transaction_open,
        type_bridge_write_transaction_open, type_bridge_write_transaction_rollback,
    };

    type DatabaseCreate = unsafe extern "C" fn(
        *const TypeBridgeDatabase,
        *const TypeBridgeProjectedTokenV1,
        *const TypeBridgeProjectedCreate,
        *const TypeBridgeQueryExecutionLimitsV1,
        *const TypeBridgeCancellation,
        *mut *mut TypeBridgeProjectedThing,
        *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    type DatabaseGet = unsafe extern "C" fn(
        *const TypeBridgeDatabase,
        *const TypeBridgeProjectedTokenV1,
        TypeBridgeByteView,
        *const TypeBridgeQueryExecutionLimitsV1,
        *const TypeBridgeCancellation,
        *mut *mut TypeBridgeProjectedThing,
        *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    type DatabaseCount = unsafe extern "C" fn(
        *const TypeBridgeDatabase,
        *const TypeBridgeProjectedTokenV1,
        *const TypeBridgeQueryExecutionLimitsV1,
        *const TypeBridgeCancellation,
        *mut u64,
        *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    type ReadGet = unsafe extern "C" fn(
        *const TypeBridgeReadTransaction,
        *const TypeBridgeProjectedTokenV1,
        TypeBridgeByteView,
        *const TypeBridgeQueryExecutionLimitsV1,
        *const TypeBridgeCancellation,
        *mut *mut TypeBridgeProjectedThing,
        *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    type ReadCount = unsafe extern "C" fn(
        *const TypeBridgeReadTransaction,
        *const TypeBridgeProjectedTokenV1,
        *const TypeBridgeQueryExecutionLimitsV1,
        *const TypeBridgeCancellation,
        *mut u64,
        *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    type WriteCreate = unsafe extern "C" fn(
        *const TypeBridgeWriteTransaction,
        *const TypeBridgeProjectedTokenV1,
        *const TypeBridgeProjectedCreate,
        *const TypeBridgeQueryExecutionLimitsV1,
        *const TypeBridgeCancellation,
        *mut *mut TypeBridgeProjectedThing,
        *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    type WriteGet = unsafe extern "C" fn(
        *const TypeBridgeWriteTransaction,
        *const TypeBridgeProjectedTokenV1,
        TypeBridgeByteView,
        *const TypeBridgeQueryExecutionLimitsV1,
        *const TypeBridgeCancellation,
        *mut *mut TypeBridgeProjectedThing,
        *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    type WriteCount = unsafe extern "C" fn(
        *const TypeBridgeWriteTransaction,
        *const TypeBridgeProjectedTokenV1,
        *const TypeBridgeQueryExecutionLimitsV1,
        *const TypeBridgeCancellation,
        *mut u64,
        *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;

    fn limits_descriptor(limits: QueryExecutionResourceLimits) -> TypeBridgeQueryExecutionLimitsV1 {
        TypeBridgeQueryExecutionLimitsV1 {
            struct_size: size_of::<TypeBridgeQueryExecutionLimitsV1>() as u32,
            version: 1,
            timeout_milliseconds: limits.timeout_milliseconds,
            items: limits.items,
            bytes: limits.bytes,
            graph_nodes: limits.graph_nodes,
            attribute_values: limits.attribute_values,
            collection_members: limits.collection_members,
            role_players: limits.role_players,
            statements: limits.statements,
            reserved0: 0,
            reserved: [0; 4],
        }
    }

    fn default_limits() -> TypeBridgeQueryExecutionLimitsV1 {
        limits_descriptor(QueryExecutionResourceLimits::default())
    }

    fn count_response(count: u64) -> Response {
        Response::Result(QueryResult::Rows(vec![
            serde_json::json!({"$count": count}),
        ]))
    }

    fn batch_identity(iid: &str) -> Response {
        documents(vec![serde_json::json!({"ordinal": 0, "iid": iid})])
    }

    fn delayed_batch_identity(iid: &str) -> Response {
        Response::Delayed(QueryResult::Documents(vec![
            serde_json::json!({"ordinal": 0, "iid": iid}),
        ]))
    }

    fn batch_hydration(mut document: serde_json::Value) -> Response {
        let object = document
            .as_object_mut()
            .expect("fixture hydration document is an object");
        object.insert("ordinal".to_owned(), serde_json::json!(0));
        object
            .entry("attributes".to_owned())
            .or_insert_with(|| serde_json::json!({}));
        documents(vec![document])
    }

    fn relation_reference() -> Response {
        documents(vec![serde_json::json!({
            "kind": 1,
            "ordinal": 0,
            "reference_ordinal": 0,
            "iid": "0x10",
            "type": "person"
        })])
    }

    #[allow(clippy::too_many_arguments)]
    fn exercise_database_lane(
        database: &TypeBridgeDatabase,
        token: &TypeBridgeProjectedTokenV1,
        create: &TypeBridgeProjectedCreate,
        iid: &[u8],
        create_call: DatabaseCreate,
        get_call: DatabaseGet,
        count_call: DatabaseCount,
    ) {
        let limits = default_limits();
        let mut thing = ptr::dangling_mut();
        let mut diagnostics = ptr::dangling_mut();
        // SAFETY: the fixture retains all immutable inputs and distinct outputs.
        let status = unsafe {
            create_call(
                database,
                token,
                create,
                &limits,
                ptr::null(),
                &mut thing,
                &mut diagnostics,
            )
        };
        assert_eq!(
            status,
            TypeBridgeStatus::Ok,
            "unexpected create diagnostic for model ordinal {}: {:?}",
            token.ordinal,
            (!diagnostics.is_null()).then(|| diagnostic(diagnostics))
        );
        assert!(!thing.is_null());
        assert!(diagnostics.is_null());
        assert_eq!(
            Arc::as_ptr(&unsafe { &*thing }.value).addr(),
            crate::thing_crud::last_reserved_thing_storage_address(),
            "publication must reuse the Arc storage reserved before dispatch"
        );
        close_thing(&mut thing);

        // SAFETY: the IID view and all handles remain valid for this call.
        assert_eq!(
            unsafe {
                get_call(
                    database,
                    token,
                    byte_view(iid),
                    &limits,
                    ptr::null(),
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert!(!thing.is_null());
        close_thing(&mut thing);

        let mut count = u64::MAX;
        // SAFETY: count and diagnostics are distinct writable output slots.
        assert_eq!(
            unsafe {
                count_call(
                    database,
                    token,
                    &limits,
                    ptr::null(),
                    &mut count,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(count, 1);
        assert!(diagnostics.is_null());
    }

    #[allow(clippy::too_many_arguments)]
    fn exercise_read_lane(
        database: &TypeBridgeDatabase,
        token: &TypeBridgeProjectedTokenV1,
        iid: &[u8],
        get_call: ReadGet,
        count_call: ReadCount,
    ) {
        let limits = default_limits();
        let mut transaction = ptr::null_mut();
        let mut diagnostics = ptr::dangling_mut();
        // SAFETY: both output owner slots are distinct and writable.
        assert_eq!(
            unsafe {
                type_bridge_read_transaction_open(
                    database,
                    ptr::null(),
                    &mut transaction,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        let mut thing = ptr::dangling_mut();
        // SAFETY: the caller retains the live read transaction through this call.
        assert_eq!(
            unsafe {
                get_call(
                    transaction,
                    token,
                    byte_view(iid),
                    &limits,
                    ptr::null(),
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        close_thing(&mut thing);
        let mut count = u64::MAX;
        // SAFETY: the borrowed call retains and does not consume the transaction.
        assert_eq!(
            unsafe {
                count_call(
                    transaction,
                    token,
                    &limits,
                    ptr::null(),
                    &mut count,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(count, 1);
        // SAFETY: close consumes exactly the still-live owner slot.
        assert_eq!(
            unsafe { type_bridge_read_transaction_close(&mut transaction, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn exercise_write_lane(
        database: &TypeBridgeDatabase,
        token: &TypeBridgeProjectedTokenV1,
        create: &TypeBridgeProjectedCreate,
        iid: &[u8],
        create_call: WriteCreate,
        get_call: WriteGet,
        count_call: WriteCount,
    ) {
        let limits = default_limits();
        let mut transaction = ptr::null_mut();
        let mut diagnostics = ptr::dangling_mut();
        // SAFETY: both output owner slots are distinct and writable.
        assert_eq!(
            unsafe {
                type_bridge_write_transaction_open(
                    database,
                    ptr::null(),
                    &mut transaction,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        let mut thing = ptr::dangling_mut();
        // SAFETY: the caller retains the live write transaction and immutable inputs.
        assert_eq!(
            unsafe {
                create_call(
                    transaction,
                    token,
                    create,
                    &limits,
                    ptr::null(),
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        close_thing(&mut thing);
        // SAFETY: the successful mutation left the borrowed transaction active.
        assert_eq!(
            unsafe {
                get_call(
                    transaction,
                    token,
                    byte_view(iid),
                    &limits,
                    ptr::null(),
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        close_thing(&mut thing);
        let mut count = u64::MAX;
        // SAFETY: the count call borrows but does not consume the transaction.
        assert_eq!(
            unsafe {
                count_call(
                    transaction,
                    token,
                    &limits,
                    ptr::null(),
                    &mut count,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(count, 1);
        // SAFETY: rollback is a guaranteed recovery operation over the owner slot.
        assert_eq!(
            unsafe { type_bridge_write_transaction_rollback(&mut transaction, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
    }

    #[test]
    fn entity_relation_database_read_and_write_matrix_is_policy_aware_and_owned() {
        let entity_database = fixture(
            vec![
                batch_identity("0x10"),
                batch_hydration(person_document("0x10")),
                documents(vec![person_document("0x10")]),
                count_response(1),
            ],
            COMMIT_SUCCESS,
        );
        exercise_database_lane(
            &entity_database.database,
            &entity_database.token,
            &entity_database.create,
            b"0x10",
            type_bridge_database_entity_insert_v2_impl,
            type_bridge_database_entity_get_by_iid_v2_impl,
            type_bridge_database_entity_count_v2_impl,
        );
        assert_eq!(entity_database.state.commits.load(Ordering::Acquire), 1);

        let relation_database = relation_fixture(
            vec![
                relation_reference(),
                batch_identity("0x20"),
                batch_hydration(membership_document("0x20")),
                documents(vec![membership_document("0x20")]),
                count_response(1),
            ],
            COMMIT_SUCCESS,
        );
        exercise_database_lane(
            &relation_database.database,
            &relation_database.token,
            &relation_database.create,
            b"0x20",
            type_bridge_database_relation_insert_v2_impl,
            type_bridge_database_relation_get_by_iid_v2_impl,
            type_bridge_database_relation_count_v2_impl,
        );
        assert_eq!(relation_database.state.commits.load(Ordering::Acquire), 1);

        let entity_read = fixture(
            vec![documents(vec![person_document("0x10")]), count_response(1)],
            COMMIT_SUCCESS,
        );
        exercise_read_lane(
            &entity_read.database,
            &entity_read.token,
            b"0x10",
            type_bridge_read_transaction_entity_get_by_iid_v2_impl,
            type_bridge_read_transaction_entity_count_v2_impl,
        );
        assert_eq!(entity_read.state.commits.load(Ordering::Acquire), 0);

        let relation_read = relation_fixture(
            vec![
                documents(vec![membership_document("0x20")]),
                count_response(1),
            ],
            COMMIT_SUCCESS,
        );
        exercise_read_lane(
            &relation_read.database,
            &relation_read.token,
            b"0x20",
            type_bridge_read_transaction_relation_get_by_iid_v2_impl,
            type_bridge_read_transaction_relation_count_v2_impl,
        );
        assert_eq!(relation_read.state.commits.load(Ordering::Acquire), 0);

        let entity_write = fixture(
            vec![
                batch_identity("0x10"),
                batch_hydration(person_document("0x10")),
                documents(vec![person_document("0x10")]),
                count_response(1),
            ],
            COMMIT_SUCCESS,
        );
        exercise_write_lane(
            &entity_write.database,
            &entity_write.token,
            &entity_write.create,
            b"0x10",
            type_bridge_write_transaction_entity_insert_v2_impl,
            type_bridge_write_transaction_entity_get_by_iid_v2_impl,
            type_bridge_write_transaction_entity_count_v2_impl,
        );
        assert_eq!(entity_write.state.commits.load(Ordering::Acquire), 0);
        assert_eq!(entity_write.state.rollbacks.load(Ordering::Acquire), 1);

        let relation_write = relation_fixture(
            vec![
                relation_reference(),
                batch_identity("0x20"),
                batch_hydration(membership_document("0x20")),
                documents(vec![membership_document("0x20")]),
                count_response(1),
            ],
            COMMIT_SUCCESS,
        );
        exercise_write_lane(
            &relation_write.database,
            &relation_write.token,
            &relation_write.create,
            b"0x20",
            type_bridge_write_transaction_relation_insert_v2_impl,
            type_bridge_write_transaction_relation_get_by_iid_v2_impl,
            type_bridge_write_transaction_relation_count_v2_impl,
        );
        assert_eq!(relation_write.state.commits.load(Ordering::Acquire), 0);
        assert_eq!(relation_write.state.rollbacks.load(Ordering::Acquire), 1);
    }

    #[test]
    fn limits_alias_allocation_and_pre_dispatch_control_reject_before_provider() {
        let invalid = fixture(vec![], COMMIT_SUCCESS);
        let mut limits = default_limits();
        limits.version = 2;
        let mut count = u64::MAX;
        let mut diagnostics = ptr::dangling_mut();
        // SAFETY: all immutable inputs and output slots are valid and distinct.
        assert_eq!(
            unsafe {
                type_bridge_database_entity_count_v2_impl(
                    &*invalid.database,
                    &invalid.token,
                    &limits,
                    ptr::null(),
                    &mut count,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(count, 0);
        assert_eq!(
            diagnostic(diagnostics).1,
            "c_query_execution_limits_invalid"
        );
        assert!(invalid.state.opened_transactions().is_empty());
        close_diagnostics(&mut diagnostics);

        let alias = fixture(vec![], COMMIT_SUCCESS);
        let mut limits = default_limits();
        diagnostics = ptr::dangling_mut();
        let original = limits;
        // SAFETY: the hostile output intentionally aliases complete limits input storage.
        assert_eq!(
            unsafe {
                type_bridge_database_entity_count_v2_impl(
                    &*alias.database,
                    &alias.token,
                    &limits,
                    ptr::null(),
                    (&mut limits as *mut TypeBridgeQueryExecutionLimitsV1).cast(),
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(limits.struct_size, original.struct_size);
        assert_eq!(limits.version, original.version);
        assert_eq!(diagnostics, ptr::dangling_mut());
        assert!(alias.state.opened_transactions().is_empty());

        let zero = fixture(vec![], COMMIT_SUCCESS);
        let mut zero_limits = default_limits();
        zero_limits.items = 0;
        let mut thing = ptr::dangling_mut();
        diagnostics = ptr::dangling_mut();
        // SAFETY: the zero item ceiling is a valid, explicit policy.
        assert_eq!(
            unsafe {
                type_bridge_database_entity_insert_v2_impl(
                    &*zero.database,
                    &zero.token,
                    &*zero.create,
                    &zero_limits,
                    ptr::null(),
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ResourceLimit
        );
        assert!(thing.is_null());
        assert!(zero.state.opened_transactions().is_empty());
        close_diagnostics(&mut diagnostics);

        let allocation = fixture(vec![], COMMIT_SUCCESS);
        let limits = default_limits();
        let failure = inject_failure(AllocationSite::ProjectedThingHandle, 0);
        // SAFETY: all inputs remain live; the deterministic reservation fails pre-provider.
        assert_eq!(
            unsafe {
                type_bridge_database_entity_insert_v2_impl(
                    &*allocation.database,
                    &allocation.token,
                    &*allocation.create,
                    &limits,
                    ptr::null(),
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ResourceLimit
        );
        drop(failure);
        assert!(allocation.state.opened_transactions().is_empty());
        assert_eq!(diagnostic(diagnostics).1, "c_allocation_exhausted");
        close_diagnostics(&mut diagnostics);

        let storage_allocation = fixture(vec![], COMMIT_SUCCESS);
        let failure = inject_failure(AllocationSite::ProjectedThingHandle, 1);
        // SAFETY: the second matching checkpoint is the per-result Arc storage.
        assert_eq!(
            unsafe {
                type_bridge_database_entity_insert_v2_impl(
                    &*storage_allocation.database,
                    &storage_allocation.token,
                    &*storage_allocation.create,
                    &limits,
                    ptr::null(),
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ResourceLimit
        );
        drop(failure);
        assert!(thing.is_null());
        assert!(storage_allocation.state.opened_transactions().is_empty());
        assert_eq!(diagnostic(diagnostics).1, "c_allocation_exhausted");
        close_diagnostics(&mut diagnostics);

        let cancelled = fixture(vec![], COMMIT_SUCCESS);
        let mut cancellation = ptr::null_mut();
        // SAFETY: the owner slot is writable, then retains one live handle.
        assert_eq!(
            unsafe { type_bridge_cancellation_open(&mut cancellation) },
            TypeBridgeStatus::Ok
        );
        // SAFETY: the cancellation handle remains live.
        assert_eq!(
            unsafe { type_bridge_cancellation_request(cancellation) },
            TypeBridgeStatus::Ok
        );
        // SAFETY: the V2 call retains the requested signal and all immutable inputs.
        assert_eq!(
            unsafe {
                type_bridge_database_entity_insert_v2_impl(
                    &*cancelled.database,
                    &cancelled.token,
                    &*cancelled.create,
                    &limits,
                    cancellation,
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Cancelled
        );
        assert_eq!(diagnostic(diagnostics).1, "provider_cancelled");
        assert!(cancelled.state.opened_transactions().is_empty());
        close_diagnostics(&mut diagnostics);
        // SAFETY: close consumes the cancellation owner slot exactly once.
        assert_eq!(
            unsafe { type_bridge_cancellation_close(&mut cancellation) },
            TypeBridgeStatus::Ok
        );

        let deadline = fixture(vec![], COMMIT_SUCCESS);
        let mut deadline_limits = default_limits();
        deadline_limits.timeout_milliseconds = 0;
        // SAFETY: a zero timeout is a valid real ceiling.
        assert_eq!(
            unsafe {
                type_bridge_database_entity_insert_v2_impl(
                    &*deadline.database,
                    &deadline.token,
                    &*deadline.create,
                    &deadline_limits,
                    ptr::null(),
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ResourceLimit
        );
        assert_eq!(diagnostic(diagnostics).1, "transaction_deadline_exceeded");
        assert!(deadline.state.opened_transactions().is_empty());
        close_diagnostics(&mut diagnostics);
    }

    #[test]
    fn one_captured_deadline_and_cancellation_precede_result_reservation() {
        let deadline = fixture(vec![], COMMIT_SUCCESS);
        let mut deadline_limits = default_limits();
        deadline_limits.timeout_milliseconds = 1;
        let default_limits = default_limits();
        let mut thing = ptr::dangling_mut();
        let mut diagnostics = ptr::dangling_mut();
        let allocation = inject_failure(AllocationSite::ProjectedThingHandle, 0);
        crate::thing_crud::delay_next_controlled_crud_checkpoint(20);
        // SAFETY: the test retains every immutable input and both output slots.
        assert_eq!(
            unsafe {
                type_bridge_database_entity_insert_v2_impl(
                    &*deadline.database,
                    &deadline.token,
                    &*deadline.create,
                    &deadline_limits,
                    ptr::null(),
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ResourceLimit
        );
        assert!(thing.is_null());
        assert_eq!(diagnostic(diagnostics).1, "transaction_deadline_exceeded");
        assert!(deadline.state.opened_transactions().is_empty());
        close_diagnostics(&mut diagnostics);

        // The deadline checkpoint did not consume the armed result reservation failure.
        assert_eq!(
            unsafe {
                type_bridge_database_entity_insert_v2_impl(
                    &*deadline.database,
                    &deadline.token,
                    &*deadline.create,
                    &default_limits,
                    ptr::null(),
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ResourceLimit
        );
        assert_eq!(diagnostic(diagnostics).1, "c_allocation_exhausted");
        assert!(deadline.state.opened_transactions().is_empty());
        close_diagnostics(&mut diagnostics);
        drop(allocation);

        let cancelled = fixture(vec![], COMMIT_SUCCESS);
        let mut cancellation = ptr::null_mut();
        assert_eq!(
            unsafe { type_bridge_cancellation_open(&mut cancellation) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe { type_bridge_cancellation_request(cancellation) },
            TypeBridgeStatus::Ok
        );
        let allocation = inject_failure(AllocationSite::ProjectedThingHandle, 0);
        // SAFETY: the requested cancellation and fixture handles remain live.
        assert_eq!(
            unsafe {
                type_bridge_database_entity_insert_v2_impl(
                    &*cancelled.database,
                    &cancelled.token,
                    &*cancelled.create,
                    &default_limits,
                    cancellation,
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Cancelled
        );
        assert_eq!(diagnostic(diagnostics).1, "provider_cancelled");
        close_diagnostics(&mut diagnostics);
        assert_eq!(
            unsafe { type_bridge_cancellation_close(&mut cancellation) },
            TypeBridgeStatus::Ok
        );

        // Cancellation also leaves the result reservation failure untouched.
        assert_eq!(
            unsafe {
                type_bridge_database_entity_insert_v2_impl(
                    &*cancelled.database,
                    &cancelled.token,
                    &*cancelled.create,
                    &default_limits,
                    ptr::null(),
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ResourceLimit
        );
        assert_eq!(diagnostic(diagnostics).1, "c_allocation_exhausted");
        assert!(cancelled.state.opened_transactions().is_empty());
        close_diagnostics(&mut diagnostics);
        drop(allocation);
    }

    #[test]
    fn deep_alias_fencing_rejects_create_package_and_server_storage_without_writes() {
        let fixture = fixture(vec![], COMMIT_SUCCESS);
        let limits = default_limits();
        let mut diagnostics = ptr::dangling_mut();
        let diagnostics_sentinel = diagnostics;

        let create_label = fixture.create.value.type_id().label().as_str();
        let create_before = create_label.as_bytes().to_vec();
        let forged_thing = create_label
            .as_ptr()
            .cast_mut()
            .cast::<*mut TypeBridgeProjectedThing>();
        // SAFETY: the hostile output aliases exposed immutable create storage;
        // deep preflight must reject it before attempting a write.
        assert_eq!(
            unsafe {
                type_bridge_database_entity_insert_v2_impl(
                    &*fixture.database,
                    &fixture.token,
                    &*fixture.create,
                    &limits,
                    ptr::null(),
                    forged_thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(create_label.as_bytes(), create_before);
        assert_eq!(diagnostics, diagnostics_sentinel);

        let package_bytes = fixture.package.state.authority_json.as_slice();
        let package_before = package_bytes[..size_of::<u64>()].to_vec();
        let forged_count = package_bytes.as_ptr().cast_mut().cast::<u64>();
        // SAFETY: the hostile count output aliases retained package bytes.
        assert_eq!(
            unsafe {
                type_bridge_database_entity_count_v2_impl(
                    &*fixture.database,
                    &fixture.token,
                    &limits,
                    ptr::null(),
                    forged_count,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(&package_bytes[..size_of::<u64>()], package_before);
        assert_eq!(diagnostics, diagnostics_sentinel);

        let mut server_version = TypeBridgeByteView {
            data: ptr::null(),
            length: 0,
        };
        assert_eq!(
            unsafe { type_bridge_database_server_version(&*fixture.database, &mut server_version) },
            TypeBridgeStatus::Ok
        );
        let server_before = unsafe {
            std::slice::from_raw_parts(server_version.data, server_version.length).to_vec()
        };
        let forged_count = server_version.data.cast_mut().cast::<u64>();
        // SAFETY: the hostile count output aliases the database's exposed version view.
        assert_eq!(
            unsafe {
                type_bridge_database_entity_count_v2_impl(
                    &*fixture.database,
                    &fixture.token,
                    &limits,
                    ptr::null(),
                    forged_count,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(
            unsafe { std::slice::from_raw_parts(server_version.data, server_version.length) },
            server_before
        );
        assert_eq!(diagnostics, diagnostics_sentinel);
        assert!(fixture.state.opened_transactions().is_empty());

        let mut read_transaction = ptr::null_mut();
        diagnostics = ptr::null_mut();
        // SAFETY: both owner output slots are distinct and writable.
        assert_eq!(
            unsafe {
                type_bridge_read_transaction_open(
                    &*fixture.database,
                    ptr::null(),
                    &mut read_transaction,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        diagnostics = diagnostics_sentinel;
        let package_before = package_bytes[..size_of::<u64>()].to_vec();
        let forged_count = package_bytes.as_ptr().cast_mut().cast::<u64>();
        // SAFETY: the hostile count output aliases package storage retained by
        // the read transaction; transaction deep preflight must reject it.
        assert_eq!(
            unsafe {
                type_bridge_read_transaction_entity_count_v2_impl(
                    read_transaction,
                    &fixture.token,
                    &limits,
                    ptr::null(),
                    forged_count,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(&package_bytes[..size_of::<u64>()], package_before);
        assert_eq!(diagnostics, diagnostics_sentinel);
        assert_eq!(fixture.state.query_count(), 0);
        diagnostics = ptr::null_mut();
        // SAFETY: close consumes exactly the still-live read transaction owner.
        assert_eq!(
            unsafe { type_bridge_read_transaction_close(&mut read_transaction, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );

        let mut write_transaction = ptr::null_mut();
        // SAFETY: both owner output slots are distinct and writable.
        assert_eq!(
            unsafe {
                type_bridge_write_transaction_open(
                    &*fixture.database,
                    ptr::null(),
                    &mut write_transaction,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        diagnostics = diagnostics_sentinel;
        let package_before = package_bytes[..size_of::<*mut TypeBridgeProjectedThing>()].to_vec();
        let forged_thing = package_bytes
            .as_ptr()
            .cast_mut()
            .cast::<*mut TypeBridgeProjectedThing>();
        // SAFETY: the hostile thing output aliases package storage retained by
        // the write transaction; transaction deep preflight must reject it.
        assert_eq!(
            unsafe {
                type_bridge_write_transaction_entity_insert_v2_impl(
                    write_transaction,
                    &fixture.token,
                    &*fixture.create,
                    &limits,
                    ptr::null(),
                    forged_thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(
            &package_bytes[..size_of::<*mut TypeBridgeProjectedThing>()],
            package_before
        );
        assert_eq!(diagnostics, diagnostics_sentinel);
        assert_eq!(fixture.state.query_count(), 0);
        diagnostics = ptr::null_mut();
        // SAFETY: rollback consumes exactly the still-live write transaction owner.
        assert_eq!(
            unsafe {
                type_bridge_write_transaction_rollback(&mut write_transaction, &mut diagnostics)
            },
            TypeBridgeStatus::Ok
        );
    }

    #[test]
    fn legacy_crud_inherits_policy_handle_deep_fences_before_output_initialization() {
        let ceiling = QueryExecutionResourceLimits::default();

        let database = fixture_with_answer_ceiling(vec![], COMMIT_SUCCESS, Some(ceiling));
        let mut server_version = TypeBridgeByteView {
            data: ptr::null(),
            length: 0,
        };
        assert_eq!(
            unsafe {
                type_bridge_database_server_version(&*database.database, &mut server_version)
            },
            TypeBridgeStatus::Ok
        );
        let server_before = unsafe {
            std::slice::from_raw_parts(server_version.data, server_version.length).to_vec()
        };
        let forged_thing = server_version
            .data
            .cast_mut()
            .cast::<*mut TypeBridgeProjectedThing>();
        let mut diagnostics = ptr::dangling_mut();
        let diagnostics_sentinel = diagnostics;
        // SAFETY: the hostile thing output aliases the V2-origin database's
        // retained server-version bytes and must be rejected without writing.
        assert_eq!(
            unsafe {
                type_bridge_database_entity_insert(
                    &*database.database,
                    &database.token,
                    &*database.create,
                    ptr::null(),
                    forged_thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(
            unsafe { std::slice::from_raw_parts(server_version.data, server_version.length) },
            server_before
        );
        assert_eq!(diagnostics, diagnostics_sentinel);
        assert_eq!(database.state.query_count(), 0);
        assert!(database.state.opened_transactions().is_empty());

        let read = relation_fixture_with_answer_ceiling(
            vec![],
            COMMIT_SUCCESS,
            Some(QueryExecutionResourceLimits::default()),
        );
        let mut read_transaction = ptr::null_mut();
        diagnostics = ptr::null_mut();
        // SAFETY: both owner output slots are distinct and writable.
        assert_eq!(
            unsafe {
                type_bridge_read_transaction_open(
                    &*read.database,
                    ptr::null(),
                    &mut read_transaction,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        let opens_before = read.state.opened_transactions();
        let package_bytes = read.package.state.authority_json.as_slice();
        let package_before = package_bytes[..size_of::<u64>()].to_vec();
        let forged_count = package_bytes.as_ptr().cast_mut().cast::<u64>();
        diagnostics = diagnostics_sentinel;
        // SAFETY: the hostile count output aliases package storage retained by
        // the V2-origin read transaction and must not be initialized.
        assert_eq!(
            unsafe {
                type_bridge_read_transaction_relation_count(
                    read_transaction,
                    &read.token,
                    ptr::null(),
                    forged_count,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(&package_bytes[..size_of::<u64>()], package_before);
        assert_eq!(diagnostics, diagnostics_sentinel);
        assert_eq!(read.state.query_count(), 0);
        assert_eq!(read.state.opened_transactions(), opens_before);
        diagnostics = ptr::null_mut();
        // SAFETY: close consumes exactly the still-live read transaction owner.
        assert_eq!(
            unsafe { type_bridge_read_transaction_close(&mut read_transaction, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );

        let write = fixture_with_answer_ceiling(
            vec![],
            COMMIT_SUCCESS,
            Some(QueryExecutionResourceLimits::default()),
        );
        let mut write_transaction = ptr::null_mut();
        // SAFETY: both owner output slots are distinct and writable.
        assert_eq!(
            unsafe {
                type_bridge_write_transaction_open(
                    &*write.database,
                    ptr::null(),
                    &mut write_transaction,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe { type_bridge_database_server_version(&*write.database, &mut server_version) },
            TypeBridgeStatus::Ok
        );
        let server_before = unsafe {
            std::slice::from_raw_parts(server_version.data, server_version.length).to_vec()
        };
        let forged_diagnostics = server_version
            .data
            .cast_mut()
            .cast::<*mut TypeBridgeExecutionDiagnostics>();
        let opens_before = write.state.opened_transactions();
        // SAFETY: the hostile diagnostics output aliases server-version bytes
        // retained by the V2-origin write transaction and must not be cleared.
        assert_eq!(
            unsafe {
                type_bridge_write_transaction_entity_delete_by_iid(
                    write_transaction,
                    &write.token,
                    byte_view(b"0x10"),
                    ptr::null(),
                    forged_diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(
            unsafe { std::slice::from_raw_parts(server_version.data, server_version.length) },
            server_before
        );
        assert_eq!(write.state.query_count(), 0);
        assert_eq!(write.state.opened_transactions(), opens_before);
        diagnostics = ptr::null_mut();
        // SAFETY: rollback consumes exactly the still-live write transaction owner.
        assert_eq!(
            unsafe {
                type_bridge_write_transaction_rollback(&mut write_transaction, &mut diagnostics)
            },
            TypeBridgeStatus::Ok
        );
    }

    fn request_after_dispatch(
        state: Arc<FakeState>,
        cancellation: *const TypeBridgeCancellation,
    ) -> std::thread::JoinHandle<TypeBridgeStatus> {
        let cancellation = cancellation.addr();
        std::thread::spawn(move || {
            while state.query_count() == 0 {
                std::thread::yield_now();
            }
            // SAFETY: the executing call retains this handle until the requester joins.
            unsafe { type_bridge_cancellation_request(cancellation as *const _) }
        })
    }

    #[test]
    fn in_flight_v2_cancellation_rolls_back_but_legacy_policy_route_ignores_it() {
        let controlled = fixture(vec![Response::Pending], COMMIT_SUCCESS);
        let limits = default_limits();
        let mut cancellation = ptr::null_mut();
        // SAFETY: the owner slot is writable.
        assert_eq!(
            unsafe { type_bridge_cancellation_open(&mut cancellation) },
            TypeBridgeStatus::Ok
        );
        let requester = request_after_dispatch(Arc::clone(&controlled.state), cancellation);
        let mut thing = ptr::dangling_mut();
        let mut diagnostics = ptr::dangling_mut();
        // SAFETY: the fixture and cancellation owner outlive the blocking call.
        assert_eq!(
            unsafe {
                type_bridge_database_entity_insert_v2_impl(
                    &*controlled.database,
                    &controlled.token,
                    &*controlled.create,
                    &limits,
                    cancellation,
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Cancelled
        );
        assert_eq!(requester.join().unwrap(), TypeBridgeStatus::Ok);
        assert!(thing.is_null());
        assert_eq!(diagnostic(diagnostics).1, "provider_cancelled");
        assert_eq!(controlled.state.query_count(), 1);
        assert_eq!(controlled.state.rollbacks.load(Ordering::Acquire), 1);
        assert_eq!(controlled.state.commits.load(Ordering::Acquire), 0);
        close_diagnostics(&mut diagnostics);
        // SAFETY: close consumes the cancellation owner slot.
        assert_eq!(
            unsafe { type_bridge_cancellation_close(&mut cancellation) },
            TypeBridgeStatus::Ok
        );

        let legacy = fixture_with_answer_ceiling(
            vec![
                delayed_batch_identity("0x10"),
                batch_hydration(person_document("0x10")),
            ],
            COMMIT_SUCCESS,
            Some(QueryExecutionResourceLimits::default()),
        );
        // SAFETY: the owner slot is writable.
        assert_eq!(
            unsafe { type_bridge_cancellation_open(&mut cancellation) },
            TypeBridgeStatus::Ok
        );
        let requester = request_after_dispatch(Arc::clone(&legacy.state), cancellation);
        // SAFETY: ABI 1.3 cancellation remains a pre-dispatch-only contract.
        assert_eq!(
            unsafe {
                type_bridge_database_entity_insert(
                    &*legacy.database,
                    &legacy.token,
                    &*legacy.create,
                    cancellation,
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(requester.join().unwrap(), TypeBridgeStatus::Ok);
        assert!(!thing.is_null());
        assert!(diagnostics.is_null());
        assert_eq!(legacy.state.query_count(), 2);
        assert_eq!(legacy.state.commits.load(Ordering::Acquire), 1);
        close_thing(&mut thing);
        // SAFETY: close consumes the cancellation owner slot.
        assert_eq!(
            unsafe { type_bridge_cancellation_close(&mut cancellation) },
            TypeBridgeStatus::Ok
        );

        let inherited = fixture_with_answer_ceiling(
            vec![],
            COMMIT_SUCCESS,
            Some(QueryExecutionResourceLimits::tightened(
                30_000,
                0,
                u64::MAX,
                u64::MAX,
                u64::MAX,
                u64::MAX,
                u64::MAX,
                u32::MAX,
            )),
        );
        // SAFETY: ABI 1.3 inherits the enclosing V2 answer ceiling without a new argument.
        assert_eq!(
            unsafe {
                type_bridge_database_entity_insert(
                    &*inherited.database,
                    &inherited.token,
                    &*inherited.create,
                    ptr::null(),
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ResourceLimit
        );
        assert!(thing.is_null());
        assert!(inherited.state.opened_transactions().is_empty());
        close_diagnostics(&mut diagnostics);
    }

    #[test]
    fn provider_failure_is_redacted_and_owned_write_rolls_back() {
        let fixture = fixture(vec![Response::Error], COMMIT_SUCCESS);
        let limits = default_limits();
        let mut thing = ptr::dangling_mut();
        let mut diagnostics = ptr::dangling_mut();
        // SAFETY: all inputs and outputs remain valid through the blocking call.
        assert_eq!(
            unsafe {
                type_bridge_database_entity_insert_v2_impl(
                    &*fixture.database,
                    &fixture.token,
                    &*fixture.create,
                    &limits,
                    ptr::null(),
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ExecutionFailed
        );
        assert!(thing.is_null());
        let (_, code, message) = diagnostic(diagnostics);
        assert_eq!(code, "provider_operation_failed");
        assert!(!message.contains("provider-secret"));
        assert_eq!(fixture.state.rollbacks.load(Ordering::Acquire), 1);
        assert_eq!(fixture.state.commits.load(Ordering::Acquire), 0);
        close_diagnostics(&mut diagnostics);
    }
}
