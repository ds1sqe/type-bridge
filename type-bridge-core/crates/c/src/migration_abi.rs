//! Candidate additive ABI 1.5 administration and migration boundary.
//!
//! This module is compiled only with the non-default `abi-1-5` feature until
//! its complete symbol, layout, compiler, and installed-consumer inventory is
//! frozen and activated atomically.

use std::mem::size_of;
use std::ptr;
use std::sync::Arc;

use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory};

use crate::abi::{
    TypeBridgeByteView, TypeBridgeDiagnostics, TypeBridgeSchemaPackage, TypeBridgeStatus, guarded,
};
use crate::diagnostic::diagnostics_handle;
use crate::generated_preflight::direct_output_preflight;
use crate::migration_runtime::{
    DatabaseAdministrationState, DatabaseDeletionPlanState, MigrationApprovalBuilderState,
    MigrationApprovalSetState, MigrationCatalogState, MigrationHistoryEntryState,
    MigrationIdentitySnapshot, MigrationPlanDirection, MigrationPlanEntryState,
    MigrationPlanExecutionState, MigrationPlanState, MigrationVerificationFindingKind,
    MigrationVerificationFindingSnapshot, MigrationVerificationState,
};
use crate::runtime::TypeBridgeDatabase;

/// Opaque bound-database administration owner.
pub struct TypeBridgeDatabaseAdministration {
    state: Arc<DatabaseAdministrationState>,
}

/// Opaque single-use pair-aware database deletion plan.
pub struct TypeBridgeDatabaseDeletionPlan {
    state: DatabaseDeletionPlanState,
}

/// Opaque immutable verified migration catalog.
pub struct TypeBridgeMigrationCatalog {
    state: Arc<MigrationCatalogState>,
}

/// Opaque independently owned migration-history entry snapshot.
pub struct TypeBridgeMigrationHistoryEntry {
    state: MigrationHistoryEntryState,
}

/// Opaque independently owned compound migration identity.
pub struct TypeBridgeMigrationIdentity {
    state: MigrationIdentitySnapshot,
}

/// Opaque immutable apply or rollback preview/authorized plan.
pub struct TypeBridgeMigrationPlan {
    state: Arc<MigrationPlanState>,
}

/// Opaque independently owned immutable plan-entry snapshot.
pub struct TypeBridgeMigrationPlanEntry {
    state: MigrationPlanEntryState,
}

/// Opaque mutable approval selection bound to one exact preview.
pub struct TypeBridgeMigrationApprovalBuilder {
    state: Option<MigrationApprovalBuilderState>,
}

/// Opaque immutable approval set bound to one exact preview.
pub struct TypeBridgeMigrationApprovalSet {
    state: MigrationApprovalSetState,
}

/// Opaque terminal apply or rollback execution report.
pub struct TypeBridgeMigrationExecutionOutcome {
    state: MigrationPlanExecutionState,
}

/// Opaque independently owned terminal backfill observation.
pub struct TypeBridgeMigrationBackfillObservation {
    state: type_bridge_schema_migration::MigrationBackfillObservation,
}

/// Opaque owned read-only migration verification report.
pub struct TypeBridgeMigrationVerificationReport {
    _catalog: Arc<MigrationCatalogState>,
    state: MigrationVerificationState,
}

/// Opaque independently owned verification-finding snapshot.
pub struct TypeBridgeMigrationVerificationFinding {
    state: MigrationVerificationFindingSnapshot,
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

/// Open canonical bundle bytes under one verified generated package authority.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_catalog_open(
    package: *const TypeBridgeSchemaPackage,
    bundle: TypeBridgeByteView,
    out_catalog: *mut *mut TypeBridgeMigrationCatalog,
    out_diagnostics: *mut *mut TypeBridgeDiagnostics,
) -> TypeBridgeStatus {
    let outputs = match direct_output_preflight(&[
        (
            out_catalog.cast(),
            size_of::<*mut TypeBridgeMigrationCatalog>(),
        ),
        (
            out_diagnostics.cast(),
            size_of::<*mut TypeBridgeDiagnostics>(),
        ),
    ]) {
        Ok(value) => value,
        Err(status) => return status,
    };
    if outputs
        .check_bytes(package.cast(), size_of::<TypeBridgeSchemaPackage>())
        .and_then(|()| outputs.check_bytes(bundle.data.cast(), bundle.length))
        .is_err()
        || out_catalog.is_null()
    {
        return TypeBridgeStatus::InvalidArgument;
    }
    if let Err(status) = unsafe { initialize_diagnostics(out_diagnostics) } {
        return status;
    }
    unsafe { out_catalog.write_unaligned(ptr::null_mut()) };
    guarded(|| {
        let Some(package) = (unsafe { package.as_ref() }) else {
            return TypeBridgeStatus::InvalidArgument;
        };
        let bytes = match unsafe {
            bundle.snapshot(
                "migration_history_bundle",
                type_bridge_schema_migration::MAX_MIGRATION_HISTORY_BUNDLE_BYTES,
            )
        } {
            Ok(value) => value,
            Err((status, diagnostics)) => {
                unsafe {
                    out_diagnostics
                        .write_unaligned(Box::into_raw(Box::new(diagnostics_handle(diagnostics))))
                };
                return status;
            }
        };
        match MigrationCatalogState::open(&package.state()._authority, &bytes) {
            Ok(state) => {
                unsafe {
                    out_catalog.write_unaligned(Box::into_raw(Box::new(
                        TypeBridgeMigrationCatalog { state },
                    )))
                };
                TypeBridgeStatus::Ok
            }
            Err(error) => return_failure(error, out_diagnostics),
        }
    })
}

