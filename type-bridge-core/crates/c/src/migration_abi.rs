//! Candidate additive ABI 1.5 administration and migration boundary.
//!
//! This module is compiled only with the non-default `abi-1-5` feature until
//! its complete symbol, layout, compiler, and installed-consumer inventory is
//! frozen and activated atomically.

use std::mem::size_of;
use std::ptr;
use std::sync::Arc;

use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory};

use crate::abi::{TypeBridgeDiagnostics, TypeBridgeStatus, guarded};
use crate::diagnostic::diagnostics_handle;
use crate::generated_preflight::direct_output_preflight;
use crate::migration_runtime::{DatabaseAdministrationState, DatabaseDeletionPlanState};
use crate::runtime::TypeBridgeDatabase;

/// Opaque bound-database administration owner.
pub struct TypeBridgeDatabaseAdministration {
    state: Arc<DatabaseAdministrationState>,
}

/// Opaque single-use pair-aware database deletion plan.
pub struct TypeBridgeDatabaseDeletionPlan {
    state: DatabaseDeletionPlanState,
}

const PAIR_ABSENT: u32 = 1;
const PAIR_STANDALONE_MANAGED: u32 = 2;
const PAIR_OWNED: u32 = 3;
const PAIR_OWNED_JOURNAL_ORPHAN: u32 = 4;
const CREATE_CREATED: u32 = 1;
const CREATE_ALREADY_EXISTS: u32 = 2;
const DELETE_ALREADY_ABSENT: u32 = 1;
const DELETE_STANDALONE_MANAGED: u32 = 2;
const DELETE_OWNED_PAIR: u32 = 3;
const DELETE_OWNED_JOURNAL_ORPHAN: u32 = 4;

fn failure_status(diagnostic: &Diagnostic) -> TypeBridgeStatus {
    match diagnostic.category() {
        DiagnosticCategory::InvalidContract => TypeBridgeStatus::InvalidArgument,
        DiagnosticCategory::UnsupportedCapability => TypeBridgeStatus::Unsupported,
        DiagnosticCategory::ResourceLimit => TypeBridgeStatus::ResourceLimit,
        DiagnosticCategory::Cancelled => TypeBridgeStatus::Cancelled,
        DiagnosticCategory::Integrity => TypeBridgeStatus::ExecutionFailed,
    }
}

unsafe fn initialize_diagnostics(
    out_diagnostics: *mut *mut TypeBridgeDiagnostics,
) -> Result<(), TypeBridgeStatus> {
    if out_diagnostics.is_null() {
        return Err(TypeBridgeStatus::InvalidArgument);
    }
    // SAFETY: output preflight proved this complete caller slot writable.
    unsafe { out_diagnostics.write_unaligned(ptr::null_mut()) };
    Ok(())
}

fn return_failure(
    diagnostic: Diagnostic,
    out_diagnostics: *mut *mut TypeBridgeDiagnostics,
) -> TypeBridgeStatus {
    let status = failure_status(&diagnostic);
    // SAFETY: every caller initializes and retains this writable output slot.
    unsafe {
        out_diagnostics.write_unaligned(Box::into_raw(Box::new(diagnostics_handle(vec![
            diagnostic,
        ]))))
    };
    status
}

unsafe fn preflight_one_output<T, Input>(
    input: *const Input,
    out: *mut T,
    out_diagnostics: *mut *mut TypeBridgeDiagnostics,
) -> Result<(), TypeBridgeStatus> {
    let outputs = direct_output_preflight(&[
        (out.cast(), size_of::<T>()),
        (
            out_diagnostics.cast(),
            size_of::<*mut TypeBridgeDiagnostics>(),
        ),
    ])?;
    outputs.check_bytes(input.cast(), size_of::<Input>())?;
    if out.is_null() {
        return Err(TypeBridgeStatus::InvalidArgument);
    }
    // SAFETY: output preflight proved the diagnostic slot writable and disjoint.
    unsafe { initialize_diagnostics(out_diagnostics) }
}