/// Borrow canonical fingerprint JSON from a live catalog.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_catalog_fingerprint(
    catalog: *const TypeBridgeMigrationCatalog,
    out_fingerprint: *mut TypeBridgeByteView,
) -> TypeBridgeStatus {
    guarded(|| {
        if catalog.is_null() || out_fingerprint.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        let bytes = unsafe { &*catalog }.state.fingerprint_json();
        unsafe {
            out_fingerprint.write_unaligned(TypeBridgeByteView {
                data: bytes.as_ptr(),
                length: bytes.len(),
            })
        };
        TypeBridgeStatus::Ok
    })
}

/// Return the bounded number of topologically ordered history entries.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_catalog_count(
    catalog: *const TypeBridgeMigrationCatalog,
    out_count: *mut usize,
) -> TypeBridgeStatus {
    unsafe { scalar(catalog, out_count, |value| value.state.len()) }
}

/// Return an independently owned history entry at one topological ordinal.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_catalog_entry_at(
    catalog: *const TypeBridgeMigrationCatalog,
    index: usize,
    out_entry: *mut *mut TypeBridgeMigrationHistoryEntry,
) -> TypeBridgeStatus {
    guarded(|| {
        if catalog.is_null() || out_entry.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        unsafe { out_entry.write_unaligned(ptr::null_mut()) };
        let Some(state) = unsafe { &*catalog }.state.entry(index) else {
            return TypeBridgeStatus::InvalidArgument;
        };
        unsafe {
            out_entry.write_unaligned(Box::into_raw(Box::new(TypeBridgeMigrationHistoryEntry {
                state,
            })))
        };
        TypeBridgeStatus::Ok
    })
}

/// Return the bounded number of canonical graph heads.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_catalog_head_count(
    catalog: *const TypeBridgeMigrationCatalog,
    out_count: *mut usize,
) -> TypeBridgeStatus {
    unsafe { scalar(catalog, out_count, |value| value.state.heads().len()) }
}

/// Return an independently owned canonical graph-head identity.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_catalog_head_at(
    catalog: *const TypeBridgeMigrationCatalog,
    index: usize,
    out_identity: *mut *mut TypeBridgeMigrationIdentity,
) -> TypeBridgeStatus {
    guarded(|| {
        if catalog.is_null() || out_identity.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        unsafe { out_identity.write_unaligned(ptr::null_mut()) };
        let Some(state) = unsafe { &*catalog }.state.heads().get(index).cloned() else {
            return TypeBridgeStatus::InvalidArgument;
        };
        unsafe {
            out_identity.write_unaligned(Box::into_raw(Box::new(TypeBridgeMigrationIdentity {
                state,
            })))
        };
        TypeBridgeStatus::Ok
    })
}

/// Close a catalog only after every entry or plan child has closed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_catalog_close(
    catalog: *mut *mut TypeBridgeMigrationCatalog,
) -> TypeBridgeStatus {
    guarded(|| {
        if catalog.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        let value = unsafe { catalog.read_unaligned() };
        if value.is_null() {
            return TypeBridgeStatus::Ok;
        }
        if Arc::strong_count(&unsafe { &*value }.state) != 1 {
            return TypeBridgeStatus::InUse;
        }
        unsafe {
            catalog.write_unaligned(ptr::null_mut());
            drop(Box::from_raw(value));
        }
        TypeBridgeStatus::Ok
    })
}

/// Return an independently owned identity for one history entry.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_history_entry_identity(
    entry: *const TypeBridgeMigrationHistoryEntry,
    out_identity: *mut *mut TypeBridgeMigrationIdentity,
) -> TypeBridgeStatus {
    unsafe { owned_identity(entry, out_identity, |entry| entry.state.identity()) }
}

/// Return the number of exact parent identities for one history entry.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_history_entry_parent_count(
    entry: *const TypeBridgeMigrationHistoryEntry,
    out_count: *mut usize,
) -> TypeBridgeStatus {
    unsafe { scalar(entry, out_count, |value| value.state.parents().len()) }
}

/// Return an independently owned exact parent identity.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_history_entry_parent_at(
    entry: *const TypeBridgeMigrationHistoryEntry,
    index: usize,
    out_identity: *mut *mut TypeBridgeMigrationIdentity,
) -> TypeBridgeStatus {
    guarded(|| {
        if entry.is_null() || out_identity.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        unsafe { out_identity.write_unaligned(ptr::null_mut()) };
        let Some(state) = unsafe { &*entry }.state.parents().get(index).cloned() else {
            return TypeBridgeStatus::InvalidArgument;
        };
        unsafe {
            out_identity.write_unaligned(Box::into_raw(Box::new(TypeBridgeMigrationIdentity {
                state,
            })))
        };
        TypeBridgeStatus::Ok
    })
}

/// Copy the exact 32-byte canonical manifest digest.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_history_entry_manifest_digest(
    entry: *const TypeBridgeMigrationHistoryEntry,
    out_digest: *mut u8,
    digest_length: usize,
) -> TypeBridgeStatus {
    guarded(|| {
        if entry.is_null() || out_digest.is_null() || digest_length != 32 {
            return TypeBridgeStatus::InvalidArgument;
        }
        let digest = unsafe { &*entry }.state.manifest_digest();
        unsafe { ptr::copy_nonoverlapping(digest.as_ptr(), out_digest, digest.len()) };
        TypeBridgeStatus::Ok
    })
}

/// Return the canonical step count.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_history_entry_step_count(
    entry: *const TypeBridgeMigrationHistoryEntry,
    out_count: *mut usize,
) -> TypeBridgeStatus {
    unsafe { scalar(entry, out_count, |value| value.state.step_count()) }
}

/// Return the stable migration safety-class tag.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_history_entry_safety(
    entry: *const TypeBridgeMigrationHistoryEntry,
    out_safety: *mut u32,
) -> TypeBridgeStatus {
    unsafe { scalar(entry, out_safety, |value| safety(value.state.safety())) }
}

/// Return exact zero or one for verified reversibility.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_history_entry_reversible(
    entry: *const TypeBridgeMigrationHistoryEntry,
    out_reversible: *mut u8,
) -> TypeBridgeStatus {
    unsafe {
        scalar(entry, out_reversible, |value| {
            u8::from(value.state.reversible())
        })
    }
}

/// Close a history-entry snapshot idempotently.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_history_entry_close(
    entry: *mut *mut TypeBridgeMigrationHistoryEntry,
) -> TypeBridgeStatus {
    unsafe { close_box(entry) }
}

/// Borrow the application-label component of a live migration identity.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_identity_app_label(
    identity: *const TypeBridgeMigrationIdentity,
    out_label: *mut TypeBridgeByteView,
) -> TypeBridgeStatus {
    unsafe { identity_view(identity, out_label, |value| &value.state.app_label) }
}

/// Borrow the migration-name component of a live migration identity.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_identity_name(
    identity: *const TypeBridgeMigrationIdentity,
    out_name: *mut TypeBridgeByteView,
) -> TypeBridgeStatus {
    unsafe { identity_view(identity, out_name, |value| &value.state.name) }
}

/// Close a migration identity idempotently.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_identity_close(
    identity: *mut *mut TypeBridgeMigrationIdentity,
) -> TypeBridgeStatus {
    unsafe { close_box(identity) }
}

fn safety(value: type_bridge_schema::SafetyClass) -> u32 {
    match value {
        type_bridge_schema::SafetyClass::FormalOnly => 1,
        type_bridge_schema::SafetyClass::SchemaMetadata => 2,
        type_bridge_schema::SafetyClass::Additive => 3,
        type_bridge_schema::SafetyClass::Conditional => 4,
        type_bridge_schema::SafetyClass::BackfillRequired => 5,
        type_bridge_schema::SafetyClass::Destructive => 6,
        type_bridge_schema::SafetyClass::Opaque => 7,
        type_bridge_schema::SafetyClass::Unsupported => 8,
    }
}

unsafe fn scalar<Input, Output: Copy>(
    input: *const Input,
    out: *mut Output,
    read: impl FnOnce(&Input) -> Output,
) -> TypeBridgeStatus {
    guarded(|| {
        if input.is_null() || out.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        unsafe { out.write_unaligned(read(&*input)) };
        TypeBridgeStatus::Ok
    })
}

unsafe fn owned_identity<Input>(
    input: *const Input,
    out: *mut *mut TypeBridgeMigrationIdentity,
    read: impl FnOnce(&Input) -> MigrationIdentitySnapshot,
) -> TypeBridgeStatus {
    guarded(|| {
        if input.is_null() || out.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        unsafe { out.write_unaligned(ptr::null_mut()) };
        let state = read(unsafe { &*input });
        unsafe {
            out.write_unaligned(Box::into_raw(Box::new(TypeBridgeMigrationIdentity {
                state,
            })))
        };
        TypeBridgeStatus::Ok
    })
}

unsafe fn identity_view(
    identity: *const TypeBridgeMigrationIdentity,
    out: *mut TypeBridgeByteView,
    read: impl FnOnce(&TypeBridgeMigrationIdentity) -> &[u8],
) -> TypeBridgeStatus {
    guarded(|| {
        if identity.is_null() || out.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        let bytes = read(unsafe { &*identity });
        unsafe {
            out.write_unaligned(TypeBridgeByteView {
                data: bytes.as_ptr(),
                length: bytes.len(),
            })
        };
        TypeBridgeStatus::Ok
    })
}

/// Build a provider-free apply preview against explicit applied and target sets.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_catalog_preview_apply(
    catalog: *const TypeBridgeMigrationCatalog,
    applied: *const *const TypeBridgeMigrationIdentity,
    applied_count: usize,
    targets: *const *const TypeBridgeMigrationIdentity,
    target_count: usize,
    use_default_head: u8,
    out_plan: *mut *mut TypeBridgeMigrationPlan,
    out_diagnostics: *mut *mut TypeBridgeDiagnostics,
) -> TypeBridgeStatus {
    if let Err(status) = unsafe { preflight_one_output(catalog, out_plan, out_diagnostics) } {
        return status;
    }
    unsafe { out_plan.write_unaligned(ptr::null_mut()) };
    guarded(|| {
        let Some(catalog) = (unsafe { catalog.as_ref() }) else {
            return TypeBridgeStatus::InvalidArgument;
        };
        let applied = match unsafe { identity_snapshots(applied, applied_count) } {
            Ok(value) => value,
            Err(status) => return status,
        };
        let targets = if use_default_head == 1 && targets.is_null() && target_count == 0 {
            None
        } else if use_default_head == 0 {
            match unsafe { identity_snapshots(targets, target_count) } {
                Ok(value) => Some(value),
                Err(status) => return status,
            }
        } else {
            return TypeBridgeStatus::InvalidArgument;
        };
        match catalog.state.preview_apply(&applied, targets.as_deref()) {
            Ok(state) => {
                unsafe {
                    out_plan
                        .write_unaligned(Box::into_raw(Box::new(TypeBridgeMigrationPlan { state })))
                };
                TypeBridgeStatus::Ok
            }
            Err(error) => return_failure(error, out_diagnostics),
        }
    })
}