/// Open administration authority bound to the database handle's exact identity.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_database_administration_open(
    database: *const TypeBridgeDatabase,
    out_administration: *mut *mut TypeBridgeDatabaseAdministration,
    out_diagnostics: *mut *mut TypeBridgeDiagnostics,
) -> TypeBridgeStatus {
    if let Err(status) =
        unsafe { preflight_one_output(database, out_administration, out_diagnostics) }
    {
        return status;
    }
    // SAFETY: the caller-provided handle slot was preflighted above.
    unsafe { out_administration.write_unaligned(ptr::null_mut()) };
    guarded(|| {
        let Some(database) = (unsafe { database.as_ref() }) else {
            return TypeBridgeStatus::InvalidArgument;
        };
        match DatabaseAdministrationState::open(database) {
            Ok(state) => {
                // SAFETY: the initialized output slot remains caller-owned.
                unsafe {
                    out_administration.write_unaligned(Box::into_raw(Box::new(
                        TypeBridgeDatabaseAdministration { state },
                    )))
                };
                TypeBridgeStatus::Ok
            }
            Err(error) => return_failure(error, out_diagnostics),
        }
    })
}

/// Return whether the exact bound managed database currently exists.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_database_administration_exists(
    administration: *const TypeBridgeDatabaseAdministration,
    out_exists: *mut u8,
    out_diagnostics: *mut *mut TypeBridgeDiagnostics,
) -> TypeBridgeStatus {
    if let Err(status) =
        unsafe { preflight_one_output(administration, out_exists, out_diagnostics) }
    {
        return status;
    }
    // SAFETY: output preflight proved this scalar writable.
    unsafe { out_exists.write_unaligned(0) };
    guarded(|| {
        let Some(administration) = (unsafe { administration.as_ref() }) else {
            return TypeBridgeStatus::InvalidArgument;
        };
        match administration.state.database_exists_blocking() {
            Ok(value) => {
                // SAFETY: initialized output slot remains caller-owned.
                unsafe { out_exists.write_unaligned(u8::from(value)) };
                TypeBridgeStatus::Ok
            }
            Err(error) => return_failure(error, out_diagnostics),
        }
    })
}

/// Create the exact bound managed database and return a normalized outcome.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_database_administration_create(
    administration: *const TypeBridgeDatabaseAdministration,
    out_outcome: *mut u32,
    out_diagnostics: *mut *mut TypeBridgeDiagnostics,
) -> TypeBridgeStatus {
    if let Err(status) =
        unsafe { preflight_one_output(administration, out_outcome, out_diagnostics) }
    {
        return status;
    }
    unsafe { out_outcome.write_unaligned(0) };
    guarded(|| {
        let Some(administration) = (unsafe { administration.as_ref() }) else {
            return TypeBridgeStatus::InvalidArgument;
        };
        match administration.state.create_database_outcome_blocking() {
            Ok(outcome) => {
                let value = match outcome {
                    type_bridge_schema_migration_typedb::ManagedDatabasePairCreateOutcome::Created => CREATE_CREATED,
                    type_bridge_schema_migration_typedb::ManagedDatabasePairCreateOutcome::AlreadyExists => CREATE_ALREADY_EXISTS,
                };
                unsafe { out_outcome.write_unaligned(value) };
                TypeBridgeStatus::Ok
            }
            Err(error) => return_failure(error, out_diagnostics),
        }
    })
}

/// Inspect the exact managed/journal pair state.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_database_administration_inspect(
    administration: *const TypeBridgeDatabaseAdministration,
    out_state: *mut u32,
    out_diagnostics: *mut *mut TypeBridgeDiagnostics,
) -> TypeBridgeStatus {
    if let Err(status) = unsafe { preflight_one_output(administration, out_state, out_diagnostics) }
    {
        return status;
    }
    unsafe { out_state.write_unaligned(0) };
    guarded(|| {
        let Some(administration) = (unsafe { administration.as_ref() }) else {
            return TypeBridgeStatus::InvalidArgument;
        };
        match administration.state.inspect_blocking() {
            Ok(state) => {
                let value = pair_state(state);
                unsafe { out_state.write_unaligned(value) };
                TypeBridgeStatus::Ok
            }
            Err(error) => return_failure(error, out_diagnostics),
        }
    })
}

/// Build a single-use pair-aware destructive admission plan.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_database_administration_plan_delete(
    administration: *const TypeBridgeDatabaseAdministration,
    out_plan: *mut *mut TypeBridgeDatabaseDeletionPlan,
    out_diagnostics: *mut *mut TypeBridgeDiagnostics,
) -> TypeBridgeStatus {
    if let Err(status) = unsafe { preflight_one_output(administration, out_plan, out_diagnostics) }
    {
        return status;
    }
    unsafe { out_plan.write_unaligned(ptr::null_mut()) };
    guarded(|| {
        let Some(administration) = (unsafe { administration.as_ref() }) else {
            return TypeBridgeStatus::InvalidArgument;
        };
        match administration.state.plan_delete_blocking() {
            Ok(state) => {
                unsafe {
                    out_plan.write_unaligned(Box::into_raw(Box::new(
                        TypeBridgeDatabaseDeletionPlan { state },
                    )))
                };
                TypeBridgeStatus::Ok
            }
            Err(error) => return_failure(error, out_diagnostics),
        }
    })
}