/// Build a provider-free rollback preview from explicit applied/removal sets.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_catalog_preview_rollback(
    catalog: *const TypeBridgeMigrationCatalog,
    applied: *const *const TypeBridgeMigrationIdentity,
    applied_count: usize,
    removals: *const *const TypeBridgeMigrationIdentity,
    removal_count: usize,
    out_plan: *mut *mut TypeBridgeMigrationPlan,
    out_diagnostics: *mut *mut TypeBridgeDiagnostics,
) -> TypeBridgeStatus {
    if let Err(status) = unsafe { preflight_one_output(catalog, out_plan, out_diagnostics) } {
        return status;
    }
    unsafe { out_plan.write_unaligned(ptr::null_mut()) };
    guarded(|| {
        let Some(catalog) = (unsafe { catalog.as_ref() }) else {
            return TypeBridgeStatus::InvalidArgument;
        };
        let applied = match unsafe { identity_snapshots(applied, applied_count) } {
            Ok(value) => value,
            Err(status) => return status,
        };
        let removals = match unsafe { identity_snapshots(removals, removal_count) } {
            Ok(value) => value,
            Err(status) => return status,
        };
        match catalog.state.preview_rollback(&applied, &removals) {
            Ok(state) => {
                unsafe {
                    out_plan
                        .write_unaligned(Box::into_raw(Box::new(TypeBridgeMigrationPlan { state })))
                };
                TypeBridgeStatus::Ok
            }
            Err(error) => return_failure(error, out_diagnostics),
        }
    })
}

/// Return the stable apply/rollback direction tag.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_plan_direction(
    plan: *const TypeBridgeMigrationPlan,
    out_direction: *mut u32,
) -> TypeBridgeStatus {
    unsafe {
        scalar(plan, out_direction, |value| match value.state.direction() {
            MigrationPlanDirection::Apply => 1,
            MigrationPlanDirection::Rollback => 2,
        })
    }
}

/// Return exact zero or one for execution authorization.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_plan_execution_authorized(
    plan: *const TypeBridgeMigrationPlan,
    out_authorized: *mut u8,
) -> TypeBridgeStatus {
    unsafe {
        scalar(plan, out_authorized, |value| {
            u8::from(value.state.execution_authorized())
        })
    }
}

/// Return the number of ordered migration entries in a plan.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_plan_count(
    plan: *const TypeBridgeMigrationPlan,
    out_count: *mut usize,
) -> TypeBridgeStatus {
    unsafe { scalar(plan, out_count, |value| value.state.len()) }
}

/// Return an independently owned immutable plan-entry snapshot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_plan_entry_at(
    plan: *const TypeBridgeMigrationPlan,
    index: usize,
    out_entry: *mut *mut TypeBridgeMigrationPlanEntry,
) -> TypeBridgeStatus {
    guarded(|| {
        if plan.is_null() || out_entry.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        unsafe { out_entry.write_unaligned(ptr::null_mut()) };
        let Some(state) = unsafe { &*plan }.state.entry(index) else {
            return TypeBridgeStatus::InvalidArgument;
        };
        unsafe {
            out_entry.write_unaligned(Box::into_raw(Box::new(TypeBridgeMigrationPlanEntry {
                state,
            })))
        };
        TypeBridgeStatus::Ok
    })
}

/// Open one mutable approval builder bound to this exact preview.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_plan_approval_builder(
    plan: *const TypeBridgeMigrationPlan,
    out_builder: *mut *mut TypeBridgeMigrationApprovalBuilder,
) -> TypeBridgeStatus {
    guarded(|| {
        if plan.is_null() || out_builder.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        unsafe { out_builder.write_unaligned(ptr::null_mut()) };
        let state = unsafe { &*plan }.state.approval_builder();
        unsafe {
            out_builder.write_unaligned(Box::into_raw(Box::new(
                TypeBridgeMigrationApprovalBuilder { state: Some(state) },
            )))
        };
        TypeBridgeStatus::Ok
    })
}

/// Rebuild one fresh executable plan from exact preview-bound approvals.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_plan_authorize(
    plan: *const TypeBridgeMigrationPlan,
    approvals: *const TypeBridgeMigrationApprovalSet,
    out_authorized: *mut *mut TypeBridgeMigrationPlan,
    out_diagnostics: *mut *mut TypeBridgeDiagnostics,
) -> TypeBridgeStatus {
    if let Err(status) = unsafe { preflight_one_output(plan, out_authorized, out_diagnostics) } {
        return status;
    }
    unsafe { out_authorized.write_unaligned(ptr::null_mut()) };
    guarded(|| {
        let (Some(plan), Some(approvals)) =
            (unsafe { plan.as_ref() }, unsafe { approvals.as_ref() })
        else {
            return TypeBridgeStatus::InvalidArgument;
        };
        match plan.state.authorize(&approvals.state) {
            Ok(state) => {
                unsafe {
                    out_authorized
                        .write_unaligned(Box::into_raw(Box::new(TypeBridgeMigrationPlan { state })))
                };
                TypeBridgeStatus::Ok
            }
            Err(error) => return_failure(error, out_diagnostics),
        }
    })
}

/// Execute one authorized plan through the exact database/provider binding.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_plan_execute(
    plan: *const TypeBridgeMigrationPlan,
    database: *const TypeBridgeDatabase,
    holder: TypeBridgeByteView,
    out_outcome: *mut *mut TypeBridgeMigrationExecutionOutcome,
    out_diagnostics: *mut *mut TypeBridgeDiagnostics,
) -> TypeBridgeStatus {
    if let Err(status) = unsafe { preflight_one_output(plan, out_outcome, out_diagnostics) } {
        return status;
    }
    unsafe { out_outcome.write_unaligned(ptr::null_mut()) };
    guarded(|| {
        let (Some(plan), Some(database)) = (unsafe { plan.as_ref() }, unsafe { database.as_ref() })
        else {
            return TypeBridgeStatus::InvalidArgument;
        };
        if holder.data.is_null() || holder.length == 0 || holder.length > 128 {
            return TypeBridgeStatus::InvalidArgument;
        }
        let bytes = unsafe { std::slice::from_raw_parts(holder.data, holder.length) };
        let Ok(holder) = std::str::from_utf8(bytes) else {
            return TypeBridgeStatus::InvalidArgument;
        };
        match plan.state.execute(database, holder) {
            Ok(state) => {
                unsafe {
                    out_outcome.write_unaligned(Box::into_raw(Box::new(
                        TypeBridgeMigrationExecutionOutcome { state },
                    )))
                };
                TypeBridgeStatus::Ok
            }
            Err(error) => return_failure(error, out_diagnostics),
        }
    })
}

/// Close a plan only after every entry, builder, approval, or authorized child closes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_plan_close(
    plan: *mut *mut TypeBridgeMigrationPlan,
) -> TypeBridgeStatus {
    guarded(|| {
        if plan.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        let value = unsafe { plan.read_unaligned() };
        if value.is_null() {
            return TypeBridgeStatus::Ok;
        }
        if Arc::strong_count(&unsafe { &*value }.state) != 1 {
            return TypeBridgeStatus::InUse;
        }
        unsafe {
            plan.write_unaligned(ptr::null_mut());
            drop(Box::from_raw(value));
        }
        TypeBridgeStatus::Ok
    })
}

/// Return an independently owned identity for one plan entry.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_plan_entry_identity(
    entry: *const TypeBridgeMigrationPlanEntry,
    out_identity: *mut *mut TypeBridgeMigrationIdentity,
) -> TypeBridgeStatus {
    unsafe { owned_identity(entry, out_identity, |value| value.state.identity()) }
}

/// Return the stable safety tag for one plan entry.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_plan_entry_safety(
    entry: *const TypeBridgeMigrationPlanEntry,
    out_safety: *mut u32,
) -> TypeBridgeStatus {
    unsafe { scalar(entry, out_safety, |value| safety(value.state.safety())) }
}

/// Return the number of mixed schema/backfill operations.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_plan_entry_operation_count(
    entry: *const TypeBridgeMigrationPlanEntry,
    out_count: *mut usize,
) -> TypeBridgeStatus {
    unsafe { scalar(entry, out_count, |value| value.state.operation_count()) }
}

/// Return the number of bounded schema transaction groups.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_plan_entry_transaction_group_count(
    entry: *const TypeBridgeMigrationPlanEntry,
    out_count: *mut usize,
) -> TypeBridgeStatus {
    unsafe {
        scalar(entry, out_count, |value| {
            value.state.transaction_group_count()
        })
    }
}

/// Return the number of canonical data/backfill operations.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_plan_entry_backfill_count(
    entry: *const TypeBridgeMigrationPlanEntry,
    out_count: *mut usize,
) -> TypeBridgeStatus {
    unsafe { scalar(entry, out_count, |value| value.state.backfill_count()) }
}

/// Close a plan-entry snapshot idempotently.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_plan_entry_close(
    entry: *mut *mut TypeBridgeMigrationPlanEntry,
) -> TypeBridgeStatus {
    unsafe { close_box(entry) }
}

/// Select one exact preview entry for explicit approval.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_approval_builder_approve(
    builder: *mut TypeBridgeMigrationApprovalBuilder,
    index: usize,
    out_diagnostics: *mut *mut TypeBridgeDiagnostics,
) -> TypeBridgeStatus {
    if out_diagnostics.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    unsafe { out_diagnostics.write_unaligned(ptr::null_mut()) };
    guarded(|| {
        let Some(builder) = (unsafe { builder.as_mut() }) else {
            return TypeBridgeStatus::InvalidArgument;
        };
        let Some(state) = builder.state.as_mut() else {
            return TypeBridgeStatus::InvalidArgument;
        };
        match state.approve(index) {
            Ok(()) => TypeBridgeStatus::Ok,
            Err(error) => return_failure(error, out_diagnostics),
        }
    })
}

/// Consume a builder into one immutable exact approval set.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_approval_builder_finish(
    builder: *mut TypeBridgeMigrationApprovalBuilder,
    out_approvals: *mut *mut TypeBridgeMigrationApprovalSet,
    out_diagnostics: *mut *mut TypeBridgeDiagnostics,
) -> TypeBridgeStatus {
    if let Err(status) = unsafe { preflight_one_output(builder, out_approvals, out_diagnostics) } {
        return status;
    }
    unsafe { out_approvals.write_unaligned(ptr::null_mut()) };
    guarded(|| {
        let Some(builder) = (unsafe { builder.as_mut() }) else {
            return TypeBridgeStatus::InvalidArgument;
        };
        let Some(state) = builder.state.take() else {
            return TypeBridgeStatus::InvalidArgument;
        };
        match state.finish() {
            Ok(state) => {
                unsafe {
                    out_approvals.write_unaligned(Box::into_raw(Box::new(
                        TypeBridgeMigrationApprovalSet { state },
                    )))
                };
                TypeBridgeStatus::Ok
            }
            Err(error) => return_failure(error, out_diagnostics),
        }
    })
}

/// Close an approval builder idempotently.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_approval_builder_close(
    builder: *mut *mut TypeBridgeMigrationApprovalBuilder,
) -> TypeBridgeStatus {
    unsafe { close_box(builder) }
}

/// Return the number of exact approvals.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_approval_set_count(
    approvals: *const TypeBridgeMigrationApprovalSet,
    out_count: *mut usize,
) -> TypeBridgeStatus {
    unsafe { scalar(approvals, out_count, |value| value.state.len()) }
}

/// Close an approval set idempotently.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_approval_set_close(
    approvals: *mut *mut TypeBridgeMigrationApprovalSet,
) -> TypeBridgeStatus {
    unsafe { close_box(approvals) }
}