/// Return the pair state captured by a deletion plan.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_database_deletion_plan_inspected_state(
    plan: *const TypeBridgeDatabaseDeletionPlan,
    out_state: *mut u32,
) -> TypeBridgeStatus {
    guarded(|| {
        if plan.is_null() || out_state.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        let Some(state) = (unsafe { &*plan }).state.inspected_state() else {
            return TypeBridgeStatus::InvalidArgument;
        };
        unsafe { out_state.write_unaligned(pair_state(state)) };
        TypeBridgeStatus::Ok
    })
}

/// Consume a deletion plan and return the exact normalized terminal outcome.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_database_deletion_plan_execute(
    plan: *mut TypeBridgeDatabaseDeletionPlan,
    out_outcome: *mut u32,
    out_diagnostics: *mut *mut TypeBridgeDiagnostics,
) -> TypeBridgeStatus {
    if let Err(status) = unsafe { preflight_one_output(plan, out_outcome, out_diagnostics) } {
        return status;
    }
    unsafe { out_outcome.write_unaligned(0) };
    guarded(|| {
        let Some(plan) = (unsafe { plan.as_mut() }) else {
            return TypeBridgeStatus::InvalidArgument;
        };
        match plan.state.execute_blocking() {
            Ok(outcome) => {
                let value = match outcome {
                    type_bridge_schema_migration_typedb::ManagedDatabasePairDeleteOutcome::AlreadyAbsent => DELETE_ALREADY_ABSENT,
                    type_bridge_schema_migration_typedb::ManagedDatabasePairDeleteOutcome::DeletedStandaloneManaged => DELETE_STANDALONE_MANAGED,
                    type_bridge_schema_migration_typedb::ManagedDatabasePairDeleteOutcome::DeletedOwnedPair => DELETE_OWNED_PAIR,
                    type_bridge_schema_migration_typedb::ManagedDatabasePairDeleteOutcome::DeletedOwnedJournalOrphan => DELETE_OWNED_JOURNAL_ORPHAN,
                };
                unsafe { out_outcome.write_unaligned(value) };
                TypeBridgeStatus::Ok
            }
            Err(error) => return_failure(error, out_diagnostics),
        }
    })
}

/// Close a deletion plan idempotently and clear its owner slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_database_deletion_plan_close(
    plan: *mut *mut TypeBridgeDatabaseDeletionPlan,
) -> TypeBridgeStatus {
    // SAFETY: this function forwards the caller-owned handle slot unchanged.
    unsafe { close_box(plan) }
}

/// Close administration unless a deletion plan still retains it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_database_administration_close(
    administration: *mut *mut TypeBridgeDatabaseAdministration,
) -> TypeBridgeStatus {
    guarded(|| {
        if administration.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        let value = unsafe { administration.read_unaligned() };
        if value.is_null() {
            return TypeBridgeStatus::Ok;
        }
        if Arc::strong_count(&unsafe { &*value }.state) != 1 {
            return TypeBridgeStatus::InUse;
        }
        unsafe {
            administration.write_unaligned(ptr::null_mut());
            drop(Box::from_raw(value));
        }
        TypeBridgeStatus::Ok
    })
}

fn pair_state(state: type_bridge_schema_migration_typedb::ManagedDatabasePairState) -> u32 {
    match state {
        type_bridge_schema_migration_typedb::ManagedDatabasePairState::Absent => PAIR_ABSENT,
        type_bridge_schema_migration_typedb::ManagedDatabasePairState::StandaloneManaged => {
            PAIR_STANDALONE_MANAGED
        }
        type_bridge_schema_migration_typedb::ManagedDatabasePairState::OwnedPair => PAIR_OWNED,
        type_bridge_schema_migration_typedb::ManagedDatabasePairState::OwnedJournalOrphan => {
            PAIR_OWNED_JOURNAL_ORPHAN
        }
    }
}

unsafe fn close_box<T>(slot: *mut *mut T) -> TypeBridgeStatus {
    guarded(|| {
        if slot.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        let value = unsafe { slot.read_unaligned() };
        if value.is_null() {
            return TypeBridgeStatus::Ok;
        }
        unsafe {
            slot.write_unaligned(ptr::null_mut());
            drop(Box::from_raw(value));
        }
        TypeBridgeStatus::Ok
    })
}