unsafe fn identity_snapshots(
    values: *const *const TypeBridgeMigrationIdentity,
    count: usize,
) -> Result<Vec<MigrationIdentitySnapshot>, TypeBridgeStatus> {
    if count > type_bridge_contract::limits::MAX_CANONICAL_COLLECTION_LEN {
        return Err(TypeBridgeStatus::ResourceLimit);
    }
    if count == 0 {
        return if values.is_null() {
            Ok(Vec::new())
        } else {
            Err(TypeBridgeStatus::InvalidArgument)
        };
    }
    if values.is_null() {
        return Err(TypeBridgeStatus::InvalidArgument);
    }
    let mut snapshots = Vec::new();
    snapshots
        .try_reserve(count)
        .map_err(|_| TypeBridgeStatus::ResourceLimit)?;
    for index in 0..count {
        let value = unsafe { values.add(index).read_unaligned() };
        let Some(value) = (unsafe { value.as_ref() }) else {
            return Err(TypeBridgeStatus::InvalidArgument);
        };
        snapshots.push(value.state.clone());
    }
    Ok(snapshots)
}

/// Return the apply/rollback direction of a terminal execution outcome.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_execution_outcome_direction(
    outcome: *const TypeBridgeMigrationExecutionOutcome,
    out_direction: *mut u32,
) -> TypeBridgeStatus {
    unsafe {
        scalar(outcome, out_direction, |value| {
            match value.state.report().direction() {
                type_bridge_schema_migration::MigrationExecutionDirection::Apply => 1,
                type_bridge_schema_migration::MigrationExecutionDirection::Rollback => 2,
            }
        })
    }
}

/// Return the stable terminal status tag.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_execution_outcome_status(
    outcome: *const TypeBridgeMigrationExecutionOutcome,
    out_status: *mut u32,
) -> TypeBridgeStatus {
    unsafe {
        scalar(outcome, out_status, |value| {
            match value.state.report().status() {
                type_bridge_schema_migration::MigrationExecutionStatus::Applied => 1,
                type_bridge_schema_migration::MigrationExecutionStatus::RolledBack => 2,
                type_bridge_schema_migration::MigrationExecutionStatus::RetrySafe => 3,
                type_bridge_schema_migration::MigrationExecutionStatus::RequiresExplicitRecovery => 4,
            }
        })
    }
}

/// Return the interrupted migration identity, or a null handle on success.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_execution_outcome_migration_identity(
    outcome: *const TypeBridgeMigrationExecutionOutcome,
    out_identity: *mut *mut TypeBridgeMigrationIdentity,
) -> TypeBridgeStatus {
    guarded(|| {
        if outcome.is_null() || out_identity.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        unsafe { out_identity.write_unaligned(ptr::null_mut()) };
        let Some(id) = unsafe { &*outcome }.state.report().migration_id() else {
            return TypeBridgeStatus::Ok;
        };
        let state = MigrationIdentitySnapshot {
            app_label: id.app_label().as_str().as_bytes().to_vec(),
            name: id.name().as_str().as_bytes().to_vec(),
        };
        unsafe {
            out_identity.write_unaligned(Box::into_raw(Box::new(TypeBridgeMigrationIdentity {
                state,
            })))
        };
        TypeBridgeStatus::Ok
    })
}

/// Return the stable interrupted-position kind, or zero when absent.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_execution_outcome_position_kind(
    outcome: *const TypeBridgeMigrationExecutionOutcome,
    out_kind: *mut u32,
) -> TypeBridgeStatus {
    unsafe {
        scalar(outcome, out_kind, |value| {
            match value.state.report().position() {
                None => 0,
                Some(type_bridge_schema_migration::MigrationExecutionReportPosition::TransactionGroup(_)) => 1,
                Some(type_bridge_schema_migration::MigrationExecutionReportPosition::BackfillStep(_)) => 2,
                Some(type_bridge_schema_migration::MigrationExecutionReportPosition::ManifestCheckpoint) => 3,
                Some(type_bridge_schema_migration::MigrationExecutionReportPosition::RollbackStep(_)) => 4,
            }
        })
    }
}

/// Return the interrupted-position ordinal and a presence bit.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_execution_outcome_position_ordinal(
    outcome: *const TypeBridgeMigrationExecutionOutcome,
    out_ordinal: *mut usize,
    out_present: *mut u8,
) -> TypeBridgeStatus {
    guarded(|| {
        if outcome.is_null() || out_ordinal.is_null() || out_present.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        let ordinal = match unsafe { &*outcome }.state.report().position() {
            Some(
                type_bridge_schema_migration::MigrationExecutionReportPosition::TransactionGroup(
                    value,
                )
                | type_bridge_schema_migration::MigrationExecutionReportPosition::BackfillStep(value)
                | type_bridge_schema_migration::MigrationExecutionReportPosition::RollbackStep(value),
            ) => Some(value),
            None
            | Some(
                type_bridge_schema_migration::MigrationExecutionReportPosition::ManifestCheckpoint,
            ) => None,
        };
        unsafe {
            out_ordinal.write_unaligned(ordinal.unwrap_or_default());
            out_present.write_unaligned(u8::from(ordinal.is_some()));
        }
        TypeBridgeStatus::Ok
    })
}

/// Return an independently owned canonical diagnostic list for this outcome.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_execution_outcome_diagnostics(
    outcome: *const TypeBridgeMigrationExecutionOutcome,
    out_diagnostics: *mut *mut TypeBridgeDiagnostics,
) -> TypeBridgeStatus {
    guarded(|| {
        if outcome.is_null() || out_diagnostics.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        let diagnostics = unsafe { &*outcome }
            .state
            .report()
            .diagnostic()
            .cloned()
            .into_iter()
            .collect();
        unsafe {
            out_diagnostics
                .write_unaligned(Box::into_raw(Box::new(diagnostics_handle(diagnostics))))
        };
        TypeBridgeStatus::Ok
    })
}

/// Return the bounded number of terminal backfill observations.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_execution_outcome_backfill_count(
    outcome: *const TypeBridgeMigrationExecutionOutcome,
    out_count: *mut usize,
) -> TypeBridgeStatus {
    unsafe {
        scalar(outcome, out_count, |value| {
            value.state.report().backfills().len()
        })
    }
}

/// Return an independently owned terminal backfill observation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_execution_outcome_backfill_at(
    outcome: *const TypeBridgeMigrationExecutionOutcome,
    index: usize,
    out_observation: *mut *mut TypeBridgeMigrationBackfillObservation,
) -> TypeBridgeStatus {
    guarded(|| {
        if outcome.is_null() || out_observation.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        unsafe { out_observation.write_unaligned(ptr::null_mut()) };
        let Some(state) = unsafe { &*outcome }
            .state
            .report()
            .backfills()
            .get(index)
            .cloned()
        else {
            return TypeBridgeStatus::InvalidArgument;
        };
        unsafe {
            out_observation.write_unaligned(Box::into_raw(Box::new(
                TypeBridgeMigrationBackfillObservation { state },
            )))
        };
        TypeBridgeStatus::Ok
    })
}

/// Close an execution outcome idempotently.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_execution_outcome_close(
    outcome: *mut *mut TypeBridgeMigrationExecutionOutcome,
) -> TypeBridgeStatus {
    unsafe { close_box(outcome) }
}

/// Return an independently owned identity for one backfill observation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_backfill_observation_identity(
    observation: *const TypeBridgeMigrationBackfillObservation,
    out_identity: *mut *mut TypeBridgeMigrationIdentity,
) -> TypeBridgeStatus {
    guarded(|| {
        if observation.is_null() || out_identity.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        unsafe { out_identity.write_unaligned(ptr::null_mut()) };
        let id = unsafe { &*observation }.state.migration_id();
        let state = MigrationIdentitySnapshot {
            app_label: id.app_label().as_str().as_bytes().to_vec(),
            name: id.name().as_str().as_bytes().to_vec(),
        };
        unsafe {
            out_identity.write_unaligned(Box::into_raw(Box::new(TypeBridgeMigrationIdentity {
                state,
            })))
        };
        TypeBridgeStatus::Ok
    })
}

/// Return the execution-order ordinal for one backfill observation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_backfill_observation_operation_ordinal(
    observation: *const TypeBridgeMigrationBackfillObservation,
    out_ordinal: *mut usize,
) -> TypeBridgeStatus {
    unsafe {
        scalar(observation, out_ordinal, |value| {
            value.state.operation_ordinal()
        })
    }
}

/// Return the original canonical manifest-step position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_backfill_observation_manifest_step_index(
    observation: *const TypeBridgeMigrationBackfillObservation,
    out_index: *mut usize,
) -> TypeBridgeStatus {
    unsafe {
        scalar(observation, out_index, |value| {
            value.state.manifest_step_index()
        })
    }
}

/// Copy the exact 32-byte canonical backfill-plan digest.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_backfill_observation_plan_digest(
    observation: *const TypeBridgeMigrationBackfillObservation,
    out_digest: *mut u8,
    digest_length: usize,
) -> TypeBridgeStatus {
    guarded(|| {
        if observation.is_null() || out_digest.is_null() || digest_length != 32 {
            return TypeBridgeStatus::InvalidArgument;
        }
        let digest = unsafe { &*observation }
            .state
            .evidence()
            .plan_fingerprint()
            .digest()
            .bytes();
        unsafe { ptr::copy_nonoverlapping(digest.as_ptr(), out_digest, digest.len()) };
        TypeBridgeStatus::Ok
    })
}

/// Return the forward/reverse direction tag.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_backfill_observation_direction(
    observation: *const TypeBridgeMigrationBackfillObservation,
    out_direction: *mut u32,
) -> TypeBridgeStatus {
    unsafe {
        scalar(observation, out_direction, |value| {
            match value.state.evidence().direction() {
                type_bridge_schema_migration::BackfillExecutionDirection::Forward => 1,
                type_bridge_schema_migration::BackfillExecutionDirection::Reverse => 2,
            }
        })
    }
}

/// Return balanced terminal matched/changed/skipped and transaction-group counts.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_backfill_observation_counts(
    observation: *const TypeBridgeMigrationBackfillObservation,
    out_matched: *mut u64,
    out_changed: *mut u64,
    out_skipped: *mut u64,
    out_transaction_groups: *mut u32,
) -> TypeBridgeStatus {
    guarded(|| {
        if observation.is_null()
            || out_matched.is_null()
            || out_changed.is_null()
            || out_skipped.is_null()
            || out_transaction_groups.is_null()
        {
            return TypeBridgeStatus::InvalidArgument;
        }
        let counts = unsafe { &*observation }.state.evidence().counts();
        unsafe {
            out_matched.write_unaligned(counts.matched());
            out_changed.write_unaligned(counts.changed());
            out_skipped.write_unaligned(counts.skipped());
            out_transaction_groups.write_unaligned(counts.transaction_groups());
        }
        TypeBridgeStatus::Ok
    })
}

/// Close a backfill observation idempotently.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_backfill_observation_close(
    observation: *mut *mut TypeBridgeMigrationBackfillObservation,
) -> TypeBridgeStatus {
    unsafe { close_box(observation) }
}

/// Verify catalog, ledger, and live semantics without mutating provider state.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_catalog_verify(
    catalog: *const TypeBridgeMigrationCatalog,
    database: *const TypeBridgeDatabase,
    out_report: *mut *mut TypeBridgeMigrationVerificationReport,
    out_diagnostics: *mut *mut TypeBridgeDiagnostics,
) -> TypeBridgeStatus {
    if let Err(status) = unsafe { preflight_one_output(catalog, out_report, out_diagnostics) } {
        return status;
    }
    unsafe { out_report.write_unaligned(ptr::null_mut()) };
    guarded(|| {
        let (Some(catalog), Some(database)) =
            (unsafe { catalog.as_ref() }, unsafe { database.as_ref() })
        else {
            return TypeBridgeStatus::InvalidArgument;
        };
        match catalog.state.verify(database) {
            Ok(state) => {
                unsafe {
                    out_report.write_unaligned(Box::into_raw(Box::new(
                        TypeBridgeMigrationVerificationReport {
                            _catalog: Arc::clone(&catalog.state),
                            state,
                        },
                    )))
                };
                TypeBridgeStatus::Ok
            }
            Err(error) => return_failure(error, out_diagnostics),
        }
    })
}

/// Return exact zero or one for a clean verification result.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_verification_report_is_clean(
    report: *const TypeBridgeMigrationVerificationReport,
    out_clean: *mut u8,
) -> TypeBridgeStatus {
    unsafe { scalar(report, out_clean, |value| u8::from(value.state.is_clean())) }
}

/// Return the bounded number of ordered drift findings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_verification_report_finding_count(
    report: *const TypeBridgeMigrationVerificationReport,
    out_count: *mut usize,
) -> TypeBridgeStatus {
    unsafe { scalar(report, out_count, |value| value.state.len()) }
}

/// Return an independently owned ordered drift-finding snapshot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_verification_report_finding_at(
    report: *const TypeBridgeMigrationVerificationReport,
    index: usize,
    out_finding: *mut *mut TypeBridgeMigrationVerificationFinding,
) -> TypeBridgeStatus {
    guarded(|| {
        if report.is_null() || out_finding.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        unsafe { out_finding.write_unaligned(ptr::null_mut()) };
        let Some(state) = unsafe { &*report }.state.finding(index) else {
            return TypeBridgeStatus::InvalidArgument;
        };
        unsafe {
            out_finding.write_unaligned(Box::into_raw(Box::new(
                TypeBridgeMigrationVerificationFinding { state },
            )))
        };
        TypeBridgeStatus::Ok
    })
}

/// Return the bounded applied-frontier identity count.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_verification_report_frontier_count(
    report: *const TypeBridgeMigrationVerificationReport,
    out_count: *mut usize,
) -> TypeBridgeStatus {
    unsafe {
        scalar(report, out_count, |value| {
            value.state.applied_frontier().len()
        })
    }
}

/// Return an independently owned applied-frontier identity.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_verification_report_frontier_at(
    report: *const TypeBridgeMigrationVerificationReport,
    index: usize,
    out_identity: *mut *mut TypeBridgeMigrationIdentity,
) -> TypeBridgeStatus {
    guarded(|| {
        if report.is_null() || out_identity.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        unsafe { out_identity.write_unaligned(ptr::null_mut()) };
        let Some(state) = unsafe { &*report }
            .state
            .applied_frontier()
            .get(index)
            .cloned()
        else {
            return TypeBridgeStatus::InvalidArgument;
        };
        unsafe {
            out_identity.write_unaligned(Box::into_raw(Box::new(TypeBridgeMigrationIdentity {
                state,
            })))
        };
        TypeBridgeStatus::Ok
    })
}

/// Close a verification report idempotently.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_verification_report_close(
    report: *mut *mut TypeBridgeMigrationVerificationReport,
) -> TypeBridgeStatus {
    unsafe { close_box(report) }
}

/// Return the stable drift-finding kind tag.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_verification_finding_kind(
    finding: *const TypeBridgeMigrationVerificationFinding,
    out_kind: *mut u32,
) -> TypeBridgeStatus {
    unsafe {
        scalar(finding, out_kind, |value| match value.state.kind {
            MigrationVerificationFindingKind::AppliedLedger => 1,
            MigrationVerificationFindingKind::LiveSemantics => 2,
            MigrationVerificationFindingKind::DesiredDivergence => 3,
            MigrationVerificationFindingKind::PendingMigrations => 4,
            MigrationVerificationFindingKind::Capabilities => 5,
        })
    }
}

/// Return the bounded number of pending identities carried by a finding.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_verification_finding_pending_count(
    finding: *const TypeBridgeMigrationVerificationFinding,
    out_count: *mut usize,
) -> TypeBridgeStatus {
    unsafe { scalar(finding, out_count, |value| value.state.pending.len()) }
}

/// Return an independently owned pending migration identity.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_verification_finding_pending_at(
    finding: *const TypeBridgeMigrationVerificationFinding,
    index: usize,
    out_identity: *mut *mut TypeBridgeMigrationIdentity,
) -> TypeBridgeStatus {
    guarded(|| {
        if finding.is_null() || out_identity.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        unsafe { out_identity.write_unaligned(ptr::null_mut()) };
        let Some(state) = unsafe { &*finding }.state.pending.get(index).cloned() else {
            return TypeBridgeStatus::InvalidArgument;
        };
        unsafe {
            out_identity.write_unaligned(Box::into_raw(Box::new(TypeBridgeMigrationIdentity {
                state,
            })))
        };
        TypeBridgeStatus::Ok
    })
}

/// Return an independently owned canonical diagnostic list for a finding.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_verification_finding_diagnostics(
    finding: *const TypeBridgeMigrationVerificationFinding,
    out_diagnostics: *mut *mut TypeBridgeDiagnostics,
) -> TypeBridgeStatus {
    guarded(|| {
        if finding.is_null() || out_diagnostics.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        let diagnostics = unsafe { &*finding }
            .state
            .diagnostic
            .clone()
            .into_iter()
            .collect();
        unsafe {
            out_diagnostics
                .write_unaligned(Box::into_raw(Box::new(diagnostics_handle(diagnostics))))
        };
        TypeBridgeStatus::Ok
    })
}

/// Close a verification finding idempotently.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_migration_verification_finding_close(
    finding: *mut *mut TypeBridgeMigrationVerificationFinding,
) -> TypeBridgeStatus {
    unsafe { close_box(finding) }
}
